//! Integration and model tests for `handle_request`, pipelining, and background migration timer hooks.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/mod.rs"]
mod common;

use cachedb::cmd::{
    ERR_MALFORMED_PAYLOAD, ERR_SYNTAX_ERROR, ERR_UNKNOWN_COMMAND, Engine, Session, handle_request,
};
use cachedb::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
use cachedb::proto::{Frame, Value};
use cachedb::server::Server;
use common::XorShift64;
use std::net::TcpListener;

#[test]
fn test_handle_request_ping_and_request_id_copied() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    for req_id in [0u32, 1, 42, 1000, u32::MAX] {
        let header = Header::new(0, req_id, Opcode::Command, 0);
        let frame = Frame::new(header, Value::Array(vec![Value::Text("ping".to_string())]));

        let outcome = handle_request(&mut engine, &mut session, &frame);
        assert!(!outcome.close);
        assert_eq!(outcome.reply_frame.header.request_id, req_id);
        assert_eq!(outcome.reply_frame.header.opcode, Opcode::Command);
        assert_eq!(outcome.reply_frame.header.flags, FLAG_RESPONSE);
        assert_eq!(outcome.reply_frame.payload, Value::Text("pong".to_string()));
    }
}

#[test]
fn test_handle_request_put_fetch_round_trip() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Put command
    let put_frame = Frame::new(
        Header::new(0, 101, Opcode::Command, 0),
        Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("my_key".to_string()),
            Value::Text("hello_world".to_string()),
        ]),
    );
    let put_outcome = handle_request(&mut engine, &mut session, &put_frame);
    assert!(!put_outcome.close);
    assert_eq!(put_outcome.reply_frame.header.request_id, 101);
    assert_eq!(put_outcome.reply_frame.header.flags, FLAG_RESPONSE);
    assert_eq!(put_outcome.reply_frame.payload, Value::Bool(true));

    // Fetch command
    let fetch_frame = Frame::new(
        Header::new(0, 102, Opcode::Command, 0),
        Value::Array(vec![
            Value::Text("fetch".to_string()),
            Value::Text("my_key".to_string()),
        ]),
    );
    let fetch_outcome = handle_request(&mut engine, &mut session, &fetch_frame);
    assert!(!fetch_outcome.close);
    assert_eq!(fetch_outcome.reply_frame.header.request_id, 102);
    assert_eq!(fetch_outcome.reply_frame.header.flags, FLAG_RESPONSE);
    assert_eq!(
        fetch_outcome.reply_frame.payload,
        Value::Text("hello_world".to_string())
    );
}

#[test]
fn test_handle_request_error_reply_flags_and_close() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Unknown command -> error 2001, flags 0x03, close = false
    let unknown_frame = Frame::new(
        Header::new(0, 500, Opcode::Command, 0),
        Value::Array(vec![Value::Text("nonexistent".to_string())]),
    );
    let outcome = handle_request(&mut engine, &mut session, &unknown_frame);
    assert!(!outcome.close);
    assert_eq!(outcome.reply_frame.header.request_id, 500);
    assert_eq!(outcome.reply_frame.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(outcome.reply_frame.payload, ERR_UNKNOWN_COMMAND.to_value());
}

#[test]
fn test_handle_request_unknown_header_opcode_closes_connection() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Opcode::Push sent from client -> returns error frame with flags 0x03, close = true
    let push_frame = Frame::new(
        Header::new(0, 999, Opcode::Push, 0),
        Value::Text("push_data".to_string()),
    );
    let outcome = handle_request(&mut engine, &mut session, &push_frame);
    assert!(outcome.close);
    assert_eq!(outcome.reply_frame.header.request_id, 999);
    assert_eq!(outcome.reply_frame.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(outcome.reply_frame.payload, ERR_UNKNOWN_COMMAND.to_value());
}

#[test]
fn test_handle_request_bad_hello_payload_closes_connection() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // HELLO frame with non-Array payload -> error 1006, close = true
    let bad_hello = Frame::new(Header::new(0, 777, Opcode::Hello, 0), Value::Int(42));
    let outcome = handle_request(&mut engine, &mut session, &bad_hello);
    assert!(outcome.close);
    assert_eq!(outcome.reply_frame.header.request_id, 777);
    assert_eq!(outcome.reply_frame.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(
        outcome.reply_frame.payload,
        ERR_MALFORMED_PAYLOAD.to_value()
    );
}

#[test]
fn test_handle_request_command_payload_not_array_gives_syntax_error() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Valid Value item (Text) but not an Array -> error 2008 (ERR_SYNTAX_ERROR), close = false
    let not_array = Frame::new(
        Header::new(0, 888, Opcode::Command, 0),
        Value::Text("ping".to_string()),
    );
    let outcome = handle_request(&mut engine, &mut session, &not_array);
    assert!(!outcome.close);
    assert_eq!(outcome.reply_frame.header.request_id, 888);
    assert_eq!(outcome.reply_frame.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(outcome.reply_frame.payload, ERR_SYNTAX_ERROR.to_value());
}

