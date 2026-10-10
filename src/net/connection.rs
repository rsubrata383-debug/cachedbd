//! Client connection state and lifecycle management.
//!
//! This module provides [`Connection`], which encapsulates an active client socket,
//! its inbound [`ReadBuffer`], outbound [`WriteQueue`], activity timestamps,
//! and interest flag state for the event loop.

use crate::net::buffer::{
    DEFAULT_MAX_READ_CAPACITY, DEFAULT_WRITE_HIGH_WATERMARK, ReadBuffer, WriteQueue,
};
use crate::net::event_loop::{EventLoop, Token};
use crate::net::sys::{EpollFlags, RawFd, as_raw_fd};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Result of an incoming read operation on a non-blocking connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    /// Read `count` bytes into the read buffer.
    Ok(usize),
    /// Socket is non-blocking and no bytes were available to read (`WouldBlock`).
    WouldBlock,
    /// Remote peer disconnected cleanly (EOF / 0 bytes returned).
    Eof,
}

/// Result of an outgoing write flush on a non-blocking connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStatus {
    /// The entire outgoing write queue has been flushed to the socket.
    Complete,
    /// Pending bytes remain queued because the socket send buffer is full (`WouldBlock`).
    Pending(usize),
}

/// Represents an active network client connection.
///
/// Encapsulates the stream, inbound streaming read buffer, outbound FIFO write queue,
/// dynamic epoll interest flags, and monotonic activity timestamps for idle timeout tracking.
#[derive(Debug)]
pub struct Connection<S = TcpStream> {
    id: Token,
    stream: S,
    peer_addr: SocketAddr,
    read_buf: ReadBuffer,
    write_queue: WriteQueue,
    is_closing: bool,
    is_closed: bool,
    last_active: Instant,
}

impl<S> Connection<S> {
    /// Creates a new connection with default buffer limits.
    #[must_use]
    pub fn new(id: Token, stream: S, peer_addr: SocketAddr) -> Self {
        Self::with_limits(
            id,
            stream,
            peer_addr,
            DEFAULT_MAX_READ_CAPACITY,
            DEFAULT_WRITE_HIGH_WATERMARK,
        )
    }

    /// Creates a new connection with explicit buffer capacity limits.
    #[must_use]
    pub fn with_limits(
        id: Token,
        stream: S,
        peer_addr: SocketAddr,
        max_read_capacity: usize,
        write_high_watermark: usize,
    ) -> Self {
        Self {
            id,
            stream,
            peer_addr,
            read_buf: ReadBuffer::with_capacities(4096, max_read_capacity),
            write_queue: WriteQueue::with_high_watermark(write_high_watermark),
            is_closing: false,
            is_closed: false,
            last_active: Instant::now(),
        }
    }

    /// Creates a new connection with explicit read and write capacity and watermark limits.
    #[must_use]
    pub fn with_custom_limits(
        id: Token,
        stream: S,
        peer_addr: SocketAddr,
        max_read_capacity: usize,
        write_high_watermark: usize,
        write_low_watermark: usize,
        max_write_capacity: usize,
    ) -> Self {
        Self {
            id,
            stream,
            peer_addr,
            read_buf: ReadBuffer::with_capacities(4096, max_read_capacity),
            write_queue: WriteQueue::with_limits(
                write_high_watermark,
                write_low_watermark,
                max_write_capacity,
            ),
            is_closing: false,
            is_closed: false,
            last_active: Instant::now(),
        }
    }

    /// Returns `true` if backpressure is currently active on this connection.
    #[must_use]
    pub fn is_backpressured(&self) -> bool {
        self.write_queue.is_backpressured()
    }

    /// Returns the unique token identifying this connection in the event loop.
    #[must_use]
    pub fn id(&self) -> Token {
        self.id
    }

    /// Returns the socket address of the remote client peer.
    #[must_use]
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Returns a reference to the underlying network stream.
    #[must_use]
    pub fn stream(&self) -> &S {
        &self.stream
    }

    /// Returns a mutable reference to the underlying network stream.
    pub fn stream_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Returns a reference to the inbound read buffer.
    #[must_use]
    pub fn read_buffer(&self) -> &ReadBuffer {
        &self.read_buf
    }

