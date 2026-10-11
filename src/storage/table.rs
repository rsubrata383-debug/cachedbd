//! Core Robin Hood hash table implementation with backward-shift deletion.
//!
//! # Design and Invariants
//!
//! This table uses open addressing with Robin Hood hashing. Key attributes:
//! - Contiguous storage of slots with power-of-two capacity.
//! - Probe index computed with `hash & mask`.
//! - Slots cache the 64-bit hash, the Probe Sequence Length (PSL), the key, and the value.
//! - Backward-shift deletion without tombstones.
//! - Fixed capacity in Step 1: returns [`TableFull`] when max load ($7/8$) would be exceeded.
//!   Overwrites on existing keys never fail even when full.

use std::error::Error;
use std::fmt;

use crate::storage::hash::{hash64, process_seed};

/// Minimum allowable table capacity (always a power of two).
pub const MIN_CAPACITY: usize = 8;

/// Maximum allowable table capacity ($2^{30} = 1,073,741,824$ slots).
pub const MAX_CAPACITY: usize = 1 << 30;

/// Numerator for maximum load factor limit ($7 / 8 = 87.5\%$).
pub const MAX_LOAD_NUMERATOR: usize = 7;

/// Denominator for maximum load factor limit ($7 / 8 = 87.5\%$).
pub const MAX_LOAD_DENOMINATOR: usize = 8;

/// Error returned when an insertion would exceed the table's maximum load factor limit.
///
/// In Week 6 Step 1, the table has fixed capacity. In Step 2, this limit triggers
/// incremental background table resizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableFull;

impl fmt::Display for TableFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "table load limit reached ({MAX_LOAD_NUMERATOR}/{MAX_LOAD_DENOMINATOR} capacity)"
        )
    }
}

impl Error for TableFull {}

/// Summary operational statistics for a [`RobinHoodTable`].
#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    /// Number of active key-value pairs stored in the table.
    pub len: usize,
    /// Total number of available slots.
    pub capacity: usize,
    /// Current load factor ($0.0 \le \text{load} \le 1.0$).
    pub load: f64,
    /// Maximum Probe Sequence Length among all occupied slots.
    pub max_psl: usize,
    /// Average Probe Sequence Length across all occupied slots.
    pub average_psl: f64,
}

/// An entry residing in a Robin Hood hash table slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot<V> {
    /// Cached 64-bit hash of the key.
    pub hash: u64,
    /// Probe sequence length (distance from the entry's ideal bucket).
    pub psl: u32,
    /// Owned binary key.
    pub key: Box<[u8]>,
    /// Associated value.
    pub value: V,
}

/// In-memory open-addressing hash table using Robin Hood probing.
#[derive(Debug, Clone)]
pub struct RobinHoodTable<V> {
    slots: Box<[Option<Slot<V>>]>,
    capacity: usize,
    len: usize,
    mask: usize,
    seed: u64,
}

impl<V> RobinHoodTable<V> {
    /// Creates a new `RobinHoodTable` with the specified capacity hint and hash seed.
    ///
    /// The capacity is clamped to a minimum of [`MIN_CAPACITY`] (8) and a maximum of
    /// [`MAX_CAPACITY`] ($2^{30}$), rounded up to the nearest power of two.
    #[must_use]
    pub fn new(capacity_hint: usize, seed: u64) -> Self {
        let capacity = if capacity_hint < MIN_CAPACITY {
            MIN_CAPACITY
        } else if capacity_hint >= MAX_CAPACITY {
            MAX_CAPACITY
        } else {
            capacity_hint
                .next_power_of_two()
                .clamp(MIN_CAPACITY, MAX_CAPACITY)
        };

        let mask = capacity.saturating_sub(1);
        let mut slots_vec = Vec::with_capacity(capacity);
        slots_vec.resize_with(capacity, || None);

        Self {
            slots: slots_vec.into_boxed_slice(),
            capacity,
            len: 0,
            mask,
            seed,
        }
    }

    /// Creates a new table using the global per-process random seed.
    #[must_use]
    pub fn with_default_seed(capacity_hint: usize) -> Self {
        Self::new(capacity_hint, process_seed())
    }

