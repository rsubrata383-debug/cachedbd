//! Integration tests for the `bump` command.

use cachedb::cmd::{
    ERR_EMPTY_KEY, ERR_INTEGER_OVERFLOW, ERR_INVALID_ARGUMENT, ERR_KEY_TOO_LARGE,
    ERR_NOT_AN_INTEGER, ERR_WRONG_ARG_COUNT, ERR_WRONG_TYPE, Engine, Session,
};
use cachedb::proto::Value;

#[test]
fn test_bump_spec_examples() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Request: ["bump", "page_views"] -> Reply: 1
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("page_views".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(1));

    // Request: ["bump", "page_views", 10] -> Reply: 11
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("page_views".to_string()),
        Value::Int(10),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(11));

    // Request: ["bump", "page_views", -2] -> Reply: 9
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("page_views".to_string()),
        Value::Int(-2),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(9));
}

#[test]
fn test_bump_steps() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Initialize key at 10
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
        Value::Int(10),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(10));

    // Default step is 1
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(11));

    // Negative step
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
        Value::Int(-5),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(6));

    // Step 0
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
        Value::Int(0),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(6));
}

#[test]
fn test_bump_missing_key() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Missing key with default step (0 + 1 = 1)
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("missing1".to_string()),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(1));

    // Missing key with positive step (0 + 5 = 5)
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("missing2".to_string()),
        Value::Int(5),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(5));

    // Missing key with negative step (0 + (-3) = -3)
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("missing3".to_string()),
        Value::Int(-3),
    ]);
    assert_eq!(engine.execute(&mut session, &req).reply, Value::Int(-3));
}

#[test]
fn test_bump_overflow_and_boundaries() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Set to i64::MAX - 1
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("max_key".to_string()),
        Value::Int(i64::MAX - 1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        Value::Int(i64::MAX - 1)
    );

    // Reach i64::MAX exactly -> works!
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("max_key".to_string()),
        Value::Int(1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        Value::Int(i64::MAX)
    );

    // i64::MAX + 1 -> error 3003, value unchanged
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("max_key".to_string()),
        Value::Int(1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INTEGER_OVERFLOW.to_value()
    );
    assert_eq!(
        engine.store().fetch(b"max_key"),
        Value::Int(i64::MAX),
        "value must remain unchanged on overflow"
    );

    // Set to i64::MIN + 1
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("min_key".to_string()),
        Value::Int(i64::MIN + 1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        Value::Int(i64::MIN + 1)
    );

    // Reach i64::MIN exactly -> works!
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("min_key".to_string()),
        Value::Int(-1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        Value::Int(i64::MIN)
    );

    // i64::MIN - 1 -> error 3003, value unchanged
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("min_key".to_string()),
        Value::Int(-1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INTEGER_OVERFLOW.to_value()
    );
    assert_eq!(
        engine.store().fetch(b"min_key"),
        Value::Int(i64::MIN),
        "value must remain unchanged on underflow"
    );
}

#[test]
fn test_bump_text_and_bytes_conversions() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Text "41" bumped by 1 gives Int 42 and stored value becomes Int 42
    let put_req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("text_num".to_string()),
        Value::Text("41".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &put_req).reply,
        Value::Bool(true)
    );

    let bump_req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("text_num".to_string()),
        Value::Int(1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &bump_req).reply,
        Value::Int(42)
    );
    assert_eq!(engine.store().fetch(b"text_num"), Value::Int(42));

    // Bytes b"41" bumped by 1 gives Int 42
    let put_req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("bytes_num".to_string()),
        Value::Bytes(b"41".to_vec()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &put_req).reply,
        Value::Bool(true)
    );

    let bump_req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("bytes_num".to_string()),
        Value::Int(1),
    ]);
    assert_eq!(
        engine.execute(&mut session, &bump_req).reply,
        Value::Int(42)
    );
    assert_eq!(engine.store().fetch(b"bytes_num"), Value::Int(42));

    // Invalid integer strings return 3004 and leave stored value unchanged:
    let invalid_strings = [
        "abc",
        "",
        " 1",
        "+1",
        "01",
        "1.5",
        "9223372036854775808",
        "-0",
    ];
    for (i, &raw) in invalid_strings.iter().enumerate() {
        let k = format!("bad_int:{i}");
        let put_req = Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text(k.clone()),
            Value::Text(raw.to_string()),
        ]);
        assert_eq!(
            engine.execute(&mut session, &put_req).reply,
            Value::Bool(true)
        );

        let bump_req = Value::Array(vec![
            Value::Text("bump".to_string()),
            Value::Text(k.clone()),
            Value::Int(1),
        ]);
        assert_eq!(
            engine.execute(&mut session, &bump_req).reply,
            ERR_NOT_AN_INTEGER.to_value(),
            "expected ERR_NOT_AN_INTEGER for '{raw}'"
        );
        assert_eq!(
            engine.store().fetch(k.as_bytes()),
            Value::Text(raw.to_string()),
            "stored value must remain unchanged on invalid text integer"
        );
    }
}

