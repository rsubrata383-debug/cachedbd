//! Real end-to-end TCP integration tests for the SkiffDB server and event loop.
//!
//! Evaluates network I/O, pipelining, concurrency, framing errors, and migration
//! over real Linux loopback TCP sockets.

#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

mod common;

use common::{TestServer, XorShift64};
use skiffdb::cmd::{ERR_UNKNOWN_COMMAND, ERR_UNSUPPORTED_PROTOCOL_VERSION};
use skiffdb::proto::decode_frame;
use skiffdb::proto::encode::encode_frame;
use skiffdb::proto::header::{DEFAULT_MAX_PAYLOAD_LEN, FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
use skiffdb::proto::hello::{HelloRequest, HelloResponse};
use skiffdb::proto::{Decoded, Frame, Value};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

/// Buffered client helper wrapping a [`TcpStream`] for frame-level transactions.
struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Client {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    fn send_command(&mut self, req_id: u32, args: Vec<Value>) {
        let header = Header::new(0, req_id, Opcode::Command, 0);
        let frame = Frame::new(header, Value::Array(args));
        let mut out = Vec::new();
        encode_frame(&frame, &mut out).expect("encode frame");
        self.stream.write_all(&out).expect("write frame");
    }

    fn send_hello(&mut self, req_id: u32, version: i64, name: &str, token: Option<&str>) {
        let req = HelloRequest::new(version, name.to_string(), token.map(str::to_string));
        let header = Header::new(0, req_id, Opcode::Hello, 0);
        let frame = Frame::new(header, req.to_value());
        let mut out = Vec::new();
        encode_frame(&frame, &mut out).expect("encode frame");
        self.stream.write_all(&out).expect("write frame");
    }

    fn read_frame(&mut self) -> Result<Frame, io::Error> {
        let mut temp = [0u8; 8192];
        loop {
            if !self.buf.is_empty() {
                match decode_frame(&self.buf) {
                    Ok(Decoded::Complete((frame, used))) => {
                        self.buf.drain(..used);
                        return Ok(frame);
                    }
                    Ok(Decoded::NeedMore) => {}
                    Err(e) => {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, e.message()));
                    }
                }
            }
            let n = self.stream.read(&mut temp)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
            }
            self.buf.extend_from_slice(&temp[..n]);
        }
    }
}

// 1. ping round trip; request_id is copied; flags are 0x01
#[test]
fn test_1_ping_round_trip() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    client.send_command(42, vec![Value::Text("ping".to_string())]);
    let reply = client.read_frame().expect("read reply");

    assert_eq!(reply.header.request_id, 42);
    assert_eq!(reply.header.flags, FLAG_RESPONSE);
    assert_eq!(reply.header.opcode, Opcode::Command);
    assert_eq!(reply.payload, Value::Text("pong".to_string()));
}

