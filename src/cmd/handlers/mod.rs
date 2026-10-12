//! Handlers executing specific database commands.

pub mod bump;
pub mod drop_cmd;
pub mod fetch;
pub mod has;
pub mod hello;
pub mod ping;
pub mod put;
pub mod quit;
pub mod size;

pub use bump::{handle_bump, parse_strict_i64};
pub use drop_cmd::handle_drop;
pub use fetch::handle_fetch;
pub use has::handle_has;
pub use hello::{build_hello_response, handle_hello};
pub use ping::handle_ping;
pub use put::handle_put;
pub use quit::handle_quit;
pub use size::handle_size;

use crate::proto::Value;

/// Computes the payload size of a [`Value`] item tree in bytes per Section 4 of `docs/spec/values.md`.
///
/// Definition:
/// - [`Value::Nil`]: 0 bytes
/// - [`Value::Int`] & [`Value::Float`]: 8 bytes
/// - [`Value::Bool`]: 1 byte
/// - [`Value::Text`]: byte length of string
/// - [`Value::Bytes`]: byte length of buffer
/// - [`Value::Array`]: sum of payload bytes of all child items
/// - [`Value::Map`]: sum of payload bytes of all key-value pair items
/// - [`Value::Error`]: byte length of error message string
#[must_use]
pub fn compute_value_size(val: &Value) -> usize {
    match val {
        Value::Nil => 0,
        Value::Int(_) | Value::Float(_) => 8,
        Value::Bool(_) => 1,
        Value::Text(s) => s.len(),
        Value::Bytes(b) => b.len(),
        Value::Array(items) => items.iter().map(compute_value_size).sum(),
        Value::Map(pairs) => pairs
            .iter()
            .map(|(k, v)| compute_value_size(k).saturating_add(compute_value_size(v)))
            .sum(),
        Value::Error { message, .. } => message.len(),
    }
}
