//! Educational non-blocking TCP echo server for Phase 1, Week 3, step 4.
//!
//! This example demonstrates event-driven networking using [`cachedb::net::event_loop::EventLoop`].
//! It shows how a single thread can concurrently handle multiple connected clients without
//! blocking, managing partial reads, partial writes, and `WouldBlock` (`EAGAIN`) conditions.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use cachedb::net::connection::{ConnectionTable, DEFAULT_MAX_CONNECTIONS, ReadStatus, WriteStatus};
use cachedb::net::event_loop::{EventLoop, Handler, TimerId, Token};
use cachedb::net::sys::{EpollFlags, as_raw_fd};
use std::io;
use std::net::TcpListener;
use std::time::Duration;

/// Reserved token identifying readiness events on the listening server socket.
const LISTENER_TOKEN: Token = 0;

/// Reserved timer identifier for periodic idle connection sweeps.
const IDLE_SWEEP_TIMER_ID: TimerId = 1;

/// An event-driven, non-blocking TCP echo server.
///
/// # Core Concepts Explained
///
/// - **`set_nonblocking(true)`**: In non-blocking mode, operations that would otherwise
///   sleep the calling thread return immediately with an error of kind [`io::ErrorKind::WouldBlock`].
/// - **Accept Loop**: When the listener is marked `READABLE`, multiple incoming connections
///   might be queued in the kernel backlog. We accept in a loop until `WouldBlock`.
/// - **Connection Table**: Centralized table managing active client sessions, unique token IDs,
///   and capacity limit enforcement.
/// - **Streaming Buffers & Write Queues**: Each connection maintains an inbound [`cachedb::net::buffer::ReadBuffer`]
///   and outbound [`cachedb::net::buffer::WriteQueue`].
/// - **Dynamic Backpressure**: When a connection's write queue exceeds high watermark limits,
///   `desired_flags()` drops `READABLE`, preventing slow clients from overwhelming server memory.
/// - **Connection Limits & Idle Timeouts**: Rejects new clients when capacity is reached and sweeps
///   idle connections periodically.
/// - **Graceful Shutdown**: Drains pending writes upon shutdown signal within a configurable timeout.
#[derive(Debug)]
pub struct NonBlockingEchoServer {
    listener: TcpListener,
    connections: ConnectionTable,
    event_loop: EventLoop,
    idle_timeout: Option<Duration>,
    sweep_interval: Duration,
    shutdown_timeout: Duration,
    is_shutting_down: bool,
    rejected_connections: usize,
}

impl NonBlockingEchoServer {
    /// Creates a new non-blocking echo server instance with default configuration.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if setting the listener to non-blocking mode or registering
    /// with the event loop fails.
    pub fn new(listener: TcpListener) -> io::Result<Self> {
        Self::with_config(
            listener,
            DEFAULT_MAX_CONNECTIONS,
            Some(Duration::from_secs(60)),
            Duration::from_secs(1),
            Duration::from_secs(5),
        )
    }

    /// Creates a non-blocking echo server instance with explicit limits and timeouts.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if setting the listener to non-blocking mode or registering
    /// with the event loop fails.
    pub fn with_config(
        listener: TcpListener,
        max_connections: usize,
        idle_timeout: Option<Duration>,
        sweep_interval: Duration,
        shutdown_timeout: Duration,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;

        let mut event_loop = EventLoop::new()?;
        event_loop.register(as_raw_fd(&listener), LISTENER_TOKEN, EpollFlags::READABLE)?;

        if idle_timeout.is_some() {
            event_loop.schedule_timer(sweep_interval, IDLE_SWEEP_TIMER_ID);
        }

        Ok(Self {
            listener,
            connections: ConnectionTable::with_max_connections(max_connections),
            event_loop,
            idle_timeout,
            sweep_interval,
            shutdown_timeout,
            is_shutting_down: false,
            rejected_connections: 0,
        })
    }

