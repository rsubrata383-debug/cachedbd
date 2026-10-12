//! Connection handshake and protocol version negotiation (HELLO).
//!
//! Implements Section 5 of `docs/spec/protocol.md` and Section 3.1 of `docs/spec/commands.md`.

use crate::proto::header::ProtoError;
use crate::proto::item::Value;

/// Maximum allowed length in bytes for a client identifying name.
pub const MAX_CLIENT_NAME_LEN: usize = 1024;

/// Connection handshake request payload sent by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloRequest {
    /// Client's protocol version (must be `1` in v0.1).
    pub protocol_version: i64,
    /// Client library name and version (e.g. `"skiff-cli 0.1.0"`).
    pub client_name: String,
    /// Optional authentication credentials or token.
    pub auth_token: Option<String>,
}

impl HelloRequest {
    /// Creates a new `HelloRequest`.
    #[must_use]
    pub const fn new(
        protocol_version: i64,
        client_name: String,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            protocol_version,
            client_name,
            auth_token,
        }
    }

    /// Serializes the request into a protocol [`Value::Array`].
    #[must_use]
    pub fn to_value(&self) -> Value {
        let auth_val = self
            .auth_token
            .as_ref()
            .map_or(Value::Nil, |token| Value::Text(token.clone()));

        Value::Array(vec![
            Value::Int(self.protocol_version),
            Value::Text(self.client_name.clone()),
            auth_val,
        ])
    }

    /// Parses a `HelloRequest` from a protocol [`Value`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::ERR_MALFORMED_PAYLOAD`] if the shape is not an Array of 2 or 3 items,
    /// if field types do not match, or if `client_name` is empty or exceeds [`MAX_CLIENT_NAME_LEN`].
    pub fn from_value(value: &Value) -> Result<Self, ProtoError> {
        let Value::Array(items) = value else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };

        if items.len() != 2 && items.len() != 3 {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }

        let Some(Value::Int(version)) = items.first() else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };

        let Some(Value::Text(name)) = items.get(1) else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };

        if name.is_empty() || name.len() > MAX_CLIENT_NAME_LEN {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }

        let auth_token = match items.get(2) {
            None | Some(Value::Nil) => None,
            Some(Value::Text(t)) => Some(t.clone()),
            Some(Value::Bytes(b)) => {
                let s = std::str::from_utf8(b).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
                Some(s.to_string())
            }
            _ => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
        };

        Ok(Self {
            protocol_version: *version,
            client_name: name.clone(),
            auth_token,
        })
    }
}

/// Connection handshake response payload returned by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloResponse {
    /// Server name (e.g. `"skiffd"`).
    pub server_name: String,
    /// Server software version (e.g. `"0.1.0"`).
    pub server_version: String,
    /// Negotiated protocol version (must be `1` in v0.1).
    pub protocol_version: i64,
    /// Unique connection identifier assigned to the client.
    pub client_id: i64,
    /// Maximum allowed frame size in bytes.
    pub max_frame_size: Option<i64>,
    /// Maximum allowed key size in bytes.
    pub max_key_size: Option<i64>,
    /// Maximum allowed value size in bytes.
    pub max_value_size: Option<i64>,
}

impl HelloResponse {
    /// Creates a new `HelloResponse` with server metadata.
    #[must_use]
    pub const fn new(
        server_name: String,
        server_version: String,
        protocol_version: i64,
        client_id: i64,
    ) -> Self {
        Self {
            server_name,
            server_version,
            protocol_version,
            client_id,
            max_frame_size: None,
            max_key_size: None,
            max_value_size: None,
        }
    }

    /// Serializes the response into a protocol [`Value::Map`].
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut entries = vec![
            (
                Value::Text("server_name".to_string()),
                Value::Text(self.server_name.clone()),
            ),
            (
                Value::Text("server_version".to_string()),
                Value::Text(self.server_version.clone()),
            ),
            (
                Value::Text("protocol_version".to_string()),
                Value::Int(self.protocol_version),
            ),
            (
                Value::Text("client_id".to_string()),
                Value::Int(self.client_id),
            ),
        ];

