//! Handler for the `drop` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::CmdError;
use crate::cmd::parse::key_arg_with_limit;
use crate::proto::Value;
use std::collections::HashSet;

/// Executes the `drop` command: `drop <key1> [key2 ...]`.
///
/// Validates ALL keys before performing any removals. Returns a [`Value::Int`] count
/// of unique keys that were removed. Duplicate keys in the argument list count once.
///
/// # Errors
///
/// Returns key validation errors ([`crate::cmd::ERR_EMPTY_KEY`] 2006, [`crate::cmd::ERR_KEY_TOO_LARGE`] 2004, [`crate::cmd::ERR_INVALID_ARGUMENT`] 2003)
/// if ANY key is invalid. Zero keys are removed if an error is returned.
pub fn handle_drop(ctx: &mut Context<'_>, args: &[Value]) -> Result<Value, CmdError> {
    // Phase 1: Validate ALL keys first
    let mut validated_keys = Vec::with_capacity(args.len());
    for arg in args {
        let key = key_arg_with_limit(arg, ctx.config.max_key_size)?;
        validated_keys.push(key);
    }

    // Phase 2: Remove keys, deduplicating keys so duplicate keys count once
    let mut removed_count: i64 = 0;
    let mut seen = HashSet::with_capacity(validated_keys.len());

    for key in validated_keys {
        if seen.insert(key) && ctx.store.remove(key) {
            removed_count = removed_count.saturating_add(1);
        }
    }

    Ok(Value::Int(removed_count))
}