    /// Returns a mutable reference to the inbound read buffer.
    pub fn read_buffer_mut(&mut self) -> &mut ReadBuffer {
        &mut self.read_buf
    }

    /// Returns a reference to the outbound write queue.
    #[must_use]
    pub fn write_queue(&self) -> &WriteQueue {
        &self.write_queue
    }

    /// Returns a mutable reference to the outbound write queue.
    pub fn write_queue_mut(&mut self) -> &mut WriteQueue {
        &mut self.write_queue
    }

    /// Returns `true` if the connection has been scheduled to close.
    #[must_use]
    pub fn is_closing(&self) -> bool {
        self.is_closing
    }

    /// Returns `true` if the connection is completely closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.is_closed
    }

    /// Returns the timestamp of the last successful read or write activity.
    #[must_use]
    pub fn last_active(&self) -> Instant {
        self.last_active
    }

    /// Returns the elapsed duration since the last read or write activity.
    #[must_use]
    pub fn idle_duration(&self) -> Duration {
        self.last_active.elapsed()
    }

    /// Returns `true` if the connection has been idle for at least `timeout`.
    #[must_use]
    pub fn is_idle(&self, timeout: Duration) -> bool {
        self.idle_duration() >= timeout
    }

    /// Resets the connection activity timestamp to the current instant.
    pub fn touch(&mut self) {
        self.last_active = Instant::now();
    }

    /// Sets the timestamp of the last activity on the connection.
    pub fn set_last_active(&mut self, instant: Instant) {
        self.last_active = instant;
    }

    /// Initiates connection closing.
    ///
    /// If there are pending outgoing writes, `is_closing` is set to `true` and the connection
    /// will be marked `is_closed` once the write queue is completely drained. If the write
    /// queue is already empty, the connection is closed immediately.
    pub fn close(&mut self) {
        self.is_closing = true;
        if self.write_queue.is_empty() {
            self.is_closed = true;
        }
    }

    /// Computes the desired [`EpollFlags`] interest for this connection.
    ///
    /// - Excludes `READABLE` if the connection is closing, closed, or backpressured.
    /// - Includes `READABLE` during normal operation.
    /// - Includes `WRITABLE` whenever the write queue has pending data.
    #[must_use]
    pub fn desired_flags(&self) -> EpollFlags {
        if self.is_closed {
            return EpollFlags::empty();
        }

        let mut flags = EpollFlags::empty();

        // Read interest: only when not closing and not under backpressure.
        if !self.is_closing && !self.write_queue.is_backpressured() {
            flags |= EpollFlags::READABLE;
        }

        // Write interest: only when there are pending bytes to flush.
        if !self.write_queue.is_empty() {
            flags |= EpollFlags::WRITABLE;
        }

        flags
    }

    /// Returns the total buffer memory in bytes allocated for this connection's read buffer and write queue.
    #[must_use]
    pub fn allocated_buffer_memory(&self) -> usize {
        self.read_buf.capacity() + self.write_queue.len()
    }
}

