//! Handler for the `fetch` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::CmdError;
use crate::cmd::parse::key_arg_with_limit;
use crate::proto::Value;

/// Executes the `fetch` command: `fetch <key>`.
///
/// Returns the stored value if present, or [`Value::Nil`] if absent.
///
/// # Errors
///
/// Returns key validation errors ([`crate::cmd::ERR_EMPTY_KEY`] 2006, [`crate::cmd::ERR_KEY_TOO_LARGE`] 2004, [`crate::cmd::ERR_INVALID_ARGUMENT`] 2003).
pub fn handle_fetch(ctx: &mut Context<'_>, args: &[Value]) -> Result<Value, CmdError> {
    let Some(key_item) = args.first() else {
        return Err(crate::cmd::ERR_INTERNAL_ERROR);
    };
    let key = key_arg_with_limit(key_item, ctx.config.max_key_size)?;
    Ok(ctx.store.fetch(key))
}
