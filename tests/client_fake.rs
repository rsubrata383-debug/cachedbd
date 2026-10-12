//! Integration and fault injection tests for [`cachedb::client::Client`].
//!
//! Evaluates round-trip commands, pipelining, typing helpers, server error mapping,
//! model conformance, and error resilience against misbehaving servers using a
//! pure `std` blocking fake server.
//!
//! Runs identically across Linux and Windows.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::too_many_lines
)]

mod common;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use cachedb::client::{Client, ClientConfig, ClientError};
use cachedb::proto::decode::decode_frame;
use cachedb::proto::encode::encode_frame;
use cachedb::proto::header::{FLAG_RESPONSE, Header, Opcode};
use cachedb::proto::{Decoded, Frame, Value};
use common::{TestBlockingServer, XorShift64};

// 1. connect + hello: verifies server_info metadata
#[test]
fn test_client_connect_and_hello() {
    let server = TestBlockingServer::start();
    let client = Client::connect(server.addr()).expect("client connect");

    let info = client.server_info();
    assert_eq!(info.server_name(), "cachedbd");
    assert_eq!(info.server_version(), "0.1.0");
    assert_eq!(info.client_id(), 1);
    assert!(info.max_frame_size().is_some());
    assert!(info.max_key_size().is_some());
    assert!(info.max_value_size().is_some());
}

// 2. ping: tests simple round-trip
#[test]
fn test_client_ping() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    assert_eq!(client.ping().expect("ping"), "pong");
}

// 3. put, fetch, drop_keys, has, size, bump round trips
#[test]
fn test_client_round_trips() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    assert_eq!(client.size().expect("initial size"), 0);

    // put
    let stored = client.put(b"counter", Value::Int(42)).expect("put counter");
    assert!(stored);
    assert_eq!(client.size().expect("size after put"), 1);

    // has
    assert!(client.has(b"counter").expect("has counter"));
    assert!(!client.has(b"nonexistent").expect("has nonexistent"));

    // fetch
    let val = client.fetch(b"counter").expect("fetch counter");
    assert_eq!(val, Some(Value::Int(42)));

    // bump
    let bumped = client.bump(b"counter", Some(8)).expect("bump counter +8");
    assert_eq!(bumped, 50);

    let bumped_default = client.bump(b"counter", None).expect("bump counter +1");
    assert_eq!(bumped_default, 51);

    // drop_keys
    let dropped = client.drop_keys(&[b"counter" as &[u8]]).expect("drop");
    assert_eq!(dropped, 1);
    assert_eq!(client.size().expect("size after drop"), 0);
    assert_eq!(client.fetch(b"counter").expect("fetch dropped"), None);
}

// 4. put_nx and put_xx
#[test]
fn test_client_put_nx_and_put_xx() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    // NX on absent -> true
    assert!(
        client
            .put_nx(b"key_nx", Value::Text("first".to_string()))
            .expect("put_nx absent")
    );
    // NX on existing -> false
    assert!(
        !client
            .put_nx(b"key_nx", Value::Text("second".to_string()))
            .expect("put_nx existing")
    );
    assert_eq!(
        client.fetch(b"key_nx").expect("fetch"),
        Some(Value::Text("first".to_string()))
    );

    // XX on absent -> false
    assert!(
        !client
            .put_xx(b"key_xx", Value::Text("val".to_string()))
            .expect("put_xx absent")
    );
    assert_eq!(client.fetch(b"key_xx").expect("fetch absent"), None);

    // Put it first, then XX on existing -> true
    client
        .put(b"key_xx", Value::Text("initial".to_string()))
        .expect("put");
    assert!(
        client
            .put_xx(b"key_xx", Value::Text("updated".to_string()))
            .expect("put_xx existing")
    );
    assert_eq!(
        client.fetch(b"key_xx").expect("fetch updated"),
        Some(Value::Text("updated".to_string()))
    );
}

