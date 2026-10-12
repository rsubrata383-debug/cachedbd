//! Command routing, dispatcher table, and execution engine.

use crate::cmd::error::{
    CmdError, ERR_AUTH_REQUIRED, ERR_READONLY_REPLICA, ERR_UNKNOWN_COMMAND, ERR_WRONG_ARG_COUNT,
};
use crate::cmd::handlers::{
    handle_drop, handle_fetch, handle_has, handle_ping, handle_put, handle_size,
};
use crate::cmd::parse::{DEFAULT_MAX_ARGS, DEFAULT_MAX_KEY_SIZE, parse_command};
use crate::proto::Value;
use crate::store::Store;

/// Default maximum value size per Section 4 of `docs/spec/values.md` (32 MiB).
pub const DEFAULT_MAX_VALUE_SIZE: usize = 32 * 1024 * 1024;

/// Dispatch flags indicating command behavior and permission requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandFlags(pub u8);

impl CommandFlags {
    /// Command only reads keyspace data.
    pub const READ: Self = Self(1 << 0);
    /// Command mutates keyspace data.
    pub const WRITE: Self = Self(1 << 1);
    /// Command is permitted before authentication (`ping`, `hello`, `quit`).
    pub const NO_AUTH: Self = Self(1 << 2);
    /// Command requires elevated administrative privilege.
    pub const ADMIN: Self = Self(1 << 3);

    /// Combines two flag sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns `true` if `self` contains all flags in `other`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Handler function signature for command execution.
pub type HandlerFn = fn(&mut Context<'_>, &[Value]) -> Result<Value, CmdError>;

/// An entry in the command dispatcher registry table.
pub struct CommandEntry {
    /// Exact lowercase command name in bytes (e.g. `b"put"`).
    pub name: &'static [u8],
    /// Minimum required argument count.
    pub min_args: usize,
    /// Maximum allowed argument count, or `None` if unlimited.
    pub max_args: Option<usize>,
    /// Behavioral and permission flags.
    pub flags: CommandFlags,
    /// Function executing the command.
    pub handler: HandlerFn,
}

/// Static registry of all registered database commands.
pub static REGISTRY: &[CommandEntry] = &[
    CommandEntry {
        name: b"ping",
        min_args: 0,
        max_args: Some(1),
        flags: CommandFlags::NO_AUTH.union(CommandFlags::READ),
        handler: handle_ping,
    },
    CommandEntry {
        name: b"put",
        min_args: 2,
        max_args: Some(3),
        flags: CommandFlags::WRITE,
        handler: handle_put,
    },
    CommandEntry {
        name: b"fetch",
        min_args: 1,
        max_args: Some(1),
        flags: CommandFlags::READ,
        handler: handle_fetch,
    },
    CommandEntry {
        name: b"drop",
        min_args: 1,
        max_args: None,
        flags: CommandFlags::WRITE,
        handler: handle_drop,
    },
    CommandEntry {
        name: b"has",
        min_args: 1,
        max_args: Some(1),
        flags: CommandFlags::READ,
        handler: handle_has,
    },
    CommandEntry {
        name: b"size",
        min_args: 0,
        max_args: Some(0),
        flags: CommandFlags::READ,
        handler: handle_size,
    },
];

/// Looks up a command entry in [`REGISTRY`] by exact byte comparison without case folding.
#[must_use]
pub fn lookup_command(verb: &[u8]) -> Option<&'static CommandEntry> {
    REGISTRY.iter().find(|entry| entry.name == verb)
}

/// Configuration settings governing command limits and security.
#[derive(Debug, Clone)]
pub struct CmdConfig {
    /// Maximum allowed key size in bytes (default 1024).
    pub max_key_size: usize,
    /// Maximum allowed value payload size in bytes (default 32 MiB).
    pub max_value_size: usize,
    /// Maximum allowed argument count (default 65,536).
    pub max_args: usize,
    /// Whether non-`NO_AUTH` commands require authenticated session (default false).
    pub require_auth: bool,
    /// Whether write commands must be rejected with 6001 (default false).
    pub read_only: bool,
}

impl Default for CmdConfig {
    fn default() -> Self {
        Self {
            max_key_size: DEFAULT_MAX_KEY_SIZE,
            max_value_size: DEFAULT_MAX_VALUE_SIZE,
            max_args: DEFAULT_MAX_ARGS,
            require_auth: false,
            read_only: false,
        }
    }
}

/// Per-connection client session state.
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique client connection identifier.
    pub client_id: u64,
    /// Whether this session has completed authentication.
    pub authenticated: bool,
}

