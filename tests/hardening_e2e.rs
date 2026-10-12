//! Linux-specific server hardening, backpressure, fairness, and idle timeout end-to-end tests.

#![cfg(target_os = "linux")]

mod common;

use common::{TestServer, expect_ok_reply, raw_handshake, read_one_frame};
use skiffdb::client::Client;
use skiffdb::proto::encode::encode_frame;
use skiffdb::proto::header::{Header, Opcode};
use skiffdb::proto::{Frame, Value};
use skiffdb::server::ServerConfig;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn setup_handshake_and_put_bigkey(
    stream: &mut TcpStream,
    acc: &mut Vec<u8>,
    client_name: &str,
    big_value: &[u8],
) {
    let _hello_resp = raw_handshake(stream, acc, client_name);

    let put_cmd = Frame::new(
        Header::new(0, 2, Opcode::Command, 0),
        Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("bigkey".to_string()),
            Value::Bytes(big_value.to_vec()),
        ]),
    );
    let mut put_bytes = Vec::new();
    encode_frame(&put_cmd, &mut put_bytes).expect("encode put");
    stream.write_all(&put_bytes).expect("send put");

    let reply = read_one_frame(stream, acc);
    expect_ok_reply(&reply, 2);
    assert_eq!(reply.payload, Value::Bool(true));
}

fn drain_and_verify_replies(
    stream: &mut TcpStream,
    acc: &mut Vec<u8>,
    start_id: u32,
    end_id: u32,
    expected_payload: &Value,
) {
    for expected_id in start_id..=end_id {
        let frame = read_one_frame(stream, acc);
        expect_ok_reply(&frame, expected_id);
        assert_eq!(&frame.payload, expected_payload);
    }
}