// 5. fetch missing = None, stored Nil also = None
#[test]
fn test_client_fetch_missing_and_nil() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    // Missing key returns None
    assert_eq!(client.fetch(b"not_found").expect("missing key"), None);
    assert!(!client.has(b"not_found").expect("has not_found"));

    // Stored Nil also returns None, but has returns true
    assert!(client.put(b"nil_key", Value::Nil).expect("put Nil"));
    assert_eq!(client.fetch(b"nil_key").expect("fetch Nil"), None);
    assert!(client.has(b"nil_key").expect("has nil_key"));
}

// 6. server error mapping: unknown command (2001) and bump on text "abc" (3004)
#[test]
fn test_client_server_error_mapping() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    // Unknown command -> Server { code: 2001 }
    let err = client
        .command(vec![Value::Text("non_existent_command".to_string())])
        .unwrap_err();
    match err {
        ClientError::Server { code, message } => {
            assert_eq!(code, 2001);
            assert_eq!(message, "unknown command");
        }
        other => panic!("expected Server error 2001, got {other:?}"),
    }

    // Bump on text "abc" -> Server { code: 3004 }
    client
        .put(b"str_key", Value::Text("abc".to_string()))
        .expect("put text");
    let bump_err = client.bump(b"str_key", Some(1)).unwrap_err();
    match bump_err {
        ClientError::Server { code, message } => {
            assert_eq!(code, 3004);
            assert_eq!(message, "value is not an integer");
        }
        other => panic!("expected Server error 3004, got {other:?}"),
    }

    // The client remains completely usable after non-fatal server errors
    assert_eq!(client.ping().expect("ping after server errors"), "pong");
}

// 7. pipeline of 1000 mixed commands in order
#[test]
fn test_client_pipeline_1000_commands() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    let count = 1000;
    let mut commands = Vec::with_capacity(count);

    for i in 0..count {
        if i % 2 == 0 {
            commands.push(vec![
                Value::Text("put".to_string()),
                Value::Bytes(format!("pipe_{i}").into_bytes()),
                Value::Int(i as i64),
            ]);
        } else {
            commands.push(vec![Value::Text("ping".to_string())]);
        }
    }

    let results = client.pipeline(commands).expect("pipeline execute");
    assert_eq!(results.len(), count);

    for (i, res) in results.into_iter().enumerate() {
        let val = res.expect("command success");
        if i % 2 == 0 {
            assert_eq!(val, Value::Bool(true));
        } else {
            assert_eq!(val, Value::Text("pong".to_string()));
        }
    }
}

// 8. pipeline where some commands fail
#[test]
fn test_client_pipeline_with_failures() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    client
        .put(b"bad_bump_key", Value::Text("letters".to_string()))
        .expect("setup bad key");

    let commands = vec![
        vec![Value::Text("ping".to_string())],
        vec![Value::Text("invalid_verb_123".to_string())],
        vec![
            Value::Text("bump".to_string()),
            Value::Bytes(b"bad_bump_key".to_vec()),
        ],
        vec![Value::Text("size".to_string())],
    ];

    let results = client.pipeline(commands).expect("pipeline with failures");
    assert_eq!(results.len(), 4);

    // Command 0: ping -> ok
    assert_eq!(
        results[0].as_ref().unwrap(),
        &Value::Text("pong".to_string())
    );

    // Command 1: unknown command -> error 2001
    match &results[1] {
        Err(ClientError::Server { code, .. }) => assert_eq!(*code, 2001),
        other => panic!("expected 2001 error, got {other:?}"),
    }

    // Command 2: bump on string -> error 3004
    match &results[2] {
        Err(ClientError::Server { code, .. }) => assert_eq!(*code, 3004),
        other => panic!("expected 3004 error, got {other:?}"),
    }

    // Command 3: size -> ok
    assert_eq!(results[3].as_ref().unwrap(), &Value::Int(1));
}

