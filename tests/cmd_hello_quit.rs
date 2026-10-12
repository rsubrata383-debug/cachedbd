//! Integration tests for `hello` and `quit` commands via [`Engine`].

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use skiffdb::cmd::{
    ERR_AUTH_REQUIRED, ERR_INVALID_ARGUMENT, ERR_UNSUPPORTED_PROTOCOL_VERSION, ERR_WRONG_ARG_COUNT,
    Engine, Session,
};
use skiffdb::proto::Value;

#[test]
fn test_hello_spec_example() {
    let mut engine = Engine::new();
    let mut session = Session::new(42);

    let req = Value::Array(vec![
        Value::Text("hello".to_string()),
        Value::Int(1),
        Value::Text("skiff-cli 0.1.0".to_string()),
    ]);

    let outcome = engine.execute(&mut session, &req);
    assert!(!outcome.close);
    assert!(session.authenticated);

    let Value::Map(entries) = outcome.reply else {
        panic!("expected Value::Map reply from hello");
    };

    let map: std::collections::HashMap<String, Value> = entries
        .into_iter()
        .map(|(k, v)| {
            let Value::Text(s) = k else {
                panic!("map key must be Text");
            };
            (s, v)
        })
        .collect();

    assert_eq!(
        map.get("server_name"),
        Some(&Value::Text("skiffd".to_string()))
    );
    assert_eq!(
        map.get("server_version"),
        Some(&Value::Text("0.1.0".to_string()))
    );
    assert_eq!(map.get("protocol_version"), Some(&Value::Int(1)));
    assert_eq!(map.get("client_id"), Some(&Value::Int(42)));
    assert_eq!(map.get("max_frame_size"), Some(&Value::Int(67_108_864)));
    assert_eq!(map.get("max_key_size"), Some(&Value::Int(1024)));
    assert_eq!(map.get("max_value_size"), Some(&Value::Int(33_554_432)));
}

#[test]
fn test_hello_with_auth_token_and_nil() {
    let mut engine = Engine::new();
    let mut session = Session::new(7);

    // With explicit text token
    let req1 = Value::Array(vec![
        Value::Text("hello".to_string()),
        Value::Int(1),
        Value::Text("client".to_string()),
        Value::Text("secret-token".to_string()),
    ]);
    let outcome1 = engine.execute(&mut session, &req1);
    assert!(!outcome1.close);
    assert!(matches!(outcome1.reply, Value::Map(_)));

    // With nil token
    let req2 = Value::Array(vec![
        Value::Text("hello".to_string()),
        Value::Int(1),
        Value::Text("client".to_string()),
        Value::Nil,
    ]);
    let outcome2 = engine.execute(&mut session, &req2);
    assert!(!outcome2.close);
    assert!(matches!(outcome2.reply, Value::Map(_)));
}

#[test]
fn test_hello_unsupported_version_closes_connection() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    for bad_ver in [0, 2, 99, -1] {
        let req = Value::Array(vec![
            Value::Text("hello".to_string()),
            Value::Int(bad_ver),
            Value::Text("client".to_string()),
        ]);
        let outcome = engine.execute(&mut session, &req);
        assert!(
            outcome.close,
            "unsupported protocol version {bad_ver} must close connection"
        );
        assert_eq!(outcome.reply, ERR_UNSUPPORTED_PROTOCOL_VERSION.to_value());
    }
}

#[test]
fn test_hello_arity_and_argument_validation() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Arity errors (2002)
    let arity_cases = [
        vec![Value::Text("hello".to_string())],
        vec![Value::Text("hello".to_string()), Value::Int(1)],
        vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("c".to_string()),
            Value::Nil,
            Value::Text("extra".to_string()),
        ],
    ];
    for args in arity_cases {
        let outcome = engine.execute(&mut session, &Value::Array(args));
        assert!(!outcome.close);
        assert_eq!(outcome.reply, ERR_WRONG_ARG_COUNT.to_value());
    }

    // Argument syntax / type errors (2003)
    let type_cases = [
        // version is Text
        vec![
            Value::Text("hello".to_string()),
            Value::Text("1".to_string()),
            Value::Text("client".to_string()),
        ],
        // client_name is Int
        vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Int(100),
        ],
        // client_name is empty
        vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text(String::new()),
        ],
        // client_name too long (> 1024 bytes)
        vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("a".repeat(1025)),
        ],
        // auth_token is Int (invalid type)
        vec![
            Value::Text("hello".to_string()),
            Value::Int(1),
            Value::Text("client".to_string()),
            Value::Int(123),
        ],
    ];
    for args in type_cases {
        let outcome = engine.execute(&mut session, &Value::Array(args));
        assert!(!outcome.close);
        assert_eq!(outcome.reply, ERR_INVALID_ARGUMENT.to_value());
    }
}