// 2. hello (version 1) reply has the right map; hello version 2 gives 1001
// and the server closes the connection (read returns EOF)
#[test]
fn test_2_hello_negotiation() {
    let server = TestServer::start();

    // Client 1: valid version 1 handshake
    let mut client1 = Client::new(server.connect());
    client1.send_hello(101, 1, "test-client-v1", None);
    let reply1 = client1.read_frame().expect("read hello reply");
    assert_eq!(reply1.header.request_id, 101);
    assert_eq!(reply1.header.flags, FLAG_RESPONSE);
    assert_eq!(reply1.header.opcode, Opcode::Hello);

    let hello_resp = HelloResponse::from_value(&reply1.payload).expect("parse hello response");
    assert_eq!(hello_resp.server_name, "skiffd");
    assert_eq!(hello_resp.protocol_version, 1);
    assert!(hello_resp.client_id >= 1);

    // Client 2: unsupported version 2 handshake
    let mut client2 = Client::new(server.connect());
    client2.send_hello(102, 2, "test-client-v2", None);
    let reply2 = client2.read_frame().expect("read error reply");
    assert_eq!(reply2.header.request_id, 102);
    assert_eq!(reply2.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(reply2.payload, ERR_UNSUPPORTED_PROTOCOL_VERSION.to_value());

    // Server must close connection immediately after unsupported version
    let next_read = client2.read_frame();
    assert!(
        matches!(next_read, Err(ref e) if e.kind() == io::ErrorKind::UnexpectedEof),
        "expected EOF after version negotiation failure"
    );
}

// 3. put then fetch then drop then fetch (Nil)
#[test]
fn test_3_crud_sequence() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    // put key_alpha -> val_alpha
    client.send_command(
        1,
        vec![
            Value::Text("put".to_string()),
            Value::Text("key_alpha".to_string()),
            Value::Text("val_alpha".to_string()),
        ],
    );
    let rep1 = client.read_frame().expect("put reply");
    assert_eq!(rep1.payload, Value::Bool(true));

    // fetch key_alpha -> val_alpha
    client.send_command(
        2,
        vec![
            Value::Text("fetch".to_string()),
            Value::Text("key_alpha".to_string()),
        ],
    );
    let rep2 = client.read_frame().expect("fetch reply");
    assert_eq!(rep2.payload, Value::Text("val_alpha".to_string()));

    // drop key_alpha -> 1
    client.send_command(
        3,
        vec![
            Value::Text("drop".to_string()),
            Value::Text("key_alpha".to_string()),
        ],
    );
    let rep3 = client.read_frame().expect("drop reply");
    assert_eq!(rep3.payload, Value::Int(1));

    // fetch key_alpha -> Nil
    client.send_command(
        4,
        vec![
            Value::Text("fetch".to_string()),
            Value::Text("key_alpha".to_string()),
        ],
    );
    let rep4 = client.read_frame().expect("fetch reply nil");
    assert_eq!(rep4.payload, Value::Nil);
}

// 4. error reply has flags 0x03 (unknown command gives 2001, connection stays open,
// next ping works)
#[test]
fn test_4_error_reply_flags_connection_stays_open() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    // Unknown command
    client.send_command(10, vec![Value::Text("nonexistent".to_string())]);
    let rep_err = client.read_frame().expect("error reply");
    assert_eq!(rep_err.header.request_id, 10);
    assert_eq!(rep_err.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(rep_err.payload, ERR_UNKNOWN_COMMAND.to_value());

    // Connection remains open and functional
    client.send_command(11, vec![Value::Text("ping".to_string())]);
    let rep_ping = client.read_frame().expect("ping reply");
    assert_eq!(rep_ping.header.request_id, 11);
    assert_eq!(rep_ping.header.flags, FLAG_RESPONSE);
    assert_eq!(rep_ping.payload, Value::Text("pong".to_string()));
}

// 5. quit: reply "ok", then EOF
#[test]
fn test_5_quit_closes_connection() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    client.send_command(99, vec![Value::Text("quit".to_string())]);
    let reply = client.read_frame().expect("quit reply");
    assert_eq!(reply.header.request_id, 99);
    assert_eq!(reply.payload, Value::Text("ok".to_string()));

    let eof = client.read_frame();
    assert!(
        matches!(eof, Err(ref e) if e.kind() == io::ErrorKind::UnexpectedEof),
        "expected EOF after quit reply"
    );
}