impl<S: Read> Connection<S> {
    /// Reads available incoming bytes from the stream into the read buffer.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if an unexpected I/O error occurs or the read buffer
    /// capacity exceeds the maximum limit.
    pub fn read_incoming(&mut self) -> io::Result<ReadStatus> {
        if self.is_closing || self.is_closed {
            return Ok(ReadStatus::Eof);
        }

        match self.read_buf.read_from(&mut self.stream) {
            Ok(0) => {
                // Clean EOF from remote peer.
                self.is_closed = true;
                Ok(ReadStatus::Eof)
            }
            Ok(n) => {
                self.last_active = Instant::now();
                Ok(ReadStatus::Ok(n))
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(ReadStatus::WouldBlock),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => Ok(ReadStatus::WouldBlock),
            Err(err) => Err(err),
        }
    }
}

impl<S: Write> Connection<S> {
    /// Flushes queued outgoing bytes from the write queue to the stream.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if writing to the underlying stream fails.
    pub fn flush_outgoing(&mut self) -> io::Result<WriteStatus> {
        if self.write_queue.is_empty() {
            if self.is_closing {
                self.is_closed = true;
            }
            return Ok(WriteStatus::Complete);
        }

        let res = self.write_queue.write_to(&mut self.stream);
        self.last_active = Instant::now();

        match res {
            Ok(_) => {
                if self.write_queue.is_empty() {
                    if self.is_closing {
                        self.is_closed = true;
                    }
                    Ok(WriteStatus::Complete)
                } else {
                    Ok(WriteStatus::Pending(self.write_queue.len()))
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                Ok(WriteStatus::Pending(self.write_queue.len()))
            }
            Err(err) => Err(err),
        }
    }

    /// Queues a slice of bytes to be sent to the client.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the write queue capacity ceiling is exceeded.
    pub fn send_slice(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_queue.enqueue(data)
    }

    /// Queues an owned byte vector to be sent to the client with zero copying.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the write queue capacity ceiling is exceeded.
    pub fn send_chunk(&mut self, chunk: Vec<u8>) -> io::Result<()> {
        self.write_queue.enqueue_chunk(chunk)
    }
}

impl Connection<TcpStream> {
    /// Returns the underlying raw OS file descriptor for event loop registration.
    #[must_use]
    pub fn raw_fd(&self) -> RawFd {
        as_raw_fd(&self.stream)
    }
}

/// Default maximum number of concurrent client connections (10,000).
pub const DEFAULT_MAX_CONNECTIONS: usize = 10_000;

/// Connection table tracking all active client connections on the server.
///
/// Encapsulates connection insertion, unique [`Token`] ID assignment, capacity limit
/// enforcement, and synchronizes interest flags with the [`EventLoop`].
#[derive(Debug)]
pub struct ConnectionTable<S = TcpStream> {
    connections: HashMap<Token, Connection<S>>,
    next_token: Token,
    max_connections: usize,
}

impl<S> Default for ConnectionTable<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> ConnectionTable<S> {
    /// Creates a new connection table with the default capacity limit (10,000).
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_connections(DEFAULT_MAX_CONNECTIONS)
    }

    /// Creates a new connection table with an explicit maximum connection limit.
    #[must_use]
    pub fn with_max_connections(max_connections: usize) -> Self {
        Self {
            connections: HashMap::new(),
            next_token: 1, // 0 is reserved for LISTENER_TOKEN
            max_connections,
        }
    }

    /// Inserts a new connection, assigning it a unique token.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] with [`io::ErrorKind::ConnectionRefused`] if the maximum
    /// connection limit has been reached.
    pub fn insert(&mut self, stream: S, peer_addr: SocketAddr) -> io::Result<Token> {
        if self.connections.len() >= self.max_connections {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "maximum concurrent connection limit reached",
            ));
        }

        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        if self.next_token == 0 {
            self.next_token = 1;
        }