    /// Returns the number of key-value pairs stored in the table.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the table contains no key-value pairs.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the total slot capacity of the table.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the hash seed used by this table instance.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Returns the maximum number of items allowed before reaching [`TableFull`].
    #[must_use]
    pub const fn max_load_limit(&self) -> usize {
        (self.capacity / MAX_LOAD_DENOMINATOR) * MAX_LOAD_NUMERATOR
    }

    /// Computes the ideal bucket index for a given 64-bit hash.
    #[inline]
    fn ideal_index(&self, hash: u64) -> usize {
        usize::try_from(hash & (self.mask as u64)).unwrap_or(0)
    }

    /// Returns a reference to the value corresponding to the supplied key and hash.
    #[must_use]
    pub fn get_with_hash(&self, hash: u64, key: &[u8]) -> Option<&V> {
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

    /// Returns a reference to the value corresponding to the supplied key, if present.
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&V> {
        self.get_with_hash(hash64(key, self.seed), key)
    }

    /// Returns a mutable reference to the value corresponding to the supplied key and hash.
    pub fn get_mut_with_hash(&mut self, hash: u64, key: &[u8]) -> Option<&mut V> {
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

    /// Returns a mutable reference to the value corresponding to the supplied key.
    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut V> {
        self.get_mut_with_hash(hash64(key, self.seed), key)
    }

    /// Returns `true` if the table contains an entry for the given key.
    #[must_use]
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    /// Inserts a key-value pair into the table using the table's internal seed.
    ///
    /// If the key already exists, updates the value and returns `Ok(Some(old_value))`.
    /// Overwriting existing keys never fails, even if the table is at capacity.
    ///
    /// If the key is new and the table is at its maximum load limit ($7/8$), returns `Err(TableFull)`.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] if inserting a new key would exceed the $7/8$ load factor limit.
    pub fn insert(&mut self, key: Box<[u8]>, value: V) -> Result<Option<V>, TableFull> {
        let hash = hash64(&key, self.seed);
        self.insert_with_hash(hash, key, value)
    }

    /// Inserts a key-value pair with a precomputed or forced hash value.
    ///
    /// This method is used internally and by tests to force collisions and cluster wraparounds.
    ///
    /// # Errors
    ///
    /// Returns [`TableFull`] if inserting a new key would exceed the $7/8$ load factor limit.
    pub fn insert_with_hash(
        &mut self,
        hash: u64,
        key: Box<[u8]>,
        value: V,
    ) -> Result<Option<V>, TableFull> {
        let ideal = self.ideal_index(hash);

        // Step 1: Probe to check if the key already exists (in-place overwrite).
        let mut psl = 0u32;
        let mut idx = ideal;
        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);

        while psl < max_probes {
            let Some(slot_ref) = self.slots.get_mut(idx) else {
                break;
            };

            match slot_ref.as_mut() {
                None => {
                    // Empty slot encountered; key is definitely absent.
                    break;
                }
                Some(existing) => {
                    if existing.psl < psl {
                        // Existing slot is richer than current probe; key cannot exist.
                        break;
                    }
                    if existing.hash == hash && existing.key.as_ref() == key.as_ref() {
                        // Key exists: overwrite in place without modifying PSL or table length.
                        let old = std::mem::replace(&mut existing.value, value);
                        return Ok(Some(old));
                    }
                }
            }

            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        // Step 2: New insertion. Check capacity limit.
        if self.len >= self.max_load_limit() {
            return Err(TableFull);
        }

        // Step 3: Robin Hood insertion with displacement.
        let mut curr_hash = hash;
        let mut curr_psl = 0u32;
        let mut curr_key = key;
        let mut curr_val = value;
        let mut curr_idx = ideal;

        while curr_psl < max_probes {
            let Some(slot_ref) = self.slots.get_mut(curr_idx) else {
                break;
            };

            if slot_ref.is_none() {
                // Empty slot found: place the element and finish.
                *slot_ref = Some(Slot {
                    hash: curr_hash,
                    psl: curr_psl,
                    key: curr_key,
                    value: curr_val,
                });
                self.len = self.len.saturating_add(1);
                return Ok(None);
            }

            if let Some(existing) = slot_ref.as_mut() {
                if existing.psl < curr_psl {
                    // Displacement: current element is poorer than existing occupant. Swap them.
                    std::mem::swap(&mut existing.hash, &mut curr_hash);
                    std::mem::swap(&mut existing.psl, &mut curr_psl);
                    std::mem::swap(&mut existing.key, &mut curr_key);
                    std::mem::swap(&mut existing.value, &mut curr_val);
                }
            }

            curr_psl = curr_psl.saturating_add(1);
            curr_idx = (curr_idx.wrapping_add(1)) & self.mask;
        }

        // Because len < capacity * 7 / 8, an empty slot is mathematically guaranteed to be reached.
        Err(TableFull)
    }

    /// Removes an entry by key and precomputed hash using backward-shift deletion.
    ///
    /// Returns the removed value if present, or `None` if the key was not found.
    pub fn remove_with_hash(&mut self, hash: u64, key: &[u8]) -> Option<V> {
        let ideal = self.ideal_index(hash);
        let mut psl = 0u32;
        let mut idx = ideal;

        let max_probes = u32::try_from(self.capacity).unwrap_or(u32::MAX);
        let mut target_idx = None;

        while psl < max_probes {
            let slot = self.slots.get(idx).and_then(Option::as_ref)?;
            if slot.psl < psl {
                return None;
            }
            if slot.hash == hash && slot.key.as_ref() == key {
                target_idx = Some(idx);
                break;
            }
            psl = psl.saturating_add(1);
            idx = (idx.wrapping_add(1)) & self.mask;
        }

        let target = target_idx?;
        let removed_value = self
            .slots
            .get_mut(target)
            .and_then(Option::take)
            .map(|s| s.value)?;
        self.len = self.len.saturating_sub(1);

        // Backward-shift deletion: shift subsequent entries backwards until an empty slot
        // or an entry with PSL == 0 is reached.
        let mut curr = target;
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

            // Move the displaced neighbor backward by one position and decrement its PSL.
            let next_slot = self.slots.get_mut(next).and_then(Option::take);
            if let Some(mut s) = next_slot {
                s.psl = s.psl.saturating_sub(1);
                if let Some(curr_slot) = self.slots.get_mut(curr) {
                    *curr_slot = Some(s);
                }
            }
            curr = next;
        }

        Some(removed_value)
    }

    /// Removes an entry by key using backward-shift deletion (no tombstones).
    ///
    /// Returns the removed value if present, or `None` if the key was not found.
    pub fn remove(&mut self, key: &[u8]) -> Option<V> {
        self.remove_with_hash(hash64(key, self.seed), key)
    }

    /// Removes all key-value pairs from the table while retaining allocated capacity.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
        self.len = 0;
    }

