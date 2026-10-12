//! Handler for the `size` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::{CmdError, ERR_INTERNAL_ERROR};
use crate::proto::Value;

/// Executes the `size` command: `size`.
///
/// Returns `Int(count)` representing the number of keys currently stored.
///
/// # Errors
///
/// Returns [`ERR_INTERNAL_ERROR`] (7001) if table length exceeds `i64::MAX`.
pub fn handle_size(ctx: &mut Context<'_>, _args: &[Value]) -> Result<Value, CmdError> {
    let len = ctx.store.len();
    let count = i64::try_from(len).map_err(|_| ERR_INTERNAL_ERROR)?;
    Ok(Value::Int(count))
}
