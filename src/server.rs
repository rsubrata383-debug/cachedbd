//! Server orchestration connecting network I/O, event loop, and command engine.

use crate::cmd::dispatch::{Engine, Session};
use crate::cmd::request::handle_request;
use crate::net::connection::{ConnectionTable, ReadStatus, WriteStatus};
use crate::net::event_loop::{EventLoop, Handler, Token};
use crate::net::sys::EpollFlags;
use crate::proto::decode_frame;
use crate::proto::encode::encode_frame;
use crate::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
use crate::proto::{Decoded, Frame, Value};
use std::collections::HashMap;
use std::io;
use std::net::{TcpListener, TcpStream};

/// Reserved token identifying readiness events on the listening server socket.
pub const LISTENER_TOKEN: Token = 0;

/// Network server driving Cachedb client connections and command execution.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    connections: ConnectionTable<TcpStream>,
    sessions: HashMap<Token, Session>,
    engine: Engine,
    next_client_id: u64,
    rejected_connections: usize,
    migration_errors: usize,
    migration_error_suppressed: bool,
    last_capacity: usize,
    last_len: usize,
}

impl Server {
    /// Creates a new `Server` backed by the provided [`TcpListener`] and default [`Engine`].
    ///
    /// Sets `listener` to non-blocking mode.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if configuring the listener socket fails.
    pub fn new(listener: TcpListener) -> io::Result<Self> {
        Self::with_engine(listener, Engine::new())
    }