#[test]
fn test_handle_request_many_frames_in_order() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    let operations = [
        (
            1u32,
            vec![
                Value::Text("put".to_string()),
                Value::Text("seq".to_string()),
                Value::Int(10),
            ],
            Value::Bool(true),
        ),
        (
            2u32,
            vec![
                Value::Text("fetch".to_string()),
                Value::Text("seq".to_string()),
            ],
            Value::Int(10),
        ),
        (
            3u32,
            vec![
                Value::Text("bump".to_string()),
                Value::Text("seq".to_string()),
                Value::Int(5),
            ],
            Value::Int(15),
        ),
        (
            4u32,
            vec![
                Value::Text("has".to_string()),
                Value::Text("seq".to_string()),
            ],
            Value::Bool(true),
        ),
        (
            5u32,
            vec![
                Value::Text("drop".to_string()),
                Value::Text("seq".to_string()),
            ],
            Value::Int(1),
        ),
        (
            6u32,
            vec![
                Value::Text("has".to_string()),
                Value::Text("seq".to_string()),
            ],
            Value::Bool(false),
        ),
        (
            7u32,
            vec![Value::Text("ping".to_string())],
            Value::Text("pong".to_string()),
        ),
    ];

    for (req_id, args, expected_reply) in operations {
        let frame = Frame::new(
            Header::new(0, req_id, Opcode::Command, 0),
            Value::Array(args),
        );
        let outcome = handle_request(&mut engine, &mut session, &frame);
        assert!(!outcome.close);
        assert_eq!(outcome.reply_frame.header.request_id, req_id);
        assert_eq!(outcome.reply_frame.payload, expected_reply);
    }
}

fn generate_random_test_value(rng: &mut XorShift64, depth: usize) -> Value {
    if depth > 3 {
        return Value::Int(rng.next_u64() as i64);
    }

    match rng.gen_range(0, 8) {
        0 => Value::Nil,
        1 => Value::Bool(rng.gen_bool()),
        2 => Value::Int(rng.next_u64() as i64),
        3 => {
            let n = rng.gen_range(0, 100);
            Value::Text(format!("text_{n}"))
        }
        4 => {
            let len = rng.gen_range(0, 64);
            let b = (0..len).map(|_| (rng.next_u64() & 0xFF) as u8).collect();
            Value::Bytes(b)
        }
        5 => {
            let count = rng.gen_range(0, 4);
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(generate_random_test_value(rng, depth + 1));
            }
            Value::Array(items)
        }
        6 => {
            let count = rng.gen_range(0, 3);
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let k = Value::Text(format!("k_{}", rng.gen_range(0, 50)));
                let v = generate_random_test_value(rng, depth + 1);
                entries.push((k, v));
            }
            Value::Map(entries)
        }
        _ => Value::Float(1.2345),
    }
}

#[test]
fn test_model_random_frames_never_panic() {
    let mut engine = Engine::new();
    let mut session = Session::new(100);
    let mut rng = XorShift64::new(0xABCD_EF01_2345_6789);

    let opcodes = [Opcode::Command, Opcode::Hello, Opcode::Push];

    for step in 0..20_000 {
        let req_id = (rng.next_u64() & 0xFFFF_FFFF) as u32;
        let opcode = opcodes[rng.gen_range(0, 3)];
        let flags = (rng.next_u64() & 0x07) as u8;

        let payload = if rng.gen_range(0, 4) == 0 {
            // Well-formed command or hello
            if rng.gen_bool() {
                Value::Array(vec![
                    Value::Text("put".to_string()),
                    Value::Text(format!("key_{}", rng.gen_range(0, 500))),
                    Value::Int(i64::from(step)),
                ])
            } else {
                Value::Array(vec![
                    Value::Text("hello".to_string()),
                    Value::Int(1),
                    Value::Text("test-client".to_string()),
                ])
            }
        } else {
            // Random structured value (valid or broken)
            generate_random_test_value(&mut rng, 0)
        };

        let frame = Frame::new(Header::new(flags, req_id, opcode, 0), payload);

        let outcome = handle_request(&mut engine, &mut session, &frame);

        // Verification invariants:
        // 1. Reply frame request_id MUST match input frame request_id
        assert_eq!(outcome.reply_frame.header.request_id, req_id);

        // 2. Flags must have FLAG_RESPONSE (0x01)
        assert_ne!(outcome.reply_frame.header.flags & FLAG_RESPONSE, 0);

        // 3. Flags must have FLAG_ERROR (0x02) if and only if reply is Value::Error
        let is_error = matches!(outcome.reply_frame.payload, Value::Error { .. });
        let has_err_flag = (outcome.reply_frame.header.flags & FLAG_ERROR) != 0;
        assert_eq!(
            is_error, has_err_flag,
            "FLAG_ERROR mismatch at step {step}: is_error={is_error}, has_err_flag={has_err_flag}"
        );
    }
}

#[test]
fn test_migration_timer_hook_finishes_resizing() -> Result<(), Box<dyn std::error::Error>> {
    use cachedb::net::event_loop::Handler;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut server = Server::new(listener);

    // Insert keys until a resize starts
    let mut i = 0i64;
    while !server.engine().store().table().is_resizing() {
        i += 1;
        let key = format!("mig_key_{i}").into_bytes();
        let _ = server.engine_mut().store_mut().put(
            &key,
            Value::Int(i),
            cachedb::store::PutCondition::Always,
        );
        assert!(i < 100_000, "table must start resizing");
    }

    // Hash table should currently have an active migration in progress
    assert!(
        server.engine().store().table().is_resizing(),
        "table should be resizing after triggering resize"
    );
    assert!(
        server.wants_tick(),
        "server must want tick while table is resizing"
    );

    // Call the tick hook repeatedly; it should step migration in small chunks (16 slots)
    let mut ticks = 0;
    while server.step_migration_tick() {
        ticks += 1;
        assert!(ticks < 10_000, "migration must complete in bounded ticks");
    }

    // Verify migration has fully concluded and invariants hold
    assert!(
        !server.engine().store().table().is_resizing(),
        "migration should be complete"
    );
    assert!(
        !server.wants_tick(),
        "server must not want tick when migration is complete"
    );
    assert!(server.engine().store().check_invariants().is_ok());

    // Verify all keys remain intact and readable
    for j in 1..=i {
        let key = format!("mig_key_{j}").into_bytes();
        assert_eq!(server.engine().store().fetch(&key), Value::Int(j));
    }

    Ok(())
}
