//! Core in-memory key-value store, expiry, and eviction policies.

pub mod eviction;
pub mod expiry;
pub mod hash;
pub mod kv;
pub mod table;
pub mod value;

pub use hash::{hash64, hash64_process, process_seed};
pub use kv::{PutCondition, Store};
pub use table::{RobinHoodTable, TableFull, TableStats, slot_size};