    /// Returns the number of connections that were rejected due to capacity limits.
    #[must_use]
    pub fn rejected_connections(&self) -> usize {
        self.rejected_connections
    }

    /// Returns `true` if the server is in the process of shutting down.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down
    }

    /// Initiates a graceful shutdown sequence.
    ///
    /// 1. Deregisters the listener socket so no further connections are accepted.
    /// 2. Marks active connections for closing and pauses reading.
    /// 3. Drains in-flight write queues until complete or `shutdown_timeout` expires.
    /// 4. Force-closes any lingering connections that exceeded the deadline.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if polling during the drain phase fails.
    pub fn initiate_graceful_shutdown(&mut self) -> io::Result<()> {
        if self.is_shutting_down {
            return Ok(());
        }
        self.is_shutting_down = true;

        // 1. Deregister the listener so no further connections are accepted.
        let _ = self.event_loop.deregister(as_raw_fd(&self.listener));

        // 2. Initiate shutdown on active connections (closes idle ones, keeps WRITABLE for pending data).
        self.connections.initiate_shutdown(&mut self.event_loop);

        // 3. Drain loop: flush pending write queues.
        let drain_deadline = std::time::Instant::now() + self.shutdown_timeout;
        let mut loop_ref = std::mem::replace(&mut self.event_loop, EventLoop::new()?);

        while !self.connections.is_empty() && std::time::Instant::now() < drain_deadline {
            let remaining = drain_deadline.saturating_duration_since(std::time::Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(50));
            let _ = loop_ref.drain_once(self, Some(poll_timeout))?;
        }

        self.event_loop = loop_ref;

        // 4. Force-close any lingering connections that exceeded the shutdown deadline.
        if !self.connections.is_empty() {
            let forced = self.connections.force_close_all(&mut self.event_loop);
            if forced > 0 {
                eprintln!(
                    "Force-closed {forced} lingering client connections on shutdown deadline."
                );
            }
        }

        Ok(())
    }

    /// Runs the server event loop continuously until shutdown is requested,
    /// then conducts a graceful shutdown draining in-flight writes.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if event loop execution encounters a fatal failure.
    pub fn run(&mut self) -> io::Result<()> {
        let mut loop_ref = std::mem::replace(&mut self.event_loop, EventLoop::new()?);
        let res = loop_ref.run(self);
        self.event_loop = loop_ref;
        res?;

        self.initiate_graceful_shutdown()
    }

    /// Runs the server for a limited number of ticks (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if any polling tick fails.
    pub fn run_ticks(&mut self, max_ticks: usize, timeout: Option<Duration>) -> io::Result<()> {
        let mut loop_ref = std::mem::replace(&mut self.event_loop, EventLoop::new()?);
        for _ in 0..max_ticks {
            loop_ref.run_once(self, timeout)?;
        }
        self.event_loop = loop_ref;
        Ok(())
    }

    /// Accepts all pending incoming connections from the kernel connection backlog.
    fn handle_accept(&mut self) -> io::Result<()> {
        if self.is_shutting_down {
            return Ok(());
        }

        loop {
            match self.listener.accept() {
                Ok((stream, peer_addr)) => {
                    stream.set_nonblocking(true)?;

                    if self.connections.is_at_capacity() {
                        eprintln!(
                            "Rejecting connection from {peer_addr}: table reached max capacity ({})",
                            self.connections.max_connections()
                        );
                        self.rejected_connections += 1;
                        drop(stream);
                        continue;
                    }

                    let token = match self.connections.insert(stream, peer_addr) {
                        Ok(t) => t,
                        Err(err) => {
                            eprintln!("Rejected connection from {peer_addr}: {err}");
                            self.rejected_connections += 1;
                            continue;
                        }
                    };

                    if let Err(err) = self
                        .connections
                        .register_with_loop(token, &mut self.event_loop)
                    {
                        eprintln!("Failed to register client {token}: {err}");
                        self.connections.remove(token);
                    }
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                    // No more pending connections in the backlog queue.
                    break;
                }
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Reads incoming bytes from a client socket until `WouldBlock` or EOF (`Ok(0)`).
    fn handle_client_read(&mut self, token: Token) -> io::Result<bool> {
        let Some(conn) = self.connections.get_mut(token) else {
            return Ok(false);
        };

        match conn.read_incoming()? {
            ReadStatus::Eof => {
                self.connections
                    .remove_and_deregister(token, &mut self.event_loop);
                return Ok(false);
            }
            ReadStatus::WouldBlock => {}
            ReadStatus::Ok(n) => {
                // Echo received bytes back to client.
                let data = conn.read_buffer().data().to_vec();
                if let Err(err) = conn.send_chunk(data) {
                    eprintln!("Failed to enqueue echo payload for client {token}: {err}");
                    self.connections
                        .remove_and_deregister(token, &mut self.event_loop);
                    return Ok(false);
                }
                conn.read_buffer_mut().consume(n);
            }
        }

        // Try an immediate write flush of newly queued bytes.
        Ok(self.handle_client_write(token))
    }

    /// Flushes queued outgoing bytes into the client socket transmit buffer.
    fn handle_client_write(&mut self, token: Token) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        match conn.flush_outgoing() {
            Ok(WriteStatus::Complete) => {
                if conn.is_closed() {
                    self.connections
                        .remove_and_deregister(token, &mut self.event_loop);
                    return false;
                }
            }
            Ok(WriteStatus::Pending(_)) => {}
            Err(_) => {
                self.connections
                    .remove_and_deregister(token, &mut self.event_loop);
                return false;
            }
        }

        // Synchronize epoll interest flags: automatically toggles WRITABLE and applies backpressure!
        let _ = self.connections.sync_interest(token, &mut self.event_loop);
        true
    }

    /// Removes a client connection and deregisters its descriptor from the event loop.
    pub fn remove_client(&mut self, token: Token) {
        self.connections
            .remove_and_deregister(token, &mut self.event_loop);
    }

    /// Returns the number of currently connected active clients.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.connections.len()
    }
}