// 9. large value 1 MB round trip
#[test]
fn test_client_large_value_1mb() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    let payload = vec![0x7A; 1024 * 1024]; // 1 MB
    assert!(
        client
            .put(b"large_key", Value::Bytes(payload.clone()))
            .expect("put 1mb")
    );

    let retrieved = client.fetch(b"large_key").expect("fetch 1mb");
    assert_eq!(retrieved, Some(Value::Bytes(payload)));

    assert_eq!(client.drop_keys(&[b"large_key" as &[u8]]).expect("drop"), 1);
}

// 10. quit
#[test]
fn test_client_quit() {
    let server = TestBlockingServer::start();
    let client = Client::connect(server.addr()).expect("client connect");

    client.quit().expect("quit successful");
}

// -----------------------------------------------------------------------------
// BAD Server Tests: Fault injection and error resilience
// -----------------------------------------------------------------------------

// Bad Server 1: Closes right after accept
#[test]
fn test_bad_server_closes_after_accept() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            drop(stream); // Close immediately
        }
    });

    let res = Client::connect(addr);
    assert_eq!(res.unwrap_err(), ClientError::ConnectionClosed);
}

// Bad Server 2: Sends garbage bytes
#[test]
fn test_bad_server_sends_garbage() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            // Send 14 bytes of invalid magic
            let _ = stream.write_all(&[
                0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
            ]);
        }
    });

    let res = Client::connect(addr);
    match res.unwrap_err() {
        ClientError::Protocol(msg) => assert!(msg.contains("magic") || msg.contains("header")),
        other => panic!("expected Protocol error, got {other:?}"),
    }
}

// Bad Server 3: Sends reply with wrong request_id
#[test]
fn test_bad_server_wrong_request_id() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);

            // Send valid hello reply with wrong request_id (999 instead of 1)
            let entries = vec![
                (
                    Value::Text("server_name".to_string()),
                    Value::Text("cachedbd".to_string()),
                ),
                (
                    Value::Text("server_version".to_string()),
                    Value::Text("0.1.0".to_string()),
                ),
                (Value::Text("protocol_version".to_string()), Value::Int(1)),
                (Value::Text("client_id".to_string()), Value::Int(1)),
            ];
            let frame = Frame::new(
                Header::new(FLAG_RESPONSE, 999, Opcode::Hello, 0),
                Value::Map(entries),
            );
            let mut out = Vec::new();
            encode_frame(&frame, &mut out).expect("encode");
            let _ = stream.write_all(&out);
        }
    });

    let res = Client::connect(addr);
    match res.unwrap_err() {
        ClientError::Protocol(msg) => assert!(msg.contains("mismatched request_id")),
        other => panic!("expected Protocol error with mismatched request_id, got {other:?}"),
    }
}

// Bad Server 4: Sends reply claiming a huge payload_len
#[test]
fn test_bad_server_huge_payload_len() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);

            // Send 14-byte header claiming payload_len = 50,000 bytes with magic 0xCA 0xDB
            let mut header = [0u8; 14];
            header[0] = 0xCA;
            header[1] = 0xDB;
            header[2] = 0x01; // version
            header[3] = FLAG_RESPONSE;
            header[4..8].copy_from_slice(&1u32.to_be_bytes()); // req_id = 1
            header[8..10].copy_from_slice(&(Opcode::Hello as u16).to_be_bytes());
            header[10..14].copy_from_slice(&50_000u32.to_be_bytes()); // 50,000 bytes payload

            let _ = stream.write_all(&header);
        }
    });

    // Configure max_reply_size to 1,000 bytes
    let config = ClientConfig::new().with_max_reply_size(1000);
    let res = Client::connect_with(addr, config);
    assert_eq!(res.unwrap_err(), ClientError::ReplyTooLarge);
}

