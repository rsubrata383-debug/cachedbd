//! Handler for the `hello` command.

use crate::cmd::dispatch::Context;
use crate::cmd::error::{CmdError, ERR_INVALID_ARGUMENT, ERR_UNSUPPORTED_PROTOCOL_VERSION};
use crate::proto::Value;
use crate::proto::hello::{HelloResponse, MAX_CLIENT_NAME_LEN};

/// Default maximum allowable payload size: 64 MB (67,108,864 bytes).
const DEFAULT_MAX_FRAME_SIZE: i64 = 64 * 1024 * 1024;

/// Executes the `hello` command: connection handshake, protocol negotiation, and optional authentication.
///
/// # Arguments
///
/// 1. `protocol_version` ([`Value::Int`]): Must be `1` in v0.1.
/// 2. `client_name` ([`Value::Text`]): Identifying name and version of the client library.
/// 3. `auth_token` ([`Value::Text`] | [`Value::Bytes`] | [`Value::Nil`], optional): Authentication credentials.
///
/// # Errors
///
/// - [`ERR_UNSUPPORTED_PROTOCOL_VERSION`] (1001) if protocol version is not `1`.
/// - [`ERR_INVALID_ARGUMENT`] (2003) if types do not match, or `client_name` is empty or too long.
pub fn handle_hello(ctx: &mut Context<'_>, args: &[Value]) -> Result<Value, CmdError> {
    let Some(version_item) = args.first() else {
        return Err(crate::cmd::ERR_INTERNAL_ERROR);
    };
    let Some(name_item) = args.get(1) else {
        return Err(crate::cmd::ERR_INTERNAL_ERROR);
    };

    let Value::Int(version) = version_item else {
        return Err(ERR_INVALID_ARGUMENT);
    };

    if *version != 1 {
        return Err(ERR_UNSUPPORTED_PROTOCOL_VERSION);
    }

    let Value::Text(name) = name_item else {
        return Err(ERR_INVALID_ARGUMENT);
    };

    if name.is_empty() || name.len() > MAX_CLIENT_NAME_LEN {
        return Err(ERR_INVALID_ARGUMENT);
    }

    if let Some(token_item) = args.get(2) {
        match token_item {
            Value::Text(_) | Value::Bytes(_) | Value::Nil => {}
            _ => return Err(ERR_INVALID_ARGUMENT),
        }
    }

    // In v0.1 with require_auth = false, session is authenticated
    ctx.session.authenticated = true;

    let client_id = i64::try_from(ctx.session.client_id).unwrap_or(0);
    let max_key_size = i64::try_from(ctx.config.max_key_size).unwrap_or(1024);
    let max_value_size = i64::try_from(ctx.config.max_value_size).unwrap_or(32 * 1024 * 1024);

    let resp = HelloResponse {
        server_name: "cachedbd".to_string(),
        server_version: "0.1.0".to_string(),
        protocol_version: 1,
        client_id,
        max_frame_size: Some(DEFAULT_MAX_FRAME_SIZE),
        max_key_size: Some(max_key_size),
        max_value_size: Some(max_value_size),
    };

    Ok(resp.to_value())
}
