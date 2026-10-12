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
}

impl Server {
    /// Creates a new `Server` backed by the provided [`TcpListener`] and default [`Engine`].
    #[must_use]
    pub fn new(listener: TcpListener) -> Self {
        Self::with_engine(listener, Engine::new())
    }

    /// Creates a new `Server` with a custom [`Engine`].
    #[must_use]
    pub fn with_engine(listener: TcpListener, engine: Engine) -> Self {
        Self {
            listener,
            connections: ConnectionTable::new(),
            sessions: HashMap::new(),
            engine,
            next_client_id: 1,
            rejected_connections: 0,
        }
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

    /// Steps background migration once (moving up to 16 slots) if resizing.
    ///
    /// Returns `true` if migration is still active after this step.
    pub fn step_migration_tick(&mut self) -> bool {
        if self.engine.store().table().is_resizing() {
            self.engine.store_mut().step_migration(16).unwrap_or(false)
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
        self.engine.store().table().is_resizing()
    }

    fn on_tick(&mut self, _event_loop: &mut EventLoop) -> io::Result<()> {
        if self.engine.store().table().is_resizing() {
            let _ = self.engine.store_mut().step_migration(16);
        }
        Ok(())
    }
}