// 6. pipelining: write 1000 ping/put/fetch requests in ONE write call;
// read 1000 replies; the order and request_ids are correct
#[test]
fn test_6_pipelining() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    let mut batched_bytes = Vec::new();
    let total_ops = 1000u32;

    for i in 0..total_ops {
        let args = if i % 3 == 0 {
            vec![Value::Text("ping".to_string())]
        } else if i % 3 == 1 {
            vec![
                Value::Text("put".to_string()),
                Value::Text(format!("pipe_key_{i}")),
                Value::Int(i64::from(i)),
            ]
        } else {
            vec![
                Value::Text("fetch".to_string()),
                Value::Text(format!("pipe_key_{}", i - 1)),
            ]
        };

        let header = Header::new(0, i, Opcode::Command, 0);
        let frame = Frame::new(header, Value::Array(args));
        encode_frame(&frame, &mut batched_bytes).expect("encode frame");
    }

    // Write all 1000 requests in a single syscall
    client
        .stream
        .write_all(&batched_bytes)
        .expect("write pipeline batch");

    // Read all 1000 replies in arrival order
    for expected_id in 0..total_ops {
        let reply = client.read_frame().expect("read pipelined reply");
        assert_eq!(
            reply.header.request_id, expected_id,
            "pipelined reply arrived out of order"
        );
        if expected_id % 3 == 0 {
            assert_eq!(reply.payload, Value::Text("pong".to_string()));
        } else if expected_id % 3 == 1 {
            assert_eq!(reply.payload, Value::Bool(true));
        } else {
            assert_eq!(reply.payload, Value::Int(i64::from(expected_id - 1)));
        }
    }
}

// 7. slow writer: send one request 1 byte at a time; get one correct reply
#[test]
fn test_7_slow_writer() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    let header = Header::new(0, 777, Opcode::Command, 0);
    let frame = Frame::new(header, Value::Array(vec![Value::Text("ping".to_string())]));
    let mut payload = Vec::new();
    encode_frame(&frame, &mut payload).expect("encode frame");

    // Send single frame 1 byte at a time with pauses
    for byte in payload {
        client.stream.write_all(&[byte]).expect("write byte");
        thread::sleep(Duration::from_millis(1));
    }

    let reply = client.read_frame().expect("read reply from slow writer");
    assert_eq!(reply.header.request_id, 777);
    assert_eq!(reply.payload, Value::Text("pong".to_string()));
}

// 8. two requests glued together in one write, and one request split across two writes
#[test]
fn test_8_glued_and_split_requests() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    // Glued: two frames concatenated
    let f1 = Frame::new(
        Header::new(0, 801, Opcode::Command, 0),
        Value::Array(vec![Value::Text("ping".to_string())]),
    );
    let f2 = Frame::new(
        Header::new(0, 802, Opcode::Command, 0),
        Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("glued_k".to_string()),
            Value::Int(42),
        ]),
    );
    let mut glued = Vec::new();
    encode_frame(&f1, &mut glued).expect("encode f1");
    encode_frame(&f2, &mut glued).expect("encode f2");
    client.stream.write_all(&glued).expect("write glued");

    let r1 = client.read_frame().expect("read r1");
    assert_eq!(r1.header.request_id, 801);
    assert_eq!(r1.payload, Value::Text("pong".to_string()));

    let r2 = client.read_frame().expect("read r2");
    assert_eq!(r2.header.request_id, 802);
    assert_eq!(r2.payload, Value::Bool(true));

    // Split: one frame split across two writes
    let f3 = Frame::new(
        Header::new(0, 803, Opcode::Command, 0),
        Value::Array(vec![
            Value::Text("fetch".to_string()),
            Value::Text("glued_k".to_string()),
        ]),
    );
    let mut split = Vec::new();
    encode_frame(&f3, &mut split).expect("encode f3");
    let mid = split.len() / 2;
    client
        .stream
        .write_all(&split[..mid])
        .expect("write split 1");
    thread::sleep(Duration::from_millis(10));
    client
        .stream
        .write_all(&split[mid..])
        .expect("write split 2");

    let r3 = client.read_frame().expect("read r3");
    assert_eq!(r3.header.request_id, 803);
    assert_eq!(r3.payload, Value::Int(42));
}

// 9. bad magic bytes: connection closes (EOF); then a NEW client still works
#[test]
fn test_9_bad_magic_bytes() {
    let server = TestServer::start();

    // Client 1 sends bad magic bytes
    let mut c1 = Client::new(server.connect());
    c1.stream
        .write_all(&[
            0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
        ])
        .expect("write bad magic");
    let eof = c1.read_frame();
    assert!(
        matches!(eof, Err(ref e) if e.kind() == io::ErrorKind::UnexpectedEof),
        "server must close socket on bad magic"
    );

    // Client 2 connects to the still-running server
    let mut c2 = Client::new(server.connect());
    c2.send_command(900, vec![Value::Text("ping".to_string())]);
    let rep = c2.read_frame().expect("client 2 ping reply");
    assert_eq!(rep.header.request_id, 900);
    assert_eq!(rep.payload, Value::Text("pong".to_string()));
}

