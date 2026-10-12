//! Handler for the `quit` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::CmdError;
use crate::proto::Value;

/// Executes the `quit` command: gracefully requests connection termination.
///
/// # Errors
///
/// Does not fail; arity checking is handled by the dispatcher.
pub fn handle_quit(ctx: &mut Context<'_>, _args: &[Value]) -> Result<Value, CmdError> {
    ctx.close = true;
    Ok(Value::Text("ok".to_string()))
}
