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

/// Internal handler managing connections, listener socket, and configuration.
#[derive(Debug)]
struct EchoServerHandler {
    listener: TcpListener,
    connections: ConnectionTable,
    idle_timeout: Option<Duration>,
    sweep_interval: Duration,
    is_shutting_down: bool,
    rejected_connections: usize,
}

impl EchoServerHandler {
    /// Accepts all pending incoming connections from the kernel connection backlog.
    fn handle_accept(&mut self, event_loop: &mut EventLoop) {
        if self.is_shutting_down {
            return;
        }

        loop {
            match self.listener.accept() {
                Ok((stream, peer_addr)) => {
                    if let Err(err) = stream.set_nonblocking(true) {
                        eprintln!(
                            "Failed to set non-blocking on accepted stream from {peer_addr}: {err}"
                        );
                        drop(stream);
                        continue;
                    }

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

                    if let Err(err) = self.connections.register_with_loop(token, event_loop) {
                        eprintln!("Failed to register client {token}: {err}");
                        self.connections.remove(token);
                    }
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                    // No more pending connections in the backlog queue.
                    break;
                }
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(ref err) if err.kind() == io::ErrorKind::ConnectionAborted => {
                    eprintln!("Connection aborted by peer before accept: {err}");
                }
                Err(ref err) if is_emfile_or_enfile(err) => {
                    eprintln!("File descriptor limit reached during accept (EMFILE/ENFILE): {err}");
                    // Stop accepting for this tick without returning fatal error.
                    break;
                }
                Err(err) => {
                    eprintln!("Non-fatal accept error: {err}");
                    break;
                }
            }
        }
    }

    /// Reads incoming bytes from a client socket until `WouldBlock` or EOF (`Ok(0)`).
    fn handle_client_read(&mut self, token: Token, event_loop: &mut EventLoop) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        loop {
            match conn.read_incoming() {
                Ok(ReadStatus::Eof) => {
                    if conn.write_queue().is_empty() {
                        self.connections.remove_and_deregister(token, event_loop);
                        return false;
                    }
                    // Half-close: client closed its write half, but queued output remains to flush.
                    // Sync interest: desired_flags() automatically drops READABLE and keeps WRITABLE.
                    let _ = self.connections.sync_interest(token, event_loop);
                    self.handle_client_write(token, event_loop);
                    return false;
                }
                Ok(ReadStatus::WouldBlock) => break,
                Ok(ReadStatus::Ok(n)) => {
                    // Echo received bytes back to client.
                    let data = conn.read_buffer().data().to_vec();
                    if let Err(err) = conn.send_chunk(data) {
                        eprintln!("Failed to enqueue echo payload for client {token}: {err}");
                        self.connections.remove_and_deregister(token, event_loop);
                        return false;
                    }
                    conn.read_buffer_mut().consume(n);
                }
                Err(err) => {
                    // Per-connection I/O error: close and deregister only this connection.
                    eprintln!("I/O read error on client {token}: {err}");
                    self.connections.remove_and_deregister(token, event_loop);
                    return false;
                }
            }
        }

        // Try an immediate write flush of newly queued bytes.
        self.handle_client_write(token, event_loop)
    }

    /// Flushes queued outgoing bytes into the client socket transmit buffer.
    fn handle_client_write(&mut self, token: Token, event_loop: &mut EventLoop) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        match conn.flush_outgoing() {
            Ok(WriteStatus::Complete) => {
                if conn.is_closed() {
                    self.connections.remove_and_deregister(token, event_loop);
                    return false;
                }
            }
            Ok(WriteStatus::Pending(_)) => {}
            Err(_) => {
                self.connections.remove_and_deregister(token, event_loop);
                return false;
            }
        }

        // Synchronize epoll interest flags: automatically toggles WRITABLE and applies backpressure!
        let _ = self.connections.sync_interest(token, event_loop);
        true
    }

    /// Removes a client connection and deregisters its descriptor from the event loop.
    fn remove_client(&mut self, token: Token, event_loop: &mut EventLoop) {
        self.connections.remove_and_deregister(token, event_loop);
    }
}

fn is_emfile_or_enfile(err: &io::Error) -> bool {
    #[cfg(unix)]
    {
        if let Some(code) = err.raw_os_error() {
            return code == libc::EMFILE || code == libc::ENFILE;
        }
    }
    #[cfg(windows)]
    {
        if let Some(code) = err.raw_os_error() {
            return code == 10024; // WSAEMFILE
        }
    }
    false
}