        let connection = Connection::new(token, stream, peer_addr);
        self.connections.insert(token, connection);
        Ok(token)
    }

    /// Inserts a pre-constructed connection into the table.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] with [`io::ErrorKind::ConnectionRefused`] if the limit is reached.
    pub fn insert_connection(&mut self, connection: Connection<S>) -> io::Result<Token> {
        if self.connections.len() >= self.max_connections {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "maximum concurrent connection limit reached",
            ));
        }

        let token = connection.id();
        self.connections.insert(token, connection);
        Ok(token)
    }

    /// Returns a reference to the connection associated with `token`, if it exists.
    #[must_use]
    pub fn get(&self, token: Token) -> Option<&Connection<S>> {
        self.connections.get(&token)
    }

    /// Returns a mutable reference to the connection associated with `token`, if it exists.
    pub fn get_mut(&mut self, token: Token) -> Option<&mut Connection<S>> {
        self.connections.get_mut(&token)
    }

    /// Removes and returns the connection associated with `token`.
    pub fn remove(&mut self, token: Token) -> Option<Connection<S>> {
        self.connections.remove(&token)
    }

    /// Returns the number of currently active connections.
    #[must_use]
    pub fn len(&self) -> usize {
        self.connections.len()
    }

    /// Returns `true` if there are no active connections.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Returns the configured maximum connection capacity limit.
    #[must_use]
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Returns `true` if the connection table has reached its maximum connection limit.
    #[must_use]
    pub fn is_at_capacity(&self) -> bool {
        self.connections.len() >= self.max_connections
    }

    /// Returns the number of remaining connection slots before the limit is reached.
    #[must_use]
    pub fn remaining_capacity(&self) -> usize {
        self.max_connections.saturating_sub(self.connections.len())
    }

    /// Updates the maximum concurrent connection capacity limit.
    pub fn set_max_connections(&mut self, limit: usize) {
        self.max_connections = limit;
    }

    /// Returns tokens of all active connections that have been idle for at least `timeout`.
    #[must_use]
    pub fn idle_tokens(&self, timeout: Duration) -> Vec<Token> {
        self.connections
            .iter()
            .filter_map(|(&token, conn)| {
                if conn.is_idle(timeout) {
                    Some(token)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Sweeps idle connections by closing them.
    ///
    /// For connections with an empty write queue, they are removed from the table immediately.
    /// For connections with pending writes, they are marked closing so they can flush.
    /// If an already-closing connection remains idle beyond `timeout`, it is forcefully removed.
    ///
    /// Returns the tokens of all connections that were closed and removed.
    pub fn sweep_idle_connections(&mut self, timeout: Duration) -> Vec<Token> {
        let idle = self.idle_tokens(timeout);
        let mut removed = Vec::new();
        for token in idle {
            if let Some(conn) = self.connections.get_mut(&token) {
                if conn.is_closing() {
                    self.connections.remove(&token);
                    removed.push(token);
                } else {
                    conn.close();
                    if conn.is_closed() {
                        self.connections.remove(&token);
                        removed.push(token);
                    }
                }
            }
        }
        removed
    }

    /// Returns `true` if a connection with the specified token exists.
    #[must_use]
    pub fn contains(&self, token: Token) -> bool {
        self.connections.contains_key(&token)
    }

    /// Returns an iterator over all active connection pairs `(&Token, &Connection<S>)`.
    pub fn iter(&self) -> impl Iterator<Item = (&Token, &Connection<S>)> {
        self.connections.iter()
    }

    /// Returns a mutable iterator over all active connection pairs `(&Token, &mut Connection<S>)`.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&Token, &mut Connection<S>)> {
        self.connections.iter_mut()
    }

    /// Returns the total read buffer capacity in bytes allocated across all active connections.
    #[must_use]
    pub fn total_read_buffer_capacity(&self) -> usize {
        self.connections
            .values()
            .map(|conn| conn.read_buffer().capacity())
            .sum()
    }

    /// Returns the total allocated buffer memory in bytes across all connections in the table.
    #[must_use]
    pub fn total_allocated_buffer_memory(&self) -> usize {
        self.connections
            .values()
            .map(Connection::allocated_buffer_memory)
            .sum()
    }
}

impl ConnectionTable<TcpStream> {
    /// Registers a newly accepted connection with the event loop using its desired flags.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the token is not found or epoll registration fails.
    pub fn register_with_loop(&self, token: Token, event_loop: &mut EventLoop) -> io::Result<()> {
        let conn = self
            .get(token)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "connection token not found"))?;

        event_loop.register(conn.raw_fd(), token, conn.desired_flags())
    }

    /// Synchronizes a connection's current desired interest flags with the event loop.
    ///
    /// Automatically applies backpressure toggling:
    /// - If the write queue is saturated, `READABLE` is removed from epoll.
    /// - If the write queue has space, `READABLE` is restored.
    /// - If the write queue has bytes to send, `WRITABLE` is registered.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the token is not found or epoll modification fails.
    pub fn sync_interest(&self, token: Token, event_loop: &mut EventLoop) -> io::Result<()> {
        let conn = self
            .get(token)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "connection token not found"))?;

        if conn.is_closed() {
            let _ = event_loop.deregister(conn.raw_fd());
            return Ok(());
        }

        event_loop.reregister(conn.raw_fd(), token, conn.desired_flags())
    }

    /// Removes a connection from the table and deregisters its descriptor from the event loop.
    pub fn remove_and_deregister(
        &mut self,
        token: Token,
        event_loop: &mut EventLoop,
    ) -> Option<Connection<TcpStream>> {
        if let Some(conn) = self.connections.remove(&token) {
            let _ = event_loop.deregister(conn.raw_fd());
            Some(conn)
        } else {
            None
        }
    }

    /// Sweeps idle connections, closing them gracefully and deregistering them from the event loop.
    ///
    /// Connections with pending writes are marked closing and synced with the event loop so
    /// `READABLE` is removed and `WRITABLE` is monitored until flushed.
    ///
    /// Returns the number of connections closed.
    pub fn sweep_idle(&mut self, timeout: Duration, event_loop: &mut EventLoop) -> usize {
        let idle = self.idle_tokens(timeout);
        let mut closed_count = 0;
        for token in idle {
            if let Some(conn) = self.connections.get_mut(&token) {
                if conn.is_closing() {
                    // Stalled in closing state beyond idle timeout: force close immediately.
                    if let Some(c) = self.connections.remove(&token) {
                        let _ = event_loop.deregister(c.raw_fd());
                        closed_count += 1;
                    }
                } else {
                    conn.close();
                    if conn.is_closed() {
                        if let Some(c) = self.connections.remove(&token) {
                            let _ = event_loop.deregister(c.raw_fd());
                            closed_count += 1;
                        }
                    } else {
                        // Keep WRITABLE to flush pending writes, drop READABLE.
                        let _ = self.sync_interest(token, event_loop);
                    }
                }
            }
        }
        closed_count
    }

    /// Initiates graceful shutdown across all connections.
    ///
    /// Marks all connections as closing, immediately removes and deregisters connections
    /// with empty write queues, and synchronizes interest for connections that have pending
    /// outgoing writes (monitoring `WRITABLE` only).
    pub fn initiate_shutdown(&mut self, event_loop: &mut EventLoop) {
        let tokens: Vec<Token> = self.connections.keys().copied().collect();
        for token in tokens {
            if let Some(conn) = self.connections.get_mut(&token) {
                conn.close();
                if conn.is_closed() {
                    if let Some(c) = self.connections.remove(&token) {
                        let _ = event_loop.deregister(c.raw_fd());
                    }
                } else {
                    let _ = self.sync_interest(token, event_loop);
                }
            }
        }
    }

    /// Forcefully closes and deregisters all remaining active connections from the event loop.
    ///
    /// Returns the number of connections terminated.
    pub fn force_close_all(&mut self, event_loop: &mut EventLoop) -> usize {
        let count = self.connections.len();
        for (_, conn) in self.connections.drain() {
            let _ = event_loop.deregister(conn.raw_fd());
        }
        count
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::unchecked_time_subtraction,
    clippy::indexing_slicing,
    clippy::doc_markdown
)]
mod tests {
    use super::*;

