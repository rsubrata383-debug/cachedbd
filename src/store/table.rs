//! Core Robin Hood hash table implementation with incremental resizing.
//!
//! # Design and Invariants
//!
//! This table uses open addressing with Robin Hood hashing and incremental dual-table
//! background resizing:
//! - Contiguous storage of slots with power-of-two capacity.
//! - Probe index computed with `hash & mask`.
//! - Slots cache the 64-bit hash, the Probe Sequence Length (PSL), the key, and the value.
//! - Backward-shift deletion without tombstones.
//! - **Incremental Resizing**: When `main_table` exceeds the $7/8$ load factor, a new
//!   `main_table` with $2\times$ capacity is allocated, the old table becomes `old_table`,
//!   and a migration cursor starts at bucket 0.
//! - **Paced Migration**: On every `insert`, `remove`, and `get_mut` (as well as explicit calls
//!   to [`RobinHoodTable::step_migration`]), a batch of [`MIGRATE_BATCH`] buckets (16 buckets)
//!   is moved from `old_table` to `main_table`. `get` takes `&self` and is strictly read-only
//!   (does not migrate). Moving a bucket moves the key, value, and cached hash directly without
//!   re-hashing the key bytes.
//! - **Single Resize Invariant**: If a resize is already running when a new resize is triggered,
//!   the running resize is first completed synchronously before initiating the next resize.
//! - **Bounded Latency**: Lookups check `main_table` first, then `old_table`. Inserts write
//!   to `main_table` and remove any duplicate from `old_table`. Worst-case latency is strictly
//!   bounded to at most [`MIGRATE_BATCH`] bucket moves plus one normal operation ($O(1)$).

use std::error::Error;
use std::fmt;

use crate::store::hash::{hash64, process_seed};

/// Minimum allowable table capacity (always a power of two).
pub const MIN_CAPACITY: usize = 8;

/// Default maximum allowable table capacity ($2^{30} = 1,073,741,824$ slots).
pub const DEFAULT_MAX_CAPACITY: usize = 1 << 30;

/// Maximum allowable table capacity alias for backward compatibility.
pub const MAX_CAPACITY: usize = DEFAULT_MAX_CAPACITY;

/// Numerator for maximum load factor limit ($7 / 8 = 87.5\%$).
pub const MAX_LOAD_NUMERATOR: usize = 7;

/// Denominator for maximum load factor limit ($7 / 8 = 87.5\%$).
pub const MAX_LOAD_DENOMINATOR: usize = 8;

/// Number of buckets moved from `old_table` to `main_table` on each operation.
pub const MIGRATE_BATCH: usize = 16;

/// Error returned when an insertion would exceed the table's maximum allowable capacity limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableFull;

impl fmt::Display for TableFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "table capacity limit reached (max capacity exceeded)")
    }
}

impl Error for TableFull {}

/// Summary operational statistics for a [`RobinHoodTable`].
#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    /// Total number of active key-value pairs stored across both tables.
    pub len: usize,
    /// Total number of available slots in the main table.
    pub capacity: usize,
    /// Current load factor of the main table ($0.0 \le \text{load} \le 1.0$).
    pub load: f64,
    /// Maximum Probe Sequence Length among all occupied slots in the main table.
    pub max_psl: usize,
    /// Average Probe Sequence Length across all occupied slots in the main table.
    pub average_psl: f64,
    /// Whether an incremental resize is currently active.
    pub resizing: bool,
    /// Number of remaining buckets left to migrate from the old table (0 if not resizing).
    pub old_remaining_buckets: usize,
}

/// An entry residing in a Robin Hood hash table slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot<V> {
    /// Cached 64-bit hash of the key.
    pub(crate) hash: u64,
    /// Probe sequence length (distance from the entry's ideal bucket).
    pub(crate) psl: u32,
    /// Owned binary key.
    pub(crate) key: Box<[u8]>,
    /// Associated value.
    pub(crate) value: V,
}

/// Returns the in-memory size in bytes of an individual table slot (`Option<Slot<V>>`).
#[must_use]
pub const fn slot_size<V>() -> usize {
    std::mem::size_of::<Option<Slot<V>>>()
}

/// Internal single table buffer holding contiguous slots.
#[derive(Debug, Clone)]
pub(crate) struct SingleTable<V> {
    pub(crate) slots: Box<[Option<Slot<V>>]>,
    pub(crate) capacity: usize,
    pub(crate) len: usize,
    pub(crate) mask: usize,
}

impl<V> SingleTable<V> {
    fn new(capacity: usize) -> Self {
        let mask = capacity.saturating_sub(1);
        let mut slots_vec = Vec::with_capacity(capacity);
        slots_vec.resize_with(capacity, || None);

        Self {
            slots: slots_vec.into_boxed_slice(),
            capacity,
            len: 0,
            mask,
        }
    }

    #[inline]
    const fn max_load_limit(&self) -> usize {
        (self.capacity / MAX_LOAD_DENOMINATOR) * MAX_LOAD_NUMERATOR
    }

    #[inline]
    const fn is_at_max_load(&self) -> bool {
        self.len >= self.max_load_limit()
    }

    #[inline]
    fn ideal_index(&self, hash: u64) -> usize {
        usize::try_from(hash & (self.mask as u64)).unwrap_or(0)
    }

    fn get_with_hash(&self, hash: u64, key: &[u8]) -> Option<&V> {
        let ideal = self.ideal_index(hash);
        let mut psl = 0u32;
        let mut idx = ideal;

        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);
        while psl < max_probes {
            let slot = self.slots.get(idx).and_then(Option::as_ref)?;
            if slot.psl < psl {
                return None;
            }
            if slot.hash == hash && slot.key.as_ref() == key {
                return Some(&slot.value);
            }
            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        None
    }

    fn lookup_probe_count(&self, hash: u64, key: &[u8]) -> usize {
        let ideal = self.ideal_index(hash);
        let mut psl = 0u32;
        let mut idx = ideal;
        let mut probes = 0usize;

        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);
        while psl < max_probes {
            probes = probes.saturating_add(1);
            let Some(slot) = self.slots.get(idx).and_then(Option::as_ref) else {
                break;
            };
            if slot.psl < psl || (slot.hash == hash && slot.key.as_ref() == key) {
                break;
            }
            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        probes
    }

    fn get_mut_with_hash(&mut self, hash: u64, key: &[u8]) -> Option<&mut V> {
        let ideal = self.ideal_index(hash);
        let mut psl = 0u32;
        let mut idx = ideal;

        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);
        let mut target = None;
        while psl < max_probes {
            let slot = self.slots.get(idx).and_then(Option::as_ref)?;
            if slot.psl < psl {
                return None;
            }
            if slot.hash == hash && slot.key.as_ref() == key {
                target = Some(idx);
                break;
            }
            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        let target_idx = target?;
        self.slots
            .get_mut(target_idx)
            .and_then(Option::as_mut)
            .map(|s| &mut s.value)
    }

    fn contains_key_with_hash(&self, hash: u64, key: &[u8]) -> bool {
        self.get_with_hash(hash, key).is_some()
    }

    fn insert_slot(&mut self, slot: Slot<V>) -> Result<(), Slot<V>> {
        let hash = slot.hash;
        let ideal = self.ideal_index(hash);
        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);

        // Check if key already exists (in-place overwrite)
        let mut psl = 0u32;
        let mut idx = ideal;
        while psl < max_probes {
            let Some(slot_ref) = self.slots.get_mut(idx) else {
                break;
            };

            match slot_ref.as_mut() {
                None => break,
                Some(existing) => {
                    if existing.psl < psl {
                        break;
                    }
                    if existing.hash == hash && existing.key.as_ref() == slot.key.as_ref() {
                        existing.value = slot.value;
                        return Ok(());
                    }
                }
            }

            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        if self.len >= self.max_load_limit() {
            return Err(slot);
        }

        let mut curr_slot = slot;
        curr_slot.psl = 0;
        let mut curr_idx = ideal;

        while curr_slot.psl < max_probes {
            let Some(slot_ref) = self.slots.get_mut(curr_idx) else {
                break;
            };

            if slot_ref.is_none() {
                *slot_ref = Some(curr_slot);
                self.len = self.len.saturating_add(1);
                return Ok(());
            }

            if let Some(existing) = slot_ref.as_mut() {
                if existing.psl < curr_slot.psl {
                    std::mem::swap(existing, &mut curr_slot);
                }
            }

            curr_slot.psl = curr_slot.psl.saturating_add(1);
            curr_idx = (curr_idx.wrapping_add(1)) & self.mask;
        }

        Err(curr_slot)
    }

