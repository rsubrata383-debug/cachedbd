//! Scale, stress, and soak tests for SkiffDB connection architecture.
//!
//! Validates memory footprint, O(1) token lookup, and fast idle sweep performance
//! when managing 10,000 concurrent idle connections.

// AGENTS.md Section 5: unwrap and expect are explicitly allowed in tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unchecked_time_subtraction
)]

use skiffdb::net::connection::{Connection, ConnectionTable};
use std::io::{self, Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

/// In-memory mock socket stream for testing high connection counts without OS descriptor limits.
#[derive(Debug, Clone, Default)]
struct MockStream {
    inbound: Cursor<Vec<u8>>,
    outbound: Vec<u8>,
}

impl Read for MockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inbound.read(buf)?;
        if n == 0 {
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            Ok(n)
        }
    }
}

impl Write for MockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.outbound.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Validates memory accounting and limit enforcement with 10,000 idle connections.
///
/// Exit criteria:
/// - Exact capacity enforcement at 10,000 entries.
/// - Average memory allocated per idle connection <= 16 KB (target is ~4 KB initial buffer).
/// - 10,001st connection rejected with `ErrorKind::ConnectionRefused`.
#[test]
fn test_ten_thousand_idle_connections_scale_and_memory() -> Result<(), Box<dyn std::error::Error>> {
    const TARGET_CONNECTIONS: usize = 10_000;
    let dummy_addr: SocketAddr = "127.0.0.1:8080".parse()?;

    let mut table = ConnectionTable::<MockStream>::with_max_connections(TARGET_CONNECTIONS);
    assert_eq!(table.max_connections(), TARGET_CONNECTIONS);
    assert_eq!(table.remaining_capacity(), TARGET_CONNECTIONS);

    let start_insert = Instant::now();
    for i in 1..=TARGET_CONNECTIONS {
        let stream = MockStream::default();
        let conn = Connection::new(i as u64, stream, dummy_addr);
        let token = table.insert_connection(conn)?;
        assert_eq!(token, i as u64);
    }
    let insert_elapsed = start_insert.elapsed();

    assert_eq!(table.len(), TARGET_CONNECTIONS);
    assert!(table.is_at_capacity());
    assert_eq!(table.remaining_capacity(), 0);

    // Verify 10,000 insertions completed quickly.
    println!("Inserted {TARGET_CONNECTIONS} connections in {insert_elapsed:?}");

    // Verify rejection of 10,001st connection.
    let overflow_conn = Connection::new(
        (TARGET_CONNECTIONS + 1) as u64,
        MockStream::default(),
        dummy_addr,
    );
    let overflow_err = table.insert_connection(overflow_conn).unwrap_err();
    assert_eq!(overflow_err.kind(), io::ErrorKind::ConnectionRefused);

    // Memory accounting verification:
    let total_read_buf_bytes = table.total_read_buffer_capacity();
    let total_allocated_bytes = table.total_allocated_buffer_memory();
    let bytes_per_connection = total_allocated_bytes / TARGET_CONNECTIONS;

    println!(
        "Total buffer memory for {TARGET_CONNECTIONS} connections: {} KB ({} bytes/connection)",
        total_allocated_bytes / 1024,
        bytes_per_connection
    );

    // Each connection allocates an initial 4 KB read buffer; write queue starts at 0 bytes.
    assert_eq!(total_read_buf_bytes, TARGET_CONNECTIONS * 4096);
    assert_eq!(total_allocated_bytes, TARGET_CONNECTIONS * 4096);
    // Well below our 16 KB per idle connection budget.
    assert!(
        bytes_per_connection <= 16 * 1024,
        "Memory per connection exceeds 16 KB threshold"
    );

    Ok(())
}

