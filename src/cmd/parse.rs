//! Command request parsing and argument validation.

use crate::cmd::error::{
    CmdError, ERR_EMPTY_KEY, ERR_INVALID_ARGUMENT, ERR_KEY_TOO_LARGE, ERR_SYNTAX_ERROR,
    ERR_TOO_MANY_ARGS,
};
use crate::proto::Value;

/// Default maximum key size in bytes per Section 4 of `docs/spec/values.md` (1 KB).
pub const DEFAULT_MAX_KEY_SIZE: usize = 1024;

/// Default maximum argument count per Section 4 of `docs/spec/values.md` (65,536).
pub const DEFAULT_MAX_ARGS: usize = 65_536;

/// Parses a decoded command payload into command verb bytes and argument slice.
///
/// # Requirements
///
/// - The input `request` must be a non-empty [`Value::Array`].
/// - The first item in the array must be [`Value::Text`] or [`Value::Bytes`].
/// - The number of arguments following the verb must not exceed `max_args`.
///
/// # Errors
///
/// - Returns [`ERR_SYNTAX_ERROR`] (2008) if `request` is not a non-empty `Array` or its
///   first item is not `Text` or `Bytes`.
/// - Returns [`ERR_TOO_MANY_ARGS`] (2007) if the argument count exceeds `max_args`.
pub fn parse_command(request: &Value, max_args: usize) -> Result<(&[u8], &[Value]), CmdError> {
    let Value::Array(items) = request else {
        return Err(ERR_SYNTAX_ERROR);
    };

    if items.is_empty() {
        return Err(ERR_SYNTAX_ERROR);
    }

    let verb = match &items[0] {
        Value::Text(s) => s.as_bytes(),
        Value::Bytes(b) => b.as_slice(),
        _ => return Err(ERR_SYNTAX_ERROR),
    };

    let args = &items[1..];
    if args.len() > max_args {
        return Err(ERR_TOO_MANY_ARGS);
    }

    Ok((verb, args))
}

/// Parses and validates a key argument against the default maximum key size (1024 bytes).
///
/// # Errors
///
/// - Returns [`ERR_INVALID_ARGUMENT`] (2003) if `val` is not `Text` or `Bytes`.
/// - Returns [`ERR_EMPTY_KEY`] (2006) if the key byte sequence is empty (0 bytes).
/// - Returns [`ERR_KEY_TOO_LARGE`] (2004) if the key byte length exceeds 1024 bytes.
pub fn key_arg(val: &Value) -> Result<&[u8], CmdError> {
    key_arg_with_limit(val, DEFAULT_MAX_KEY_SIZE)
}

/// Parses and validates a key argument against an explicit maximum key size.
///
/// # Errors
///
/// - Returns [`ERR_INVALID_ARGUMENT`] (2003) if `val` is not `Text` or `Bytes`.
/// - Returns [`ERR_EMPTY_KEY`] (2006) if the key byte sequence is empty (0 bytes).
/// - Returns [`ERR_KEY_TOO_LARGE`] (2004) if the key byte length exceeds `max_key_size`.
pub fn key_arg_with_limit(val: &Value, max_key_size: usize) -> Result<&[u8], CmdError> {
    let bytes = match val {
        Value::Text(s) => s.as_bytes(),
        Value::Bytes(b) => b.as_slice(),
        _ => return Err(ERR_INVALID_ARGUMENT),
    };

    if bytes.is_empty() {
        return Err(ERR_EMPTY_KEY);
    }

    if bytes.len() > max_key_size {
        return Err(ERR_KEY_TOO_LARGE);
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_valid_text_and_bytes() {
        let req_text = Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("k".to_string()),
            Value::Text("v".to_string()),
        ]);
        let (verb, args) = parse_command(&req_text, 65_536).unwrap();
        assert_eq!(verb, b"put");
        assert_eq!(args.len(), 2);

        let req_bytes = Value::Array(vec![
            Value::Bytes(b"fetch".to_vec()),
            Value::Bytes(b"k".to_vec()),
        ]);
        let (verb, args) = parse_command(&req_bytes, 65_536).unwrap();
        assert_eq!(verb, b"fetch");
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn test_parse_command_syntax_errors() {
        // Not an array
        let not_arr = Value::Text("ping".to_string());
        assert_eq!(parse_command(&not_arr, 65_536), Err(ERR_SYNTAX_ERROR));

        // Empty array
        let empty_arr = Value::Array(vec![]);
        assert_eq!(parse_command(&empty_arr, 65_536), Err(ERR_SYNTAX_ERROR));

        // First item not Text or Bytes
        let int_verb = Value::Array(vec![Value::Int(123)]);
        assert_eq!(parse_command(&int_verb, 65_536), Err(ERR_SYNTAX_ERROR));
    }

    #[test]
    fn test_parse_command_max_args() {
        let items = vec![
            Value::Text("ping".to_string()),
            Value::Int(1),
            Value::Int(2),
        ];
        let req = Value::Array(items);
        assert_eq!(parse_command(&req, 1), Err(ERR_TOO_MANY_ARGS));
        assert!(parse_command(&req, 2).is_ok());
    }

    #[test]
    fn test_key_arg_validation() {
        // Text OK
        assert_eq!(
            key_arg(&Value::Text("user:1".to_string())).unwrap(),
            b"user:1"
        );
        // Bytes OK
        assert_eq!(
            key_arg(&Value::Bytes(b"binary\x00key".to_vec())).unwrap(),
            b"binary\x00key"
        );

        // Wrong type -> 2003
        assert_eq!(key_arg(&Value::Int(42)), Err(ERR_INVALID_ARGUMENT));
        assert_eq!(key_arg(&Value::Nil), Err(ERR_INVALID_ARGUMENT));

        // Empty -> 2006
        assert_eq!(key_arg(&Value::Text(String::new())), Err(ERR_EMPTY_KEY));
        assert_eq!(key_arg(&Value::Bytes(vec![])), Err(ERR_EMPTY_KEY));

        // Key too large -> 2004
        let large_key = vec![b'x'; 1025];
        assert_eq!(key_arg(&Value::Bytes(large_key)), Err(ERR_KEY_TOO_LARGE));

        // Boundary: 1024 OK
        let max_key = vec![b'x'; 1024];
        assert!(key_arg(&Value::Bytes(max_key)).is_ok());
    }
}