impl Handler for NonBlockingEchoServer {
    fn on_event(&mut self, token: Token, flags: EpollFlags) -> io::Result<()> {
        if token == LISTENER_TOKEN {
            self.handle_accept()?;
        } else {
            // Check for error or peer hangup first.
            if flags.contains(EpollFlags::ERROR) || flags.contains(EpollFlags::HANGUP) {
                self.remove_client(token);
                return Ok(());
            }

            if flags.contains(EpollFlags::READABLE) {
                let _ = self.handle_client_read(token)?;
            }

            if flags.contains(EpollFlags::WRITABLE) {
                let _ = self.handle_client_write(token);
            }
        }
        Ok(())
    }

    fn on_timeout(&mut self, timer_id: TimerId) -> io::Result<()> {
        if timer_id == IDLE_SWEEP_TIMER_ID && !self.is_shutting_down {
            if let Some(idle_timeout) = self.idle_timeout {
                self.connections
                    .sweep_idle(idle_timeout, &mut self.event_loop);
            }
            self.event_loop
                .schedule_timer(self.sweep_interval, IDLE_SWEEP_TIMER_ID);
        }
        Ok(())
    }
}

fn main() -> io::Result<()> {
    cachedb::net::sys::install_shutdown_signal_handlers()?;

    let listener = TcpListener::bind("127.0.0.1:7381")?;
    println!("Non-blocking TCP echo server listening on 127.0.0.1:7381");
    println!("Press Ctrl+C (SIGINT or SIGTERM) to stop gracefully.");

    let mut server = NonBlockingEchoServer::new(listener)?;
    server.run()?;
    println!("Server shut down gracefully.");
    Ok(())
}
