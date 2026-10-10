//! Persistence layer: journaling, snapshotting, and crash recovery.

pub mod journal;
pub mod recovery;
pub mod snapshot;
