//! Server orchestration connecting network I/O, event loop, and command engine.

use crate::cmd::dispatch::{Engine, Session};
use crate::cmd::request::handle_request;
use crate::net::connection::{Connection, ConnectionTable, ReadStatus, WriteStatus};
use crate::net::event_loop::{EventLoop, Handler, TimerId, Token};
use crate::net::sys::EpollFlags;
use crate::proto::decode_frame;
use crate::proto::encode::encode_frame;
use crate::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
use crate::proto::{Decoded, Frame, Value};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Reserved token identifying readiness events on the listening server socket.
pub const LISTENER_TOKEN: Token = 0;

/// Monotonic timer identifier for periodic idle connection evaluation.
pub const IDLE_CHECK_TIMER_ID: TimerId = 1;

/// Configuration parameters governing server socket limits, watermarks, and fairness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Inactivity timeout before an idle client connection is terminated (`None` = never).
    pub idle_timeout: Option<Duration>,
    /// High-watermark threshold in bytes for a connection's write queue before backpressure pauses reads.
    pub write_queue_high_watermark: usize,
    /// Low-watermark threshold in bytes for a connection's write queue to resume reads.
    pub write_queue_low_watermark: usize,
    /// Maximum number of complete protocol frames processed per client per readiness event.
    pub max_frames_per_event: usize,
    /// Maximum incoming socket bytes read per client per readiness event.
    pub max_read_bytes_per_event: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Some(Duration::from_secs(300)),
            write_queue_high_watermark: 4 * 1024 * 1024,
            write_queue_low_watermark: 1024 * 1024,
            max_frames_per_event: 256,
            max_read_bytes_per_event: 256 * 1024,
        }
    }
}

/// Operational telemetry metrics and atomic counters for server monitoring.
#[derive(Debug, Default)]
pub struct ServerStats {
    /// Total number of accepted client connections.
    pub connections_accepted: AtomicU64,
    /// Total number of connections terminated due to inactivity exceeding the idle timeout.
    pub connections_closed_idle: AtomicU64,
    /// Total number of times client connections transitioned into backpressure pause.
    pub backpressure_pauses: AtomicU64,
    /// Peak write queue size in bytes observed across all connections.
    pub max_write_queue_bytes_seen: AtomicU64,
    /// Total number of protocol frames decoded and processed.
    pub frames_processed: AtomicU64,
}

// NOTE(atomic-ordering): Relaxed ordering (`Ordering::Relaxed`) is used for all
// telemetry counters in `ServerStats` because these are statistical metrics used
// solely for observability, diagnostics, and testing. They do not guard critical
// sections, synchronize access to other memory locations, or establish happens-before
// relationships across threads.

/// Network server driving Cachedb client connections and command execution.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    connections: ConnectionTable<TcpStream>,
    sessions: HashMap<Token, Session>,
    engine: Engine,
    config: ServerConfig,
    stats: Arc<ServerStats>,
    next_client_id: u64,
    rejected_connections: usize,
    migration_errors: usize,
    migration_error_suppressed: bool,
    last_capacity: usize,
    last_len: usize,
    pending_work: VecDeque<Token>,
    timer_scheduled: bool,
}

impl Server {
    /// Creates a new `Server` backed by the provided [`TcpListener`] and default configuration.
    ///
    /// Sets `listener` to non-blocking mode.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if configuring the listener socket fails.
    pub fn new(listener: TcpListener) -> io::Result<Self> {
        Self::with_config(listener, Engine::new(), ServerConfig::default())
    }

    /// Creates a new `Server` with a custom [`Engine`] and default configuration.
    ///
    /// Sets `listener` to non-blocking mode so the accept loop never blocks.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if configuring the listener socket fails.
    pub fn with_engine(listener: TcpListener, engine: Engine) -> io::Result<Self> {
        Self::with_config(listener, engine, ServerConfig::default())
    }