    fn insert_with_hash(
        &mut self,
        hash: u64,
        key: Box<[u8]>,
        value: V,
    ) -> Result<Option<V>, TableFull> {
        let ideal = self.ideal_index(hash);
        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);

        // Step 1: Probe to check if the key already exists (in-place overwrite).
        let mut psl = 0u32;
        let mut idx = ideal;
        while psl < max_probes {
            let Some(slot_ref) = self.slots.get_mut(idx) else {
                break;
            };

            match slot_ref.as_mut() {
                None => break,
                Some(existing) => {
                    if existing.psl < psl {
                        break;
                    }
                    if existing.hash == hash && existing.key.as_ref() == key.as_ref() {
                        let old = std::mem::replace(&mut existing.value, value);
                        return Ok(Some(old));
                    }
                }
            }

            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        // Step 2: Check capacity limit before new insertion.
        if self.len >= self.max_load_limit() {
            return Err(TableFull);
        }

        // Step 3: Robin Hood insertion with displacement.
        let slot = Slot {
            hash,
            psl: 0,
            key,
            value,
        };
        self.insert_slot(slot).map(|()| None).map_err(|_| TableFull)
    }

    fn remove_at_and_shift(&mut self, target_idx: usize) -> Option<Slot<V>> {
        let removed = self.slots.get_mut(target_idx).and_then(Option::take)?;
        self.len = self.len.saturating_sub(1);

        let mut curr = target_idx;
        loop {
            let next = (curr.wrapping_add(1)) & self.mask;
            let should_move = match self.slots.get(next).and_then(Option::as_ref) {
                None => false,
                Some(s) if s.psl == 0 => false,
                Some(_) => true,
            };

            if !should_move {
                break;
            }

            let next_slot = self.slots.get_mut(next).and_then(Option::take);
            if let Some(mut s) = next_slot {
                s.psl = s.psl.saturating_sub(1);
                if let Some(curr_slot) = self.slots.get_mut(curr) {
                    *curr_slot = Some(s);
                }
            }
            curr = next;
        }

        Some(removed)
    }

    fn find_index_with_hash(&self, hash: u64, key: &[u8]) -> Option<usize> {
        let ideal = self.ideal_index(hash);
        let mut psl = 0u32;
        let mut idx = ideal;

        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);
        while psl < max_probes {
            let slot = self.slots.get(idx).and_then(Option::as_ref)?;
            if slot.psl < psl {
                return None;
            }
            if slot.hash == hash && slot.key.as_ref() == key {
                return Some(idx);
            }
            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        None
    }

    fn remove_with_hash(&mut self, hash: u64, key: &[u8]) -> Option<V> {
        let target_idx = self.find_index_with_hash(hash, key)?;
        self.remove_at_and_shift(target_idx).map(|s| s.value)
    }

    fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
        self.len = 0;
    }

    fn iter(&self) -> impl Iterator<Item = (&[u8], &V)> {
        self.slots
            .iter()
            .filter_map(|slot_opt| slot_opt.as_ref().map(|s| (s.key.as_ref(), &s.value)))
    }

    fn stats_internal(&self) -> (usize, f64) {
        if self.len == 0 {
            return (0, 0.0);
        }

        let mut max_psl = 0usize;
        let mut total_psl = 0usize;

        for slot_opt in &self.slots {
            if let Some(s) = slot_opt.as_ref() {
                let psl = s.psl as usize;
                if psl > max_psl {
                    max_psl = psl;
                }
                total_psl = total_psl.saturating_add(psl);
            }
        }

        (max_psl, (total_psl as f64) / (self.len as f64))
    }

    fn check_invariants(&self) -> Result<(), String> {
        let mut count = 0usize;

        for i in 0..self.capacity {
            if let Some(slot) = self.slots.get(i).and_then(Option::as_ref) {
                count = count.saturating_add(1);

                let ideal = self.ideal_index(slot.hash);
                let expected_psl = (i.wrapping_add(self.capacity).wrapping_sub(ideal)) & self.mask;
                if slot.psl as usize != expected_psl {
                    return Err(format!(
                        "Invariant 1 violated at slot {i}: psl is {}, expected {expected_psl} (ideal={ideal})",
                        slot.psl
                    ));
                }

                // Invariant: empty slot ends the probe chain.
                // If a slot has psl > 0, the previous slot (with circular wraparound) must be occupied.
                if slot.psl > 0 {
                    let prev_idx = (i.wrapping_add(self.capacity).wrapping_sub(1)) & self.mask;
                    if self.slots.get(prev_idx).and_then(Option::as_ref).is_none() {
                        return Err(format!(
                            "Invariant 3 violated at slot {i}: slot has psl={} > 0 but previous slot {prev_idx} is empty",
                            slot.psl
                        ));
                    }
                }

                let next_idx = (i.wrapping_add(1)) & self.mask;
                if let Some(next_slot) = self.slots.get(next_idx).and_then(Option::as_ref) {
                    if next_slot.psl > slot.psl.saturating_add(1) {
                        return Err(format!(
                            "Invariant 2 violated between slots {i} and {next_idx}: psl[{i}] = {}, psl[{next_idx}] = {}",
                            slot.psl, next_slot.psl
                        ));
                    }
                }

                if self.find_index_with_hash(slot.hash, slot.key.as_ref()) != Some(i) {
                    return Err(format!("Invariant 5 violated: duplicate key at slot {i}"));
                }
            }
        }

        if self.len != count {
            return Err(format!(
                "Invariant 4 violated: len is {}, but counted {count} occupied slots",
                self.len
            ));
        }

        Ok(())
    }
}

/// In-memory open-addressing hash table using Robin Hood probing and incremental resizing.
#[derive(Debug, Clone)]
pub struct RobinHoodTable<V> {
    pub(crate) main_table: SingleTable<V>,
    pub(crate) old_table: Option<SingleTable<V>>,
    pub(crate) migrate_cursor: usize,
    seed: u64,
    max_capacity: usize,
}

impl<V> RobinHoodTable<V> {
    /// Creates a new `RobinHoodTable` with the specified capacity hint and hash seed.
    ///
    /// The capacity is clamped to a minimum of [`MIN_CAPACITY`] (8) and a maximum of
    /// [`DEFAULT_MAX_CAPACITY`] ($2^{30}$), rounded up to the nearest power of two.
    #[must_use]
    pub fn new(capacity_hint: usize, seed: u64) -> Self {
        Self::with_max_capacity(capacity_hint, seed, DEFAULT_MAX_CAPACITY)
    }

    /// Creates a new table using the global per-process random seed.
    #[must_use]
    pub fn with_default_seed(capacity_hint: usize) -> Self {
        Self::new(capacity_hint, process_seed())
    }

    /// Creates a new table with a custom maximum capacity limit (useful for testing).
    #[must_use]
    pub fn with_max_capacity(capacity_hint: usize, seed: u64, max_capacity: usize) -> Self {
        let max_cap = max_capacity.clamp(MIN_CAPACITY, DEFAULT_MAX_CAPACITY);
        let capacity = if capacity_hint < MIN_CAPACITY {
            MIN_CAPACITY
        } else if capacity_hint >= max_cap {
            max_cap
        } else {
            capacity_hint
                .next_power_of_two()
                .clamp(MIN_CAPACITY, max_cap)
        };

        Self {
            main_table: SingleTable::new(capacity),
            old_table: None,
            migrate_cursor: 0,
            seed,
            max_capacity: max_cap,
        }
    }

    /// Returns the total number of key-value pairs stored across both tables.
    #[must_use]
    pub fn len(&self) -> usize {
        let old_len = self.old_table.as_ref().map_or(0, |t| t.len);
        self.main_table.len.saturating_add(old_len)
    }