    /// Creates a new `Server` with a custom [`Engine`].
    ///
    /// Sets `listener` to non-blocking mode so the accept loop never blocks.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if configuring the listener socket fails.
    pub fn with_engine(listener: TcpListener, engine: Engine) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let initial_cap = engine.store().table().capacity();
        let initial_len = engine.store().len();
        Ok(Self {
            listener,
            connections: ConnectionTable::new(),
            sessions: HashMap::new(),
            engine,
            next_client_id: 1,
            rejected_connections: 0,
            migration_errors: 0,
            migration_error_suppressed: false,
            last_capacity: initial_cap,
            last_len: initial_len,
        })
    }

    /// Returns a reference to the listening server socket.
    #[must_use]
    pub const fn listener(&self) -> &TcpListener {
        &self.listener
    }

    /// Returns a reference to the underlying command execution engine.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Returns a mutable reference to the underlying command execution engine.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Returns the number of currently active client connections.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.connections.len()
    }

    /// Returns a reference to the active client sessions map.
    #[must_use]
    pub const fn sessions(&self) -> &HashMap<Token, Session> {
        &self.sessions
    }

    /// Returns the number of connections rejected due to table capacity limits.
    #[must_use]
    pub const fn rejected_connections(&self) -> usize {
        self.rejected_connections
    }

    /// Returns the cumulative number of failed background migration steps.
    #[must_use]
    pub const fn migration_errors(&self) -> usize {
        self.migration_errors
    }

    /// Steps background migration once (moving up to 16 slots) if resizing.
    ///
    /// Returns `true` if migration is still active after this step.
    /// If migration fails, suppresses further tick migration retries until a new resize
    /// or successful insert occurs.
    pub fn step_migration_tick(&mut self) -> bool {
        let current_cap = self.engine.store().table().capacity();
        let current_len = self.engine.store().len();
        if current_cap != self.last_capacity || current_len > self.last_len {
            self.migration_error_suppressed = false;
            self.last_capacity = current_cap;
            self.last_len = current_len;
        }

        if self.migration_error_suppressed {
            return false;
        }

        if self.engine.store().table().is_resizing() {
            match self.engine.store_mut().step_migration(16) {
                Ok(still_resizing) => {
                    self.last_len = self.engine.store().len();
                    still_resizing
                }
                Err(err) => {
                    self.migration_errors = self.migration_errors.saturating_add(1);
                    if !self.migration_error_suppressed {
                        eprintln!("warning: background migration failed: {err}");
                        self.migration_error_suppressed = true;
                    }
                    false
                }
            }
        } else {
            false
        }
    }

    /// Accepts all pending incoming connections from the kernel backlog.
    pub fn handle_accept(&mut self, event_loop: &mut EventLoop) {
        loop {
            match self.listener.accept() {
                Ok((stream, peer_addr)) => {
                    if stream.set_nonblocking(true).is_err() {
                        drop(stream);
                        continue;
                    }

                    if self.connections.is_at_capacity() {
                        self.rejected_connections = self.rejected_connections.saturating_add(1);
                        drop(stream);
                        continue;
                    }

                    let Ok(token) = self.connections.insert(stream, peer_addr) else {
                        self.rejected_connections = self.rejected_connections.saturating_add(1);
                        continue;
                    };

                    if self
                        .connections
                        .register_with_loop(token, event_loop)
                        .is_err()
                    {
                        self.connections.remove(token);
                        continue;
                    }

                    let client_id = self.next_client_id;
                    self.next_client_id = self.next_client_id.wrapping_add(1);
                    if self.next_client_id == 0 {
                        self.next_client_id = 1;
                    }
                    self.sessions.insert(token, Session::new(client_id));
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }

    /// Reads incoming bytes from a client socket and processes frames via pipelining.
    pub fn handle_client_read(&mut self, token: Token, event_loop: &mut EventLoop) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        let mut read_done = false;
        loop {
            match conn.read_incoming() {
                Ok(ReadStatus::Eof) => {
                    read_done = true;
                    break;
                }
                Ok(ReadStatus::WouldBlock) => break,
                Ok(ReadStatus::Ok(_)) => {}
                Err(_) => {
                    self.remove_client(token, event_loop);
                    return false;
                }
            }
        }

        // Pipelining: decode and dispatch all completed frames in arrival order
        let mut close_after_pipeline = false;
        loop {
            let data = conn.read_buffer().data();
            if data.is_empty() {
                break;
            }

            match decode_frame(data) {
                Ok(Decoded::NeedMore) => break,
                Ok(Decoded::Complete((frame, used_bytes))) => {
                    conn.read_buffer_mut().consume(used_bytes);

                    let session = self.sessions.entry(token).or_insert_with(|| {
                        let cid = self.next_client_id;
                        self.next_client_id = self.next_client_id.wrapping_add(1);
                        Session::new(cid)
                    });

                    let outcome = handle_request(&mut self.engine, session, &frame);

                    let mut out = Vec::new();
                    if encode_frame(&outcome.reply_frame, &mut out).is_ok() {
                        let _ = conn.send_chunk(out);
                    } else {
                        // Fallback on encode failure (e.g. reply exceeds limit): send ERR_INTERNAL_ERROR (7001)
                        let err_header = Header::new(
                            FLAG_RESPONSE | FLAG_ERROR,
                            outcome.reply_frame.header.request_id,
                            outcome.reply_frame.header.opcode,
                            0,
                        );
                        let err_frame =
                            Frame::new(err_header, crate::cmd::ERR_INTERNAL_ERROR.to_value());
                        let mut fallback_out = Vec::new();
                        if encode_frame(&err_frame, &mut fallback_out).is_ok() {
                            let _ = conn.send_chunk(fallback_out);
                        }
                    }

                    if outcome.close {
                        close_after_pipeline = true;
                        break;
                    }
                }
                Err(proto_err) => {
                    // Fatal framing error: reply if header was salvageable, then close
                    match proto_err.code() {
                        1001 | 1002 | 1005 | 1006 | 1007 => {
                            let header =
                                Header::new(FLAG_RESPONSE | FLAG_ERROR, 0, Opcode::Command, 0);
                            let reply_val = Value::Error {
                                code: proto_err.code(),
                                message: proto_err.message().to_string(),
                            };
                            let err_frame = Frame::new(header, reply_val);
                            let mut out = Vec::new();
                            if encode_frame(&err_frame, &mut out).is_ok() {
                                let _ = conn.send_chunk(out);
                            }
                        }
                        _ => {}
                    }
                    close_after_pipeline = true;
                    break;
                }
            }
        }

        if close_after_pipeline {
            conn.close();
        }

        if read_done && conn.write_queue().is_empty() {
            self.remove_client(token, event_loop);
            return false;
        }

        self.handle_client_write(token, event_loop)
    }

    /// Flushes queued outgoing bytes into client socket transmit buffers.
    pub fn handle_client_write(&mut self, token: Token, event_loop: &mut EventLoop) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        match conn.flush_outgoing() {
            Ok(WriteStatus::Complete) => {
                if conn.is_closed() {
                    self.remove_client(token, event_loop);
                    return false;
                }
            }
            Ok(WriteStatus::Pending(_)) => {}
            Err(_) => {
                self.remove_client(token, event_loop);
                return false;
            }
        }

        let _ = self.connections.sync_interest(token, event_loop);
        true
    }

    /// Removes a client connection, deregisters it from epoll, and drops its session.
    pub fn remove_client(&mut self, token: Token, event_loop: &mut EventLoop) {
        self.connections.remove_and_deregister(token, event_loop);
        self.sessions.remove(&token);
    }
}

