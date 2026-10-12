//! Linux-specific server hardening, backpressure, fairness, and idle timeout end-to-end tests.

#![cfg(target_os = "linux")]

mod common;

use cachedb::client::Client;
use cachedb::proto::encode::encode_frame;
use cachedb::proto::header::{Header, Opcode};
use cachedb::proto::{Frame, Value};
use cachedb::server::ServerConfig;
use common::TestServer;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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
    // Handshake
    let hello_cmd = Frame::new(
        Header::new(0, 1, Opcode::Hello, 0),
        Value::Array(vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("test-client".to_string()),
        ]),
    );
    let mut hello_bytes = Vec::new();
    encode_frame(&hello_cmd, &mut hello_bytes).expect("encode hello");
    stream.write_all(&hello_bytes).expect("send hello");

    let mut hello_reply_buf = vec![0u8; 1024];
    let n = stream.read(&mut hello_reply_buf).expect("read hello reply");
    assert!(n > 0, "must receive hello reply");

    // Client sends 50,000 pings without reading on a background writer thread
    let mut write_stream = stream.try_clone().expect("clone stream for writing");
    let writer_handle = std::thread::spawn(move || {
        for req_id in 2..=50_001 {
            let cmd = Frame::new(
                Header::new(0, req_id, Opcode::Command, 0),
                Value::Array(vec![Value::Text("ping".to_string())]),
            );
            let mut buf = Vec::new();
            encode_frame(&cmd, &mut buf).expect("encode ping");
            if write_stream.write_all(&buf).is_err() {
                break;
            }
        }
    });

    // Wait until backpressure triggers on the server
    let start_wait = Instant::now();
    while server.stats().backpressure_pauses.load(Ordering::Relaxed) == 0 {
        assert!(
            start_wait.elapsed() <= Duration::from_secs(5),
            "server failed to trigger backpressure within 5s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(server.stats().backpressure_pauses.load(Ordering::Relaxed) >= 1);
    let max_bytes = server
        .stats()
        .max_write_queue_bytes_seen
        .load(Ordering::Relaxed);
    // Max observed write queue size must be bounded around high watermark + one reply frame
    assert!(
        max_bytes <= 64 * 1024 + 4096,
        "write queue exceeded watermark bound: {max_bytes}"
    );

    // Client resumes and drains all 50,000 replies
    let mut drain_buf = vec![0u8; 64 * 1024];
    let mut total_drained = 0usize;
    let expected_replies = 50_000usize;
    let mut frames_counted = 0usize;

    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set timeout");
    let mut read_acc = Vec::new();

    while frames_counted < expected_replies {
        let n = stream.read(&mut drain_buf).expect("read reply chunk");
        assert!(n > 0, "unexpected EOF while draining");
        total_drained += n;
        read_acc.extend_from_slice(&drain_buf[..n]);

        while !read_acc.is_empty() {
            match cachedb::proto::decode_frame(&read_acc) {
                Ok(cachedb::proto::Decoded::Complete((_, used))) => {
                    frames_counted += 1;
                    read_acc.drain(..used);
                }
                Ok(cachedb::proto::Decoded::NeedMore) => break,
                Err(e) => panic!("decode error while draining: {e:?}"),
            }
        }
    }

    writer_handle.join().expect("join writer");
    assert_eq!(frames_counted, expected_replies);
    assert!(total_drained > 0);
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

    // Client 1 connects and blasts pings without reading
    let mut stream1 = server.connect();
    let hello_cmd = Frame::new(
        Header::new(0, 1, Opcode::Hello, 0),
        Value::Array(vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("slow-client".to_string()),
        ]),
    );
    let mut hello_bytes = Vec::new();
    encode_frame(&hello_cmd, &mut hello_bytes).expect("encode hello");
    stream1.write_all(&hello_bytes).expect("send hello");

    let mut hello_reply_buf = vec![0u8; 1024];
    let n1 = stream1
        .read(&mut hello_reply_buf)
        .expect("read hello reply");
    assert!(n1 > 0, "must receive hello reply");

    let mut write_stream1 = stream1.try_clone().expect("clone stream");
    let sender1 = std::thread::spawn(move || {
        for req_id in 2..=50_000 {
            let cmd = Frame::new(
                Header::new(0, req_id, Opcode::Command, 0),
                Value::Array(vec![Value::Text("ping".to_string())]),
            );
            let mut buf = Vec::new();
            encode_frame(&cmd, &mut buf).expect("encode ping");
            if write_stream1.write_all(&buf).is_err() {
                break;
            }
        }
    });

    // Wait until client 1 triggers backpressure
    let start_wait = Instant::now();
    while server.stats().backpressure_pauses.load(Ordering::Relaxed) == 0 {
        assert!(
            start_wait.elapsed() <= Duration::from_secs(5),
            "client 1 failed to trigger backpressure within 5s"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    // Client 2 connects and sends ping while client 1 is paused
    let start_c2 = Instant::now();
    let mut c2 = Client::connect(server.addr()).expect("c2 connect");
    let pong = c2.ping().expect("c2 ping");
    let elapsed = start_c2.elapsed();

    assert_eq!(pong, "pong");
    assert!(
        elapsed < Duration::from_millis(100),
        "client 2 response took {elapsed:?}, expected < 100ms"
    );

    drop(stream1);
    let _ = sender1.join();
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

    // Client connects and does nothing for 300 ms
    raw_stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    let mut buf = [0u8; 16];
    let start = Instant::now();
    let res = raw_stream.read(&mut buf);
    let elapsed = start.elapsed();

    // Server must close within 2 s (typically around 300-450 ms)
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

    // Client 1 prepares 100,000 commands
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

    // Let Client 1 start transmitting
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
    let hello_cmd = Frame::new(
        Header::new(0, 1, Opcode::Hello, 0),
        Value::Array(vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("pause-disconnect-client".to_string()),
        ]),
    );
    let mut hello_bytes = Vec::new();
    encode_frame(&hello_cmd, &mut hello_bytes).expect("encode hello");
    stream.write_all(&hello_bytes).expect("send hello");

    let mut hello_reply_buf = vec![0u8; 1024];
    let n_hello = stream.read(&mut hello_reply_buf).expect("read hello reply");
    assert!(n_hello > 0, "must receive hello reply");

    // Blast pings until backpressure triggers
    for req_id in 2..=10_000 {
        let cmd = Frame::new(
            Header::new(0, req_id, Opcode::Command, 0),
            Value::Array(vec![Value::Text("ping".to_string())]),
        );
        let mut buf = Vec::new();
        encode_frame(&cmd, &mut buf).expect("encode ping");
        if stream.write_all(&buf).is_err() {
            break;
        }
        if server.stats().backpressure_pauses.load(Ordering::Relaxed) > 0 {
            break;
        }
    }

    assert!(
        server.stats().backpressure_pauses.load(Ordering::Relaxed) >= 1,
        "must trigger backpressure before disconnect"
    );

    // Abruptly drop/close the socket while paused
    drop(stream);

    // Give server event loop a moment to reap the closed socket
    std::thread::sleep(Duration::from_millis(100));

    // Verify server remains responsive and healthy
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