    /// Returns `true` if the table contains no key-value pairs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the total slot capacity of the main table.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.main_table.capacity
    }

    /// Returns the hash seed used by this table instance.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Returns the maximum capacity limit configured for this table.
    #[must_use]
    pub const fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    /// Returns `true` if an incremental background resize is currently in progress.
    #[must_use]
    pub const fn is_resizing(&self) -> bool {
        self.old_table.is_some()
    }

    /// Returns the in-memory size in bytes of an individual table slot (`Option<Slot<V>>`).
    #[must_use]
    pub const fn slot_size(&self) -> usize {
        slot_size::<V>()
    }

    /// Steps incremental table migration by inspecting and moving up to `buckets` slots from `old_table`.
    ///
    /// # Reasoning: Backward-Shift Removal Prevents Holes in `old_table`
    ///
    /// When migrating from `old_table`:
    /// - If `old_table[migrate_cursor]` is empty (`None`), the cursor moves forward (`migrate_cursor += 1`).
    /// - If `old_table[migrate_cursor]` is occupied, taking it directly would leave an empty hole in the middle
    ///   of an open-addressing probe cluster, causing subsequent lookups in `old_table` to stop prematurely.
    /// - Instead, we remove it using backward-shift deletion (`remove_at_and_shift`). Removal only fills
    ///   the hole at `migrate_cursor`, and every slot before `migrate_cursor` is empty, so shifted items
    ///   never go below `migrate_cursor`.
    /// - Therefore, after every single removal, `old_table` remains 100% valid with no holes, preserving all
    ///   Robin Hood invariants throughout migration.
    /// - If moving the item into `main_table` fails, the item is restored to `old_table` and the error is returned.
    /// - `old_table` is only set to `None` when `old_table.len == 0`.
    ///
    /// Returns `Ok(true)` if a resize is still running, or `Ok(false)` if completed or not resizing.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] if `main_table` cannot accept the migrated slot; the slot is restored into `old_table`.
    pub fn step_migration(&mut self, buckets: usize) -> Result<bool, TableFull> {
        let mut moved = 0usize;
        while moved < buckets {
            let Some(ref mut old) = self.old_table else {
                return Ok(false);
            };

            if old.len == 0 {
                self.old_table = None;
                self.migrate_cursor = 0;
                return Ok(false);
            }

            if self.migrate_cursor >= old.capacity {
                debug_assert!(
                    false,
                    "migrate_cursor reached end of old_table with {} items remaining",
                    old.len
                );
                self.migrate_cursor = 0;
            }

            let is_empty = old
                .slots
                .get(self.migrate_cursor)
                .and_then(Option::as_ref)
                .is_none();

            if is_empty {
                self.migrate_cursor = self.migrate_cursor.saturating_add(1);
                moved = moved.saturating_add(1);
            } else {
                // Ensure main_table can accept a new key before removing from old_table
                if self.main_table.len >= self.main_table.max_load_limit() {
                    return Err(TableFull);
                }

                // Remove occupied bucket with backward shift to maintain contiguous clusters in old_table
                let Some(slot) = old.remove_at_and_shift(self.migrate_cursor) else {
                    break;
                };

                // Insert into main_table with stored hash without re-hashing key bytes
                match self.main_table.insert_slot(slot) {
                    Ok(()) => {
                        moved = moved.saturating_add(1);
                    }
                    Err(returned_slot) => {
                        // Put the item back into old_table so no key is ever lost!
                        let restore_res = old.insert_slot(returned_slot);
                        debug_assert!(
                            restore_res.is_ok(),
                            "restoring slot into old_table must succeed because slot was just removed"
                        );
                        return Err(TableFull);
                    }
                }
            }
        }

        if let Some(ref old) = self.old_table {
            if old.len == 0 {
                self.old_table = None;
                self.migrate_cursor = 0;
                return Ok(false);
            }
            if self.migrate_cursor >= old.capacity {
                debug_assert!(
                    false,
                    "migrate_cursor reached end of old_table with {} items remaining",
                    old.len
                );
                self.migrate_cursor = 0;
            }
        }

        Ok(self.old_table.is_some())
    }

    /// Performs a single background migration step of [`MIGRATE_BATCH`] buckets.
    ///
    /// A failed background migration step is not fatal, both tables stay valid,
    /// and the next operation retries.
    fn background_migrate(&mut self) {
        if self.step_migration(MIGRATE_BATCH).is_err() {
            // A failed background migration step is not fatal, both tables stay valid,
            // and the next operation retries.
        }
    }

    /// Checks if a resize is needed and initiates it.
    ///
    /// # Resize Rules
    ///
    /// If a resize is already running, do not start another one. First finish the running one
    /// (move all remaining buckets synchronously), then start the new resize.
    fn check_and_start_resize(&mut self) -> Result<(), TableFull> {
        // If a resize is already running, complete it first before starting a new one.
        if self.old_table.is_some() {
            self.step_migration(usize::MAX)?;
            if self.old_table.is_some() {
                return Err(TableFull);
            }
        }

        if self.main_table.capacity >= self.max_capacity {
            return Err(TableFull);
        }

        let new_capacity = self
            .main_table
            .capacity
            .saturating_mul(2)
            .clamp(MIN_CAPACITY, self.max_capacity);

        let old = std::mem::replace(&mut self.main_table, SingleTable::new(new_capacity));
        self.old_table = Some(old);
        self.migrate_cursor = 0;
        Ok(())
    }

    /// Returns a reference to the value corresponding to the supplied key.
    ///
    /// This method takes `&self` and is read-only; it does NOT mutate the table or trigger migration.
    /// Reads remain cheap, non-mutating, and shareable across threads/tasks.
    /// Lookup order: checks `main_table` first, then `old_table` (if resizing).
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&V> {
        let hash = hash64(key, self.seed);
        self.get_with_hash(hash, key)
    }

    /// Read-only lookup alias for [`Self::get`].
    #[must_use]
    pub fn get_ref(&self, key: &[u8]) -> Option<&V> {
        self.get(key)
    }

    /// Returns a reference to the value corresponding to the supplied key and precomputed hash.
    ///
    /// Lookup order: checks `main_table` first, then `old_table` (if resizing).
    #[must_use]
    pub fn get_with_hash(&self, hash: u64, key: &[u8]) -> Option<&V> {
        // 1. Look in main_table first
        if let Some(val) = self.main_table.get_with_hash(hash, key) {
            return Some(val);
        }

        // 2. Look in old_table if it exists
        if let Some(ref old) = self.old_table {
            return old.get_with_hash(hash, key);
        }

        None
    }

    /// Returns the number of slot reads/probes inspected during a lookup for `key`.
    #[doc(hidden)]
    #[must_use]
    pub fn lookup_probe_count(&self, key: &[u8]) -> usize {
        let hash = hash64(key, self.seed);
        let mut count = self.main_table.lookup_probe_count(hash, key);
        if let Some(ref old) = self.old_table {
            if self.main_table.get_with_hash(hash, key).is_none() {
                count = count.saturating_add(old.lookup_probe_count(hash, key));
            }
        }
        count
    }

    /// Returns a mutable reference to the value corresponding to the supplied key.
    ///
    /// Mutates the table and steps migration by up to [`MIGRATE_BATCH`] (16) buckets.
    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut V> {
        self.background_migrate();
        let hash = hash64(key, self.seed);
        self.get_mut_with_hash(hash, key)
    }

    /// Returns a mutable reference to the value corresponding to the key and precomputed hash.
    pub fn get_mut_with_hash(&mut self, hash: u64, key: &[u8]) -> Option<&mut V> {
        if self.main_table.contains_key_with_hash(hash, key) {
            return self.main_table.get_mut_with_hash(hash, key);
        }

        // If present in old_table, extract the slot to migrate it to main_table
        let slot = {
            let old = self.old_table.as_mut()?;
            let target_idx = old.find_index_with_hash(hash, key)?;
            old.remove_at_and_shift(target_idx)?
        };

        match self.main_table.insert_slot(slot) {
            Ok(()) => {
                if let Some(ref old) = self.old_table {
                    if old.len == 0 {
                        self.old_table = None;
                        self.migrate_cursor = 0;
                    }
                }
                self.main_table.get_mut_with_hash(hash, key)
            }
            Err(returned_slot) => {
                let old = self.old_table.as_mut()?;
                let restore_res = old.insert_slot(returned_slot);
                debug_assert!(
                    restore_res.is_ok(),
                    "restoring slot into old_table must succeed because slot was just removed"
                );
                if let Err(unrestored) = restore_res {
                    for s in &mut old.slots {
                        if s.is_none() {
                            *s = Some(unrestored);
                            old.len = old.len.saturating_add(1);
                            break;
                        }
                    }
                }
                old.get_mut_with_hash(hash, key)
            }
        }
    }

    /// Returns `true` if the table contains an entry for the given key.
    #[must_use]
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    /// Inserts a key-value pair into the table, automatically resizing when load exceeds 7/8.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] only if the table has reached [`DEFAULT_MAX_CAPACITY`] ($2^{30}$).
    pub fn insert(&mut self, key: Box<[u8]>, value: V) -> Result<Option<V>, TableFull> {
        let hash = hash64(&key, self.seed);
        self.insert_with_hash(hash, key, value)
    }

    /// Inserts a key-value pair with a precomputed or forced hash value.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] only if the table has reached the configured maximum capacity.
    pub fn insert_with_hash(
        &mut self,
        hash: u64,
        key: Box<[u8]>,
        value: V,
    ) -> Result<Option<V>, TableFull> {
        // Step 1: Paced migration step
        self.background_migrate();

        // Step 2: Check if key exists in main_table
        let key_in_main = self.main_table.contains_key_with_hash(hash, &key);

        // If key is not in main_table and main_table is at or above 7/8 load, trigger resize
        if !key_in_main && self.main_table.is_at_max_load() {
            self.check_and_start_resize()?;
        }

        // Step 3: Remove key from old_table if present, preserving the slot so it can be restored on failure
        let old_slot = if let Some(ref mut old) = self.old_table {
            if let Some(target_idx) = old.find_index_with_hash(hash, &key) {
                old.remove_at_and_shift(target_idx)
            } else {
                None
            }
        } else {
            None
        };

        // Step 4: Insert into main_table
        let old_in_main = match self.main_table.insert_with_hash(hash, key, value) {
            Ok(prev) => {
                if let Some(ref old) = self.old_table {
                    if old.len == 0 {
                        self.old_table = None;
                        self.migrate_cursor = 0;
                    }
                }
                prev
            }
            Err(err) => {
                // Restore removed slot to old_table so no key is lost on insert failure
                if let Some(slot) = old_slot {
                    if let Some(ref mut old) = self.old_table {
                        let restore_res = old.insert_slot(slot);
                        debug_assert!(
                            restore_res.is_ok(),
                            "restoring slot into old_table must succeed because slot was just removed"
                        );
                        if let Err(unrestored) = restore_res {
                            for s in &mut old.slots {
                                if s.is_none() {
                                    *s = Some(unrestored);
                                    old.len = old.len.saturating_add(1);
                                    break;
                                }
                            }
                        }
                    }
                }
                return Err(err);
            }
        };

        let old_in_old = old_slot.map(|s| s.value);
        Ok(old_in_main.or(old_in_old))
    }

    /// Removes an entry by key from both tables, returning the value that was found.
    pub fn remove(&mut self, key: &[u8]) -> Option<V> {
        self.background_migrate();
        let hash = hash64(key, self.seed);
        self.remove_with_hash(hash, key)
    }

    /// Removes an entry by key and precomputed hash from both tables.
    pub fn remove_with_hash(&mut self, hash: u64, key: &[u8]) -> Option<V> {
        let val_main = self.main_table.remove_with_hash(hash, key);
        let val_old = if let Some(ref mut old) = self.old_table {
            let removed = old.remove_with_hash(hash, key);
            if old.len == 0 {
                self.old_table = None;
                self.migrate_cursor = 0;
            }
            removed
        } else {
            None
        };

        val_main.or(val_old)
    }

    /// Removes all key-value pairs from the table while retaining allocated capacity.
    pub fn clear(&mut self) {
        self.main_table.clear();
        self.old_table = None;
        self.migrate_cursor = 0;
    }

    /// Returns an iterator over borrowed key-value pairs in arbitrary table order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &V)> {
        let main_iter = self.main_table.iter();
        let old_iter = self.old_table.iter().flat_map(SingleTable::iter);
        main_iter.chain(old_iter)
    }

    /// Computes and returns detailed operational statistics for the table.
    #[must_use]
    pub fn stats(&self) -> TableStats {
        let (max_psl, average_psl) = self.main_table.stats_internal();
        let total_len = self.len();
        let resizing = self.is_resizing();
        let old_remaining_buckets = self
            .old_table
            .as_ref()
            .map_or(0, |old| old.capacity.saturating_sub(self.migrate_cursor));

        TableStats {
            len: total_len,
            capacity: self.main_table.capacity,
            load: (self.main_table.len as f64) / (self.main_table.capacity as f64),
            max_psl,
            average_psl,
            resizing,
            old_remaining_buckets,
        }
    }

    /// Validates all Robin Hood table invariants across both tables.
    ///
    /// # Invariants Checked
    ///
    /// 1. `main_table` satisfies all 5 standard Robin Hood invariants.
    /// 2. If `old_table` exists:
    ///    - `old_table` satisfies all standard Robin Hood invariants.
    ///    - All slots before `migrate_cursor` in `old_table` are strictly empty (`None`).
    ///    - No key exists in both `main_table` and `old_table`.
    /// 3. Total `len()` equals `main_table.len + old_table.len`.
    ///
    /// # Errors
    ///
    /// Returns an error message describing the invariant violation if any rule is broken.
    pub fn check_invariants(&self) -> Result<(), String> {
        self.main_table.check_invariants()?;

        if let Some(ref old) = self.old_table {
            old.check_invariants()?;

            for i in 0..self.migrate_cursor {
                if let Some(slot) = old.slots.get(i).and_then(Option::as_ref) {
                    return Err(format!(
                        "Invariant violated: slot {i} before migrate_cursor ({}) in old_table is occupied (key: {:?})",
                        self.migrate_cursor, slot.key
                    ));
                }
            }

            for slot_opt in &old.slots {
                if let Some(s) = slot_opt.as_ref() {
                    if self.main_table.contains_key_with_hash(s.hash, &s.key) {
                        return Err(format!(
                            "Invariant violated: key {:?} exists in both main_table and old_table",
                            s.key
                        ));
                    }
                }
            }
        }

        let expected_len = self.main_table.len + self.old_table.as_ref().map_or(0, |o| o.len);
        if self.len() != expected_len {
            return Err(format!(
                "Invariant violated: recorded len {} does not match sum of tables {expected_len}",
                self.len()
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct XorShift64(u64);

    impl XorShift64 {
        fn new(seed: u64) -> Self {
            assert_ne!(seed, 0, "XorShift64 seed must not be zero");
            Self(seed)
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    #[test]
    fn test_table_basic_insert_get_overwrite_remove() {
        let mut table = RobinHoodTable::new(16, 42);

        // Insert basics
        assert_eq!(table.insert(Box::from(&b"k1"[..]), 100), Ok(None));
        assert_eq!(table.insert(Box::from(&b"k2"[..]), 200), Ok(None));
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(b"k1"), Some(&100));
        assert_eq!(table.get(b"k2"), Some(&200));
        assert_eq!(table.get(b"missing"), None);
        assert!(table.contains_key(b"k1"));
        assert!(!table.contains_key(b"missing"));

        // Overwrite
        assert_eq!(table.insert(Box::from(&b"k1"[..]), 999), Ok(Some(100)));
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(b"k1"), Some(&999));

        // get_mut
        if let Some(val) = table.get_mut(b"k1") {
            *val += 1;
        }
        assert_eq!(table.get(b"k1"), Some(&1000));

        // Remove
        assert_eq!(table.remove(b"k1"), Some(1000));
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(b"k1"), None);
        assert_eq!(table.remove(b"k1"), None);

        // Empty key (table accepts any byte sequence)
        assert_eq!(table.insert(Box::from(&b""[..]), 555), Ok(None));
        assert_eq!(table.get(b""), Some(&555));
        assert_eq!(table.remove(b""), Some(555));

        // Large 1 MB key
        let large_key = vec![0xABu8; 1024 * 1024].into_boxed_slice();
        assert_eq!(table.insert(large_key.clone(), 777), Ok(None));
        assert_eq!(table.get(&large_key), Some(&777));
        assert_eq!(table.remove(&large_key), Some(777));

        table.check_invariants().expect("invariants must hold");
    }

    #[test]
    fn test_forced_collisions_and_cluster_wraparound() {
        let mut table = RobinHoodTable::new(8, 0);
        let mask = table.main_table.mask as u64;

        let hash_last_slot = mask;

        assert_eq!(
            table.insert_with_hash(hash_last_slot, Box::from(&b"wrap1"[..]), 1),
            Ok(None)
        );
        table.check_invariants().expect("invariants after wrap1");

        assert_eq!(
            table.insert_with_hash(hash_last_slot, Box::from(&b"wrap2"[..]), 2),
            Ok(None)
        );
        table.check_invariants().expect("invariants after wrap2");

        assert_eq!(
            table.insert_with_hash(hash_last_slot, Box::from(&b"wrap3"[..]), 3),
            Ok(None)
        );
        table.check_invariants().expect("invariants after wrap3");

        assert_eq!(table.get_with_hash(hash_last_slot, b"wrap1"), Some(&1));
        assert_eq!(table.get_with_hash(hash_last_slot, b"wrap2"), Some(&2));
        assert_eq!(table.get_with_hash(hash_last_slot, b"wrap3"), Some(&3));
    }

    #[test]
    fn test_robin_hood_swap_displaces_richer_element() {
        let mut table = RobinHoodTable::new(8, 0);

        assert_eq!(
            table.insert_with_hash(0, Box::from(&b"A"[..]), 10),
            Ok(None)
        );
        assert_eq!(
            table.insert_with_hash(0, Box::from(&b"B"[..]), 20),
            Ok(None)
        );
        assert_eq!(
            table.insert_with_hash(1, Box::from(&b"C"[..]), 30),
            Ok(None)
        );
        assert_eq!(
            table.insert_with_hash(0, Box::from(&b"D"[..]), 40),
            Ok(None)
        );

        table.check_invariants().expect("invariants after swap");

        assert_eq!(
            table
                .main_table
                .slots
                .first()
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"A"[..], 0))
        );
        assert_eq!(
            table
                .main_table
                .slots
                .get(1)
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"B"[..], 1))
        );
        assert_eq!(
            table
                .main_table
                .slots
                .get(2)
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"D"[..], 2))
        );
        assert_eq!(
            table
                .main_table
                .slots
                .get(3)
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"C"[..], 2))
        );

        assert_eq!(table.get_with_hash(0, b"A"), Some(&10));
        assert_eq!(table.get_with_hash(0, b"B"), Some(&20));
        assert_eq!(table.get_with_hash(1, b"C"), Some(&30));
        assert_eq!(table.get_with_hash(0, b"D"), Some(&40));
    }

    #[test]
    fn test_backward_shift_deletion_maintains_clusters() {
        let mut table = RobinHoodTable::new(16, 0);

        for i in 1..=5 {
            let key = format!("clust-{i}").into_bytes().into_boxed_slice();
            assert_eq!(table.insert_with_hash(2, key, i), Ok(None));
        }
        table.check_invariants().expect("cluster built");

        assert_eq!(table.remove_with_hash(2, b"clust-1"), Some(1));
        table
            .check_invariants()
            .expect("invariants after removing first");
        for i in 2..=5 {
            let key = format!("clust-{i}").into_bytes();
            assert_eq!(table.get_with_hash(2, &key), Some(&i));
        }

        assert_eq!(table.remove_with_hash(2, b"clust-3"), Some(3));
        table
            .check_invariants()
            .expect("invariants after removing middle");
        assert_eq!(table.get_with_hash(2, b"clust-2"), Some(&2));
        assert_eq!(table.get_with_hash(2, b"clust-4"), Some(&4));
        assert_eq!(table.get_with_hash(2, b"clust-5"), Some(&5));

        assert_eq!(table.remove_with_hash(2, b"clust-5"), Some(5));
        table
            .check_invariants()
            .expect("invariants after removing last");
        assert_eq!(table.get_with_hash(2, b"clust-2"), Some(&2));
        assert_eq!(table.get_with_hash(2, b"clust-4"), Some(&4));
    }

    #[test]
    fn test_table_full_at_load_limit_and_overwrites_allowed() {
        // With max_capacity fixed at 8, 7/8 load limit is exactly 7 items
        let mut table = RobinHoodTable::with_max_capacity(8, 1234, 8);
        assert_eq!(table.capacity(), 8);

        for i in 0..7 {
            let k = format!("k{i}").into_bytes().into_boxed_slice();
            assert_eq!(table.insert(k, i), Ok(None));
        }
        assert_eq!(table.len(), 7);

        // 8th insertion must fail with TableFull since table cannot grow past 8
        let k8 = Box::from(&b"k8"[..]);
        assert_eq!(table.insert(k8, 8), Err(TableFull));

        // Reading still works when full
        assert_eq!(table.get(b"k0"), Some(&0));
        assert_eq!(table.get(b"k6"), Some(&6));

        // Overwrites on existing keys must succeed even when table is at max load limit
        assert_eq!(table.insert(Box::from(&b"k0"[..]), 999), Ok(Some(0)));
        assert_eq!(table.get(b"k0"), Some(&999));
        assert_eq!(table.len(), 7);

        // Remove one item -> now insertion succeeds again
        assert_eq!(table.remove(b"k6"), Some(6));
        assert_eq!(table.len(), 6);
        assert_eq!(table.insert(Box::from(&b"k_new"[..]), 77), Ok(None));
        assert_eq!(table.len(), 7);
        assert_eq!(table.insert(Box::from(&b"k_fail"[..]), 88), Err(TableFull));
    }

    #[test]
    fn test_insert_100k_keys_from_small_table_no_err_all_found() {
        let mut table = RobinHoodTable::new(8, 0x5EED);
        let total_keys = 100_000usize;

        for i in 0..total_keys {
            let key = format!("k-{i}").into_bytes().into_boxed_slice();
            assert!(
                table.insert(key, i as u64).is_ok(),
                "insert must not fail due to load"
            );
        }

        assert_eq!(table.len(), total_keys);

        // All keys are found
        for i in 0..total_keys {
            let key = format!("k-{i}").into_bytes();
            assert_eq!(table.get(&key), Some(&(i as u64)));
        }

        table
            .check_invariants()
            .expect("invariants after 100k inserts");
    }

    #[test]
    fn test_resize_in_middle_step_by_step_and_invariants() {
        let mut table = RobinHoodTable::new(8, 0x1111);

        // Insert 7 keys (load 7/8 in capacity 8)
        for i in 0..7 {
            let key = format!("k{i}").into_bytes().into_boxed_slice();
            assert!(table.insert(key, i).is_ok());
        }
        assert!(!table.is_resizing());

        // 8th key triggers resize
        let k7 = Box::from(&b"k7"[..]);
        assert!(table.insert(k7, 7).is_ok());
        assert!(table.is_resizing(), "table must be in resizing state");

        // Step migration 1 bucket at a time and verify invariants and lookups at every point
        while table.is_resizing() {
            assert_eq!(table.len(), 8);
            for i in 0..8 {
                let k = format!("k{i}").into_bytes();
                assert_eq!(
                    table.get_ref(&k),
                    Some(&i),
                    "key k{i} must be found mid-resize"
                );
            }
            table
                .check_invariants()
                .expect("invariants must hold at each migration step");

            let _ = table.step_migration(1);
        }

        assert_eq!(table.len(), 8);
        for i in 0..8 {
            let k = format!("k{i}").into_bytes();
            assert_eq!(table.get(&k), Some(&i));
        }
        table
            .check_invariants()
            .expect("invariants after resize completion");
    }

    #[test]
    fn test_resize_overwrite_in_middle_of_migration() {
        let mut table = RobinHoodTable::new(8, 0x2222);

        for i in 0..7 {
            let key = format!("item-{i}").into_bytes().into_boxed_slice();
            assert!(table.insert(key, i).is_ok());
        }

        // Trigger resize
        assert!(table.insert(Box::from(&b"item-7"[..]), 7).is_ok());
        assert!(table.is_resizing());

        // Migrate a few buckets so some keys are moved, but old_table is still active
        let _ = table.step_migration(1);
        assert!(table.is_resizing());

        // Overwrite a key in the middle of resize
        let overwrite_key = Box::from(&b"item-5"[..]);
        let old_val = table
            .insert(overwrite_key, 9999)
            .expect("overwrite succeeds");
        assert_eq!(old_val, Some(5));

        assert_eq!(table.get(b"item-5"), Some(&9999));
        table
            .check_invariants()
            .expect("key must not exist in both tables after overwrite");

        // Finish migration
        assert_eq!(table.step_migration(usize::MAX), Ok(false));
        assert!(!table.is_resizing());
        assert_eq!(table.get(b"item-5"), Some(&9999));
        table.check_invariants().expect("invariants after finish");
    }

    #[test]
    fn test_resize_remove_in_middle_when_in_old_and_in_main() {
        let mut table = RobinHoodTable::new(8, 0x3333);

        for i in 0..7 {
            let key = format!("elem-{i}").into_bytes().into_boxed_slice();
            assert!(table.insert(key, i).is_ok());
        }

        // Trigger resize
        assert!(table.insert(Box::from(&b"elem-7"[..]), 7).is_ok());
        assert!(table.is_resizing());

        // item-7 was written into main_table
        // item-0 was moved by the 1st migration step in insert() or still in old_table
        // Remove an element that is in main_table
        assert_eq!(table.remove(b"elem-7"), Some(7));
        assert_eq!(table.get(b"elem-7"), None);
        assert_eq!(table.len(), 7);
        table
            .check_invariants()
            .expect("invariants after removing from main_table");

        // Remove an element that is in old_table (if any remain)
        if table.is_resizing() {
            // Find a key residing in old_table
            let old_key = table.old_table.as_ref().and_then(|old| {
                old.slots
                    .iter()
                    .find_map(|s| s.as_ref().map(|slot| slot.key.clone()))
            });

            if let Some(k) = old_key {
                let removed = table.remove(&k);
                assert!(removed.is_some(), "must remove key from old_table");
                assert_eq!(table.get(&k), None);
                table
                    .check_invariants()
                    .expect("invariants after removing from old_table");
            }
        }
    }

    #[test]
    fn test_key_moved_by_step_migration_found_with_same_value() {
        let mut table = RobinHoodTable::new(8, 0x4444);

        for i in 0..7 {
            let key = format!("mv-{i}").into_bytes().into_boxed_slice();
            table.insert(key, i * 10).unwrap();
        }

        // Trigger resize
        table.insert(Box::from(&b"mv-7"[..]), 70).unwrap();
        assert!(table.is_resizing());

        // Move buckets one by one to verify value preservation
        while table.is_resizing() {
            let _ = table.step_migration(1);
        }

        for i in 0..8 {
            let k = format!("mv-{i}").into_bytes();
            assert_eq!(table.get(&k), Some(&(i * 10)));
        }
    }

    #[test]
    fn test_step_migration_on_table_with_no_resize_returns_false() {
        let mut table = RobinHoodTable::<u64>::new(16, 0);
        assert!(!table.is_resizing());
        assert_eq!(table.step_migration(16), Ok(false));
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_step_migration_usize_max_finishes_in_one_call() {
        let mut table = RobinHoodTable::new(8, 0x5555);

        for i in 0..8 {
            let key = format!("fast-{i}").into_bytes().into_boxed_slice();
            table.insert(key, i).unwrap();
        }

        assert!(table.is_resizing());
        assert_eq!(table.step_migration(usize::MAX), Ok(false));
        assert!(!table.is_resizing());
        assert_eq!(table.stats().old_remaining_buckets, 0);

        for i in 0..8 {
            let k = format!("fast-{i}").into_bytes();
            assert_eq!(table.get(&k), Some(&i));
        }
    }

    #[test]
    fn test_resize_at_max_capacity_returns_table_full() {
        // Table starts at 8, max capacity clamped at 16
        let mut table = RobinHoodTable::with_max_capacity(8, 0x6666, 16);

        // Fill 7 items in capacity 8
        for i in 0..7 {
            let k = format!("c{i}").into_bytes().into_boxed_slice();
            table.insert(k, i).unwrap();
        }
        assert_eq!(table.capacity(), 8);

        // 8th item triggers resize to capacity 16
        table.insert(Box::from(&b"c7"[..]), 7).unwrap();
        table.step_migration(usize::MAX).unwrap();
        assert_eq!(table.capacity(), 16);

        // Fill up to 14 items (7/8 of 16)
        for i in 8..14 {
            let k = format!("c{i}").into_bytes().into_boxed_slice();
            table.insert(k, i).unwrap();
        }
        assert_eq!(table.len(), 14);

        // 15th item would exceed 7/8 load factor and cannot resize past 16 -> returns TableFull
        let k14 = Box::from(&b"c14"[..]);
        assert_eq!(table.insert(k14, 14), Err(TableFull));

        // Overwrites still work when at max capacity
        assert_eq!(table.insert(Box::from(&b"c0"[..]), 999), Ok(Some(0)));
        assert_eq!(table.get(b"c0"), Some(&999));
    }

    #[test]
    fn test_hash_not_computed_again_during_migration() {
        let mut table = RobinHoodTable::new(8, 0x7777);

        // Force a custom hash that does NOT match hash64(&key, seed)
        let forced_hash = 0xDEAD_BEEF_CAFE_1234_u64;
        let custom_key: Box<[u8]> = Box::from(&b"my-custom-key"[..]);

        table
            .insert_with_hash(forced_hash, custom_key.clone(), 42)
            .unwrap();

        // Fill other keys to trigger a resize
        for i in 0..7 {
            let k = format!("k{i}").into_bytes().into_boxed_slice();
            table.insert(k, i).unwrap();
        }

        assert!(table.is_resizing());
        table.step_migration(usize::MAX).unwrap();
        assert!(!table.is_resizing());

        // Find the slot containing custom_key in main_table
        let slot = table
            .main_table
            .slots
            .iter()
            .find_map(|s| s.as_ref().filter(|slot| slot.key == custom_key))
            .expect("custom key must exist in main_table");

        // The hash in main_table must be identical to the forced_hash!
        assert_eq!(
            slot.hash, forced_hash,
            "hash must be transferred directly without re-hashing"
        );
        assert_eq!(table.get_with_hash(forced_hash, &custom_key), Some(&42));
    }

    #[test]
    fn test_old_table_backward_shift_migration_cluster_wrap_invariants_at_every_step() {
        let mut table = RobinHoodTable::new(8, 0x4321);

        // Force collisions forming a cluster wrapping around 7 -> 0:
        // mask for capacity 8 is 7.
        let keys_hashes: [(Box<[u8]>, u64); 7] = [
            (Box::from(&b"cw0"[..]), 6u64),  // ideal 6
            (Box::from(&b"cw1"[..]), 14u64), // ideal 6 (14 & 7 = 6) -> lands at 7
            (Box::from(&b"cw2"[..]), 22u64), // ideal 6 -> lands at 0 (wraparound!)
            (Box::from(&b"cw3"[..]), 30u64), // ideal 6 -> lands at 1
            (Box::from(&b"cw4"[..]), 7u64),  // ideal 7 -> lands at 2
            (Box::from(&b"cw5"[..]), 15u64), // ideal 7 -> lands at 3
            (Box::from(&b"cw6"[..]), 23u64), // ideal 7 -> lands at 4
        ];

        for (k, h) in &keys_hashes {
            table.insert_with_hash(*h, k.clone(), 100).unwrap();
        }

        // 8th key triggers resize to capacity 16
        table
            .insert_with_hash(5u64, Box::from(&b"cw7"[..]), 100)
            .unwrap();
        assert!(table.is_resizing());

        let all_keys_hashes: [(Box<[u8]>, u64); 8] = [
            (Box::from(&b"cw0"[..]), 6u64),
            (Box::from(&b"cw1"[..]), 14u64),
            (Box::from(&b"cw2"[..]), 22u64),
            (Box::from(&b"cw3"[..]), 30u64),
            (Box::from(&b"cw4"[..]), 7u64),
            (Box::from(&b"cw5"[..]), 15u64),
            (Box::from(&b"cw6"[..]), 23u64),
            (Box::from(&b"cw7"[..]), 5u64),
        ];

        // At every single step of migration:
        while table.is_resizing() {
            // 1. Check that old_table passes check_invariants
            if let Some(ref old) = table.old_table {
                old.check_invariants()
                    .expect("old_table must pass check_invariants at every step of migration");
            }
            // 2. Check full table invariants
            table
                .check_invariants()
                .expect("table must pass check_invariants at every step of migration");

            // 3. Verify get finds every key that has not yet finished moving or was already moved
            for (k, h) in &all_keys_hashes {
                assert!(
                    table.get_with_hash(*h, k).is_some(),
                    "Key {k:?} must be found during migration"
                );
            }

            table.step_migration(1).unwrap();
        }

        assert!(!table.is_resizing());
        table.check_invariants().expect("final invariants pass");
        for (k, h) in &all_keys_hashes {
            assert!(table.get_with_hash(*h, k).is_some());
        }
    }

    #[test]
    fn test_one_seed_for_both_tables_after_full_resize() {
        let seed = 0xABCD_1234_u64;
        let mut table = RobinHoodTable::new(8, seed);

        // Insert 20 keys to force multiple resizes (capacity 8 -> 16 -> 32)
        for i in 0..20 {
            let key = format!("k-{i}").into_bytes().into_boxed_slice();
            table.insert(key, i as u64).unwrap();
        }

        // Finish any ongoing migration
        let _ = table.step_migration(usize::MAX);
        assert!(!table.is_resizing());
        assert_eq!(table.capacity(), 32);

        // 1. Every key is found with read-only get(&self)
        for i in 0..20 {
            let key = format!("k-{i}").into_bytes();
            assert_eq!(table.get(&key), Some(&(i as u64)));
        }

        // 2. Every stored hash in main_table equals hash64(key, seed)
        let mut found_count = 0;
        for slot in table.main_table.slots.iter().flatten() {
            let expected_hash = hash64(&slot.key, seed);
            assert_eq!(
                slot.hash, expected_hash,
                "Stored hash for key {:?} must match hash64 with table seed",
                slot.key
            );
            found_count += 1;
        }
        assert_eq!(found_count, 20);
    }

    #[test]
    fn test_migration_error_does_not_lose_key_when_main_is_full() {
        let mut table = RobinHoodTable::new(8, 0x9999);

        // Fill old_table with 7 items
        for i in 0..7 {
            let k = format!("old-k{i}").into_bytes().into_boxed_slice();
            table.insert(k, i).unwrap();
        }

        // Trigger resize by inserting 8th item
        table.insert(Box::from(&b"trigger"[..]), 99).unwrap();
        assert!(table.is_resizing());

        // Artificially fill main_table up to its max_load_limit (14 for capacity 16)
        // so main_table.insert_slot will return Err
        for i in 100..114 {
            let k = format!("fill-{i}").into_bytes().into_boxed_slice();
            let h = hash64(&k, table.seed);
            let _ = table.main_table.insert_with_hash(h, k, i);
        }
        assert!(table.main_table.is_at_max_load());

        // Step migration by 1: moving an item into full main_table returns TableFull!
        let res = table.step_migration(1);
        assert_eq!(res, Err(TableFull));

        // The item must NOT be lost! It was restored to old_table.
        table
            .check_invariants()
            .expect("invariants must hold after migration error restore");
        for i in 0..7 {
            let k = format!("old-k{i}").into_bytes();
            assert!(
                table.get(&k).is_some(),
                "Key old-k{i} must not be lost after failed migration step"
            );
        }
    }

    #[test]
    fn test_invariant_empty_slot_before_psl_greater_than_zero_fails() {
        let mut table = RobinHoodTable::<u64>::new(8, 0x1234);
        // Manually break the "empty slot ends probe" invariant in main_table:
        // Slot 3 has psl 1, but slot 2 is empty!
        table.main_table.slots[3] = Some(Slot {
            hash: 2, // ideal 2
            psl: 1,  // placed at 3
            key: Box::from(&b"broken"[..]),
            value: 42,
        });
        table.main_table.slots[2] = None;
        table.main_table.len = 1;

        let err = table.check_invariants().unwrap_err();
        assert!(
            err.contains("previous slot 2 is empty"),
            "Error must identify that previous slot is empty: {err}"
        );
    }

    #[test]
    fn test_get_mut_main_table_at_load_limit_restores_to_old_table() {
        let mut table = RobinHoodTable::new(8, 0x1111);
        for i in 0..7 {
            let k = format!("old-item-{i}").into_bytes().into_boxed_slice();
            table.insert(k, i * 10).unwrap();
        }
        table.insert(Box::from(&b"trigger"[..]), 99).unwrap();
        assert!(table.is_resizing());

        for i in 100..114 {
            let k = format!("main-fill-{i}").into_bytes().into_boxed_slice();
            let h = hash64(&k, table.seed);
            let _ = table.main_table.insert_with_hash(h, k, i);
        }
        assert!(table.main_table.is_at_max_load());

        let val_mut = table.get_mut(b"old-item-3");
        assert_eq!(val_mut, Some(&mut 30));
        if let Some(v) = val_mut {
            *v = 300;
        }

        assert_eq!(table.get(b"old-item-3"), Some(&300));
        table
            .check_invariants()
            .expect("invariants must hold after failed get_mut migration restore");
    }

    #[test]
    fn test_check_and_start_resize_fails_when_migration_cannot_complete() {
        let mut table = RobinHoodTable::new(8, 0x2222);
        for i in 0..7 {
            let k = format!("k-{i}").into_bytes().into_boxed_slice();
            table.insert(k, i).unwrap();
        }
        table.insert(Box::from(&b"trigger"[..]), 99).unwrap();
        assert!(table.is_resizing());

        table.max_capacity = table.main_table.capacity;
        for i in 100..114 {
            let k = format!("fill-{i}").into_bytes().into_boxed_slice();
            let h = hash64(&k, table.seed);
            let _ = table.main_table.insert_with_hash(h, k, i);
        }

        let res = table.check_and_start_resize();
        assert_eq!(res, Err(TableFull));
        assert!(table.old_table.is_some(), "old_table must not be dropped");
    }

    #[test]
    fn test_insert_with_hash_restores_key_to_old_table_on_main_table_full() {
        let mut table = RobinHoodTable::new(8, 0x3333);
        for i in 0..7 {
            let k = format!("k-{i}").into_bytes().into_boxed_slice();
            table.insert(k, i * 5).unwrap();
        }
        table.insert(Box::from(&b"trigger"[..]), 99).unwrap();
        assert!(table.is_resizing());

        for i in 100..114 {
            let k = format!("fill-{i}").into_bytes().into_boxed_slice();
            let h = hash64(&k, table.seed);
            let _ = table.main_table.insert_with_hash(h, k, i);
        }
        assert!(table.main_table.is_at_max_load());

        let target_key = Box::from(&b"k-2"[..]);
        let res = table.insert(target_key, 9999);
        assert_eq!(res, Err(TableFull));

        assert_eq!(table.get(b"k-2"), Some(&10));
        table
            .check_invariants()
            .expect("invariants must hold after failed insert restore");
    }

    #[test]
    fn test_overwrite_succeeds_when_main_table_at_load_limit_and_old_table_not_empty() {
        let mut table = RobinHoodTable::new(8, 0x5555);
        for i in 0..7 {
            let k = format!("k-{i}").into_bytes().into_boxed_slice();
            table.insert(k, i * 10).unwrap();
        }
        table.insert(Box::from(&b"trigger"[..]), 99).unwrap();
        assert!(table.is_resizing());
        assert!(table.old_table.is_some());

        // Fill main_table up to its max load limit: capacity 16, limit = 14 items.
        // It currently contains 1 item ("trigger"). Insert 13 more items.
        for i in 0..13 {
            let k = format!("main-{i}").into_bytes().into_boxed_slice();
            let h = hash64(&k, table.seed);
            table
                .main_table
                .insert_with_hash(h, k, i * 100)
                .expect("insert into main_table succeeds");
        }
        assert!(table.main_table.is_at_max_load());
        assert!(table.old_table.as_ref().map_or(0, |o| o.len) > 0);

        // Overwrite existing key in main_table ("main-5").
        // Background migration fails because main_table is at load limit, but this must not cause
        // the overwrite to fail since overwriting does not increase main_table load.
        let target_key = Box::from(&b"main-5"[..]);
        let res = table.insert(target_key, 9999);
        assert_eq!(res, Ok(Some(500)));
        assert_eq!(table.get(b"main-5"), Some(&9999));

        // Ensure no keys were lost from either table
        for i in 0..7 {
            let k = format!("k-{i}").into_bytes();
            assert_eq!(table.get(&k), Some(&(i * 10)), "key k-{i} lost");
        }
        assert_eq!(table.get(b"trigger"), Some(&99));
        for i in 0..13 {
            let k = format!("main-{i}").into_bytes();
            let expected = if i == 5 { 9999 } else { i * 100 };
            assert_eq!(table.get(&k), Some(&expected), "key main-{i} lost");
        }

        table
            .check_invariants()
            .expect("invariants must hold after overwrite at load limit");
    }

    enum DifferentialOp {
        Insert,
        Remove,
        Get,
    }

    fn pick_phase_op(phase: usize, r: usize) -> DifferentialOp {
        match phase {
            0 => {
                if r < 80 {
                    DifferentialOp::Insert
                } else if r < 90 {
                    DifferentialOp::Remove
                } else {
                    DifferentialOp::Get
                }
            }
            1 => {
                if r < 80 {
                    DifferentialOp::Remove
                } else if r < 90 {
                    DifferentialOp::Insert
                } else {
                    DifferentialOp::Get
                }
            }
            _ => {
                if r < 40 {
                    DifferentialOp::Insert
                } else if r < 70 {
                    DifferentialOp::Remove
                } else {
                    DifferentialOp::Get
                }
            }
        }
    }

    fn verify_differential_maps(
        our_table: &RobinHoodTable<u64>,
        ref_map: &HashMap<&[u8], u64>,
        key_pool: &[Box<[u8]>],
    ) {
        assert_eq!(our_table.len(), ref_map.len());
        for (&k, v) in ref_map {
            assert_eq!(our_table.get(k), Some(v), "missing key in our_table");
        }
        for (k, v) in our_table.iter() {
            assert_eq!(ref_map.get(k), Some(v), "unexpected key in our_table");
        }
        for k in key_pool {
            assert_eq!(
                our_table.get(k),
                ref_map.get(k.as_ref()),
                "key space mismatch"
            );
        }
        our_table
            .check_invariants()
            .expect("invariants must hold at end of test");
    }

    fn run_differential_seed(seed: u64, key_pool: &[Box<[u8]>]) {
        let mut rng = XorShift64::new(seed);
        let mut our_table = RobinHoodTable::new(8, seed);
        let mut ref_map: HashMap<&[u8], u64> = HashMap::new();
        let num_keys = key_pool.len();

        let mut ops_while_resizing = 0usize;
        let mut finished_resizes = 0usize;
        let mut total_op = 0usize;

        for phase in 0..3 {
            for _ in 0..75_000 {
                let was_resizing = our_table.is_resizing();
                let r_raw = rng.next_u64();
                let key_idx = ((r_raw >> 32) % (num_keys as u64)) as usize;
                let r = ((r_raw >> 48) % 100) as usize;
                let key = &key_pool[key_idx];
                let val = r_raw;

                if r < 10 {
                    let step_buckets = 1 + (((r_raw >> 16) % 4) as usize);
                    if our_table.step_migration(step_buckets) == Err(TableFull) {
                        // A failed background migration step is not fatal, the tables stay valid, and the next operation retries.
                    }
                } else {
                    match pick_phase_op(phase, r) {
                        DifferentialOp::Insert => {
                            let ref_res = ref_map.insert(key.as_ref(), val);
                            let our_res = our_table
                                .insert(key.clone(), val)
                                .expect("table must grow dynamically");
                            assert_eq!(our_res, ref_res, "insert mismatch at op {total_op}");
                        }
                        DifferentialOp::Remove => {
                            let ref_res = ref_map.remove(key.as_ref());
                            let our_res = our_table.remove(key.as_ref());
                            assert_eq!(our_res, ref_res, "remove mismatch at op {total_op}");
                        }
                        DifferentialOp::Get => {
                            let ref_res = ref_map.get(key.as_ref());
                            let our_res = our_table.get(key.as_ref());
                            assert_eq!(our_res, ref_res, "get mismatch at op {total_op}");
                        }
                    }
                }

                let is_resizing_now = our_table.is_resizing();
                if was_resizing {
                    ops_while_resizing = ops_while_resizing.saturating_add(1);
                }
                if was_resizing && !is_resizing_now {
                    finished_resizes = finished_resizes.saturating_add(1);
                }

                if total_op < 2_000 || total_op % 200 == 0 {
                    assert_eq!(
                        our_table.len(),
                        ref_map.len(),
                        "len mismatch at op {total_op}"
                    );
                    our_table
                        .check_invariants()
                        .unwrap_or_else(|e| panic!("invariants failed at op {total_op}: {e}"));
                }

                total_op = total_op.saturating_add(1);
            }
        }

        assert!(
            ops_while_resizing >= 1_000,
            "ops while resizing ({ops_while_resizing}) must be at least 1,000"
        );
        assert!(
            finished_resizes >= 10,
            "finished resizes ({finished_resizes}) must be at least 10"
        );

        println!(
            "Seed {seed:#018X}: ops_while_resizing = {ops_while_resizing}, finished_resizes = {finished_resizes}"
        );

        verify_differential_maps(&our_table, &ref_map, key_pool);
    }

    #[test]
    fn test_differential_3_phases_5_seeds() {
        let seeds: [u64; 5] = [
            0x1234_5678_9ABC_DEF0,
            0xCAFE_BABE_0123_4567,
            0xFEED_FACE_DEAD_BEEF,
            0x0123_4567_89AB_CDEF,
            0x9876_5432_10FE_DCBA,
        ];

        let num_keys: usize = 20_000;
        let key_pool: Vec<Box<[u8]>> = (0..num_keys)
            .map(|i| Box::from((i as u64).to_be_bytes()))
            .collect();

        for &seed in &seeds {
            run_differential_seed(seed, &key_pool);
        }
    }

    #[test]
    fn test_stats_100k_keys_70_percent_load_max_psl_small() {
        let capacity = 262_144;
        let mut table = RobinHoodTable::with_default_seed(capacity);
        let num_keys = (capacity as f64 * 0.70) as usize;

        for i in 0..num_keys {
            let key = format!("user:{i}:data").into_bytes().into_boxed_slice();
            table
                .insert(key, i as u64)
                .expect("insert succeeds within 70% load");
        }

        let stats = table.stats();
        println!(
            "\n[RobinHoodTable Stats] len={}, cap={}, load={:.2}%, max_psl={}, avg_psl={:.3}",
            stats.len,
            stats.capacity,
            stats.load * 100.0,
            stats.max_psl,
            stats.average_psl
        );

        assert_eq!(stats.len, num_keys);
        assert_eq!(stats.capacity, capacity);
        assert!(
            stats.max_psl < 64,
            "max_psl was {}, expected < 64",
            stats.max_psl
        );
        assert!(
            stats.average_psl < 5.0,
            "average_psl was {}, expected < 5.0",
            stats.average_psl
        );
    }
}