impl Handler for Server {
    fn on_event(
        &mut self,
        event_loop: &mut EventLoop,
        token: Token,
        flags: EpollFlags,
    ) -> io::Result<()> {
        if token == LISTENER_TOKEN {
            self.handle_accept(event_loop);
            return Ok(());
        }

        if flags.contains(EpollFlags::READABLE) {
            self.handle_client_read(token, event_loop);
        }

        if flags.contains(EpollFlags::WRITABLE) {
            self.handle_client_write(token, event_loop);
        }

        Ok(())
    }

    fn wants_tick(&self) -> bool {
        if self.migration_error_suppressed {
            let current_cap = self.engine.store().table().capacity();
            let current_len = self.engine.store().len();
            if current_cap == self.last_capacity && current_len <= self.last_len {
                return false;
            }
        }
        self.engine.store().table().is_resizing()
    }

    fn on_tick(&mut self, _event_loop: &mut EventLoop) -> io::Result<()> {
        self.step_migration_tick();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
    use crate::proto::{Decoded, Frame, Value};
    use crate::store::hash::hash64;
    use crate::store::table::RobinHoodTable;
    use crate::store::{PutCondition, Store};

    #[test]
    fn test_server_step_migration_tick_suppressed_on_error() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let table = RobinHoodTable::with_max_capacity(8, 0x1234, 16);
        let store = Store::with_table(table);
        let mut engine = Engine::new();
        *engine.store_mut() = store;

        let mut server = Server::with_engine(listener, engine)?;
        assert_eq!(server.migration_errors(), 0);

        // Fill initial table
        for i in 0..7 {
            let k = format!("k-{i}").into_bytes();
            let _ = server
                .engine_mut()
                .store_mut()
                .put(&k, Value::Int(i), PutCondition::Always);
        }

        // Trigger resize
        let trigger = b"trigger".to_vec();
        let _ = server
            .engine_mut()
            .store_mut()
            .put(&trigger, Value::Int(99), PutCondition::Always);
        assert!(server.engine().store().table().is_resizing());

        // Fill main_table up to its max load limit
        let seed = server.engine.store().table().seed;
        for i in 0..13 {
            let k = format!("fill-{i}").into_bytes().into_boxed_slice();
            let h = hash64(&k, seed);
            let _ = server
                .engine_mut()
                .store_mut()
                .table_mut()
                .main_table
                .insert_with_hash(h, k, Value::Int(i));
        }

        assert!(server.engine().store().table().is_resizing());
        assert!(server.wants_tick());

        // Migration step fails because main_table is full
        let still = server.step_migration_tick();
        assert!(!still);
        assert_eq!(server.migration_errors(), 1);

        // wants_tick is suppressed
        assert!(!server.wants_tick());

        // Repeated ticks are suppressed and do not spin or increment counter
        assert!(!server.step_migration_tick());
        assert_eq!(server.migration_errors(), 1);

        Ok(())
    }

    #[test]
    fn test_server_reply_encode_failure_fallback_encodes_7001() {
        // Construct a reply frame that cannot be serialized (nesting depth 10 > 8)
        let mut deep = Value::Int(42);
        for _ in 0..10 {
            deep = Value::Array(vec![deep]);
        }

        let header = Header::new(0, 1234, Opcode::Command, 0);
        let reply_frame = Frame::new(header, deep);

        let mut out = Vec::new();
        let encode_res = encode_frame(&reply_frame, &mut out);
        assert!(encode_res.is_err(), "depth 10 must fail encode_frame");

        // Verify fallback logic
        let err_header = Header::new(
            FLAG_RESPONSE | FLAG_ERROR,
            reply_frame.header.request_id,
            reply_frame.header.opcode,
            0,
        );
        let err_frame = Frame::new(err_header, crate::cmd::ERR_INTERNAL_ERROR.to_value());
        let mut fallback_out = Vec::new();
        assert!(encode_frame(&err_frame, &mut fallback_out).is_ok());

        // Verify fallback frame decodes as error 7001 with original request_id
        match decode_frame(&fallback_out) {
            Ok(Decoded::Complete((f, _))) => {
                assert_eq!(f.header.request_id, 1234);
                assert_eq!(f.header.flags, FLAG_RESPONSE | FLAG_ERROR);
                assert_eq!(f.payload, crate::cmd::ERR_INTERNAL_ERROR.to_value());
            }
            _ => panic!("fallback frame must decode cleanly"),
        }
    }
}