// Bad Server 5: Sends half a frame then closes
#[test]
fn test_bad_server_half_frame_then_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);

            // Send valid header declaring 20 bytes payload, but only send 5 bytes then close
            let mut header = [0u8; 14];
            header[0] = 0xCA;
            header[1] = 0xDB;
            header[2] = 0x01;
            header[3] = FLAG_RESPONSE;
            header[4..8].copy_from_slice(&1u32.to_be_bytes());
            header[8..10].copy_from_slice(&(Opcode::Hello as u16).to_be_bytes());
            header[10..14].copy_from_slice(&20u32.to_be_bytes());

            let _ = stream.write_all(&header);
            let _ = stream.write_all(&[1, 2, 3, 4, 5]);
            drop(stream); // Disconnect
        }
    });

    let res = Client::connect(addr);
    assert_eq!(res.unwrap_err(), ClientError::ConnectionClosed);
}

// Bad Server 6: Never replies (read timeout)
#[test]
fn test_bad_server_read_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        if let Ok((_stream, _)) = listener.accept() {
            thread::sleep(Duration::from_millis(500));
        }
    });

    let config = ClientConfig::new().with_read_timeout(Some(Duration::from_millis(50)));
    let res = Client::connect_with(addr, config);

    match res.unwrap_err() {
        ClientError::Io(err) => {
            assert!(
                err.kind() == std::io::ErrorKind::TimedOut
                    || err.kind() == std::io::ErrorKind::WouldBlock
            );
        }
        other => panic!("expected timeout Io error, got {other:?}"),
    }
}

// -----------------------------------------------------------------------------
// Model Test: 20,000 random commands verified against HashMap
// -----------------------------------------------------------------------------

#[test]
fn test_model_20k_random_commands() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    // Strictly compliant XorShift64 PRNG per rule 11.1
    let mut rng = XorShift64::new(0x89AB_CDEF_0123_4567);
    let mut model: HashMap<Vec<u8>, Value> = HashMap::new();

    // Pool of 40 keys to force collisions and concurrent key updates
    let key_pool: Vec<Vec<u8>> = (0..40)
        .map(|i| format!("model_key_{i}").into_bytes())
        .collect();

    for _ in 0..20_000 {
        let op = rng.gen_range(0, 8);
        let key_idx = rng.gen_range(0, key_pool.len());
        let key = &key_pool[key_idx];

        match op {
            0 => {
                // put
                let val = Value::Int(rng.next_u64() as i64);
                let actual = client.put(key, val.clone()).expect("put");
                model.insert(key.clone(), val);
                assert!(actual);
            }
            1 => {
                // put_nx
                let val = Value::Int(rng.next_u64() as i64);
                let expected = !model.contains_key(key);
                let actual = client.put_nx(key, val.clone()).expect("put_nx");
                assert_eq!(actual, expected);
                if expected {
                    model.insert(key.clone(), val);
                }
            }
            2 => {
                // put_xx
                let val = Value::Int(rng.next_u64() as i64);
                let expected = model.contains_key(key);
                let actual = client.put_xx(key, val.clone()).expect("put_xx");
                assert_eq!(actual, expected);
                if expected {
                    model.insert(key.clone(), val);
                }
            }
            3 => {
                // fetch
                let expected = model.get(key).cloned();
                let actual = client.fetch(key).expect("fetch");
                assert_eq!(actual, expected);
            }
            4 => {
                // drop_keys
                let expected = i64::from(model.remove(key).is_some());
                let actual = client.drop_keys(&[&key[..]]).expect("drop_keys");
                assert_eq!(actual, expected);
            }
            5 => {
                // has
                let expected = model.contains_key(key);
                let actual = client.has(key).expect("has");
                assert_eq!(actual, expected);
            }
            6 => {
                // size
                let expected = model.len() as i64;
                let actual = client.size().expect("size");
                assert_eq!(actual, expected);
            }
            7 => {
                // bump on Int values
                let step = (rng.next_u64() as i64) % 100;
                let current = model.get(key);
                let current_int = match current {
                    Some(Value::Int(n)) => Some(*n),
                    _ => None,
                };

                let new_val = match current_int {
                    Some(n) => n.checked_add(step).expect("no overflow"),
                    None => step,
                };

                let actual = client.bump(key, Some(step)).expect("bump");
                assert_eq!(actual, new_val);
                model.insert(key.clone(), Value::Int(new_val));
            }
            _ => unreachable!(),
        }
    }
}