    struct MockSocket {
        pub incoming: io::Cursor<Vec<u8>>,
        pub outgoing: Vec<u8>,
        pub write_limit: Option<usize>,
    }

    impl MockSocket {
        fn new(input: Vec<u8>) -> Self {
            Self {
                incoming: io::Cursor::new(input),
                outgoing: Vec::new(),
                write_limit: None,
            }
        }

        fn with_write_limit(input: Vec<u8>, limit: usize) -> Self {
            Self {
                incoming: io::Cursor::new(input),
                outgoing: Vec::new(),
                write_limit: Some(limit),
            }
        }
    }

    impl Read for MockSocket {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.incoming.read(buf)
        }
    }

    impl Write for MockSocket {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let max_write = self.write_limit.unwrap_or(buf.len()).min(buf.len());
            if max_write == 0 && self.write_limit.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "mock socket would block",
                ));
            }
            let written = self.outgoing.write(&buf[..max_write])?;
            if let Some(limit) = &mut self.write_limit {
                *limit = limit.saturating_sub(written);
            }
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_connection_read_and_write() -> io::Result<()> {
        let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid address");
        let socket = MockSocket::new(b"ping frame".to_vec());
        let mut conn = Connection::new(1, socket, addr);

        assert_eq!(conn.id(), 1);
        assert_eq!(conn.peer_addr(), addr);
        assert!(!conn.is_closing());
        assert!(!conn.is_closed());

        // Read incoming data.
        let status = conn.read_incoming()?;
        assert_eq!(status, ReadStatus::Ok(10));
        assert_eq!(conn.read_buffer().data(), b"ping frame");

        // Send response.
        conn.send_slice(b"pong response")?;
        assert!(conn.desired_flags().contains(EpollFlags::WRITABLE));

        let write_status = conn.flush_outgoing()?;
        assert_eq!(write_status, WriteStatus::Complete);
        assert_eq!(&conn.stream().outgoing[..], b"pong response");
        assert!(!conn.desired_flags().contains(EpollFlags::WRITABLE));

        Ok(())
    }

    #[test]
    fn test_connection_backpressure_and_flags() -> io::Result<()> {
        let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid address");
        let socket = MockSocket::new(Vec::new());
        let mut conn = Connection::with_limits(2, socket, addr, 1024, 50);

        // Initially: wants READABLE, no WRITABLE.
        assert_eq!(conn.desired_flags(), EpollFlags::READABLE);

        // Enqueue under watermark: wants READABLE | WRITABLE.
        conn.send_slice(&[0u8; 30])?;
        assert_eq!(
            conn.desired_flags(),
            EpollFlags::READABLE | EpollFlags::WRITABLE
        );

        // Enqueue over watermark: backpressure pauses READABLE!
        conn.send_slice(&[0u8; 30])?; // 60 total >= 50
        assert_eq!(conn.desired_flags(), EpollFlags::WRITABLE);
        assert!(!conn.desired_flags().contains(EpollFlags::READABLE));

        // Close connection: desires only WRITABLE until flushed.
        conn.close();
        assert!(conn.is_closing());
        assert!(!conn.is_closed());

        // Flush write queue: becomes completely closed.
        let _ = conn.flush_outgoing()?;
        assert!(conn.is_closed());
        assert_eq!(conn.desired_flags(), EpollFlags::empty());
        Ok(())
    }

    #[test]
    fn test_connection_backpressure_hysteresis() -> io::Result<()> {
        let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid address");
        // Socket allows writing only 30 bytes before WouldBlock.
        let socket = MockSocket::with_write_limit(Vec::new(), 30);
        // High = 50, Low = 20, Max = 100
        let mut conn = Connection::with_custom_limits(3, socket, addr, 1024, 50, 20, 100);

        assert!(!conn.is_backpressured());
        assert_eq!(conn.desired_flags(), EpollFlags::READABLE);

        // Send 40 bytes (< high 50): not backpressured.
        conn.send_slice(&[0u8; 40])?;
        assert!(!conn.is_backpressured());
        assert_eq!(
            conn.desired_flags(),
            EpollFlags::READABLE | EpollFlags::WRITABLE
        );

        // Send 20 bytes (total 60 >= high 50): backpressure engages!
        conn.send_slice(&[0u8; 20])?;
        assert!(conn.is_backpressured());
        assert_eq!(conn.desired_flags(), EpollFlags::WRITABLE);
        assert!(!conn.desired_flags().contains(EpollFlags::READABLE));

        // Attempting to exceed max capacity (100) should return OutOfMemory.
        let res = conn.send_slice(&[0u8; 50]);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::OutOfMemory);

        // Flush partially (30 bytes written, leaving 30 bytes in queue).
        // Since 30 > low 20, hysteresis holds: backpressure remains active!
        let status = conn.flush_outgoing()?;
        assert_eq!(status, WriteStatus::Pending(30));
        assert_eq!(conn.write_queue().len(), 30);
        assert!(conn.is_backpressured());
        assert_eq!(conn.desired_flags(), EpollFlags::WRITABLE);
        assert!(!conn.desired_flags().contains(EpollFlags::READABLE));

        // Increase write limit to allow flushing the remaining 30 bytes.
        conn.stream_mut().write_limit = Some(30);
        let status = conn.flush_outgoing()?;
        assert_eq!(status, WriteStatus::Complete);
        assert_eq!(conn.write_queue().len(), 0);

        // Fully flushed (0 <= low 20): backpressure clears!
        assert!(!conn.is_backpressured());
        assert_eq!(conn.desired_flags(), EpollFlags::READABLE);

        Ok(())
    }

    #[test]
    fn test_connection_table_limits_and_management() -> io::Result<()> {
        let addr1: SocketAddr = "127.0.0.1:8081".parse().expect("valid address");
        let addr2: SocketAddr = "127.0.0.1:8082".parse().expect("valid address");
        let addr3: SocketAddr = "127.0.0.1:8083".parse().expect("valid address");

        let mut table = ConnectionTable::with_max_connections(2);
        assert_eq!(table.len(), 0);
        assert!(table.is_empty());
        assert_eq!(table.max_connections(), 2);

        let t1 = table.insert(MockSocket::new(Vec::new()), addr1)?;
        let t2 = table.insert(MockSocket::new(Vec::new()), addr2)?;
        assert_eq!(table.len(), 2);
        assert!(table.contains(t1));
        assert!(table.contains(t2));

        // Third connection should be rejected with ConnectionRefused.
        let res3 = table.insert(MockSocket::new(Vec::new()), addr3);
        assert!(res3.is_err());
        assert_eq!(res3.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);

        // Remove t1.
        let removed = table.remove(t1);
        assert!(removed.is_some());
        assert_eq!(table.len(), 1);
        assert!(!table.contains(t1));

        // Now inserting another connection should succeed.
        let t3 = table.insert(MockSocket::new(Vec::new()), addr3)?;
        assert_eq!(table.len(), 2);
        assert!(table.contains(t3));

        Ok(())
    }

    #[test]
    fn test_connection_capacity_inspection() -> io::Result<()> {
        let addr1: SocketAddr = "127.0.0.1:8081".parse().expect("valid address");
        let addr2: SocketAddr = "127.0.0.1:8082".parse().expect("valid address");
        let addr3: SocketAddr = "127.0.0.1:8083".parse().expect("valid address");

        let mut table = ConnectionTable::with_max_connections(2);
        assert!(!table.is_at_capacity());
        assert_eq!(table.remaining_capacity(), 2);

        let t1 = table.insert(MockSocket::new(Vec::new()), addr1)?;
        assert!(!table.is_at_capacity());
        assert_eq!(table.remaining_capacity(), 1);

        let _t2 = table.insert(MockSocket::new(Vec::new()), addr2)?;
        assert!(table.is_at_capacity());
        assert_eq!(table.remaining_capacity(), 0);

        // Third connection rejected when at capacity.
        let res = table.insert(MockSocket::new(Vec::new()), addr3);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);

        // Dynamically increase capacity limit.
        table.set_max_connections(3);
        assert!(!table.is_at_capacity());
        assert_eq!(table.remaining_capacity(), 1);

        let t3 = table.insert(MockSocket::new(Vec::new()), addr3)?;
        assert!(table.contains(t1));
        assert!(table.contains(t3));
        assert!(table.is_at_capacity());

        Ok(())
    }

    #[test]
    fn test_connection_idle_timeout_and_sweep() -> io::Result<()> {
        let addr1: SocketAddr = "127.0.0.1:8081".parse().expect("valid address");
        let addr2: SocketAddr = "127.0.0.1:8082".parse().expect("valid address");

        let mut table = ConnectionTable::with_max_connections(10);
        let t1 = table.insert(MockSocket::new(Vec::new()), addr1)?;
        let t2 = table.insert(MockSocket::new(Vec::new()), addr2)?;

        // Simulate inactivity on t1 by setting last_active into the past.
        let timeout = Duration::from_millis(50);
        let past = Instant::now() - Duration::from_millis(100);

        {
            let conn1 = table.get_mut(t1).expect("t1 exists");
            conn1.last_active = past;
            assert!(conn1.is_idle(timeout));
            assert!(conn1.idle_duration() >= timeout);

            // touch() resets last_active.
            conn1.touch();
            assert!(!conn1.is_idle(timeout));

            // Re-age conn1.
            conn1.last_active = past;
        }

        // t2 remains active.
        assert!(!table.get(t2).expect("t2 exists").is_idle(timeout));

        // Idle tokens query.
        let idle = table.idle_tokens(timeout);
        assert_eq!(idle, vec![t1]);

        // Sweep idle connections: t1 has empty write queue so it is immediately closed and removed.
        let removed = table.sweep_idle_connections(timeout);
        assert_eq!(removed, vec![t1]);
        assert!(!table.contains(t1));
        assert!(table.contains(t2));

        // Test idle connection with pending writes.
        let conn2 = table.get_mut(t2).expect("t2 exists");
        conn2.send_slice(b"queued response")?;
        conn2.last_active = past;

        // First sweep: marks conn2 closing, but keeps it alive to allow draining.
        let removed2 = table.sweep_idle_connections(timeout);
        assert!(removed2.is_empty());
        assert!(table.contains(t2));
        assert!(table.get(t2).expect("t2 exists").is_closing());

        // Second sweep while still idle: force closes stalled closing connection.
        let removed3 = table.sweep_idle_connections(timeout);
        assert_eq!(removed3, vec![t2]);
        assert!(!table.contains(t2));

        Ok(())
    }
}