/// Validates O(1) token lookup and activity updates across 10,000 connections.
#[test]
fn test_ten_thousand_connections_lookup_and_touch() -> Result<(), Box<dyn std::error::Error>> {
    const TARGET_CONNECTIONS: usize = 10_000;
    let dummy_addr: SocketAddr = "127.0.0.1:8080".parse()?;

    let mut table = ConnectionTable::<MockStream>::with_max_connections(TARGET_CONNECTIONS);
    for i in 1..=TARGET_CONNECTIONS {
        let conn = Connection::new(i as u64, MockStream::default(), dummy_addr);
        table.insert_connection(conn)?;
    }

    let lookup_start = Instant::now();
    // Lookup every 10th connection (1,000 lookups).
    for i in (1..=TARGET_CONNECTIONS).step_by(10) {
        let token = i as u64;
        let conn = table.get(token).expect("connection must exist");
        assert_eq!(conn.id(), token);
        assert!(!conn.is_closing());
        assert!(!conn.is_closed());
    }
    let lookup_elapsed = lookup_start.elapsed();
    println!("1,000 lookups across 10,000 connections took {lookup_elapsed:?}");
    // Generous upper-bound timing assertion to avoid flakiness under CI load
    assert!(
        lookup_elapsed < Duration::from_millis(100),
        "1,000 lookups exceeded generous 100ms threshold: {lookup_elapsed:?}"
    );

    // Mutably touch 500 connections.
    for i in (1..=TARGET_CONNECTIONS).step_by(20) {
        let token = i as u64;
        if let Some(conn) = table.get_mut(token) {
            conn.touch();
        }
    }

    Ok(())
}

/// Validates idle sweeping performance over 10,000 connections.
#[test]
fn test_ten_thousand_connections_idle_sweep_performance() -> Result<(), Box<dyn std::error::Error>>
{
    const TARGET_CONNECTIONS: usize = 10_000;
    let dummy_addr: SocketAddr = "127.0.0.1:8080".parse()?;

    let mut table = ConnectionTable::<MockStream>::with_max_connections(TARGET_CONNECTIONS);
    let now = Instant::now();
    let idle_threshold = Duration::from_secs(60);

    for i in 1..=TARGET_CONNECTIONS {
        let mut conn = Connection::new(i as u64, MockStream::default(), dummy_addr);
        // Even tokens are set to 100 seconds ago (idle), odd tokens set to now (active).
        if i % 2 == 0 {
            conn.set_last_active(
                now.checked_sub(Duration::from_secs(100))
                    .expect("valid time subtraction"),
            );
        } else {
            conn.set_last_active(now);
        }
        table.insert_connection(conn)?;
    }

    // Benchmark idle token detection across all 10,000 entries.
    let sweep_start = Instant::now();
    let idle_tokens = table.idle_tokens(idle_threshold);
    let sweep_elapsed = sweep_start.elapsed();

    println!("Swept 10,000 connections in {sweep_elapsed:?}");
    // Generous upper-bound timing assertion to verify fast sweep without CI flakiness
    assert!(
        sweep_elapsed < Duration::from_millis(100),
        "Idle sweep exceeded generous 100ms threshold: {sweep_elapsed:?}"
    );

    // Exactly 5,000 even-indexed connections were idle.
    assert_eq!(idle_tokens.len(), 5_000);

    // Sweep and purge idle connections from the table.
    let purged = table.sweep_idle_connections(idle_threshold);
    assert_eq!(purged.len(), 5_000);
    assert_eq!(table.len(), 5_000);
    assert_eq!(table.remaining_capacity(), 5_000);

    // Verify all remaining connections are the active odd ones.
    for i in (1..=TARGET_CONNECTIONS).step_by(2) {
        assert!(table.contains(i as u64));
    }

    Ok(())
}

/// Validates teardown and drain performance over 10,000 connections.
#[test]
fn test_ten_thousand_connections_drain_teardown() -> Result<(), Box<dyn std::error::Error>> {
    const TARGET_CONNECTIONS: usize = 10_000;
    let dummy_addr: SocketAddr = "127.0.0.1:8080".parse()?;

    let mut table = ConnectionTable::<MockStream>::with_max_connections(TARGET_CONNECTIONS);
    for i in 1..=TARGET_CONNECTIONS {
        let conn = Connection::new(i as u64, MockStream::default(), dummy_addr);
        table.insert_connection(conn)?;
    }
    assert_eq!(table.len(), TARGET_CONNECTIONS);

    // Drain all 10,000 connections.
    let drain_start = Instant::now();
    for i in 1..=TARGET_CONNECTIONS {
        let conn = table.remove(i as u64);
        assert!(conn.is_some());
    }
    let drain_elapsed = drain_start.elapsed();

    println!("Drained and removed 10,000 connections in {drain_elapsed:?}");
    assert!(
        drain_elapsed < Duration::from_millis(200),
        "Draining 10,000 connections exceeded 200ms threshold: {drain_elapsed:?}"
    );
    assert!(table.is_empty());
    assert_eq!(table.remaining_capacity(), TARGET_CONNECTIONS);

    Ok(())
}