// 18. pipeline chunking prevents deadlock with server that restricts reads
#[test]
fn test_client_pipeline_chunking_deadlock_prevention() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = Vec::new();
        let mut temp = [0u8; 8192];

        // 1. Read hello and reply
        loop {
            match decode_frame(&buf) {
                Ok(Decoded::Complete((frame, used))) => {
                    buf.drain(..used);
                    assert_eq!(frame.header.opcode, Opcode::Hello);
                    let hello_reply = Frame::new(
                        Header::new(FLAG_RESPONSE, frame.header.request_id, Opcode::Hello, 0),
                        Value::Map(vec![
                            (
                                Value::Text("server_name".to_string()),
                                Value::Text("cachedbd".to_string()),
                            ),
                            (
                                Value::Text("server_version".to_string()),
                                Value::Text("0.1.0".to_string()),
                            ),
                            (Value::Text("protocol_version".to_string()), Value::Int(1)),
                            (Value::Text("client_id".to_string()), Value::Int(1)),
                        ]),
                    );
                    let mut out = Vec::new();
                    encode_frame(&hello_reply, &mut out).expect("encode");
                    stream.write_all(&out).expect("write");
                    break;
                }
                Ok(Decoded::NeedMore) => {
                    let n = stream.read(&mut temp).expect("read");
                    buf.extend_from_slice(&temp[..n]);
                }
                Err(e) => panic!("error {e:?}"),
            }
        }

        // 2. Read exactly 256 commands (Chunk 1) and send their replies BEFORE reading chunk 2!
        for expected_id in 2..(2 + 256) {
            loop {
                match decode_frame(&buf) {
                    Ok(Decoded::Complete((frame, used))) => {
                        buf.drain(..used);
                        assert_eq!(frame.header.request_id, expected_id as u32);
                        let reply = Frame::new(
                            Header::new(FLAG_RESPONSE, frame.header.request_id, Opcode::Command, 0),
                            Value::Text("pong".to_string()),
                        );
                        let mut out = Vec::new();
                        encode_frame(&reply, &mut out).expect("encode");
                        stream.write_all(&out).expect("write reply");
                        break;
                    }
                    Ok(Decoded::NeedMore) => {
                        let n = stream.read(&mut temp).expect("read");
                        buf.extend_from_slice(&temp[..n]);
                    }
                    Err(e) => panic!("error {e:?}"),
                }
            }
        }

        // 3. Now read the remaining 44 commands (Chunk 2, 300 total) and reply
        for expected_id in (2 + 256)..(2 + 300) {
            loop {
                match decode_frame(&buf) {
                    Ok(Decoded::Complete((frame, used))) => {
                        buf.drain(..used);
                        assert_eq!(frame.header.request_id, expected_id as u32);
                        let reply = Frame::new(
                            Header::new(FLAG_RESPONSE, frame.header.request_id, Opcode::Command, 0),
                            Value::Text("pong".to_string()),
                        );
                        let mut out = Vec::new();
                        encode_frame(&reply, &mut out).expect("encode");
                        stream.write_all(&out).expect("write reply");
                        break;
                    }
                    Ok(Decoded::NeedMore) => {
                        let n = stream.read(&mut temp).expect("read");
                        buf.extend_from_slice(&temp[..n]);
                    }
                    Err(e) => panic!("error {e:?}"),
                }
            }
        }
    });

    let config = ClientConfig::new().with_read_timeout(Some(Duration::from_secs(3)));
    let mut client = Client::connect_with(addr, config).expect("connect");

    let mut commands = Vec::with_capacity(300);
    for _ in 0..300 {
        commands.push(vec![Value::Text("ping".to_string())]);
    }

    let replies = client.pipeline(commands).expect("pipeline 300");
    assert_eq!(replies.len(), 300);
    for r in replies {
        assert_eq!(r.expect("pong"), Value::Text("pong".to_string()));
    }
}