impl Handler for EchoServerHandler {
    fn on_event(
        &mut self,
        event_loop: &mut EventLoop,
        token: Token,
        flags: EpollFlags,
    ) -> io::Result<()> {
        if token == LISTENER_TOKEN {
            self.handle_accept(event_loop);
        } else {
            // Immediate error without data readiness: close immediately.
            if flags.contains(EpollFlags::ERROR) && !flags.contains(EpollFlags::READABLE) {
                self.remove_client(token, event_loop);
                return Ok(());
            }

            // If READABLE is set (even if HANGUP is also set), read and process pending data first!
            if flags.contains(EpollFlags::READABLE) {
                let _ = self.handle_client_read(token, event_loop);
            }

            if flags.contains(EpollFlags::WRITABLE) {
                let _ = self.handle_client_write(token, event_loop);
            }

            // If peer hung up (or errored) and connection has drained all bytes or closed, remove.
            if flags.contains(EpollFlags::HANGUP) || flags.contains(EpollFlags::ERROR) {
                let should_remove = match self.connections.get(token) {
                    Some(c) => c.is_closed() || c.write_queue().is_empty(),
                    None => false,
                };
                if should_remove {
                    self.remove_client(token, event_loop);
                }
            }
        }
        Ok(())
    }

    fn on_timeout(&mut self, event_loop: &mut EventLoop, timer_id: TimerId) -> io::Result<()> {
        if timer_id == IDLE_SWEEP_TIMER_ID && !self.is_shutting_down {
            if let Some(idle_timeout) = self.idle_timeout {
                self.connections.sweep_idle(idle_timeout, event_loop);
            }
            event_loop.schedule_timer(self.sweep_interval, IDLE_SWEEP_TIMER_ID);
        }
        Ok(())
    }
}

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
    state: EchoServerHandler,
    event_loop: EventLoop,
    shutdown_timeout: Duration,
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
            state: EchoServerHandler {
                listener,
                connections: ConnectionTable::with_max_connections(max_connections),
                idle_timeout,
                sweep_interval,
                is_shutting_down: false,
                rejected_connections: 0,
            },
            event_loop,
            shutdown_timeout,
        })
    }

    /// Returns the number of connections that were rejected due to capacity limits.
    #[must_use]
    pub fn rejected_connections(&self) -> usize {
        self.state.rejected_connections
    }

    /// Returns `true` if the server is in the process of shutting down.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.state.is_shutting_down
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
        if self.state.is_shutting_down {
            return Ok(());
        }
        self.state.is_shutting_down = true;

        // 1. Deregister the listener so no further connections are accepted.
        let _ = self.event_loop.deregister(as_raw_fd(&self.state.listener));

        // 2. Initiate shutdown on active connections (closes idle ones, keeps WRITABLE for pending data).
        self.state
            .connections
            .initiate_shutdown(&mut self.event_loop);

        // 3. Drain loop: flush pending write queues.
        let drain_deadline = std::time::Instant::now() + self.shutdown_timeout;

        while !self.state.connections.is_empty() && std::time::Instant::now() < drain_deadline {
            let remaining = drain_deadline.saturating_duration_since(std::time::Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(50));
            let _ = self
                .event_loop
                .drain_once(&mut self.state, Some(poll_timeout))?;
        }

        // 4. Force-close any lingering connections that exceeded the shutdown deadline.
        if !self.state.connections.is_empty() {
            let forced = self.state.connections.force_close_all(&mut self.event_loop);
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
        let res = self.event_loop.run(&mut self.state);
        res?;

        self.initiate_graceful_shutdown()
    }

    /// Runs the server for a limited number of ticks (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if any polling tick fails.
    pub fn run_ticks(&mut self, max_ticks: usize, timeout: Option<Duration>) -> io::Result<()> {
        for _ in 0..max_ticks {
            self.event_loop.run_once(&mut self.state, timeout)?;
        }
        Ok(())
    }

    /// Sets the maximum read buffer capacity limit for newly accepted client connections.
    pub fn set_max_read_capacity(&mut self, capacity: usize) {
        self.state.connections.set_max_read_capacity(capacity);
    }

    /// Removes a client connection and deregisters its descriptor from the event loop.
    pub fn remove_client(&mut self, token: Token) {
        self.state
            .connections
            .remove_and_deregister(token, &mut self.event_loop);
    }

    /// Returns the number of currently connected active clients.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.state.connections.len()
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