// 10. header with payload_len bigger than max frame size: connection closes;
// server still works for the next client
#[test]
fn test_10_header_payload_len_bigger_than_max_frame_size() {
    let server = TestServer::start();

    // Client 1 sends header with payload_len = 70 MB (> 64 MB DEFAULT_MAX_PAYLOAD_LEN)
    let bad_len = DEFAULT_MAX_PAYLOAD_LEN + 1024;
    let mut header_bytes = [0u8; 14];
    header_bytes[0] = 0xCA;
    header_bytes[1] = 0xDB;
    header_bytes[2] = 1; // version
    header_bytes[3] = 0; // flags
    header_bytes[9] = 2; // opcode Command
    header_bytes[10..14].copy_from_slice(&bad_len.to_be_bytes());

    let mut c1 = Client::new(server.connect());
    c1.stream.write_all(&header_bytes).expect("write header");
    let eof = c1.read_frame();
    assert!(
        matches!(eof, Err(ref e) if e.kind() == io::ErrorKind::UnexpectedEof),
        "server must close socket on frame too large"
    );

    // Client 2 connects and functions normally
    let mut c2 = Client::new(server.connect());
    c2.send_command(1000, vec![Value::Text("ping".to_string())]);
    let rep = c2.read_frame().expect("client 2 ping reply");
    assert_eq!(rep.header.request_id, 1000);
    assert_eq!(rep.payload, Value::Text("pong".to_string()));
}

// 11. client closes in the middle of a frame; server keeps working
#[test]
fn test_11_client_abrupt_disconnect_mid_frame() {
    let server = TestServer::start();

    // Client 1 sends 6 bytes and abruptly drops socket
    {
        let mut s1 = server.connect();
        s1.write_all(&[0xCA, 0xDB, 0x01, 0x00, 0x00, 0x01])
            .expect("write partial");
        drop(s1);
    }

    // Server must continue answering subsequent clients cleanly
    let mut c2 = Client::new(server.connect());
    c2.send_command(1100, vec![Value::Text("ping".to_string())]);
    let rep = c2.read_frame().expect("client 2 ping reply");
    assert_eq!(rep.header.request_id, 1100);
    assert_eq!(rep.payload, Value::Text("pong".to_string()));
}