    /// Creates a new `Server` with a custom [`Engine`] and explicit [`ServerConfig`].
    ///
    /// Sets `listener` to non-blocking mode.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if configuring the listener socket fails.
    pub fn with_config(
        listener: TcpListener,
        engine: Engine,
        config: ServerConfig,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let initial_cap = engine.store().table().capacity();
        let initial_len = engine.store().len();
        let mut connections = ConnectionTable::new();
        connections.set_watermarks(
            config.write_queue_high_watermark,
            config.write_queue_low_watermark,
        );
        Ok(Self {
            listener,
            connections,
            sessions: HashMap::new(),
            engine,
            config,
            stats: Arc::new(ServerStats::default()),
            next_client_id: 1,
            rejected_connections: 0,
            migration_errors: 0,
            migration_error_suppressed: false,
            last_capacity: initial_cap,
            last_len: initial_len,
            pending_work: VecDeque::new(),
            timer_scheduled: false,
        })
    }

    /// Returns a reference to the active server configuration.
    #[must_use]
    pub const fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Returns a shared reference to the server telemetry counters.
    #[must_use]
    pub fn stats(&self) -> Arc<ServerStats> {
        Arc::clone(&self.stats)
    }

    /// Returns the interval duration between periodic idle connection sweeps.
    #[must_use]
    pub fn idle_check_interval(&self) -> Duration {
        match self.config.idle_timeout {
            Some(timeout) => Duration::from_secs(1)
                .min(timeout / 2)
                .max(Duration::from_millis(50)),
            None => Duration::from_secs(1),
        }
    }