#[cfg(all(test, target_os = "linux"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

    #[test]
    fn test_client_reset_mid_session_does_not_crash_server() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let mut server = NonBlockingEchoServer::new(listener)?;

        // Connect client 1.
        let mut client1 = TcpStream::connect(addr)?;
        for _ in 0..50 {
            server.state.handle_accept(&mut server.event_loop);
            if server.client_count() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(server.client_count(), 1);

        // Connect client 2.
        let client2 = TcpStream::connect(addr)?;
        for _ in 0..50 {
            server.state.handle_accept(&mut server.event_loop);
            if server.client_count() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(server.client_count(), 2);

        // Client 1 sends data and is echoed.
        client1.write_all(b"hello")?;
        let token1 = 1; // first accepted connection token
        let res1 = server
            .state
            .on_event(&mut server.event_loop, token1, EpollFlags::READABLE);
        assert!(res1.is_ok(), "client 1 read should succeed");

        // Client 2 sets linger to 0 and drops immediately (TCP RST).
        cachedb::net::sys::set_linger_zero(cachedb::net::sys::as_raw_fd(&client2))?;
        drop(client2);

        // Wait briefly for OS to deliver RST packet to local loopback socket.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // When event loop processes client 2 read event:
        let token2 = 2; // second accepted connection token
        let res2 = server
            .state
            .on_event(&mut server.event_loop, token2, EpollFlags::READABLE);

        // FAILING CHECK ON UNPATCHED CODE:
        // On unpatched code, res2 is Err (ConnectionReset), which killed the server!
        // The regression test asserts that res2 is Ok(()) and client 2 is removed, while client 1 remains!
        assert!(
            res2.is_ok(),
            "per-connection reset error MUST NOT propagate error out of handler: got {res2:?}"
        );
        assert_eq!(
            server.client_count(),
            1,
            "failed client should be removed from connection table"
        );
        assert!(
            server.state.connections.get(token1).is_some(),
            "healthy client 1 must remain active"
        );

        Ok(())
    }

    #[test]
    fn test_client_buffer_overflow_disconnects_client_only() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let mut server = NonBlockingEchoServer::new(listener)?;
        // Set a small read buffer capacity limit (e.g. 64 bytes).
        server.set_max_read_capacity(64);

        // Connect client 1 and client 2.
        let client1 = TcpStream::connect(addr)?;
        for _ in 0..50 {
            server.state.handle_accept(&mut server.event_loop);
            if server.client_count() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _client2 = TcpStream::connect(addr)?;
        for _ in 0..50 {
            server.state.handle_accept(&mut server.event_loop);
            if server.client_count() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert_eq!(server.client_count(), 2);
        let token1 = 1;
        let token2 = 2;

        // Simulate an unconsumed oversized frame filling the read buffer to its limit (64 bytes):
        {
            let conn2 = server.state.connections.get_mut(token2).unwrap();
            conn2
                .read_buffer_mut()
                .read_from(&mut std::io::Cursor::new(vec![b'x'; 64]))?;
        }

        let res2 = server
            .state
            .on_event(&mut server.event_loop, token2, EpollFlags::READABLE);

        // Buffer overflow MUST NOT return error from handler: client disconnected cleanly
        assert!(
            res2.is_ok(),
            "buffer overflow MUST NOT return error from handler: got {res2:?}"
        );
        assert_eq!(server.client_count(), 1);
        assert!(server.state.connections.get(token1).is_some());

        drop(client1);
        Ok(())
    }

    #[test]
    fn test_half_close_preserves_queued_output() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let mut server = NonBlockingEchoServer::new(listener)?;

        let mut client = TcpStream::connect(addr)?;
        client.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        client.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;

        for _ in 0..50 {
            server.state.handle_accept(&mut server.event_loop);
            if server.client_count() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(server.client_count(), 1);
        let token = 1;

        // Send 512 KB payload (larger than socket buffers).
        let payload = vec![0x42u8; 512 * 1024];
        let payload_len = payload.len();

        // Spawn client reader in background thread so socket buffer never deadlocks.
        let mut client_clone = client.try_clone()?;
        let reader_handle = std::thread::spawn(move || -> io::Result<Vec<u8>> {
            let mut received = Vec::with_capacity(payload_len);
            let mut buf = [0u8; 16384];
            loop {
                match std::io::Read::read(&mut client_clone, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => received.extend_from_slice(&buf[..n]),
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(err) => return Err(err),
                }
            }
            Ok(received)
        });

        // Write payload to server.
        client.write_all(&payload)?;

        // Client initiates half-close (EOF to server).
        client.shutdown(std::net::Shutdown::Write)?;

        // Drive server ticks until all incoming data is read, queued, and completely flushed.
        for _ in 0..200 {
            let _ = server
                .state
                .on_event(&mut server.event_loop, token, EpollFlags::READABLE);
            let _ = server
                .state
                .on_event(&mut server.event_loop, token, EpollFlags::WRITABLE);
            if server.client_count() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let received = reader_handle
            .join()
            .map_err(|_| io::Error::other("reader thread panicked"))??;

        assert_eq!(
            received.len(),
            payload.len(),
            "client must receive all echoed bytes after half-close"
        );
        assert_eq!(received, payload);

        Ok(())
    }
}

#[cfg(all(test, not(target_os = "linux")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod non_linux_tests {
    use super::*;

    #[test]
    fn test_nonblocking_echo_server_unsupported_on_non_linux() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let server = NonBlockingEchoServer::new(listener);
        assert!(server.is_err());
        assert_eq!(server.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }
}
