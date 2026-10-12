//! Command layer error types and code constants.

use crate::proto::Value;
use std::fmt;

/// A command execution error describing error code, human-readable message,
/// and whether the server must terminate the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmdError {
    /// Numeric error code per `docs/spec/errors.md`.
    pub code: u16,
    /// Exact message string from the error catalog.
    pub message: &'static str,
    /// Whether this error requires closing the client connection.
    pub close: bool,
}

impl CmdError {
    /// Constructs a new [`CmdError`].
    #[must_use]
    pub const fn new(code: u16, message: &'static str, close: bool) -> Self {
        Self {
            code,
            message,
            close,
        }
    }

    /// Converts this error into a wire-protocol [`Value::Error`] reply item.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Error {
            code: self.code,
            message: self.message.to_string(),
        }
    }
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ERR {} {}", self.code, self.message)
    }
}

impl std::error::Error for CmdError {}

// 1000–1999: Protocol and Framing Errors (Fatal, connection closed)
/// 1001: Client sends unsupported version in HELLO.
pub const ERR_UNSUPPORTED_PROTOCOL_VERSION: CmdError =
    CmdError::new(1001, "unsupported protocol version", true);
/// 1002: Client sets reserved or unsupported flag bit.
pub const ERR_UNSUPPORTED_FLAG: CmdError = CmdError::new(1002, "unsupported flag", true);
/// 1003: Frame payload exceeds maximum permitted size.
pub const ERR_FRAME_TOO_LARGE: CmdError = CmdError::new(1003, "frame exceeds maximum size", true);
/// 1004: Header does not begin with `0xCA 0xDB`.
pub const ERR_INVALID_MAGIC: CmdError = CmdError::new(1004, "invalid protocol magic header", true);
/// 1005: Varint continuation exceeds 10 bytes or stream terminates early.
pub const ERR_MALFORMED_VARINT: CmdError = CmdError::new(1005, "malformed varint encoding", true);
/// 1006: Payload byte stream ends before declared item length.
pub const ERR_MALFORMED_PAYLOAD: CmdError = CmdError::new(1006, "payload decoding failed", true);
/// 1007: Payload contains unrecognized type tag byte without length prefix.
pub const ERR_UNKNOWN_TYPE_TAG: CmdError = CmdError::new(1007, "unknown type tag", true);

// 2000–2999: Command Syntax and Argument Errors
/// 2001: Command verb does not exist.
pub const ERR_UNKNOWN_COMMAND: CmdError = CmdError::new(2001, "unknown command", false);
/// 2002: Command receives fewer or more arguments than required.
pub const ERR_WRONG_ARG_COUNT: CmdError =
    CmdError::new(2002, "wrong number of arguments for command", false);
/// 2003: Argument cannot be parsed into expected target type.
pub const ERR_INVALID_ARGUMENT: CmdError = CmdError::new(2003, "invalid argument syntax", false);
/// 2004: Key length exceeds `max_key_size`.
pub const ERR_KEY_TOO_LARGE: CmdError = CmdError::new(2004, "key exceeds maximum size", false);
/// 2005: Value payload exceeds 32 MB limit.
pub const ERR_VALUE_TOO_LARGE: CmdError = CmdError::new(2005, "value exceeds maximum size", false);
/// 2006: Command supplied a 0-length key.
pub const ERR_EMPTY_KEY: CmdError = CmdError::new(2006, "empty key is not allowed", false);
/// 2007: Number of arguments exceeds 65,536 limit.
pub const ERR_TOO_MANY_ARGS: CmdError = CmdError::new(2007, "too many arguments", false);
/// 2008: Generic argument syntax error.
pub const ERR_SYNTAX_ERROR: CmdError = CmdError::new(2008, "syntax error", false);

// 3000–3999: Value and Data Type Errors
/// 3001: Calling type-specific command on a mismatched value type.
pub const ERR_WRONG_TYPE: CmdError = CmdError::new(
    3001,
    "operation against a key holding the wrong kind of value",
    false,
);
/// 3003: Atomic increment/decrement would exceed signed 64-bit integer limits.
pub const ERR_INTEGER_OVERFLOW: CmdError = CmdError::new(3003, "integer overflow", false);
/// 3004: Target string/bytes value cannot be parsed as a 64-bit integer.
pub const ERR_NOT_AN_INTEGER: CmdError = CmdError::new(3004, "value is not an integer", false);