/// Real TCP socket loopback test validating capacity and rejection limits with real OS descriptors.
#[test]
fn test_real_tcp_sockets_connection_table_limits() -> Result<(), Box<dyn std::error::Error>> {
    const MAX_REAL_SOCKETS: usize = 50;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;

    let mut table = ConnectionTable::<TcpStream>::with_max_connections(MAX_REAL_SOCKETS);
    let mut client_streams = Vec::with_capacity(MAX_REAL_SOCKETS);

    for _ in 0..MAX_REAL_SOCKETS {
        let client = TcpStream::connect(addr)?;
        client.set_nonblocking(true)?;
        client_streams.push(client);

        let (server_stream, peer) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e.into()),
            }
        };
        server_stream.set_nonblocking(true)?;
        table.insert(server_stream, peer)?;
    }

    assert_eq!(table.len(), MAX_REAL_SOCKETS);
    assert!(table.is_at_capacity());

    // Connect 51st client: accepted at TCP level but rejected by ConnectionTable capacity.
    let client_extra = TcpStream::connect(addr)?;
    let (server_extra, peer_extra) = loop {
        match listener.accept() {
            Ok(pair) => break pair,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(e) => return Err(e.into()),
        }
    };
    let res = table.insert(server_extra, peer_extra);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);

    drop(client_extra);
    drop(client_streams);
    Ok(())
}

/// Stress test with 10,000 real TCP sockets on systems where `ulimit -n >= 25000`.
///
/// Run explicitly with:
/// `cargo test --test soak test_ten_thousand_real_tcp_idle_connections -- --ignored`
#[test]
#[ignore = "requires elevated ulimit -n (>= 25,000 open file descriptors)"]
fn test_ten_thousand_real_tcp_idle_connections() -> Result<(), Box<dyn std::error::Error>> {
    const TARGET: usize = 10_000;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;

    let mut table = ConnectionTable::<TcpStream>::with_max_connections(TARGET);
    let mut clients = Vec::with_capacity(TARGET);

    for i in 0..TARGET {
        let client = TcpStream::connect(addr)?;
        client.set_nonblocking(true)?;
        clients.push(client);

        let (server_stream, peer) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_micros(100));
                }
                Err(e) => return Err(e.into()),
            }
        };
        server_stream.set_nonblocking(true)?;
        table.insert(server_stream, peer)?;

        if (i + 1) % 1_000 == 0 {
            println!("Connected {} / {} real TCP sockets", i + 1, TARGET);
        }
    }

    assert_eq!(table.len(), TARGET);
    assert!(table.is_at_capacity());

    drop(clients);
    Ok(())
}

#[cfg(target_os = "linux")]
const LISTENER_TOKEN: skiffdb::net::event_loop::Token = 0;

#[cfg(target_os = "linux")]
struct SoakEchoHandler {
    listener: TcpListener,
    connections: ConnectionTable<TcpStream>,
}

#[cfg(target_os = "linux")]
impl SoakEchoHandler {
    fn handle_accept(&mut self, event_loop: &mut skiffdb::net::event_loop::EventLoop) {
        loop {
            match self.listener.accept() {
                Ok((stream, peer_addr)) => {
                    if let Err(err) = stream.set_nonblocking(true) {
                        eprintln!("Failed to set client non-blocking: {err}");
                        continue;
                    }
                    match self.connections.insert(stream, peer_addr) {
                        Ok(token) => {
                            if let Err(err) = self.connections.register_with_loop(token, event_loop)
                            {
                                eprintln!("Failed to register client {token}: {err}");
                                self.connections.remove(token);
                            }
                        }
                        Err(err) => {
                            eprintln!("Connection rejected: {err}");
                        }
                    }
                }
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(ref err) if err.kind() == io::ErrorKind::ConnectionAborted => {}
                Err(err) => {
                    eprintln!("Accept error: {err}");
                    break;
                }
            }
        }
    }