    /// Schedules the periodic idle timeout evaluation timer on the event loop.
    ///
    /// This MUST be called prior to running the event loop (e.g. in `main.rs` and `TestServer`)
    /// so the first client connection is properly monitored even if no subsequent accept events occur.
    pub fn schedule_initial_timers(&mut self, event_loop: &mut EventLoop) {
        if self.config.idle_timeout.is_some() {
            let interval = self.idle_check_interval();
            event_loop.schedule_timer(interval, IDLE_CHECK_TIMER_ID);
            self.timer_scheduled = true;
        }
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
        if !self.timer_scheduled && self.config.idle_timeout.is_some() {
            self.schedule_initial_timers(event_loop);
        }

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

                    self.stats
                        .connections_accepted
                        .fetch_add(1, Ordering::Relaxed);

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

    fn dispatch_frame(
        engine: &mut Engine,
        sessions: &mut HashMap<Token, Session>,
        next_client_id: &mut u64,
        stats: &ServerStats,
        token: Token,
        conn: &mut Connection<TcpStream>,
        frame: &Frame,
    ) -> (bool, bool) {
        let session = sessions.entry(token).or_insert_with(|| {
            let cid = *next_client_id;
            *next_client_id = next_client_id.wrapping_add(1);
            if *next_client_id == 0 {
                *next_client_id = 1;
            }
            Session::new(cid)
        });

        let outcome = handle_request(engine, session, frame);

        let mut out = Vec::new();
        let was_backpressured = conn.is_backpressured();
        if encode_frame(&outcome.reply_frame, &mut out).is_ok() {
            let _ = conn.send_chunk(out);
        } else {
            let err_header = Header::new(
                FLAG_RESPONSE | FLAG_ERROR,
                outcome.reply_frame.header.request_id,
                outcome.reply_frame.header.opcode,
                0,
            );
            let err_frame = Frame::new(err_header, crate::cmd::ERR_INTERNAL_ERROR.to_value());
            let mut fallback_out = Vec::new();
            if encode_frame(&err_frame, &mut fallback_out).is_ok() {
                let _ = conn.send_chunk(fallback_out);
            }
        }

        let queue_len = conn.write_queue().len();
        stats
            .max_write_queue_bytes_seen
            .fetch_max(queue_len as u64, Ordering::Relaxed);

        if conn.is_backpressured() && !was_backpressured {
            stats.backpressure_pauses.fetch_add(1, Ordering::Relaxed);
        }

        (outcome.close, conn.is_backpressured())
    }

    fn send_salvaged_proto_error(conn: &mut Connection<TcpStream>, code: u16, message: &str) {
        match code {
            1001 | 1002 | 1005 | 1006 | 1007 => {
                let header = Header::new(FLAG_RESPONSE | FLAG_ERROR, 0, Opcode::Command, 0);
                let reply_val = Value::Error {
                    code,
                    message: message.to_string(),
                };
                let err_frame = Frame::new(header, reply_val);
                let mut out = Vec::new();
                if encode_frame(&err_frame, &mut out).is_ok() {
                    let _ = conn.send_chunk(out);
                }
            }
            _ => {}
        }
    }

    /// Reads incoming bytes from a client socket and processes frames via pipelining.
    ///
    /// Respects fairness limits ([`ServerConfig::max_read_bytes_per_event`] and
    /// [`ServerConfig::max_frames_per_event`]) and backpressure watermarks.
    pub fn handle_client_read(&mut self, token: Token, event_loop: &mut EventLoop) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        let mut read_done = false;
        let mut total_bytes_read = 0;
        loop {
            if total_bytes_read >= self.config.max_read_bytes_per_event {
                break;
            }
            match conn.read_incoming() {
                Ok(ReadStatus::Eof) => {
                    read_done = true;
                    break;
                }
                Ok(ReadStatus::WouldBlock) => break,
                Ok(ReadStatus::Ok(n)) => {
                    total_bytes_read += n;
                }
                Err(_) => {
                    self.remove_client(token, event_loop);
                    return false;
                }
            }
        }

        // Pipelining: decode and dispatch completed frames up to fairness limit and watermark
        let mut close_after_pipeline = false;
        let mut frames_processed = 0;
        loop {
            if conn.is_backpressured() {
                break;
            }

            if frames_processed >= self.config.max_frames_per_event {
                break;
            }

            let data = conn.read_buffer().data();
            if data.is_empty() {
                break;
            }

            match decode_frame(data) {
                Ok(Decoded::NeedMore) => break,
                Ok(Decoded::Complete((frame, used_bytes))) => {
                    conn.read_buffer_mut().consume(used_bytes);
                    frames_processed += 1;
                    self.stats.frames_processed.fetch_add(1, Ordering::Relaxed);

                    let (should_close, is_paused) = Self::dispatch_frame(
                        &mut self.engine,
                        &mut self.sessions,
                        &mut self.next_client_id,
                        &self.stats,
                        token,
                        conn,
                        &frame,
                    );
                    if should_close {
                        close_after_pipeline = true;
                        break;
                    }
                    if is_paused {
                        break;
                    }
                }
                Err(proto_err) => {
                    Self::send_salvaged_proto_error(conn, proto_err.code(), proto_err.message());
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

        let write_ok = self.handle_client_write(token, event_loop);
        if !write_ok {
            return false;
        }

        // Schedule remaining unconsumed data in pending_work for subsequent loop tick:
        if let Some(conn) = self.connections.get(token) {
            if !conn.is_backpressured()
                && !conn.is_closing()
                && !conn.is_closed()
                && !conn.read_buffer().is_empty()
                && !self.pending_work.contains(&token)
            {
                self.pending_work.push_back(token);
            }
        }

        true
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

        // If backpressure was cleared and unconsumed data exists in read buffer,
        // enqueue for processing in next tick:
        if let Some(conn) = self.connections.get(token) {
            if !conn.is_backpressured()
                && !conn.is_closing()
                && !conn.is_closed()
                && !conn.read_buffer().is_empty()
                && !self.pending_work.contains(&token)
            {
                self.pending_work.push_back(token);
            }
        }

        true
    }

    /// Removes a client connection, deregisters it from epoll, and drops its session.
    pub fn remove_client(&mut self, token: Token, event_loop: &mut EventLoop) {
        self.connections.remove_and_deregister(token, event_loop);
        self.sessions.remove(&token);
        self.pending_work.retain(|&t| t != token);
    }

    /// Sweeps active connections and terminates any exceeding [`ServerConfig::idle_timeout`].
    pub fn check_idle_timeouts(&mut self, event_loop: &mut EventLoop) {
        let Some(idle_timeout) = self.config.idle_timeout else {
            return;
        };

        let idle_tokens = self.connections.idle_tokens(idle_timeout);
        for token in idle_tokens {
            self.stats
                .connections_closed_idle
                .fetch_add(1, Ordering::Relaxed);
            self.remove_client(token, event_loop);
        }
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

        if flags.contains(EpollFlags::HANGUP) || flags.contains(EpollFlags::ERROR) {
            if flags.contains(EpollFlags::READABLE) {
                self.handle_client_read(token, event_loop);
            } else if flags.contains(EpollFlags::WRITABLE) {
                self.handle_client_write(token, event_loop);
            } else {
                self.remove_client(token, event_loop);
            }
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

    fn on_timeout(&mut self, event_loop: &mut EventLoop, timer_id: TimerId) -> io::Result<()> {
        if timer_id == IDLE_CHECK_TIMER_ID {
            self.check_idle_timeouts(event_loop);
            let interval = self.idle_check_interval();
            event_loop.schedule_timer(interval, IDLE_CHECK_TIMER_ID);
        }
        Ok(())
    }

    fn wants_tick(&self) -> bool {
        if !self.pending_work.is_empty() {
            return true;
        }
        if self.migration_error_suppressed {
            let current_cap = self.engine.store().table().capacity();
            let current_len = self.engine.store().len();
            if current_cap == self.last_capacity && current_len <= self.last_len {
                return false;
            }
        }
        self.engine.store().table().is_resizing()
    }

    fn on_tick(&mut self, event_loop: &mut EventLoop) -> io::Result<()> {
        self.step_migration_tick();

        let count = self.pending_work.len();
        for _ in 0..count {
            if let Some(token) = self.pending_work.pop_front() {
                if self.connections.get(token).is_some() {
                    self.handle_client_read(token, event_loop);
                }
            }
        }

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

    #[test]
    fn test_watermark_state_machine_transition() {
        use crate::net::buffer::WriteQueue;

        let high = 1000;
        let low = 400;
        let mut queue = WriteQueue::with_limits(high, low, 5000);

        assert!(!queue.is_backpressured());

        // Enqueue under high watermark (999 bytes)
        queue.enqueue(&[0u8; 999]).unwrap();
        assert_eq!(queue.len(), 999);
        assert!(!queue.is_backpressured());

        // Enqueue crossing high watermark (10 bytes -> 1009 bytes >= 1000)
        queue.enqueue(&[0u8; 10]).unwrap();
        assert_eq!(queue.len(), 1009);
        assert!(queue.is_backpressured());

        // Partial drain down to 600 bytes (> low watermark 400): hysteresis keeps backpressured
        let mut sink = [0u8; 409];
        let mut first_sink: &mut [u8] = &mut sink[..];
        let written = queue.write_to(&mut first_sink).unwrap();
        assert_eq!(written, 409);
        assert_eq!(queue.len(), 600);
        assert!(queue.is_backpressured());

        // Drain down to exactly low watermark (400 bytes): clears backpressure
        let mut sink2 = [0u8; 200];
        let mut second_sink: &mut [u8] = &mut sink2[..];
        let written2 = queue.write_to(&mut second_sink).unwrap();
        assert_eq!(written2, 200);
        assert_eq!(queue.len(), 400);
        assert!(!queue.is_backpressured());

        // Drain completely to 0 bytes
        let mut sink3 = [0u8; 400];
        let mut third_sink: &mut [u8] = &mut sink3[..];
        let written3 = queue.write_to(&mut third_sink).unwrap();
        assert_eq!(written3, 400);
        assert_eq!(queue.len(), 0);
        assert!(!queue.is_backpressured());
    }

    #[test]
    fn test_connection_watermark_interest_flags() {
        use crate::net::connection::Connection;
        use std::net::SocketAddr;

        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let stream = std::io::Cursor::new(Vec::new());
        let mut conn = Connection::with_custom_limits(1, stream, addr, 4096, 1000, 400, 5000);

        // Initially empty: READABLE only, not backpressured
        assert_eq!(conn.desired_flags(), EpollFlags::READABLE);
        assert!(!conn.is_backpressured());

        // Enqueue 1009 bytes (> high watermark 1000)
        conn.send_slice(&[0u8; 1009]).unwrap();
        assert!(conn.is_backpressured());
        // Under backpressure: READABLE interest dropped, WRITABLE only
        assert_eq!(conn.desired_flags(), EpollFlags::WRITABLE);
    }
}
