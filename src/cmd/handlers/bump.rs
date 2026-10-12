//! Handler for the `bump` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::{
    CmdError, ERR_INTEGER_OVERFLOW, ERR_INVALID_ARGUMENT, ERR_NOT_AN_INTEGER, ERR_OUT_OF_MEMORY,
    ERR_WRONG_TYPE,
};
use crate::cmd::parse::key_arg_with_limit;
use crate::proto::Value;
use crate::store::PutCondition;

/// Parses a byte slice as a strict base-10 64-bit signed integer.
///
/// Rules:
/// - Optional single leading `'-'` (no `'+'`)
/// - ASCII digits only, no spaces
/// - No leading zeros except `"0"` itself (`"-0"` is rejected)
/// - Fits strictly in `i64` (`i64::MIN..=i64::MAX`)
#[must_use]
pub fn parse_strict_i64(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }

    let (is_negative, digits) = if bytes[0] == b'-' {
        (true, &bytes[1..])
    } else {
        (false, bytes)
    };

    if digits.is_empty() {
        return None;
    }

    // No leading zeros except "0" itself, and "-0" is prohibited
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    if is_negative && digits == b"0" {
        return None;
    }

    let mut acc: i64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        let digit = i64::from(b - b'0');
        if is_negative {
            acc = acc.checked_mul(10)?.checked_sub(digit)?;
        } else {
            acc = acc.checked_mul(10)?.checked_add(digit)?;
        }
    }

    Some(acc)
}

/// Executes the `bump` command: `bump <key> [step]`.
///
/// Atomically increments or decrements a 64-bit integer stored at `key`.
/// If the key does not exist, it starts at `0` and adds `step`.
///
/// # Errors
///
/// - [`crate::cmd::ERR_WRONG_ARG_COUNT`] (2002): Handled at dispatch stage.
/// - [`crate::cmd::ERR_INVALID_ARGUMENT`] (2003): Non-string key or non-integer step.
/// - [`crate::cmd::ERR_KEY_TOO_LARGE`] (2004): Key exceeds `max_key_size`.
/// - [`crate::cmd::ERR_EMPTY_KEY`] (2006): Key is empty (0 bytes).
/// - [`crate::cmd::ERR_INTEGER_OVERFLOW`] (3003): Arithmetic exceeds signed 64-bit integer bounds.
/// - [`crate::cmd::ERR_NOT_AN_INTEGER`] (3004): Stored value is non-integer string/bytes or scalar.
/// - [`crate::cmd::ERR_WRONG_TYPE`] (3001): Stored value is Array or Map collection.
/// - [`crate::cmd::ERR_OUT_OF_MEMORY`] (4003): Table reached maximum capacity when inserting missing key.
pub fn handle_bump(ctx: &mut Context<'_>, args: &[Value]) -> Result<Value, CmdError> {
    let key = key_arg_with_limit(&args[0], ctx.config.max_key_size)?;

    let step = if args.len() > 1 {
        match &args[1] {
            Value::Int(s) => *s,
            _ => return Err(ERR_INVALID_ARGUMENT),
        }
    } else {
        1i64
    };

    if let Some(val) = ctx.store.get_mut(key) {
        match val {
            Value::Int(n) => {
                let new_n = n.checked_add(step).ok_or(ERR_INTEGER_OVERFLOW)?;
                *val = Value::Int(new_n);
                Ok(Value::Int(new_n))
            }
            Value::Text(s) => {
                let n = parse_strict_i64(s.as_bytes()).ok_or(ERR_NOT_AN_INTEGER)?;
                let new_n = n.checked_add(step).ok_or(ERR_INTEGER_OVERFLOW)?;
                *val = Value::Int(new_n);
                Ok(Value::Int(new_n))
            }
            Value::Bytes(b) => {
                let n = parse_strict_i64(b).ok_or(ERR_NOT_AN_INTEGER)?;
                let new_n = n.checked_add(step).ok_or(ERR_INTEGER_OVERFLOW)?;
                *val = Value::Int(new_n);
                Ok(Value::Int(new_n))
            }
            Value::Array(_) | Value::Map(_) => Err(ERR_WRONG_TYPE),
            Value::Float(_) | Value::Bool(_) | Value::Nil | Value::Error { .. } => {
                Err(ERR_NOT_AN_INTEGER)
            }
        }
    } else {
        let initial = 0i64.checked_add(step).ok_or(ERR_INTEGER_OVERFLOW)?;
        ctx.store
            .put(key, Value::Int(initial), PutCondition::Always)
            .map_err(|_| ERR_OUT_OF_MEMORY)?;
        Ok(Value::Int(initial))
    }
}
