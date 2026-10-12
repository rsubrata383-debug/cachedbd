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

    #[test]
    fn test_error_constants_match_spec_catalog() {
        let expected: &[(CmdError, u16, &str, bool)] = &[
            (ERR_UNKNOWN_COMMAND, 2001, "unknown command", false),
            (
                ERR_WRONG_ARG_COUNT,
                2002,
                "wrong number of arguments for command",
                false,
            ),
            (ERR_INVALID_ARGUMENT, 2003, "invalid argument syntax", false),
            (ERR_KEY_TOO_LARGE, 2004, "key exceeds maximum size", false),
            (
                ERR_VALUE_TOO_LARGE,
                2005,
                "value exceeds maximum size",
                false,
            ),
            (ERR_EMPTY_KEY, 2006, "empty key is not allowed", false),
            (ERR_TOO_MANY_ARGS, 2007, "too many arguments", false),
            (ERR_SYNTAX_ERROR, 2008, "syntax error", false),
            (
                ERR_WRONG_TYPE,
                3001,
                "operation against a key holding the wrong kind of value",
                false,
            ),
            (ERR_INTEGER_OVERFLOW, 3003, "integer overflow", false),
            (ERR_NOT_AN_INTEGER, 3004, "value is not an integer", false),
            (
                ERR_OUT_OF_MEMORY,
                4003,
                "out of memory: maxmemory limit reached",
                false,
            ),
            (ERR_AUTH_REQUIRED, 5001, "authentication required", false),
            (
                ERR_READONLY_REPLICA,
                6001,
                "cannot write to read-only replica",
                false,
            ),
            (ERR_INTERNAL_ERROR, 7001, "internal server error", false),
        ];

        for &(err, code, msg, close) in expected {
            assert_eq!(err.code, code, "error code mismatch for {msg}");
            assert_eq!(err.message, msg, "error message mismatch for code {code}");
            assert_eq!(err.close, close, "close flag mismatch for code {code}");

            let val = err.to_value();
            match val {
                Value::Error {
                    code: v_code,
                    message: v_msg,
                } => {
                    assert_eq!(v_code, code);
                    assert_eq!(v_msg, msg);
                }
                _ => panic!("expected Value::Error"),
            }
        }
    }
}