#[test]
fn test_bump_incompatible_stored_types() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Stored Float -> 3004
    let _ = engine.execute(
        &mut session,
        &Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("float_k".to_string()),
            Value::Float(42.5),
        ]),
    );
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("float_k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_NOT_AN_INTEGER.to_value()
    );

    // Stored Bool -> 3004
    let _ = engine.execute(
        &mut session,
        &Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("bool_k".to_string()),
            Value::Bool(true),
        ]),
    );
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("bool_k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_NOT_AN_INTEGER.to_value()
    );

    // Stored Nil -> 3004
    let _ = engine.execute(
        &mut session,
        &Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("nil_k".to_string()),
            Value::Nil,
        ]),
    );
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("nil_k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_NOT_AN_INTEGER.to_value()
    );

    // Stored Array -> 3001 (WRONG_TYPE)
    let _ = engine.execute(
        &mut session,
        &Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("arr_k".to_string()),
            Value::Array(vec![Value::Int(1)]),
        ]),
    );
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("arr_k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_TYPE.to_value()
    );

    // Stored Map -> 3001 (WRONG_TYPE)
    let _ = engine.execute(
        &mut session,
        &Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("map_k".to_string()),
            Value::Map(vec![(Value::Text("a".to_string()), Value::Int(1))]),
        ]),
    );
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("map_k".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_TYPE.to_value()
    );
}

#[test]
fn test_bump_invalid_arguments_and_boundaries() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Step as Text -> 2003
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
        Value::Text("1".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );

    // Step as Float -> 2003
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
        Value::Float(1.5),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_INVALID_ARGUMENT.to_value()
    );

    // Arity: 0 arguments -> 2002
    let req = Value::Array(vec![Value::Text("bump".to_string())]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );

    // Arity: 3 arguments -> 2002
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("k".to_string()),
        Value::Int(1),
        Value::Int(2),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_WRONG_ARG_COUNT.to_value()
    );

    // Empty key -> 2006
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text(String::new()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_EMPTY_KEY.to_value()
    );

    // Big key (> 1024 bytes) -> 2004
    let req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Bytes(vec![b'k'; 1025]),
    ]);
    assert_eq!(
        engine.execute(&mut session, &req).reply,
        ERR_KEY_TOO_LARGE.to_value()
    );

    // Key set by put, then bumped, fetch returns Int
    let put_req = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("counter".to_string()),
        Value::Int(100),
    ]);
    assert_eq!(
        engine.execute(&mut session, &put_req).reply,
        Value::Bool(true)
    );

    let bump_req = Value::Array(vec![
        Value::Text("bump".to_string()),
        Value::Text("counter".to_string()),
        Value::Int(50),
    ]);
    assert_eq!(
        engine.execute(&mut session, &bump_req).reply,
        Value::Int(150)
    );

    let fetch_req = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("counter".to_string()),
    ]);
    assert_eq!(
        engine.execute(&mut session, &fetch_req).reply,
        Value::Int(150)
    );
}
