//! Integration tests for Cachedb command parsing, validation, dispatching, and handlers.

mod common;

use cachedb::cmd::{
    CmdConfig, ERR_AUTH_REQUIRED, ERR_EMPTY_KEY, ERR_INVALID_ARGUMENT, ERR_KEY_TOO_LARGE,
    ERR_OUT_OF_MEMORY, ERR_READONLY_REPLICA, ERR_SYNTAX_ERROR, ERR_TOO_MANY_ARGS,
    ERR_UNKNOWN_COMMAND, ERR_VALUE_TOO_LARGE, ERR_WRONG_ARG_COUNT, Engine, Session,
};
use cachedb::proto::Value;
use cachedb::store::{PutCondition, RobinHoodTable, Store};
use common::XorShift64;
use std::collections::HashMap;

#[test]
fn test_commands_spec_examples() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // 1. ping
    // Request: ["ping"] -> Reply: "pong"
    let req = Value::Array(vec![Value::Text("ping".to_string())]);
    let outcome = engine.execute(&mut session, &req);
    assert_eq!(outcome.reply, Value::Text("pong".to_string()));
    assert!(!outcome.close);

    // Request: ["ping", "heartbeat"] -> Reply: "heartbeat"
    let req = Value::Array(vec![
        Value::Text("ping".to_string()),
        Value::Text("heartbeat".to_string()),
    ]);
    let outcome = engine.execute(&mut session, &req);
    assert_eq!(outcome.reply, Value::Text("heartbeat".to_string()));

    // 2. put
    // Request: ["put", "user:1", "alice"] -> Reply: true
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("user:1".to_string()),
        Value::Text("alice".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    // Request: ["put", "user:1", "bob", "nx"] -> Reply: false
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("user:1".to_string()),
        Value::Text("bob".to_string()),
        Value::Text("nx".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(false));

    // Request: ["put", "user:1", "bob", "xx"] -> Reply: true
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("user:1".to_string()),
        Value::Text("bob".to_string()),
        Value::Text("xx".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    // 3. fetch
    // Request: ["fetch", "user:1"] -> Reply: "bob"
    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("user:1".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        Value::Text("bob".to_string())
    );

    // Request: ["fetch", "user:999"] -> Reply: nil
    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("user:999".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Nil);

    // 4. has
    // Request: ["has", "user:1"] -> Reply: true
    let req = Value::Array(vec![
        Value::Text("has".to_string()),
        Value::Text("user:1".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    // Request: ["has", "user:999"] -> Reply: false
    let req = Value::Array(vec![
        Value::Text("has".to_string()),
        Value::Text("user:999".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(false));

    // 5. drop
    // Request: ["drop", "user:1"] -> Reply: 1
    let req = Value::Array(vec![
        Value::Text("drop".to_string()),
        Value::Text("user:1".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(1));

    // Request: ["drop", "user:1"] -> Reply: 0
    let req = Value::Array(vec![
        Value::Text("drop".to_string()),
        Value::Text("user:1".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(0));

    // Request: ["drop", "k1", "k2", "k3"] -> Reply: 2 (if k1, k2 existed, k3 did not)
    let _ = engine
        .store
        .put(b"k1", Value::Int(10), PutCondition::Always);
    let _ = engine
        .store
        .put(b"k2", Value::Int(20), PutCondition::Always);
    let req = Value::Array(vec![
        Value::Text("drop".to_string()),
        Value::Text("k1".to_string()),
        Value::Text("k2".to_string()),
        Value::Text("k3".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(2));

    // 6. size
    // Request: ["size"] -> Reply: Int(42) when 42 keys stored
    for i in 0i64..42i64 {
        let k = format!("k:{i}").into_bytes();
        let _ = engine.store.put(&k, Value::Int(i), PutCondition::Always);
    }
    let req = Value::Array(vec![Value::Text("size".to_string())]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(42));
}

#[test]
fn test_error_unknown_and_syntax() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // 2001: Unknown command
    let req = Value::Array(vec![Value::Text("unknown_cmd".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_UNKNOWN_COMMAND.to_value()
    );
    let req = Value::Array(vec![Value::Text("PUT".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_UNKNOWN_COMMAND.to_value()
    );
    let req = Value::Array(vec![Value::Text("Ping".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_UNKNOWN_COMMAND.to_value()
    );

    // 2008: Syntax error
    let req = Value::Int(42);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_SYNTAX_ERROR.to_value()
    );
    let req = Value::Array(vec![]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_SYNTAX_ERROR.to_value()
    );
    let req = Value::Array(vec![Value::Int(100)]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_SYNTAX_ERROR.to_value()
    );
}

#[test]
fn test_error_wrong_arg_count() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // 2002: Wrong arg count
    let req = Value::Array(vec![
        Value::Text("ping".to_string()),
        Value::Text("a".to_string()),
        Value::Text("b".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Text("v".to_string()),
        Value::Text("nx".to_string()),
        Value::Text("extra".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![Value::Text("fetch".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("k1".to_string()),
        Value::Text("k2".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![Value::Text("drop".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![Value::Text("has".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("has".to_string()),
        Value::Text("k1".to_string()),
        Value::Text("k2".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("size".to_string()),
        Value::Text("extra".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );
}

#[test]
fn test_error_invalid_argument() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // 2003: Invalid argument
    let req = Value::Array(vec![Value::Text("ping".to_string()), Value::Int(123)]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Int(123),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Error {
            code: 999,
            message: "err".to_string(),
        },
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Text("v".to_string()),
        Value::Text("invalid_cond".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    let req = Value::Array(vec![Value::Text("fetch".to_string()), Value::Int(123)]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    let req = Value::Array(vec![Value::Text("drop".to_string()), Value::Int(123)]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    let req = Value::Array(vec![Value::Text("has".to_string()), Value::Int(123)]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
}

#[test]
fn test_error_limits_and_sizes() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // 2004: Key too large
    let large_key = vec![b'k'; 1025];
    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Bytes(large_key),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_KEY_TOO_LARGE.to_value()
    );

    // 2005: Value too large
    let mut small_engine = Engine::new();
    small_engine.config.max_value_size = 10;
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Bytes(vec![b'v'; 11]),
    ]);
    assert_eq!(
        small_engine.execute(&mut session, &req).reply,
        ERR_VALUE_TOO_LARGE.to_value()
    );

    // 2006: Empty key
    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text(String::new()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_EMPTY_KEY.to_value()
    );

    // 2007: Too many args
    let mut limited_args_engine = Engine::new();
    limited_args_engine.config.max_args = 2;
    let req = Value::Array(vec![
        Value::Text("drop".to_string()),
        Value::Text("a".to_string()),
        Value::Text("b".to_string()),
        Value::Text("c".to_string()),
    ]);
    assert_eq!(
        limited_args_engine.execute(&mut session, &req).reply,
        ERR_TOO_MANY_ARGS.to_value()
    );
}

#[test]
fn test_error_auth_and_readonly() {
    let mut session = Session::new(1);

    // 5001: Auth required
    let mut auth_engine = Engine::new();
    auth_engine.config.require_auth = true;
    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("k".to_string()),
    ]);
    assert_eq!(
        auth_engine.execute(&mut session, &req).reply,
        ERR_AUTH_REQUIRED.to_value()
    );
    // But ping has NO_AUTH
    let req = Value::Array(vec![Value::Text("ping".to_string())]);
    assert_eq!(
        auth_engine.execute(&mut session, &req).reply,
        Value::Text("pong".to_string())
    );

    // 6001: Readonly replica
    let mut ro_engine = Engine::new();
    ro_engine.config.read_only = true;
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(
        ro_engine.execute(&mut session, &req).reply,
        ERR_READONLY_REPLICA.to_value()
    );
    let req = Value::Array(vec![
        Value::Text("drop".to_string()),
        Value::Text("k".to_string()),
    ]);
    assert_eq!(
        ro_engine.execute(&mut session, &req).reply,
        ERR_READONLY_REPLICA.to_value()
    );
    // Read commands succeed
    let req = Value::Array(vec![Value::Text("size".to_string())]);
    assert_eq!(ro_engine.execute(&mut session, &req).reply, Value::Int(0));
}

#[test]
fn test_error_out_of_memory() {
    let mut session = Session::new(1);

    // 4003: Out of memory (TableFull)
    let bounded_table = RobinHoodTable::with_max_capacity(8, 0x1234, 8);
    let bounded_store = Store::with_table(bounded_table);
    let mut oom_engine = Engine::with_config(bounded_store, CmdConfig::default());
    // Fill up to max load (8 * 7 / 8 = 7 items)
    for i in 0..7 {
        let req = Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text(format!("k:{i}")),
            Value::Int(i),
        ]);
        assert_eq!(
            oom_engine.execute(&mut session, &req).reply,
            Value::Bool(true)
        );
    }
    // 8th insert attempts resize past max_capacity 8 -> returns TableFull -> 4003
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("overflow".to_string()),
        Value::Int(999),
    ]);
    assert_eq!(
        oom_engine.execute(&mut session, &req).reply,
        ERR_OUT_OF_MEMORY.to_value()
    );
}

#[test]
fn test_key_length_and_value_size_boundaries() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Key boundaries: 0 -> 2006
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Bytes(vec![]),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_EMPTY_KEY.to_value()
    );

    // Key boundary: 1 byte -> OK
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Bytes(vec![b'a']),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    // Key boundary: 1024 bytes -> OK
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Bytes(vec![b'x'; 1024]),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    // Key boundary: 1025 bytes -> 2004
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Bytes(vec![b'x'; 1025]),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_KEY_TOO_LARGE.to_value()
    );

    // Value size boundary with small max_value_size
    let mut small_cfg_engine = Engine::new();
    small_cfg_engine.config.max_value_size = 20;

    // Exactly 20 bytes -> OK
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Bytes(vec![b'z'; 20]),
    ]);
    assert_eq!(
        small_cfg_engine.execute(&mut session, &req).reply,
        Value::Bool(true)
    );

    // 21 bytes -> 2005
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
        Value::Bytes(vec![b'z'; 21]),
    ]);
    assert_eq!(
        small_cfg_engine.execute(&mut session, &req).reply,
        ERR_VALUE_TOO_LARGE.to_value()
    );
}

#[test]
fn test_binary_keys_and_equivalence() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Binary keys with 0x00 and 0xFF
    let bin_key = vec![0x00, 0xAA, 0xFF, 0x00, 0x55];
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Bytes(bin_key.clone()),
        Value::Text("binary_val".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    let fetch_req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Bytes(bin_key),
    ]);
    assert_eq!(
        engine.execute(&mut session, &fetch_req).reply,
        Value::Text("binary_val".to_string())
    );

    // Text key and Bytes key with identical bytes refer to the exact same key
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("shared_key".to_string()),
        Value::Int(12345),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));

    let req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Bytes(b"shared_key".to_vec()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(12345));
}

#[test]
fn test_case_sensitivity() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Keys are case sensitive
    let req1 = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("myKey".to_string()),
        Value::Text("val_upper".to_string()),
    ]);
    let req2 = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("mykey".to_string()),
        Value::Text("val_lower".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req1).reply, Value::Bool(true));
    assert_eq!(engine.execute(&mut session, &req2).reply, Value::Bool(true));

    let fetch1 = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("myKey".to_string()),
    ]);
    let fetch2 = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("mykey".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &fetch1).reply,
        Value::Text("val_upper".to_string())
    );
    assert_eq!(
        engine.execute(&mut session, &fetch2).reply,
        Value::Text("val_lower".to_string())
    );

    // Verbs are case sensitive (uppercase gives 2001)
    let verbs = [
        "PUT", "Put", "pUt", "PING", "Ping", "FETCH", "Fetch", "DROP", "Drop", "HAS", "Has",
        "SIZE", "Size",
    ];
    for v in verbs {
        let req = Value::Array(vec![Value::Text(v.to_string())]);
        assert_eq!(
            engine.execute(&mut session, &req).reply,
            ERR_UNKNOWN_COMMAND.to_value(),
            "verb {v} should be rejected as unknown"
        );
    }
}

#[test]
fn test_order_of_checks() {
    let mut engine = Engine::new();
    engine.config.require_auth = true;
    engine.config.read_only = true;
    let mut session = Session::new(1); // unauthenticated

    // Check 1: Parse error (2008) precedes Unknown Command and Auth
    let req = Value::Int(99);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_SYNTAX_ERROR.to_value()
    );

    // Check 2: Unknown Command (2001) precedes Auth (5001)
    let req = Value::Array(vec![Value::Text("unknown_command".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_UNKNOWN_COMMAND.to_value()
    );

    // Check 3: Auth (5001) precedes Arity (2002) for non-NO_AUTH command
    // (put requires 2-3 args, here we pass 0 args, but unauthenticated must give 5001)
    let req = Value::Array(vec![Value::Text("put".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_AUTH_REQUIRED.to_value()
    );

    // Now authenticate session
    session.authenticated = true;

    // Check 4: Arity (2002) precedes Read-Only (6001)
    // (put with 1 arg has wrong arity; on read_only replica, it must give 2002 not 6001)
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );

    // Check 5: Read-Only (6001) precedes Handler execution (such as key validation 2006)
    // (put with empty key "" has valid arity 2, but on read_only replica it must give 6001, not 2006)
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text(String::new()),
        Value::Text("v".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_READONLY_REPLICA.to_value()
    );
}

#[test]
fn test_table_unchanged_after_error() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Insert key1
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("key1".to_string()),
        Value::Text("val1".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Bool(true));
    assert_eq!(engine.store.len(), 1);

    // Failed put with invalid condition
    let req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("key2".to_string()),
        Value::Text("val2".to_string()),
        Value::Text("bad_cond".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );
    assert_eq!(engine.store.len(), 1);
    assert!(!engine.store.contains(b"key2"));

    // Failed drop where second key is invalid (empty)
    // Validate ALL keys first: key1 must NOT be dropped!
    let req = Value::Array(vec![
        Value::Text("drop".to_string()),
        Value::Text("key1".to_string()),
        Value::Text(String::new()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_EMPTY_KEY.to_value()
    );
    assert_eq!(engine.store.len(), 1);
    assert_eq!(engine.store.fetch(b"key1"), Value::Text("val1".to_string()));
}

struct ModelStats {
    commands_run: usize,
    puts_unconditional: usize,
    puts_if_absent: usize,
    puts_if_present: usize,
    drops_run: usize,
    resizes_seen: usize,
}

fn execute_model_command(
    engine: &mut Engine,
    session: &mut Session,
    ref_map: &mut HashMap<Vec<u8>, Value>,
    rng: &mut XorShift64,
    step: usize,
    key_space_size: usize,
    stats: &mut ModelStats,
) {
    let op_type = rng.gen_range(0, 6);
    let key_id = rng.gen_range(0, key_space_size);
    let key_bytes = format!("key:{key_id}").into_bytes();
    let key_val = Value::Bytes(key_bytes.clone());

    match op_type {
        0 => {
            // put (always)
            let val_int = i64::try_from(step).unwrap_or(0) * 3;
            let req = Value::Array(vec![
                Value::Text("put".to_string()),
                key_val,
                Value::Int(val_int),
            ]);
            let outcome = engine.execute(session, &req);
            assert_eq!(outcome.reply, Value::Bool(true));
            ref_map.insert(key_bytes, Value::Int(val_int));
            stats.puts_unconditional += 1;
        }
        1 => {
            // put nx
            let val_int = i64::try_from(step).unwrap_or(0) * 5;
            let req = Value::Array(vec![
                Value::Text("put".to_string()),
                key_val,
                Value::Int(val_int),
                Value::Text("nx".to_string()),
            ]);
            let outcome = engine.execute(session, &req);
            let already_exists = ref_map.contains_key(&key_bytes);
            if already_exists {
                assert_eq!(outcome.reply, Value::Bool(false));
            } else {
                assert_eq!(outcome.reply, Value::Bool(true));
                ref_map.insert(key_bytes, Value::Int(val_int));
            }
            stats.puts_if_absent += 1;
        }
        2 => {
            // put xx
            let val_int = i64::try_from(step).unwrap_or(0) * 7;
            let req = Value::Array(vec![
                Value::Text("put".to_string()),
                key_val,
                Value::Int(val_int),
                Value::Text("xx".to_string()),
            ]);
            let outcome = engine.execute(session, &req);
            let already_exists = ref_map.contains_key(&key_bytes);
            if already_exists {
                assert_eq!(outcome.reply, Value::Bool(true));
                ref_map.insert(key_bytes, Value::Int(val_int));
            } else {
                assert_eq!(outcome.reply, Value::Bool(false));
            }
            stats.puts_if_present += 1;
        }
        3 => {
            // fetch
            let req = Value::Array(vec![Value::Text("fetch".to_string()), key_val]);
            let outcome = engine.execute(session, &req);
            let expected = ref_map.get(&key_bytes).cloned().unwrap_or(Value::Nil);
            assert_eq!(outcome.reply, expected);
        }
        4 => {
            // drop (1 to 3 keys)
            let drop_count = rng.gen_range(1, 4);
            let mut req_items = vec![Value::Text("drop".to_string())];
            let mut keys_to_drop = Vec::with_capacity(drop_count);
            for _ in 0..drop_count {
                let k_id = rng.gen_range(0, key_space_size);
                let k_b = format!("key:{k_id}").into_bytes();
                req_items.push(Value::Bytes(k_b.clone()));
                keys_to_drop.push(k_b);
            }

            let mut expected_removed = 0i64;
            let mut seen = std::collections::HashSet::new();
            for k in keys_to_drop {
                if seen.insert(k.clone()) && ref_map.remove(&k).is_some() {
                    expected_removed += 1;
                }
            }

            let outcome = engine.execute(session, &Value::Array(req_items));
            assert_eq!(outcome.reply, Value::Int(expected_removed));
            stats.drops_run += 1;
        }
        5 => {
            // has & size check
            let req_has = Value::Array(vec![Value::Text("has".to_string()), key_val]);
            let outcome_has = engine.execute(session, &req_has);
            assert_eq!(
                outcome_has.reply,
                Value::Bool(ref_map.contains_key(&key_bytes))
            );

            let req_size = Value::Array(vec![Value::Text("size".to_string())]);
            let outcome_size = engine.execute(session, &req_size);
            let expected_size = i64::try_from(ref_map.len()).unwrap_or(0);
            assert_eq!(outcome_size.reply, Value::Int(expected_size));
        }
        _ => unreachable!(),
    }

    stats.commands_run += 1;
}

#[test]
fn test_model_50k_random_commands() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);
    let mut ref_map: HashMap<Vec<u8>, Value> = HashMap::new();

    let mut rng = XorShift64::new(0xFEED_FACE_CAFE_BABE);
    let key_space_size = 4_000;
    let num_commands = 50_000;

    let mut stats = ModelStats {
        commands_run: 0,
        puts_unconditional: 0,
        puts_if_absent: 0,
        puts_if_present: 0,
        drops_run: 0,
        resizes_seen: 0,
    };
    let mut prev_capacity = engine.store.table().capacity();

    for i in 0..num_commands {
        execute_model_command(
            &mut engine,
            &mut session,
            &mut ref_map,
            &mut rng,
            i,
            key_space_size,
            &mut stats,
        );

        let curr_cap = engine.store.table().capacity();
        if curr_cap > prev_capacity {
            stats.resizes_seen += 1;
            prev_capacity = curr_cap;
        }
    }

    assert_eq!(stats.commands_run, num_commands);
    assert_eq!(engine.store.len(), ref_map.len());
    assert!(
        stats.resizes_seen >= 5,
        "expected at least 5 resizes, saw {}",
        stats.resizes_seen
    );
    engine
        .store
        .check_invariants()
        .expect("invariants must hold after model test");

    println!(
        "\n[Model Test Summary] commands={}, puts_unconditional={}, puts_if_absent={}, puts_if_present={}, drops={}, resizes_seen={}, final_len={}",
        stats.commands_run,
        stats.puts_unconditional,
        stats.puts_if_absent,
        stats.puts_if_present,
        stats.drops_run,
        stats.resizes_seen,
        engine.store.len()
    );
}

#[test]
fn test_no_handler_returns_non_value_error_for_cmderror() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    let test_cases: Vec<(Value, u16)> = vec![
        (
            Value::Array(vec![Value::Text("nonexistent".to_string())]),
            2001,
        ),
        (
            Value::Array(vec![
                Value::Text("put".to_string()),
                Value::Text("k".to_string()),
            ]),
            2002,
        ),
        (
            Value::Array(vec![Value::Text("fetch".to_string()), Value::Int(1)]),
            2003,
        ),
        (
            Value::Array(vec![
                Value::Text("has".to_string()),
                Value::Text(String::new()),
            ]),
            2006,
        ),
        (Value::Int(123), 2008),
    ];

    for (req, expected_code) in test_cases {
        let outcome = engine.execute(&mut session, &req);
        match outcome.reply {
            Value::Error { code, .. } => {
                assert_eq!(code, expected_code, "expected error code {expected_code}");
            }
            other => panic!("expected Value::Error, got {other:?}"),
        }
    }
}