#[test]
fn test_slow_reader_backpressure_and_resume() {
    let config = ServerConfig {
        write_queue_high_watermark: 64 * 1024,
        write_queue_low_watermark: 16 * 1024,
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set write timeout");

    let mut acc = Vec::new();
    let big_value = (0..256 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<u8>>();
    setup_handshake_and_put_bigkey(&mut stream, &mut acc, "test-client", &big_value);

    // Pipeline 300 big fetches on writer thread
    let mut write_stream = stream.try_clone().expect("clone stream for writing");
    write_stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    let writer_handle = std::thread::spawn(move || {
        let mut batch_buf = Vec::new();
        for req_id in 3..=302 {
            let cmd = Frame::new(
                Header::new(0, req_id, Opcode::Command, 0),
                Value::Array(vec![
                    Value::Text("fetch".to_string()),
                    Value::Text("bigkey".to_string()),
                ]),
            );
            encode_frame(&cmd, &mut batch_buf).expect("encode fetch");
        }
        write_stream.write_all(&batch_buf).expect("write fetches");
    });

    // Wait until backpressure triggers on the server (max 5s, poll every 10ms)
    let start_wait = Instant::now();
    while server.stats().backpressure_pauses.load(Ordering::Relaxed) == 0 {
        assert!(
            start_wait.elapsed() <= Duration::from_secs(5),
            "server failed to trigger backpressure within 5s; stats: pauses={}, max_queue={}, closed={}, frames={}",
            server.stats().backpressure_pauses.load(Ordering::Relaxed),
            server
                .stats()
                .max_write_queue_bytes_seen
                .load(Ordering::Relaxed),
            server.stats().connections_closed.load(Ordering::Relaxed),
            server.stats().frames_processed.load(Ordering::Relaxed),
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(server.stats().backpressure_pauses.load(Ordering::Relaxed) >= 1);
    let max_bytes = server
        .stats()
        .max_write_queue_bytes_seen
        .load(Ordering::Relaxed);
    let max_allowed = (64 * 1024) + (256 * 1024) + 64;
    assert!(
        max_bytes as usize <= max_allowed,
        "max_write_queue_bytes_seen was {max_bytes}, exceeded watermark limit {max_allowed}"
    );

    // Drain all 300 replies and verify in order
    drain_and_verify_replies(&mut stream, &mut acc, 3, 302, &Value::Bytes(big_value));
    writer_handle.join().expect("join writer");
}

#[test]
fn test_slow_reader_does_not_starve_other_clients() {
    let config = ServerConfig {
        write_queue_high_watermark: 64 * 1024,
        write_queue_low_watermark: 16 * 1024,
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream1 = server.connect();
    stream1
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set timeout");
    stream1
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");

    let mut acc1 = Vec::new();
    let big_value = (0..256 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<u8>>();
    setup_handshake_and_put_bigkey(&mut stream1, &mut acc1, "slow-client", &big_value);

    // Client 1 sends 300 big fetches without reading
    let mut write_stream1 = stream1.try_clone().expect("clone stream");
    write_stream1
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    let sender1 = std::thread::spawn(move || {
        let mut batch_buf = Vec::new();
        for req_id in 3..=302 {
            let cmd = Frame::new(
                Header::new(0, req_id, Opcode::Command, 0),
                Value::Array(vec![
                    Value::Text("fetch".to_string()),
                    Value::Text("bigkey".to_string()),
                ]),
            );
            encode_frame(&cmd, &mut batch_buf).expect("encode fetch");
        }
        write_stream1.write_all(&batch_buf).expect("write fetches");
    });

    // Wait until client 1 triggers backpressure
    let start_wait = Instant::now();
    while server.stats().backpressure_pauses.load(Ordering::Relaxed) == 0 {
        assert!(
            start_wait.elapsed() <= Duration::from_secs(5),
            "client 1 failed to trigger backpressure within 5s; stats: pauses={}, max_queue={}, closed={}, frames={}",
            server.stats().backpressure_pauses.load(Ordering::Relaxed),
            server
                .stats()
                .max_write_queue_bytes_seen
                .load(Ordering::Relaxed),
            server.stats().connections_closed.load(Ordering::Relaxed),
            server.stats().frames_processed.load(Ordering::Relaxed),
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // Client 2 connects and performs 200 ping round trips while client 1 is paused
    let mut c2 = Client::connect(server.addr()).expect("c2 connect");
    for _ in 0..200 {
        let start_ping = Instant::now();
        let pong = c2.ping().expect("c2 ping");
        let elapsed = start_ping.elapsed();
        assert_eq!(pong, "pong");
        assert!(
            elapsed < Duration::from_millis(500),
            "client 2 response took {elapsed:?}, expected < 500ms"
        );
    }

    // Client 1 reads all 300 replies correctly
    drain_and_verify_replies(&mut stream1, &mut acc1, 3, 302, &Value::Bytes(big_value));
    sender1.join().expect("join sender1");
}

#[test]
fn test_idle_timeout_closes_connection() {
    let config = ServerConfig {
        idle_timeout: Some(Duration::from_millis(300)),
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let client = Client::connect(server.addr()).expect("client connect");
    let mut raw_stream = server.connect();
    drop(client);

    raw_stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    let mut buf = [0u8; 16];
    let start = Instant::now();
    let res = raw_stream.read(&mut buf);
    let elapsed = start.elapsed();

    match res {
        Ok(0) => (),
        Err(ref e)
            if e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        other => panic!("expected EOF or reset on idle timeout, got: {other:?}"),
    }

    assert!(
        elapsed < Duration::from_secs(2),
        "idle timeout close took {elapsed:?}, expected < 2s"
    );
    assert!(
        server
            .stats()
            .connections_closed_idle
            .load(Ordering::Relaxed)
            >= 1,
        "connections_closed_idle counter must be incremented"
    );
}

#[test]
fn test_idle_timeout_with_garbage_incomplete_frame() {
    let config = ServerConfig {
        idle_timeout: Some(Duration::from_millis(300)),
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = TcpStream::connect(server.addr()).expect("connect");

    // Send 8 bytes of garbage (incomplete frame, less than 14-byte header)
    stream
        .write_all(&[0xCA, 0xDB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00])
        .expect("write garbage");

    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    let mut buf = [0u8; 16];
    let start = Instant::now();
    let res = stream.read(&mut buf);
    let elapsed = start.elapsed();

    match res {
        Ok(0) => (),
        Err(ref e)
            if e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        other => panic!("expected EOF on idle incomplete frame, got: {other:?}"),
    }

    assert!(
        elapsed < Duration::from_secs(2),
        "idle close took {elapsed:?}, expected < 2s"
    );
    assert!(
        server
            .stats()
            .connections_closed_idle
            .load(Ordering::Relaxed)
            >= 1,
        "connections_closed_idle counter must be incremented"
    );
}

#[test]
fn test_idle_timeout_reset_with_keepalive_pings() {
    let config = ServerConfig {
        idle_timeout: Some(Duration::from_millis(300)),
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut client = Client::connect(server.addr()).expect("client connect");

    // Send a ping every 100 ms for 1 s (total 10 pings) with 300 ms idle timeout
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        let reply = client.ping().expect("ping during keepalive");
        assert_eq!(reply, "pong");
    }

    assert_eq!(
        server
            .stats()
            .connections_closed_idle
            .load(Ordering::Relaxed),
        0,
        "active client must not be closed by idle timeout"
    );
}

#[test]
fn test_fairness_large_pipeline_does_not_starve_ping() {
    let config = ServerConfig {
        max_frames_per_event: 256,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let total_commands = 100_000usize;
    let server_addr = server.addr();

    let client1_handle = std::thread::spawn(move || {
        let mut c1 = Client::connect(server_addr).expect("c1 connect");
        let commands: Vec<Vec<Value>> = (0..total_commands)
            .map(|_| vec![Value::Text("ping".to_string())])
            .collect();
        let replies = c1.pipeline(commands).expect("c1 pipeline");
        assert_eq!(replies.len(), total_commands);
    });

    std::thread::sleep(Duration::from_millis(20));

    // Client 2 connects and pings: must complete in < 50 ms despite huge pipeline
    let start_c2 = Instant::now();
    let mut c2 = Client::connect(server.addr()).expect("c2 connect");
    let pong = c2.ping().expect("c2 ping");
    let elapsed = start_c2.elapsed();

    assert_eq!(pong, "pong");
    assert!(
        elapsed < Duration::from_millis(50),
        "client 2 ping took {elapsed:?}, expected < 50ms"
    );

    client1_handle.join().expect("join client1");
}

#[test]
fn test_disconnect_while_paused_cleans_up() {
    let config = ServerConfig {
        write_queue_high_watermark: 64 * 1024,
        write_queue_low_watermark: 16 * 1024,
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");

    let mut acc = Vec::new();
    let big_value = (0..256 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<u8>>();
    setup_handshake_and_put_bigkey(&mut stream, &mut acc, "pause-disconnect-client", &big_value);

    // Client sends big fetches on background writer thread
    let mut write_stream = stream.try_clone().expect("clone stream");
    write_stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    let writer_handle = std::thread::spawn(move || {
        let mut batch_buf = Vec::new();
        for req_id in 3..=302 {
            let cmd = Frame::new(
                Header::new(0, req_id, Opcode::Command, 0),
                Value::Array(vec![
                    Value::Text("fetch".to_string()),
                    Value::Text("bigkey".to_string()),
                ]),
            );
            encode_frame(&cmd, &mut batch_buf).expect("encode fetch");
        }
        let _ = write_stream.write_all(&batch_buf);
    });

    // Wait until backpressure triggers on the server
    let start_wait = Instant::now();
    while server.stats().backpressure_pauses.load(Ordering::Relaxed) == 0 {
        assert!(
            start_wait.elapsed() <= Duration::from_secs(5),
            "must trigger backpressure before disconnect; stats: pauses={}, max_queue={}, closed={}, frames={}",
            server.stats().backpressure_pauses.load(Ordering::Relaxed),
            server
                .stats()
                .max_write_queue_bytes_seen
                .load(Ordering::Relaxed),
            server.stats().connections_closed.load(Ordering::Relaxed),
            server.stats().frames_processed.load(Ordering::Relaxed),
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(server.stats().backpressure_pauses.load(Ordering::Relaxed) >= 1);
    let closed_before = server.stats().connections_closed.load(Ordering::Relaxed);

    // Abruptly drop/close the socket while paused
    drop(stream);
    let _ = writer_handle.join();

    // Assert connections_closed increases within 2s
    let start_closed = Instant::now();
    while server.stats().connections_closed.load(Ordering::Relaxed) <= closed_before {
        assert!(
            start_closed.elapsed() <= Duration::from_secs(2),
            "connections_closed counter failed to increment within 2s; stats: pauses={}, max_queue={}, closed={}, frames={}",
            server.stats().backpressure_pauses.load(Ordering::Relaxed),
            server
                .stats()
                .max_write_queue_bytes_seen
                .load(Ordering::Relaxed),
            server.stats().connections_closed.load(Ordering::Relaxed),
            server.stats().frames_processed.load(Ordering::Relaxed),
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // Verify server remains responsive and healthy for a second client
    let mut healthy_client = Client::connect(server.addr()).expect("healthy connect");
    assert_eq!(healthy_client.ping().expect("healthy ping"), "pong");
}

#[test]
fn test_idle_timeout_none_stays_open() {
    let config = ServerConfig {
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut client = Client::connect(server.addr()).expect("client connect");

    // Connection stays completely idle for 1 second
    std::thread::sleep(Duration::from_secs(1));

    // Connection must still be open and operational
    let reply = client.ping().expect("ping after 1s idle with None");
    assert_eq!(reply, "pong");
}

#[test]
fn test_half_close_pipelined_pings() {
    let config = ServerConfig {
        max_frames_per_event: 16,
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");

    let mut acc = Vec::new();
    let _hello_resp = raw_handshake(&mut stream, &mut acc, "half-close-client");

    // Write 1,000 ping requests in one write
    let mut all_pings = Vec::new();
    for req_id in 2..=1001 {
        let cmd = Frame::new(
            Header::new(0, req_id, Opcode::Command, 0),
            Value::Array(vec![Value::Text("ping".to_string())]),
        );
        encode_frame(&cmd, &mut all_pings).expect("encode ping");
    }
    stream.write_all(&all_pings).expect("write 1000 pings");

    // Shutdown write half (half-close)
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown write");

    // Read all 1,000 replies
    let expected_pong = Value::Text("pong".to_string());
    for req_id in 2..=1001 {
        let frame = read_one_frame(&mut stream, &mut acc);
        expect_ok_reply(&frame, req_id);
        assert_eq!(frame.payload, expected_pong);
    }

    // After all 1,000 replies, server should close connection (read returns EOF)
    let mut eof_buf = [0u8; 16];
    let n = stream
        .read(&mut eof_buf)
        .expect("read after half-close EOF");
    assert_eq!(n, 0, "expected EOF from server after reading all replies");
}

#[test]
fn test_half_close_with_backpressure_and_big_fetches() {
    let config = ServerConfig {
        write_queue_high_watermark: 64 * 1024,
        write_queue_low_watermark: 16 * 1024,
        max_frames_per_event: 16,
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");

    let mut acc = Vec::new();
    let big_value = (0..256 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<u8>>();
    setup_handshake_and_put_bigkey(&mut stream, &mut acc, "half-close-big-client", &big_value);

    // Write 50 big fetches (12.5 MB of replies with 64 KB watermark guarantees backpressure)
    let total_fetches = 50u32;
    let mut all_fetches = Vec::new();
    for req_id in 3..=(2 + total_fetches) {
        let cmd = Frame::new(
            Header::new(0, req_id, Opcode::Command, 0),
            Value::Array(vec![
                Value::Text("fetch".to_string()),
                Value::Text("bigkey".to_string()),
            ]),
        );
        encode_frame(&cmd, &mut all_fetches).expect("encode fetch");
    }
    stream.write_all(&all_fetches).expect("write fetches");

    // Shutdown write
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown write");

    // Drain all replies until EOF
    drain_and_verify_replies(
        &mut stream,
        &mut acc,
        3,
        2 + total_fetches,
        &Value::Bytes(big_value),
    );

    // Verify EOF after draining all replies
    let mut eof_buf = [0u8; 16];
    let n = stream
        .read(&mut eof_buf)
        .expect("read after big half-close EOF");
    assert_eq!(
        n, 0,
        "expected EOF from server after reading all fetch replies"
    );

    assert!(
        server.stats().backpressure_pauses.load(Ordering::Relaxed) >= 1,
        "must have triggered backpressure during big half-close"
    );
}

#[test]
fn test_half_close_incomplete_frame_drops_and_closes() {
    let config = ServerConfig {
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");

    // Send half a frame (7 bytes: incomplete header)
    stream
        .write_all(&[0xCA, 0xDB, 0x01, 0x00, 0x00, 0x00, 0x00])
        .expect("write partial frame");

    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown write");

    // Server must close socket gracefully (EOF) within 2 seconds
    let mut buf = [0u8; 16];
    let res = stream.read(&mut buf);
    match res {
        Ok(0) => {}
        Err(ref e)
            if e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        other => panic!("expected EOF on half-close with incomplete frame, got: {other:?}"),
    }

    // Server stays completely healthy for next client
    let mut healthy_client = Client::connect(server.addr()).expect("connect next client");
    assert_eq!(healthy_client.ping().expect("ping"), "pong");
}

#[test]
fn test_half_close_single_request_receives_reply() {
    let config = ServerConfig {
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");

    // Send ping command then immediately shutdown write
    let ping_cmd = Frame::new(
        Header::new(0, 1, Opcode::Command, 0),
        Value::Array(vec![Value::Text("ping".to_string())]),
    );
    let mut ping_bytes = Vec::new();
    encode_frame(&ping_cmd, &mut ping_bytes).expect("encode ping");
    stream.write_all(&ping_bytes).expect("send ping");

    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown write");

    // Must receive reply then EOF
    let mut acc = Vec::new();
    let reply = read_one_frame(&mut stream, &mut acc);
    expect_ok_reply(&reply, 1);
    assert_eq!(reply.payload, Value::Text("pong".to_string()));

    let mut eof_buf = [0u8; 16];
    let n = stream.read(&mut eof_buf).expect("read after ping reply");
    assert_eq!(n, 0, "must receive EOF after half-close reply");
}

#[test]
fn test_malformed_hello_payload_rejected_with_1006_and_closed() {
    use skiffdb::proto::header::{FLAG_ERROR, FLAG_RESPONSE};

    let config = ServerConfig {
        idle_timeout: None,
        ..ServerConfig::default()
    };
    let server = TestServer::start_with_config(config);

    let mut stream = server.connect();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set write timeout");

    // Send malformed hello frame (buggy test payload: ["hello", 1, "x"])
    let bad_hello = Frame::new(
        Header::new(0, 1, Opcode::Hello, 0),
        Value::Array(vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("x".to_string()),
        ]),
    );
    let mut bad_bytes = Vec::new();
    encode_frame(&bad_hello, &mut bad_bytes).expect("encode malformed hello");
    stream.write_all(&bad_bytes).expect("send malformed hello");

    // Read one reply frame
    let mut acc = Vec::new();
    let reply = read_one_frame(&mut stream, &mut acc);

    // Assert it is an ERROR reply with code 1006 (ERR_MALFORMED_PAYLOAD)
    assert!(
        reply.header.flags & FLAG_RESPONSE != 0,
        "reply must have FLAG_RESPONSE set"
    );
    assert!(
        reply.header.flags & FLAG_ERROR != 0,
        "reply must have FLAG_ERROR set"
    );
    assert_eq!(reply.header.request_id, 1);
    match reply.payload {
        Value::Error { code, .. } => {
            assert_eq!(code, 1006, "expected ERR_MALFORMED_PAYLOAD (1006)");
        }
        other => panic!("expected Value::Error, got {other:?}"),
    }

    // Assert server closes the connection (next read is EOF or reset)
    let mut buf = [0u8; 16];
    let res = stream.read(&mut buf);
    match res {
        Ok(0) => {}
        Err(ref e)
            if e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        other => panic!("expected EOF or ConnectionReset after malformed hello, got {other:?}"),
    }

    // Subsequent client can connect and ping cleanly
    let mut client = Client::connect(server.addr()).expect("subsequent client connect");
    assert_eq!(client.ping().expect("subsequent client ping"), "pong");
}

/// Verifies that pipelined batches do not suffer the ~40 ms Nagle + delayed-ACK stall.
///
/// Prior to enabling `TCP_NODELAY` on server-accepted sockets, each pipelined batch of
/// small replies caused the server kernel to transmit the first frame and hold subsequent
/// replies waiting for an ACK under Nagle's algorithm, while the client kernel delayed its
/// ACK by ~40 ms. 200 batches of 16 commands would take ~8 seconds.
///
/// With `TCP_NODELAY` enabled on both ends, 200 batches of 16 pipelined `ping` commands
/// execute in well under 1 second. We assert a generous bound of 3 seconds to avoid CI noise.
#[test]
fn test_pipelined_pings_no_nagle_delayed_ack_stall() {
    let mut server = TestServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    let ping_batch: Vec<Vec<Value>> = (0..16)
        .map(|_| vec![Value::Text("ping".to_string())])
        .collect();

    let start = Instant::now();
    for _ in 0..200 {
        let replies = client
            .pipeline(ping_batch.clone())
            .expect("pipeline batch must succeed");
        assert_eq!(replies.len(), 16);
        for reply in replies {
            assert_eq!(
                reply.expect("ping command reply must be Ok"),
                Value::Text("pong".to_string())
            );
        }
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(3),
        "pipelined batches took {elapsed:?}, expected under 3s without Nagle/delayed-ACK stall"
    );

    assert_eq!(
        server.stats().nodelay_failures.load(Ordering::Relaxed),
        0,
        "nodelay_failures counter must be 0"
    );

    server.stop();
}