// 4000–4999: Storage, Key, and Memory Errors
/// 4003: Memory limit is reached and eviction policy is `none` or unable to free space.
pub const ERR_OUT_OF_MEMORY: CmdError =
    CmdError::new(4003, "out of memory: maxmemory limit reached", false);

// 5000–5999: Security and Authentication Errors
/// 5001: Client sends command before successful authentication.
pub const ERR_AUTH_REQUIRED: CmdError = CmdError::new(5001, "authentication required", false);

// 6000–6999: Replication and Clustering Errors
/// 6001: Client attempts write command on a follower node.
pub const ERR_READONLY_REPLICA: CmdError =
    CmdError::new(6001, "cannot write to read-only replica", false);

// 7000–7999: Server and Persistence Errors
/// 7001: Unhandled internal runtime condition.
pub const ERR_INTERNAL_ERROR: CmdError = CmdError::new(7001, "internal server error", false);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_error_constants_match_spec_catalog() {
        let spec_text = include_str!("../../docs/spec/errors.md");

        let all_errors: &[(&str, CmdError)] = &[
            (
                "ERR_UNSUPPORTED_PROTOCOL_VERSION",
                ERR_UNSUPPORTED_PROTOCOL_VERSION,
            ),
            ("ERR_UNSUPPORTED_FLAG", ERR_UNSUPPORTED_FLAG),
            ("ERR_FRAME_TOO_LARGE", ERR_FRAME_TOO_LARGE),
            ("ERR_INVALID_MAGIC", ERR_INVALID_MAGIC),
            ("ERR_MALFORMED_VARINT", ERR_MALFORMED_VARINT),
            ("ERR_MALFORMED_PAYLOAD", ERR_MALFORMED_PAYLOAD),
            ("ERR_UNKNOWN_TYPE_TAG", ERR_UNKNOWN_TYPE_TAG),
            ("ERR_UNKNOWN_COMMAND", ERR_UNKNOWN_COMMAND),
            ("ERR_WRONG_ARG_COUNT", ERR_WRONG_ARG_COUNT),
            ("ERR_INVALID_ARGUMENT", ERR_INVALID_ARGUMENT),
            ("ERR_KEY_TOO_LARGE", ERR_KEY_TOO_LARGE),
            ("ERR_VALUE_TOO_LARGE", ERR_VALUE_TOO_LARGE),
            ("ERR_EMPTY_KEY", ERR_EMPTY_KEY),
            ("ERR_TOO_MANY_ARGS", ERR_TOO_MANY_ARGS),
            ("ERR_SYNTAX_ERROR", ERR_SYNTAX_ERROR),
            ("ERR_WRONG_TYPE", ERR_WRONG_TYPE),
            ("ERR_INTEGER_OVERFLOW", ERR_INTEGER_OVERFLOW),
            ("ERR_NOT_AN_INTEGER", ERR_NOT_AN_INTEGER),
            ("ERR_OUT_OF_MEMORY", ERR_OUT_OF_MEMORY),
            ("ERR_AUTH_REQUIRED", ERR_AUTH_REQUIRED),
            ("ERR_READONLY_REPLICA", ERR_READONLY_REPLICA),
            ("ERR_INTERNAL_ERROR", ERR_INTERNAL_ERROR),
        ];

        let mut spec_entries: HashMap<u16, (String, String, bool)> = HashMap::new();

        for line in spec_text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
                continue;
            }
            let parts: Vec<&str> = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if parts.len() < 5 {
                continue;
            }
            let code_str = parts[0].trim_matches('*').trim();
            let Ok(code) = code_str.parse::<u16>() else {
                continue;
            };
            let name = parts[1].trim_matches('`').trim().to_string();
            let msg = parts[2]
                .trim_matches('`')
                .trim_matches('"')
                .trim()
                .to_string();
            let closed = parts[4].contains("Yes");

            spec_entries.insert(code, (name, msg, closed));
        }

        for &(name, err) in all_errors {
            let (spec_name, spec_msg, spec_closed) = spec_entries
                .get(&err.code)
                .unwrap_or_else(|| panic!("error code {} not in errors.md", err.code));

            assert_eq!(spec_name, name, "name mismatch for code {}", err.code);
            assert_eq!(
                spec_msg, err.message,
                "message mismatch for code {}",
                err.code
            );
            assert_eq!(
                *spec_closed, err.close,
                "close flag mismatch for code {}",
                err.code
            );

            let val = err.to_value();
            match val {
                Value::Error { code, message } => {
                    assert_eq!(code, err.code);
                    assert_eq!(message, err.message);
                }
                _ => panic!("expected Value::Error"),
            }
        }
    }
}
