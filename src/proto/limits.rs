//! Operational and security limits for the protocol framing layer.

use crate::proto::header::DEFAULT_MAX_PAYLOAD_LEN;
use crate::proto::item::MAX_NESTING_DEPTH;

/// Default maximum argument count for a COMMAND request (65,536 items per spec).
pub const DEFAULT_MAX_ARGS: usize = 65_536;

/// Hard operational limits enforced at the protocol framing layer.
///
/// Note: Key size limits (1 KB) and value size limits (32 MB) belong to the command
/// execution layer (Week 7) and are intentionally not configured here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum allowable frame size (header + payload) in bytes, default 64 MB (67,108,864 bytes).
    pub max_frame_size: usize,
    /// Maximum number of arguments in a COMMAND request array, default 65,536.
    pub max_args: usize,
    /// Maximum collection nesting depth, default 8 levels.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_size: DEFAULT_MAX_PAYLOAD_LEN as usize,
            max_args: DEFAULT_MAX_ARGS,
            max_depth: MAX_NESTING_DEPTH,
        }
    }
}