#[test]
fn test_quit_spec_example_and_arity() {
    let mut engine = Engine::new();
    let mut session = Session::new(1);

    // Graceful quit sets close = true and returns Text("ok")
    let req_quit = Value::Array(vec![Value::Text("quit".to_string())]);
    let outcome = engine.execute(&mut session, &req_quit);
    assert!(outcome.close);
    assert_eq!(outcome.reply, Value::Text("ok".to_string()));

    // With arguments: returns 2002 and does NOT close
    let req_quit_extra = Value::Array(vec![
        Value::Text("quit".to_string()),
        Value::Text("now".to_string()),
    ]);
    let outcome_extra = engine.execute(&mut session, &req_quit_extra);
    assert!(!outcome_extra.close);
    assert_eq!(outcome_extra.reply, ERR_WRONG_ARG_COUNT.to_value());
}

#[test]
fn test_commands_allowed_without_hello_first() {
    let mut engine = Engine::new();
    let mut session = Session::new(10);
    assert!(!session.authenticated);

    // Client directly sends ping without hello
    let req_ping = Value::Array(vec![Value::Text("ping".to_string())]);
    let outcome_ping = engine.execute(&mut session, &req_ping);
    assert_eq!(outcome_ping.reply, Value::Text("pong".to_string()));

    // Client directly sends put without hello
    let req_put = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("my_key".to_string()),
        Value::Text("my_val".to_string()),
    ]);
    let outcome_put = engine.execute(&mut session, &req_put);
    assert_eq!(outcome_put.reply, Value::Bool(true));

    // Client directly sends fetch without hello
    let req_fetch = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("my_key".to_string()),
    ]);
    let outcome_fetch = engine.execute(&mut session, &req_fetch);
    assert_eq!(outcome_fetch.reply, Value::Text("my_val".to_string()));
}

#[test]
fn test_hello_require_auth_does_not_authenticate_and_no_auth_commands_work() {
    use skiffdb::cmd::handle_request;
    use skiffdb::proto::Frame;
    use skiffdb::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};

    let mut engine = Engine::new();
    engine.config_mut().require_auth = true;

    let mut session = Session::new(55);
    assert!(!session.authenticated);

    // 1. `hello` command returns ERR_AUTH_REQUIRED (5001) without authenticating
    let req_hello_no_token = Value::Array(vec![
        Value::Text("hello".to_string()),
        Value::Int(1),
        Value::Text("client-a".to_string()),
    ]);
    let outcome = engine.execute(&mut session, &req_hello_no_token);
    assert!(!outcome.close);
    assert_eq!(outcome.reply, ERR_AUTH_REQUIRED.to_value());
    assert!(!session.authenticated);

    // With auth token provided, still returns ERR_AUTH_REQUIRED (auth backend in later phase)
    let req_hello_with_token = Value::Array(vec![
        Value::Text("hello".to_string()),
        Value::Int(1),
        Value::Text("client-a".to_string()),
        Value::Text("secret_token".to_string()),
    ]);
    let outcome = engine.execute(&mut session, &req_hello_with_token);
    assert!(!outcome.close);
    assert_eq!(outcome.reply, ERR_AUTH_REQUIRED.to_value());
    assert!(!session.authenticated);

    // 2. Data commands (put/fetch) are rejected with ERR_AUTH_REQUIRED (5001)
    let req_put = Value::Array(vec![
        Value::Text("put".to_string()),
        Value::Text("secret_key".to_string()),
        Value::Text("secret_val".to_string()),
    ]);
    let outcome = engine.execute(&mut session, &req_put);
    assert!(!outcome.close);
    assert_eq!(outcome.reply, ERR_AUTH_REQUIRED.to_value());

    let req_fetch = Value::Array(vec![
        Value::Text("fetch".to_string()),
        Value::Text("secret_key".to_string()),
    ]);
    let outcome = engine.execute(&mut session, &req_fetch);
    assert!(!outcome.close);
    assert_eq!(outcome.reply, ERR_AUTH_REQUIRED.to_value());

    // 3. NO_AUTH commands (ping, quit) still work
    let req_ping = Value::Array(vec![Value::Text("ping".to_string())]);
    let outcome = engine.execute(&mut session, &req_ping);
    assert!(!outcome.close);
    assert_eq!(outcome.reply, Value::Text("pong".to_string()));

    let req_quit = Value::Array(vec![Value::Text("quit".to_string())]);
    let outcome = engine.execute(&mut session, &req_quit);
    assert!(outcome.close);
    assert_eq!(outcome.reply, Value::Text("ok".to_string()));

    // 4. HELLO wire frame (Opcode 1) also returns ERR_AUTH_REQUIRED without close
    let mut frame_session = Session::new(99);
    let hello_frame = Frame::new(
        Header::new(0, 1234, Opcode::Hello, 0),
        Value::Array(vec![
            Value::Int(1),
            Value::Text("wire-client".to_string()),
            Value::Text("some_token".to_string()),
        ]),
    );
    let outcome = handle_request(&mut engine, &mut frame_session, &hello_frame);
    assert!(!outcome.close);
    assert_eq!(outcome.reply_frame.header.request_id, 1234);
    assert_eq!(outcome.reply_frame.header.flags, FLAG_RESPONSE | FLAG_ERROR);
    assert_eq!(outcome.reply_frame.payload, ERR_AUTH_REQUIRED.to_value());
    assert!(!frame_session.authenticated);
}
