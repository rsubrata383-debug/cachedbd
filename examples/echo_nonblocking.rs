//! Educational non-blocking TCP echo server for Phase 1, Week 3, step 4.
//!
//! This example demonstrates event-driven networking using [`cachedb::net::event_loop::EventLoop`].
//! It shows how a single thread can concurrently handle multiple connected clients without
//! blocking, managing partial reads, partial writes, and `WouldBlock` (`EAGAIN`) conditions.

use cachedb::net::event_loop::{EventLoop, Handler, Token};
use cachedb::net::sys::{EpollFlags, as_raw_fd};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Reserved token identifying readiness events on the listening server socket.
const LISTENER_TOKEN: Token = 0;

/// Base token assigned to accepted remote client connections.
const FIRST_CLIENT_TOKEN: Token = 1;

/// Represents an active, non-blocking client connection and its pending outgoing buffer.
#[derive(Debug)]
pub struct ClientConnection {
    /// Non-blocking TCP socket stream.
    pub stream: TcpStream,
    /// Outgoing byte queue holding echoed data waiting to be transmitted.
    pub send_queue: VecDeque<u8>,
}

/// An event-driven, non-blocking TCP echo server.
///
/// # Core Concepts Explained
///
/// - **`set_nonblocking(true)`**: In non-blocking mode, operations that would otherwise
///   sleep the calling thread return immediately with an error of kind [`io::ErrorKind::WouldBlock`]
///   (`EAGAIN` in POSIX).
/// - **Accept Loop**: When the listener is marked `READABLE`, multiple incoming connections
///   might be queued in the kernel backlog. We must accept in a loop until `WouldBlock`.
/// - **Partial Writes**: Sockets have bounded kernel transmit buffers. If an application writes
///   more data than the kernel buffer can fit, `write` writes as many bytes as possible and
///   returns the count, or returns `WouldBlock`. The remaining bytes must be queued in user space
///   (`send_queue`) until the socket becomes `WRITABLE`.
/// - **Dynamic Interest Registration**: Level-triggered `epoll` reports `WRITABLE` continuously
///   as long as the socket send buffer has free space. If an idle client with an empty send queue
///   listens for `WRITABLE`, `epoll_wait` returns immediately with 0 delay, resulting in 100% CPU
///   busy-polling. Therefore, we monitor `WRITABLE` *only* when `send_queue` is non-empty!
#[derive(Debug)]
pub struct NonBlockingEchoServer {
    listener: TcpListener,
    clients: HashMap<Token, ClientConnection>,
    next_token: Token,
    event_loop: EventLoop,
}

impl NonBlockingEchoServer {
    /// Creates a new non-blocking echo server instance and registers the listener.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if setting the listener to non-blocking mode or registering
    /// with the event loop fails.
    pub fn new(listener: TcpListener) -> io::Result<Self> {
        listener.set_nonblocking(true)?;

        let event_loop = EventLoop::new()?;
        event_loop.register(as_raw_fd(&listener), LISTENER_TOKEN, EpollFlags::READABLE)?;

        Ok(Self {
            listener,
            clients: HashMap::new(),
            next_token: FIRST_CLIENT_TOKEN,
            event_loop,
        })
    }

    /// Runs the server event loop continuously until stopped.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if event loop execution encounters a fatal failure.
    pub fn run(&mut self) -> io::Result<()> {
        let mut loop_ref = std::mem::replace(&mut self.event_loop, EventLoop::new()?);
        let res = loop_ref.run(self);
        self.event_loop = loop_ref;
        res
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
        loop {
            match self.listener.accept() {
                Ok((stream, _peer_addr)) => {
                    stream.set_nonblocking(true)?;

                    let token = self.next_token;
                    self.next_token += 1;

                    // Register interest in readable events for this new client.
                    let fd = as_raw_fd(&stream);
                    if let Err(err) = self.event_loop.register(
                        fd,
                        token,
                        EpollFlags::READABLE | EpollFlags::HANGUP,
                    ) {
                        eprintln!("Failed to register client {token}: {err}");
                        continue;
                    }

                    self.clients.insert(
                        token,
                        ClientConnection {
                            stream,
                            send_queue: VecDeque::new(),
                        },
                    );
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                    // No more pending connections in the backlog queue.
                    break;
                }
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {
                    // Interrupted by signal; retry accept.
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Reads incoming bytes from a client socket until `WouldBlock` or EOF (`Ok(0)`).
    fn handle_client_read(&mut self, token: Token) -> io::Result<bool> {
        let Some(client) = self.clients.get_mut(&token) else {
            return Ok(false);
        };

        let mut buf = [0u8; 1024];
        let mut client_closed = false;

        loop {
            match client.stream.read(&mut buf) {
                Ok(0) => {
                    // Peer performed an orderly shutdown (EOF).
                    client_closed = true;
                    break;
                }
                Ok(n) => {
                    // Append incoming chunk into the echo queue.
                    client.send_queue.extend(&buf[..n]);
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                    // Drained available receive buffer bytes for now.
                    break;
                }
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(_) => {
                    // Connection reset or unrecoverable read failure.
                    client_closed = true;
                    break;
                }
            }
        }

        if client_closed {
            self.remove_client(token);
            return Ok(false);
        }

        // Try an immediate write flush of newly queued bytes.
        self.handle_client_write(token)
    }

    /// Flushes queued outgoing bytes into the client socket transmit buffer.
    fn handle_client_write(&mut self, token: Token) -> io::Result<bool> {
        let Some(client) = self.clients.get_mut(&token) else {
            return Ok(false);
        };

        let mut write_failed = false;

        while !client.send_queue.is_empty() {
            let (first_slice, _) = client.send_queue.as_slices();
            match client.stream.write(first_slice) {
                Ok(0) => {
                    // Unable to make progress; buffer full.
                    break;
                }
                Ok(n) => {
                    client.send_queue.drain(..n);
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                    // Kernel socket transmit buffer is full for now.
                    break;
                }
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(_) => {
                    // Write error (broken pipe, connection reset).
                    write_failed = true;
                    break;
                }
            }
        }

        if write_failed {
            self.remove_client(token);
            return Ok(false);
        }

        // Dynamically adjust epoll interest flags:
        // - If send_queue has remaining bytes, we MUST monitor WRITABLE to finish flushing.
        // - If send_queue is empty, we MUST NOT monitor WRITABLE (prevents busy loops).
        if let Some(client) = self.clients.get(&token) {
            let fd = as_raw_fd(&client.stream);
            let flags = if client.send_queue.is_empty() {
                EpollFlags::READABLE | EpollFlags::HANGUP
            } else {
                EpollFlags::READABLE | EpollFlags::WRITABLE | EpollFlags::HANGUP
            };

            let _ = self.event_loop.reregister(fd, token, flags);
        }

        Ok(true)
    }

    /// Removes a client connection and deregisters its descriptor from the event loop.
    pub fn remove_client(&mut self, token: Token) {
        if let Some(client) = self.clients.remove(&token) {
            let fd = as_raw_fd(&client.stream);
            let _ = self.event_loop.deregister(fd);
        }
    }

    /// Returns the number of currently connected active clients.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.clients.len()
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
                let _ = self.handle_client_write(token)?;
            }
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