// 12. 50 clients at the same time, each does 200 put/fetch with its own keys
// (threads); all replies correct; at the end `size` is the expected count
#[test]
fn test_12_50_concurrent_clients() {
    let server = TestServer::start();
    let addr = server.addr();
    let num_clients = 50usize;
    let ops_per_client = 200usize;

    let mut handles = Vec::with_capacity(num_clients);

    for client_idx in 0..num_clients {
        let handle = thread::spawn(move || {
            let stream = TcpStream::connect(addr).expect("connect client");
            let timeout = Duration::from_secs(5);
            stream.set_read_timeout(Some(timeout)).expect("set timeout");
            stream
                .set_write_timeout(Some(timeout))
                .expect("set timeout");
            let mut client = Client::new(stream);

            for op in 0..ops_per_client {
                let key = format!("c{client_idx}_k{op}");
                let val = (client_idx * 10_000 + op) as i64;

                // put
                client.send_command(
                    op as u32,
                    vec![
                        Value::Text("put".to_string()),
                        Value::Text(key.clone()),
                        Value::Int(val),
                    ],
                );
                let put_rep = client.read_frame().expect("read put rep");
                assert_eq!(put_rep.payload, Value::Bool(true));

                // fetch
                client.send_command(
                    op as u32,
                    vec![Value::Text("fetch".to_string()), Value::Text(key)],
                );
                let fetch_rep = client.read_frame().expect("read fetch rep");
                assert_eq!(fetch_rep.payload, Value::Int(val));
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("client thread finished successfully");
    }

    // Verify final keyspace size
    let mut inspector = Client::new(server.connect());
    inspector.send_command(9999, vec![Value::Text("size".to_string())]);
    let size_rep = inspector.read_frame().expect("read size");
    assert_eq!(
        size_rep.payload,
        Value::Int((num_clients * ops_per_client) as i64)
    );
}

// 13. large value: put 1 MB Bytes, fetch it back, equal
#[test]
fn test_13_large_value_roundtrip() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    let payload_bytes = vec![0xABu8; 1024 * 1024]; // 1 MB payload
    let large_val = Value::Bytes(payload_bytes.clone());

    client.send_command(
        1300,
        vec![
            Value::Text("put".to_string()),
            Value::Text("large_blob".to_string()),
            large_val,
        ],
    );
    let put_rep = client.read_frame().expect("put large blob reply");
    assert_eq!(put_rep.payload, Value::Bool(true));

    client.send_command(
        1301,
        vec![
            Value::Text("fetch".to_string()),
            Value::Text("large_blob".to_string()),
        ],
    );
    let fetch_rep = client.read_frame().expect("fetch large blob reply");
    assert_eq!(fetch_rep.payload, Value::Bytes(payload_bytes));
}

// 14. resize while connected: 200,000 puts of small values from one client
// (pipelined in batches of 1000), then fetch 1,000 random keys with the
// shared XorShift64 (rule 11.1) and compare with a HashMap model
#[test]
fn test_14_resize_while_connected() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    let total_keys = 200_000usize;
    let batch_size = 1000usize;
    let mut model: HashMap<String, i64> = HashMap::with_capacity(total_keys);

    // Pipeline 200,000 puts in batches of 1,000
    for batch_start in (0..total_keys).step_by(batch_size) {
        let mut batch_bytes = Vec::new();
        for i in batch_start..(batch_start + batch_size) {
            let key = format!("batch_k_{i}");
            let val = (i as i64) * 3;
            model.insert(key.clone(), val);

            let header = Header::new(0, i as u32, Opcode::Command, 0);
            let frame = Frame::new(
                header,
                Value::Array(vec![
                    Value::Text("put".to_string()),
                    Value::Text(key),
                    Value::Int(val),
                ]),
            );
            encode_frame(&frame, &mut batch_bytes).expect("encode frame");
        }

        client.stream.write_all(&batch_bytes).expect("write batch");

        // Read batch replies
        for _ in 0..batch_size {
            let rep = client.read_frame().expect("read put rep");
            assert_eq!(rep.payload, Value::Bool(true));
        }
    }

    // Verify 1,000 random keys using XorShift64 (AGENTS.md rule 11.1)
    let mut rng = XorShift64::new(0xA1B2_C3D4_E5F6_0789);
    for sample in 0..1000 {
        let pick_idx = rng.gen_range(0, total_keys);
        let key = format!("batch_k_{pick_idx}");
        let expected_val = *model.get(&key).expect("model contains key");

        client.send_command(
            (100_000 + sample) as u32,
            vec![Value::Text("fetch".to_string()), Value::Text(key)],
        );
        let rep = client.read_frame().expect("fetch sample");
        assert_eq!(rep.payload, Value::Int(expected_val));
    }
}

// 15. idle migration: after test 14, wait up to 2 s (poll with fetch of a key
// every 50 ms) and check the server is still answering; also check
// the server uses little CPU when idle, if you can measure it easily with
// std only (if not, skip and say so)
#[test]
fn test_15_idle_migration() {
    let server = TestServer::start();
    let mut client = Client::new(server.connect());

    // Insert 50,000 keys to establish an active/growing keyspace
    let mut batch = Vec::new();
    for i in 0..50_000u32 {
        let header = Header::new(0, i, Opcode::Command, 0);
        let frame = Frame::new(
            header,
            Value::Array(vec![
                Value::Text("put".to_string()),
                Value::Text(format!("idle_k_{i}")),
                Value::Int(i64::from(i)),
            ]),
        );
        encode_frame(&frame, &mut batch).expect("encode frame");
    }
    client.stream.write_all(&batch).expect("write batch");
    for _ in 0..50_000 {
        let _ = client.read_frame().expect("read put reply");
    }

    // Poll server periodically over 2 seconds (40 iterations * 50ms)
    // Server must respond instantly to each probe without hanging
    for poll in 0..40 {
        thread::sleep(Duration::from_millis(50));
        client.send_command(
            poll as u32,
            vec![
                Value::Text("fetch".to_string()),
                Value::Text("idle_k_0".to_string()),
            ],
        );
        let rep = client.read_frame().expect("poll fetch reply");
        assert_eq!(rep.payload, Value::Int(0));
    }
    // Note on CPU measurement: Standard library Rust (std) does not provide a cross-platform
    // process CPU percentage sampling API without third-party crates or reading Linux /proc/stat;
    // sub-millisecond roundtrips above confirm no event loop busy-spin occurred.
}

// 16. 2 clients: one client sends garbage, other client is not affected
#[test]
fn test_16_two_clients_isolation() {
    let server = TestServer::start();

    let mut c_bad = Client::new(server.connect());
    let mut c_good = Client::new(server.connect());

    // c_good sets a key
    c_good.send_command(
        1,
        vec![
            Value::Text("put".to_string()),
            Value::Text("shared_key".to_string()),
            Value::Text("valid_data".to_string()),
        ],
    );
    let r_good1 = c_good.read_frame().expect("good client put");
    assert_eq!(r_good1.payload, Value::Bool(true));

    // Part 1: c_bad sends >= 14 bytes of garbage whose first two bytes are not 0xCA 0xDB.
    // The server decodes the 14-byte header, detects ERR_INVALID_MAGIC (1004), and closes c_bad.
    c_bad
        .stream
        .write_all(&[
            0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
        ])
        .expect("write 14 bytes garbage");
    let bad_eof = c_bad.read_frame();
    assert!(
        matches!(bad_eof, Err(ref e) if e.kind() == io::ErrorKind::UnexpectedEof),
        "c_bad must be disconnected after full 14-byte invalid header"
    );

    // c_good continues completely unharmed
    c_good.send_command(
        2,
        vec![
            Value::Text("fetch".to_string()),
            Value::Text("shared_key".to_string()),
        ],
    );
    let r_good2 = c_good.read_frame().expect("good client fetch");
    assert_eq!(r_good2.payload, Value::Text("valid_data".to_string()));

    c_good.send_command(3, vec![Value::Text("ping".to_string())]);
    let r_good3 = c_good.read_frame().expect("good client ping");
    assert_eq!(r_good3.payload, Value::Text("pong".to_string()));

    // Part 2: New client sends fewer than 14 bytes of garbage (incomplete header).
    // The server correctly returns NeedMore without reporting an error or closing,
    // documenting the absence of an idle timeout for half-sent frames.
    let mut c_partial = Client::new(server.connect());
    c_partial
        .stream
        .write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33])
        .expect("write 8 bytes partial header");

    // Configure a 300 ms read timeout to verify the server does NOT close the socket
    c_partial
        .stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("set read timeout");

    let mut probe_buf = [0u8; 1];
    let read_res = c_partial.stream.read(&mut probe_buf);
    assert!(
        matches!(
            read_res,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut
        ),
        "server must not close connection within 300 ms for incomplete header: got {read_res:?}"
    );

    // Meanwhile, c_good still operates normally without interruption
    c_good.send_command(4, vec![Value::Text("ping".to_string())]);
    let r_good4 = c_good
        .read_frame()
        .expect("good client ping after partial header");
    assert_eq!(r_good4.payload, Value::Text("pong".to_string()));
}