    /// Returns an iterator over borrowed key-value pairs in arbitrary table order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &V)> {
        self.slots
            .iter()
            .filter_map(|slot_opt| slot_opt.as_ref().map(|s| (s.key.as_ref(), &s.value)))
    }

    /// Computes and returns detailed operational statistics for the table.
    #[must_use]
    pub fn stats(&self) -> TableStats {
        if self.len == 0 {
            return TableStats {
                len: 0,
                capacity: self.capacity,
                load: 0.0,
                max_psl: 0,
                average_psl: 0.0,
            };
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

        TableStats {
            len: self.len,
            capacity: self.capacity,
            load: (self.len as f64) / (self.capacity as f64),
            max_psl,
            average_psl: (total_psl as f64) / (self.len as f64),
        }
    }

    /// Validates all 5 Robin Hood table invariants.
    ///
    /// # Invariants Checked
    ///
    /// 1. **Accurate PSL**: `psl` of each full slot == distance to its ideal slot with wraparound.
    /// 2. **PSL Gradient**: For neighbouring full slots $i$ and $i+1$, `psl[i+1] <= psl[i] + 1`.
    /// 3. **Cluster Continuity**: No gaps within a displaced cluster; empty slots terminate probes.
    /// 4. **Length Consistency**: `len` matches the exact count of occupied slots.
    /// 5. **Key Uniqueness**: No duplicate keys exist in the table.
    ///
    /// # Errors
    ///
    /// Returns an error message describing the invariant violation if any rule is broken.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut count = 0usize;
        let mut seen_keys = std::collections::HashSet::new();

        for i in 0..self.capacity {
            if let Some(slot) = self.slots.get(i).and_then(Option::as_ref) {
                count = count.saturating_add(1);

                // Invariant 1: PSL matches distance to ideal slot
                let ideal = self.ideal_index(slot.hash);
                let expected_psl = (i.wrapping_add(self.capacity).wrapping_sub(ideal)) & self.mask;
                if slot.psl as usize != expected_psl {
                    return Err(format!(
                        "Invariant 1 violated at slot {i}: psl is {}, expected {expected_psl} (ideal={ideal})",
                        slot.psl
                    ));
                }

                // Invariant 2: Neighbouring full slots PSL gradient
                let next_idx = (i.wrapping_add(1)) & self.mask;
                if let Some(next_slot) = self.slots.get(next_idx).and_then(Option::as_ref) {
                    if next_slot.psl > slot.psl.saturating_add(1) {
                        return Err(format!(
                            "Invariant 2 violated between slots {i} and {next_idx}: psl[{i}] = {}, psl[{next_idx}] = {}",
                            slot.psl, next_slot.psl
                        ));
                    }
                }

                // Invariant 5: No duplicate keys
                if !seen_keys.insert(slot.key.clone()) {
                    return Err(format!("Invariant 5 violated: duplicate key at slot {i}"));
                }
            }
        }

        // Invariant 4: Count matches recorded len
        if self.len != count {
            return Err(format!(
                "Invariant 4 violated: len is {}, but counted {count} occupied slots",
                self.len
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
            Self(if seed == 0 {
                0xCAFE_BABE_DEAD_BEEF
            } else {
                seed
            })
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
        let mask = table.mask as u64;

        // Force hash whose ideal slot is the last slot (slot 7 in capacity 8)
        let hash_last_slot = mask; // 7

        assert_eq!(
            table.insert_with_hash(hash_last_slot, Box::from(&b"wrap1"[..]), 1),
            Ok(None)
        );
        table.check_invariants().expect("invariants after wrap1");

        // Second item with identical hash: should wrap around to slot 0 with PSL 1
        assert_eq!(
            table.insert_with_hash(hash_last_slot, Box::from(&b"wrap2"[..]), 2),
            Ok(None)
        );
        table.check_invariants().expect("invariants after wrap2");

        // Third item with identical hash: wraps around to slot 1 with PSL 2
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

        // Insert item A with ideal slot 0 (hash = 0). It gets slot 0, psl = 0.
        assert_eq!(
            table.insert_with_hash(0, Box::from(&b"A"[..]), 10),
            Ok(None)
        );

        // Insert item B with ideal slot 0 (hash = 0). It probes slot 0 (occupied), gets slot 1, psl = 1.
        assert_eq!(
            table.insert_with_hash(0, Box::from(&b"B"[..]), 20),
            Ok(None)
        );

        // Insert item C with ideal slot 1 (hash = 1).
        // It checks slot 1: occupant B has psl 1, while C has psl 0. B is poorer, C moves to slot 2, psl = 1.
        assert_eq!(
            table.insert_with_hash(1, Box::from(&b"C"[..]), 30),
            Ok(None)
        );

        // Now insert item D with ideal slot 0 (hash = 0).
        // At slot 0: A has psl 0, D has psl 0. D continues with psl = 1.
        // At slot 1: B has psl 1, D has psl 1. D continues with psl = 2.
        // At slot 2: C has psl 1, D has psl 2! D is poorer than C!
        // D displaces C into slot 2, C is bumped forward with psl = 2 into slot 3.
        assert_eq!(
            table.insert_with_hash(0, Box::from(&b"D"[..]), 40),
            Ok(None)
        );

        table.check_invariants().expect("invariants after swap");

        // Verify slot positions and PSLs explicitly:
        // Slot 0: A (psl 0)
        assert_eq!(
            table
                .slots
                .first()
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"A"[..], 0))
        );
        // Slot 1: B (psl 1)
        assert_eq!(
            table
                .slots
                .get(1)
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"B"[..], 1))
        );
        // Slot 2: D displaced C (psl 2)
        assert_eq!(
            table
                .slots
                .get(2)
                .and_then(Option::as_ref)
                .map(|s| (s.key.as_ref(), s.psl)),
            Some((&b"D"[..], 2))
        );
        // Slot 3: C displaced by D (psl 2)
        assert_eq!(
            table
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

        // Create a cluster of 5 items all colliding on ideal slot 2
        for i in 1..=5 {
            let key = format!("clust-{i}").into_bytes().into_boxed_slice();
            assert_eq!(table.insert_with_hash(2, key, i), Ok(None));
        }
        table.check_invariants().expect("cluster built");

        // 1. Remove the first item in the cluster (ideal slot 2)
        assert_eq!(table.remove_with_hash(2, b"clust-1"), Some(1));
        table
            .check_invariants()
            .expect("invariants after removing first");
        for i in 2..=5 {
            let key = format!("clust-{i}").into_bytes();
            assert_eq!(table.get_with_hash(2, &key), Some(&i));
        }

        // 2. Remove middle item (clust-3)
        assert_eq!(table.remove_with_hash(2, b"clust-3"), Some(3));
        table
            .check_invariants()
            .expect("invariants after removing middle");
        assert_eq!(table.get_with_hash(2, b"clust-2"), Some(&2));
        assert_eq!(table.get_with_hash(2, b"clust-4"), Some(&4));
        assert_eq!(table.get_with_hash(2, b"clust-5"), Some(&5));

        // 3. Remove last item (clust-5)
        assert_eq!(table.remove_with_hash(2, b"clust-5"), Some(5));
        table
            .check_invariants()
            .expect("invariants after removing last");
        assert_eq!(table.get_with_hash(2, b"clust-2"), Some(&2));
        assert_eq!(table.get_with_hash(2, b"clust-4"), Some(&4));
    }

    #[test]
    fn test_table_full_at_load_limit_and_overwrites_allowed() {
        // Capacity 8: 7/8 load limit is exactly 7 items
        let mut table = RobinHoodTable::new(8, 1234);
        assert_eq!(table.capacity(), 8);
        assert_eq!(table.max_load_limit(), 7);

        for i in 0..7 {
            let k = format!("k{i}").into_bytes().into_boxed_slice();
            assert_eq!(table.insert(k, i), Ok(None));
        }
        assert_eq!(table.len(), 7);

        // 8th insertion must fail with TableFull
        let k8 = Box::from(&b"k8"[..]);
        assert_eq!(table.insert(k8, 8), Err(TableFull));

        // Reading still works when full
        assert_eq!(table.get(b"k0"), Some(&0));
        assert_eq!(table.get(b"k6"), Some(&6));

        // Overwrites on existing keys must succeed even when table is at load limit
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
    fn test_differential_vs_std_hashmap_20k_ops() {
        let mut rng = XorShift64::new(0x9876_5432_10FE_DCBA);
        let mut our_table = RobinHoodTable::new(2048, 0x1337);
        let mut ref_map = HashMap::new();

        // Small key pool (50 keys) to generate frequent overwrites, removes, and re-insertions
        let key_pool: Vec<Box<[u8]>> = (0..50)
            .map(|i| format!("key-{i}").into_bytes().into_boxed_slice())
            .collect();

        for op in 0..20_000 {
            let key_idx = (rng.next_u64() % 50) as usize;
            let key = &key_pool[key_idx];
            let val = rng.next_u64();

            match rng.next_u64() % 4 {
                0 | 1 => {
                    // Insert / Overwrite
                    let ref_res = ref_map.insert(key.clone(), val);
                    let our_res = our_table
                        .insert(key.clone(), val)
                        .expect("table within capacity");
                    assert_eq!(our_res, ref_res, "insert result mismatch at op {op}");
                }
                2 => {
                    // Remove
                    let ref_res = ref_map.remove(key);
                    let our_res = our_table.remove(key);
                    assert_eq!(our_res, ref_res, "remove result mismatch at op {op}");
                }
                3 => {
                    // Get
                    let ref_res = ref_map.get(key);
                    let our_res = our_table.get(key);
                    assert_eq!(our_res, ref_res, "get result mismatch at op {op}");
                }
                _ => unreachable!(),
            }

            assert_eq!(our_table.len(), ref_map.len(), "length mismatch at op {op}");

            if op % 100 == 0 {
                our_table
                    .check_invariants()
                    .expect("invariants at check point");
            }
        }

        our_table
            .check_invariants()
            .expect("invariants at end of differential test");
    }

    #[test]
    fn test_stats_100k_keys_70_percent_load_max_psl_small() {
        // Table capacity 262,144 (2^18). Insert ~70% = 183,500 keys.
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
