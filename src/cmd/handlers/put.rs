//! Handler for the `put` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::{CmdError, ERR_INVALID_ARGUMENT, ERR_OUT_OF_MEMORY, ERR_VALUE_TOO_LARGE};
use crate::cmd::handlers::compute_value_size;
use crate::cmd::parse::key_arg_with_limit;
use crate::proto::Value;
use crate::store::PutCondition;

/// Executes the `put` command: `put <key> <value> [nx|xx]`.
///
/// # Errors
///
/// - Key errors ([`crate::cmd::ERR_EMPTY_KEY`] 2006, [`crate::cmd::ERR_KEY_TOO_LARGE`] 2004, [`ERR_INVALID_ARGUMENT`] 2003).
/// - [`ERR_INVALID_ARGUMENT`] (2003) if the value is a [`Value::Error`] item.
/// - [`ERR_VALUE_TOO_LARGE`] (2005) if the value size exceeds `config.max_value_size`.
/// - [`ERR_INVALID_ARGUMENT`] (2003) if condition is supplied but is not lowercase `Text("nx")` or `Text("xx")`.
/// - [`ERR_OUT_OF_MEMORY`] (4003) if the underlying store reaches maximum capacity.
pub fn handle_put(ctx: &mut Context<'_>, args: &[Value]) -> Result<Value, CmdError> {
    let Some(key_item) = args.first() else {
        return Err(crate::cmd::ERR_INTERNAL_ERROR);
    };
    let Some(val) = args.get(1) else {
        return Err(crate::cmd::ERR_INTERNAL_ERROR);
    };
    let key = key_arg_with_limit(key_item, ctx.config.max_key_size)?;

    if matches!(val, Value::Error { .. }) {
        return Err(ERR_INVALID_ARGUMENT);
    }

    let val_size = compute_value_size(val);
    if val_size > ctx.config.max_value_size {
        return Err(ERR_VALUE_TOO_LARGE);
    }

    let condition = if args.len() == 3 {
        match args.get(2) {
            Some(Value::Text(s)) if s == "nx" => PutCondition::OnlyIfAbsent,
            Some(Value::Text(s)) if s == "xx" => PutCondition::OnlyIfPresent,
            _ => return Err(ERR_INVALID_ARGUMENT),
        }
    } else {
        PutCondition::Always
    };

    match ctx.store.put(key, val.clone(), condition) {
        Ok(stored) => Ok(Value::Bool(stored)),
        Err(_) => Err(ERR_OUT_OF_MEMORY),
    }
}