    fn handle_client_read(
        &mut self,
        token: skiffdb::net::event_loop::Token,
        event_loop: &mut skiffdb::net::event_loop::EventLoop,
    ) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        loop {
            match conn.read_incoming() {
                Ok(skiffdb::net::connection::ReadStatus::Eof) => {
                    if conn.write_queue().is_empty() {
                        self.connections.remove_and_deregister(token, event_loop);
                        return false;
                    }
                    let _ = self.connections.sync_interest(token, event_loop);
                    return self.handle_client_write(token, event_loop);
                }
                Ok(skiffdb::net::connection::ReadStatus::WouldBlock) => break,
                Ok(skiffdb::net::connection::ReadStatus::Ok(n)) => {
                    let data = conn.read_buffer().data().to_vec();
                    if let Err(err) = conn.send_chunk(data) {
                        eprintln!("Failed to enqueue echo payload for client {token}: {err}");
                        self.connections.remove_and_deregister(token, event_loop);
                        return false;
                    }
                    conn.read_buffer_mut().consume(n);
                }
                Err(err) => {
                    eprintln!("I/O read error on client {token}: {err}");
                    self.connections.remove_and_deregister(token, event_loop);
                    return false;
                }
            }
        }

        self.handle_client_write(token, event_loop)
    }

    fn handle_client_write(
        &mut self,
        token: skiffdb::net::event_loop::Token,
        event_loop: &mut skiffdb::net::event_loop::EventLoop,
    ) -> bool {
        let Some(conn) = self.connections.get_mut(token) else {
            return false;
        };

        match conn.flush_outgoing() {
            Ok(skiffdb::net::connection::WriteStatus::Complete) => {
                if conn.is_closed() {
                    self.connections.remove_and_deregister(token, event_loop);
                    return false;
                }
            }
            Ok(skiffdb::net::connection::WriteStatus::Pending(_)) => {}
            Err(err) => {
                eprintln!("I/O write error on client {token}: {err}");
                self.connections.remove_and_deregister(token, event_loop);
                return false;
            }
        }

        let _ = self.connections.sync_interest(token, event_loop);
        true
    }
}

