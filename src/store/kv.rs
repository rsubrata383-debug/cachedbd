//! In-memory key-value store wrapping [`RobinHoodTable`].

use crate::proto::Value;
use crate::store::table::{RobinHoodTable, TableFull};

/// Condition for conditional insertions into [`Store`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutCondition {
    /// Always insert or overwrite the key.
    Always,
    /// Only insert if the key does not already exist (`nx`).
    OnlyIfAbsent,
    /// Only overwrite if the key already exists (`xx`).
    OnlyIfPresent,
}

/// In-memory key-value store mapping binary keys to [`Value`] items.
///
/// Wraps a single-threaded [`RobinHoodTable<Value>`]. Free of protocol-level limits
/// and specification errors (which are enforced at the command dispatch layer).
#[derive(Debug)]
pub struct Store {
    table: RobinHoodTable<Value>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    /// Creates a new empty store with default table capacity and process seed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: RobinHoodTable::with_default_seed(8),
        }
    }

    /// Creates a new empty store with an initial capacity and explicit random seed.
    #[must_use]
    pub fn with_capacity_and_seed(capacity: usize, seed: u64) -> Self {
        Self {
            table: RobinHoodTable::new(capacity, seed),
        }
    }

    /// Creates a store with a pre-configured [`RobinHoodTable`].
    #[must_use]
    pub const fn with_table(table: RobinHoodTable<Value>) -> Self {
        Self { table }
    }

    /// Returns a reference to the underlying [`RobinHoodTable`].
    #[must_use]
    pub const fn table(&self) -> &RobinHoodTable<Value> {
        &self.table
    }

    /// Returns a mutable reference to the underlying [`RobinHoodTable`].
    #[must_use]
    pub fn table_mut(&mut self) -> &mut RobinHoodTable<Value> {
        &mut self.table
    }

    /// Returns a borrowed reference to the value associated with `key`, if present.
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.table.get(key)
    }

    /// Returns a mutable reference to the value associated with `key`, if present.
    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut Value> {
        self.table.get_mut(key)
    }

    /// Retrieves an owned clone of the value stored at `key`, or [`Value::Nil`] if absent.
    ///
    /// # Note
    ///
    /// Cloning duplicates heap memory for large values. This may be optimized in future
    /// phases using reference-counted shared buffers or zero-copy slicing.
    #[must_use]
    pub fn fetch(&self, key: &[u8]) -> Value {
        self.get(key).cloned().unwrap_or(Value::Nil)
    }

    /// Inserts or updates `value` at `key` subject to `condition`.
    ///
    /// Returns `Ok(true)` if the value was stored, or `Ok(false)` if `condition`
    /// was not satisfied.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] if the underlying table reached maximum capacity ($2^{30}$).
    pub fn put(
        &mut self,
        key: &[u8],
        value: Value,
        condition: PutCondition,
    ) -> Result<bool, TableFull> {
        match condition {
            PutCondition::Always => {
                if let Some(existing) = self.table.get_mut(key) {
                    *existing = value;
                } else {
                    let key_box = Box::from(key);
                    self.table.insert(key_box, value)?;
                }
                Ok(true)
            }
            PutCondition::OnlyIfAbsent => {
                if self.table.contains_key(key) {
                    Ok(false)
                } else {
                    let key_box = Box::from(key);
                    self.table.insert(key_box, value)?;
                    Ok(true)
                }
            }
            PutCondition::OnlyIfPresent => {
                if let Some(existing) = self.table.get_mut(key) {
                    *existing = value;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Removes `key` from the store, returning `true` if it existed.
    pub fn remove(&mut self, key: &[u8]) -> bool {
        self.table.remove(key).is_some()
    }

    /// Returns `true` if the store contains an entry for `key`.
    #[must_use]
    pub fn contains(&self, key: &[u8]) -> bool {
        self.table.contains_key(key)
    }

    /// Returns the number of entries currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Returns `true` if the store contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Passes through an incremental migration step of `count` buckets.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] if migrating items cannot fit into the new table.
    pub fn step_migration(&mut self, count: usize) -> Result<bool, TableFull> {
        self.table.step_migration(count)
    }

    /// Validates table invariants.
    ///
    /// # Errors
    ///
    /// Returns a description string if any hash table invariant is violated.
    pub fn check_invariants(&self) -> Result<(), String> {
        self.table.check_invariants()
    }
}