// 19. bad server sends reply with wrong opcode in middle of pipeline -> Protocol error and client broken
#[test]
fn test_client_pipeline_wrong_opcode_breaks_client() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = Vec::new();
        let mut temp = [0u8; 8192];

        // Handshake
        loop {
            match decode_frame(&buf) {
                Ok(Decoded::Complete((frame, used))) => {
                    buf.drain(..used);
                    let hello_reply = Frame::new(
                        Header::new(FLAG_RESPONSE, frame.header.request_id, Opcode::Hello, 0),
                        Value::Map(vec![
                            (
                                Value::Text("server_name".to_string()),
                                Value::Text("cachedbd".to_string()),
                            ),
                            (
                                Value::Text("server_version".to_string()),
                                Value::Text("0.1.0".to_string()),
                            ),
                            (Value::Text("protocol_version".to_string()), Value::Int(1)),
                            (Value::Text("client_id".to_string()), Value::Int(1)),
                        ]),
                    );
                    let mut out = Vec::new();
                    encode_frame(&hello_reply, &mut out).expect("encode");
                    stream.write_all(&out).expect("write");
                    break;
                }
                Ok(Decoded::NeedMore) => {
                    let n = stream.read(&mut temp).expect("read");
                    buf.extend_from_slice(&temp[..n]);
                }
                Err(e) => panic!("{e:?}"),
            }
        }

        // Read 2 commands
        let mut cmd_count = 0;
        loop {
            match decode_frame(&buf) {
                Ok(Decoded::Complete((frame, used))) => {
                    buf.drain(..used);
                    cmd_count += 1;
                    let opcode = if cmd_count == 1 {
                        Opcode::Command // normal
                    } else {
                        Opcode::Push // WRONG opcode for a command response!
                    };
                    let reply = Frame::new(
                        Header::new(FLAG_RESPONSE, frame.header.request_id, opcode, 0),
                        Value::Text("pong".to_string()),
                    );
                    let mut out = Vec::new();
                    encode_frame(&reply, &mut out).expect("encode");
                    let _ = stream.write_all(&out);
                    if cmd_count == 2 {
                        break;
                    }
                }
                Ok(Decoded::NeedMore) => {
                    let n = stream.read(&mut temp).expect("read");
                    buf.extend_from_slice(&temp[..n]);
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    });

    let mut client = Client::connect(addr).expect("connect");
    let commands = vec![
        vec![Value::Text("ping".to_string())],
        vec![Value::Text("ping".to_string())],
    ];

    let res = client.pipeline(commands);
    match res.unwrap_err() {
        ClientError::Protocol(msg) => {
            assert!(msg.contains("unexpected opcode"));
        }
        other => panic!("expected Protocol error, got {other:?}"),
    }

    // Assert client is broken: subsequent call immediately returns ConnectionClosed
    let post_err = client.ping().unwrap_err();
    assert_eq!(post_err, ClientError::ConnectionClosed);
}

// 20. read buffer: 20,000 small replies in one pipeline and a 10 MB reply
#[test]
fn test_client_read_buffer_20k_small_replies_and_10mb_reply() {
    let server = TestBlockingServer::start();
    let mut client = Client::connect(server.addr()).expect("connect");

    // 20,000 small replies in one pipeline
    let total = 20_000;
    let mut commands = Vec::with_capacity(total);
    for _ in 0..total {
        commands.push(vec![Value::Text("ping".to_string())]);
    }

    let replies = client.pipeline(commands).expect("pipeline 20k");
    assert_eq!(replies.len(), total);
    for r in replies {
        assert_eq!(r.expect("pong"), Value::Text("pong".to_string()));
    }

    // 10 MB reply
    let payload_10mb = vec![0xAB; 10 * 1024 * 1024];
    assert!(
        client
            .put(b"big_key", Value::Bytes(payload_10mb.clone()))
            .expect("put 10mb")
    );

    let fetched = client.fetch(b"big_key").expect("fetch 10mb");
    assert_eq!(fetched, Some(Value::Bytes(payload_10mb)));
}