        if let Some(mfs) = self.max_frame_size {
            entries.push((Value::Text("max_frame_size".to_string()), Value::Int(mfs)));
        }
        if let Some(mks) = self.max_key_size {
            entries.push((Value::Text("max_key_size".to_string()), Value::Int(mks)));
        }
        if let Some(mvs) = self.max_value_size {
            entries.push((Value::Text("max_value_size".to_string()), Value::Int(mvs)));
        }

        Value::Map(entries)
    }

    /// Parses a `HelloResponse` from a protocol [`Value`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::ERR_MALFORMED_PAYLOAD`] if the shape is not a Map or if required
    /// fields (`server_version`, `protocol_version`, `client_id`) are missing or of invalid type.
    pub fn from_value(value: &Value) -> Result<Self, ProtoError> {
        let Value::Map(entries) = value else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };

        let mut server_name = None;
        let mut server_version = None;
        let mut protocol_version = None;
        let mut client_id = None;
        let mut max_frame_size = None;
        let mut max_key_size = None;
        let mut max_value_size = None;

        for (k, v) in entries {
            let Value::Text(key) = k else {
                continue;
            };
            match (key.as_str(), v) {
                ("server_name", Value::Text(s)) => server_name = Some(s.clone()),
                ("server_version", Value::Text(s)) => server_version = Some(s.clone()),
                ("protocol_version", Value::Int(i)) => protocol_version = Some(*i),
                ("client_id", Value::Int(i)) => client_id = Some(*i),
                ("max_frame_size", Value::Int(i)) => max_frame_size = Some(*i),
                ("max_key_size", Value::Int(i)) => max_key_size = Some(*i),
                ("max_value_size", Value::Int(i)) => max_value_size = Some(*i),
                _ => {}
            }
        }

        let Some(server_version) = server_version else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };
        let Some(protocol_version) = protocol_version else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };
        let Some(client_id) = client_id else {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        };

        Ok(Self {
            server_name: server_name.unwrap_or_else(|| "skiffd".to_string()),
            server_version,
            protocol_version,
            client_id,
            max_frame_size,
            max_key_size,
            max_value_size,
        })
    }
}

