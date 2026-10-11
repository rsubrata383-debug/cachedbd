//! In-memory storage engine: Robin Hood hash table and non-cryptographic hashing.

pub mod hash;
pub mod table;

pub use hash::{hash64, hash64_process, process_seed};
pub use table::{RobinHoodTable, TableFull, TableStats};