impl Session {
    /// Creates a new unauthenticated session.
    #[must_use]
    pub const fn new(client_id: u64) -> Self {
        Self {
            client_id,
            authenticated: false,
        }
    }
}

/// Execution context passed into command handlers.
pub struct Context<'a> {
    /// Mutable handle to the underlying keyspace store.
    pub store: &'a mut Store,
    /// Read-only handle to engine configuration.
    pub config: &'a CmdConfig,
    /// Mutable handle to client session state.
    pub session: &'a mut Session,
}

/// Outcome of executing a command request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Reply value to send to the client.
    pub reply: Value,
    /// Whether the server must close the connection after sending reply.
    pub close: bool,
}

/// Core in-memory database command execution engine.
#[derive(Debug, Default)]
pub struct Engine {
    /// Active keyspace store.
    pub store: Store,
    /// Active command limits and security configuration.
    pub config: CmdConfig,
}

impl Engine {
    /// Creates a new engine with default configuration and an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Store::new(),
            config: CmdConfig::default(),
        }
    }

    /// Creates an engine with custom store and configuration.
    #[must_use]
    pub const fn with_config(store: Store, config: CmdConfig) -> Self {
        Self { store, config }
    }

    /// Executes a decoded command request frame against this engine.
    ///
    /// # Order of Checks
    ///
    /// 1. **Parse**: The payload must be a non-empty `Array` with `Text` or `Bytes` verb
    ///    and at most `max_args` arguments.
    /// 2. **Unknown Command (2001)**: The verb must match a registered command exactly.
    /// 3. **Authentication (5001)**: If `require_auth` is enabled and the session is
    ///    unauthenticated, commands lacking [`CommandFlags::NO_AUTH`] are rejected.
    /// 4. **Arity (2002)**: The argument count must satisfy `min_args <= count <= max_args`.
    /// 5. **Read-Only (6001)**: If `read_only` is enabled, commands with [`CommandFlags::WRITE`]
    ///    are rejected.
    /// 6. **Handler**: The command handler function is called.
    pub fn execute(&mut self, session: &mut Session, request: &Value) -> Outcome {
        // Step 1: Parse
        let (verb, args) = match parse_command(request, self.config.max_args) {
            Ok(parsed) => parsed,
            Err(err) => {
                return Outcome {
                    reply: err.to_value(),
                    close: err.close,
                };
            }
        };

        // Step 2: Unknown command (exact bytes, no case folding)
        let Some(entry) = lookup_command(verb) else {
            return Outcome {
                reply: ERR_UNKNOWN_COMMAND.to_value(),
                close: ERR_UNKNOWN_COMMAND.close,
            };
        };

        // Step 3: Authentication
        if self.config.require_auth
            && !session.authenticated
            && !entry.flags.contains(CommandFlags::NO_AUTH)
        {
            return Outcome {
                reply: ERR_AUTH_REQUIRED.to_value(),
                close: ERR_AUTH_REQUIRED.close,
            };
        }

        // Step 4: Arity
        let arg_count = args.len();
        if arg_count < entry.min_args || entry.max_args.is_some_and(|max| arg_count > max) {
            return Outcome {
                reply: ERR_WRONG_ARG_COUNT.to_value(),
                close: ERR_WRONG_ARG_COUNT.close,
            };
        }

        // Step 5: Read-Only
        if self.config.read_only && entry.flags.contains(CommandFlags::WRITE) {
            return Outcome {
                reply: ERR_READONLY_REPLICA.to_value(),
                close: ERR_READONLY_REPLICA.close,
            };
        }

        // Step 6: Handler
        let mut ctx = Context {
            store: &mut self.store,
            config: &self.config,
            session,
        };

        match (entry.handler)(&mut ctx, args) {
            Ok(reply) => Outcome {
                reply,
                close: false,
            },
            Err(err) => Outcome {
                reply: err.to_value(),
                close: err.close,
            },
        }
    }
}