/// Negotiates the client protocol version against server capabilities.
///
/// In v0.1, only protocol version 1 is supported.
///
/// # Errors
///
/// Returns [`ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION`] (1001) if `client_version != 1`.
pub const fn negotiate_version(client_version: i64) -> Result<u8, ProtoError> {
    if client_version == 1 {
        Ok(1)
    } else {
        Err(ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::decode::decode_frame;
    use crate::proto::encode::encode_frame;
    use crate::proto::header::{FLAG_RESPONSE, Header, Opcode};
    use crate::proto::{Decoded, Frame};

    struct XorShift64(u64);

    impl XorShift64 {
        fn new(seed: u64) -> Self {
            Self(if seed == 0 {
                0xCAFE_BABE_DEAD_BEEF
            } else {
                seed
            })
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    #[test]
    fn test_hello_request_round_trip() {
        let req = HelloRequest::new(
            1,
            "skiff-cli 0.1.0".to_string(),
            Some("secret-token".to_string()),
        );
        let val = req.to_value();
        let parsed = HelloRequest::from_value(&val).expect("parses successfully");
        assert_eq!(req, parsed);

        // Without auth token
        let req2 = HelloRequest::new(1, "test-client".to_string(), None);
        let val2 = req2.to_value();
        let parsed2 = HelloRequest::from_value(&val2).expect("parses successfully");
        assert_eq!(req2, parsed2);
    }

    #[test]
    fn test_hello_response_round_trip() {
        let mut resp = HelloResponse::new("skiffd".to_string(), "0.1.0".to_string(), 1, 42);
        resp.max_frame_size = Some(67_108_864);
        resp.max_key_size = Some(1024);
        resp.max_value_size = Some(33_554_432);

        let val = resp.to_value();
        let parsed = HelloResponse::from_value(&val).expect("parses successfully");
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_hello_frames_encode_decode() {
        // Full Frame with Hello Opcode
        let req = HelloRequest::new(1, "skiff-cli".to_string(), None);
        let req_frame = Frame::new(Header::new(0x00, 1, Opcode::Hello, 0), req.to_value());

        let mut buf = Vec::new();
        encode_frame(&req_frame, &mut buf).expect("encode frame");

        let decoded = decode_frame(&buf).expect("decode frame");
        let (frame, used) = match decoded {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("expected complete"),
        };
        assert_eq!(used, buf.len());
        assert_eq!(frame.header.opcode, Opcode::Hello);
        let parsed_req = HelloRequest::from_value(&frame.payload).expect("parses request");
        assert_eq!(parsed_req, req);

        // Response Frame
        let resp = HelloResponse::new("skiffd".to_string(), "0.1.0".to_string(), 1, 100);
        let resp_frame = Frame::new(
            Header::new(FLAG_RESPONSE, 1, Opcode::Hello, 0),
            resp.to_value(),
        );

        let mut resp_buf = Vec::new();
        encode_frame(&resp_frame, &mut resp_buf).expect("encode frame");

        let decoded_resp = decode_frame(&resp_buf).expect("decode frame");
        let (f2, u2) = match decoded_resp {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("expected complete"),
        };
        assert_eq!(u2, resp_buf.len());
        assert_eq!(f2.header.flags, FLAG_RESPONSE);
        let parsed_resp = HelloResponse::from_value(&f2.payload).expect("parses response");
        assert_eq!(parsed_resp, resp);
    }

    #[test]
    fn test_hello_exact_bytes_full_frame() {
        // 1. HelloRequest exact byte representation
        let req = HelloRequest::new(1, "skiff-cli".to_string(), None);
        let req_frame = Frame::new(Header::new(0x00, 1, Opcode::Hello, 0), req.to_value());
        let mut req_buf = Vec::new();
        encode_frame(&req_frame, &mut req_buf).expect("encode req frame");

        let expected_req_bytes: &[u8] = &[
            0xCA, 0xDB, // Magic
            0x01, // Version 1
            0x00, // Flags (Request)
            0x00, 0x00, 0x00, 0x01, // Request ID 1
            0x00, 0x01, // Opcode: Hello (0x0001)
            0x00, 0x00, 0x00, 0x10, // Payload length: 16 bytes
            // Payload (Array of 3 elements: Int(1), Text("skiff-cli"), Nil)
            0x06, // Tag: Array
            0x03, // Count: 3
            0x01, 0x02, // Int(1) -> zigzag(1) = 2
            0x04, 0x09, b's', b'k', b'i', b'f', b'f', b'-', b'c', b'l',
            b'i', // Text("skiff-cli")
            0x00, // Nil
        ];
        assert_eq!(&req_buf[..], expected_req_bytes);

        // Decode from exact bytes
        let decoded_req = decode_frame(expected_req_bytes).expect("decode expected req");
        let (f_req, u_req) = match decoded_req {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("expected complete"),
        };
        assert_eq!(u_req, expected_req_bytes.len());
        assert_eq!(
            HelloRequest::from_value(&f_req.payload).expect("parse req"),
            req
        );

        // 2. HelloResponse exact byte representation
        let resp = HelloResponse::new("skiffd".to_string(), "0.1.0".to_string(), 1, 42);
        let resp_frame = Frame::new(
            Header::new(FLAG_RESPONSE, 1, Opcode::Hello, 0),
            resp.to_value(),
        );
        let mut resp_buf = Vec::new();
        encode_frame(&resp_frame, &mut resp_buf).expect("encode resp frame");

        let expected_resp_bytes: &[u8] = &[
            0xCA, 0xDB, // Magic
            0x01, // Version 1
            0x01, // Flags (FLAG_RESPONSE)
            0x00, 0x00, 0x00, 0x01, // Request ID 1
            0x00, 0x01, // Opcode: Hello (0x0001)
            0x00, 0x00, 0x00, 0x4F, // Payload length: 79 bytes
            // Payload (Map of 4 entries)
            0x07, // Tag: Map
            0x04, // Count: 4
            // Entry 1: "server_name" -> "skiffd"
            0x04, 0x0B, b's', b'e', b'r', b'v', b'e', b'r', b'_', b'n', b'a', b'm', b'e', 0x04,
            0x06, b's', b'k', b'i', b'f', b'f', b'd',
            // Entry 2: "server_version" -> "0.1.0"
            0x04, 0x0E, b's', b'e', b'r', b'v', b'e', b'r', b'_', b'v', b'e', b'r', b's', b'i',
            b'o', b'n', 0x04, 0x05, b'0', b'.', b'1', b'.', b'0',
            // Entry 3: "protocol_version" -> 1
            0x04, 0x10, b'p', b'r', b'o', b't', b'o', b'c', b'o', b'l', b'_', b'v', b'e', b'r',
            b's', b'i', b'o', b'n', 0x01, 0x02, // Entry 4: "client_id" -> 42
            0x04, 0x09, b'c', b'l', b'i', b'e', b'n', b't', b'_', b'i', b'd', 0x01,
            0x54, // zigzag(42) = 84 = 0x54
        ];
        assert_eq!(&resp_buf[..], expected_resp_bytes);

        // Decode from exact bytes
        let decoded_resp = decode_frame(expected_resp_bytes).expect("decode expected resp");
        let (f_resp, u_resp) = match decoded_resp {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("expected complete"),
        };
        assert_eq!(u_resp, expected_resp_bytes.len());
        assert_eq!(
            HelloResponse::from_value(&f_resp.payload).expect("parse resp"),
            resp
        );
    }

    #[test]
    fn test_negotiate_version() {
        assert_eq!(negotiate_version(1), Ok(1));
        assert_eq!(
            negotiate_version(0),
            Err(ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION)
        );
        assert_eq!(
            negotiate_version(2),
            Err(ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION)
        );
        assert_eq!(
            negotiate_version(255),
            Err(ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION)
        );
        assert_eq!(
            negotiate_version(-1),
            Err(ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn test_hello_invalid_shapes() {
        // Not an Array
        assert_eq!(
            HelloRequest::from_value(&Value::Nil),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        assert_eq!(
            HelloRequest::from_value(&Value::Int(1)),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Too few items (1 item)
        assert_eq!(
            HelloRequest::from_value(&Value::Array(vec![Value::Int(1)])),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Too many items (4 items)
        assert_eq!(
            HelloRequest::from_value(&Value::Array(vec![
                Value::Int(1),
                Value::Text("client".to_string()),
                Value::Nil,
                Value::Int(99)
            ])),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Empty client name
        assert_eq!(
            HelloRequest::from_value(&Value::Array(vec![
                Value::Int(1),
                Value::Text(String::new())
            ])),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Wrong types
        assert_eq!(
            HelloRequest::from_value(&Value::Array(vec![
                Value::Text("1".to_string()),
                Value::Text("client".to_string())
            ])),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Response missing required fields
        assert_eq!(
            HelloResponse::from_value(&Value::Nil),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        assert_eq!(
            HelloResponse::from_value(&Value::Map(vec![])),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
    }

    #[test]
    fn test_fuzz_never_panic_on_arbitrary_values() {
        let mut rng = XorShift64::new(0x9876_5432_10FE_DCBA);

        for _ in 0..2000 {
            let val = match rng.next_u64() % 7 {
                0 => Value::Nil,
                1 => Value::Int(rng.next_u64() as i64),
                2 => Value::Text(format!("rand_{}", rng.next_u64())),
                3 => Value::Bytes(vec![rng.next_u64() as u8; (rng.next_u64() % 16) as usize]),
                4 => Value::Array(vec![
                    Value::Int(rng.next_u64() as i64),
                    Value::Text(format!("cli_{}", rng.next_u64())),
                ]),
                5 => Value::Map(vec![(
                    Value::Text("protocol_version".to_string()),
                    Value::Int(rng.next_u64() as i64),
                )]),
                _ => Value::Bool(rng.next_u64() % 2 == 0),
            };

            let _ = HelloRequest::from_value(&val);
            let _ = HelloResponse::from_value(&val);
        }
    }
}
