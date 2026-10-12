//! Handler for the `ping` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::{CmdError, ERR_INVALID_ARGUMENT};
use crate::proto::Value;

/// Executes the `ping` command.
///
/// # Semantics
///
/// - 0 arguments: returns `Text("pong")`.
/// - 1 argument: returns the argument unmodified if it is `Text` or `Bytes`.
///
/// # Errors
///
/// Returns [`ERR_INVALID_ARGUMENT`] (2003) if the argument is not `Text` or `Bytes`.
pub fn handle_ping(_ctx: &mut Context<'_>, args: &[Value]) -> Result<Value, CmdError> {
    if args.is_empty() {
        return Ok(Value::Text("pong".to_string()));
    }
    match &args[0] {
        Value::Text(_) | Value::Bytes(_) => Ok(args[0].clone()),
        _ => Err(ERR_INVALID_ARGUMENT),
    }
}
