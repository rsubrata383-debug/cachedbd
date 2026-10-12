//! Command parsing, validation, and dispatching.

pub mod dispatch;
pub mod error;
pub mod handlers;
pub mod parse;

pub use dispatch::{
    CmdConfig, CommandEntry, CommandFlags, Context, Engine, HandlerFn, Outcome, REGISTRY, Session,
    lookup_command,
};
pub use error::{
    CmdError, ERR_AUTH_REQUIRED, ERR_EMPTY_KEY, ERR_INTEGER_OVERFLOW, ERR_INTERNAL_ERROR,
    ERR_INVALID_ARGUMENT, ERR_KEY_TOO_LARGE, ERR_NOT_AN_INTEGER, ERR_OUT_OF_MEMORY,
    ERR_READONLY_REPLICA, ERR_SYNTAX_ERROR, ERR_TOO_MANY_ARGS, ERR_UNKNOWN_COMMAND,
    ERR_VALUE_TOO_LARGE, ERR_WRONG_ARG_COUNT, ERR_WRONG_TYPE,
};
pub use parse::{
    DEFAULT_MAX_ARGS, DEFAULT_MAX_KEY_SIZE, key_arg, key_arg_with_limit, parse_command,
};