#[cfg(target_os = "linux")]
impl skiffdb::net::event_loop::Handler for SoakEchoHandler {
    fn on_event(
        &mut self,
        event_loop: &mut skiffdb::net::event_loop::EventLoop,
        token: skiffdb::net::event_loop::Token,
        flags: skiffdb::net::sys::EpollFlags,
    ) -> io::Result<()> {
        if token == LISTENER_TOKEN {
            self.handle_accept(event_loop);
        } else {
            let mut conn_alive = if flags.contains(skiffdb::net::sys::EpollFlags::READABLE) {
                self.handle_client_read(token, event_loop)
            } else {
                true
            };
            if conn_alive && flags.contains(skiffdb::net::sys::EpollFlags::WRITABLE) {
                conn_alive = self.handle_client_write(token, event_loop);
            }
            if conn_alive
                && (flags.contains(skiffdb::net::sys::EpollFlags::HANGUP)
                    || flags.contains(skiffdb::net::sys::EpollFlags::ERROR))
            {
                self.connections.remove_and_deregister(token, event_loop);
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn get_vm_rss_kb() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in content.lines() {
        if line.starts_with("VmRSS:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1].parse().ok();
            }
        }
    }
    None
}

/// End-to-end stress test validating that the epoll EventLoop can coordinate
/// 10,000 real concurrent TCP connections, route I/O while all connections remain
/// idle, and drain cleanly on shutdown.
#[test]
#[cfg(target_os = "linux")]
#[ignore = "requires ulimit -n >= 25,000"]
// Integration soak benchmark spanning setup, batch connection, idle checks, echo, and drain.
#[allow(clippy::too_many_lines)]
fn test_ten_thousand_real_tcp_event_loop_idle_connections() -> Result<(), Box<dyn std::error::Error>>
{
    const TARGET_CONNECTIONS: usize = 10_000;
    // std::net::TcpListener::bind sets a hardcoded listen backlog limit of 128.
    // Batch size MUST stay strictly below 128 to ensure the kernel backlog never
    // overflows and drops incoming SYN packets before the event loop can accept them.
    const BATCH_SIZE: usize = 100;
    const {
        assert!(
            BATCH_SIZE < 128,
            "BATCH_SIZE must be strictly less than std's 128 listen backlog limit"
        );
    };

    let mut event_loop = skiffdb::net::event_loop::EventLoop::new()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;

    event_loop.register(
        skiffdb::net::sys::as_raw_fd(&listener),
        LISTENER_TOKEN,
        skiffdb::net::sys::EpollFlags::READABLE,
    )?;

    let connections = ConnectionTable::<TcpStream>::with_max_connections(TARGET_CONNECTIONS);
    let mut handler = SoakEchoHandler {
        listener,
        connections,
    };

    let rss_before = get_vm_rss_kb();
    let connect_start = Instant::now();
    let mut clients = Vec::with_capacity(TARGET_CONNECTIONS);

    // Connect 10,000 real clients in batches of 100 to prevent listen backlog overflow.
    for batch_idx in 0..(TARGET_CONNECTIONS / BATCH_SIZE) {
        for i in 0..BATCH_SIZE {
            let client_num = batch_idx * BATCH_SIZE + i;
            let client =
                TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|err| {
                    io::Error::new(
                        err.kind(),
                        format!("Failed to connect client #{client_num} to {addr}: {err}"),
                    )
                })?;
            client.set_nonblocking(true)?;
            clients.push(client);
        }
        let batch_deadline = Instant::now() + Duration::from_secs(10);
        while handler.connections.len() < clients.len() {
            event_loop.run_once(&mut handler, Some(Duration::from_millis(10)))?;
            assert!(
                Instant::now() < batch_deadline,
                "Timed out accepting connections in batch: accepted {} / {}",
                handler.connections.len(),
                clients.len()
            );
        }
    }

    let connect_elapsed = connect_start.elapsed();
    let rss_after = get_vm_rss_kb();

    // Verify all 10,000 are accepted, registered in epoll, and idle.
    assert_eq!(handler.connections.len(), TARGET_CONNECTIONS);
    assert!(handler.connections.is_at_capacity());
    thread::sleep(Duration::from_millis(50));
    let idle = handler.connections.idle_tokens(Duration::from_millis(20));
    assert_eq!(
        idle.len(),
        TARGET_CONNECTIONS,
        "Expected all 10,000 connections to be registered and idle"
    );

    // While 10,000 stay idle, pick 3 clients, send messages, and verify each gets echoed back.
    let test_indices = [0, 5_000, 9_999];
    let test_payloads = [
        b"ping-from-client-0\n".to_vec(),
        b"ping-from-client-5000\n".to_vec(),
        b"ping-from-client-9999\n".to_vec(),
    ];

    for (&idx, payload) in test_indices.iter().zip(test_payloads.iter()) {
        clients[idx].write_all(payload)?;
    }

    let mut responses: Vec<Vec<u8>> = vec![Vec::new(); 3];
    let echo_deadline = Instant::now() + Duration::from_secs(10);

    while responses
        .iter()
        .zip(test_payloads.iter())
        .any(|(r, p)| r.len() < p.len())
    {
        event_loop.run_once(&mut handler, Some(Duration::from_millis(10)))?;
        for (i, &idx) in test_indices.iter().enumerate() {
            if responses[i].len() < test_payloads[i].len() {
                let mut buf = [0u8; 128];
                match clients[idx].read(&mut buf) {
                    Ok(n) if n > 0 => responses[i].extend_from_slice(&buf[..n]),
                    Ok(_) => {}
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        assert!(
            Instant::now() < echo_deadline,
            "Timed out waiting for 3 echo responses during 10,000 idle connection test"
        );
    }

    for (i, &idx) in test_indices.iter().enumerate() {
        assert_eq!(
            responses[i], test_payloads[i],
            "Client {idx} received mismatched echo payload"
        );
    }

    // Print benchmark timings and memory accounting.
    println!("=== 10,000 Real TCP EventLoop Soak Benchmark (Linux) ===");
    println!("Connected and accepted 10,000 real TCP clients in {connect_elapsed:?}");
    match (rss_before, rss_after) {
        (Some(before), Some(after)) => {
            let delta = after.saturating_sub(before);
            let per_conn = (delta as f64) / (TARGET_CONNECTIONS as f64);
            println!(
                "VmRSS memory: before = {before} kB, after = {after} kB, delta = {delta} kB (~{per_conn:.2} kB/conn)"
            );
        }
        _ => {
            println!("VmRSS memory: /proc/self/status unavailable");
        }
    }

    // Close all clients, run ticks, and verify the connection table is completely empty.
    drop(clients);

    let drain_deadline = Instant::now() + Duration::from_secs(15);
    while !handler.connections.is_empty() {
        event_loop.run_once(&mut handler, Some(Duration::from_millis(10)))?;
        assert!(
            Instant::now() < drain_deadline,
            "Timed out draining connections: {} still active in table",
            handler.connections.len()
        );
    }
    assert_eq!(handler.connections.len(), 0);
    assert!(handler.connections.is_empty());

    Ok(())
}
