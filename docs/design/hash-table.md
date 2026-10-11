# Hash Table Architecture and Design (Week 6 Step 1)

This document describes the design and invariants of Cachedb's scratch-built in-memory hash table.

---

## 1. Overview and Requirements

Cachedb is an in-memory key-value database. Its primary keyspace table must be fast, memory-compact, deterministic, and predictable under high load.

Key design attributes:
- **Location**: `src/store/` with `mod.rs`, `hash.rs`, `table.rs`.
- **Keys**: `Box<[u8]>`. In Cachedb, keys are arbitrary binary byte buffers. Storing keys as `Box<[u8]>` avoids unnecessary capacity overhead compared to `Vec<u8>`.
- **Values**: Generic `RobinHoodTable<V>`. This allows unit tests and property tests to store lightweight types (e.g. `u64`), while the production engine stores `proto::Value`.
- **Single-Threaded**: Per `docs/design/threading.md`, the table is owned strictly by a single thread (the shard owner). It contains zero locks, zero mutexes, and zero atomic synchronization primitives.

---

## 2. Table Layout and Open Addressing

The table uses **open addressing** with **Robin Hood hashing**:
- **Array of Slots**: Stored as a contiguous buffer of slots.
- **Power-of-Two Capacity**: Table capacity $C$ is always a power of two ($C = 2^k$, minimum 8). The initial bucket index for a hash is computed with a fast bitwise mask:
  $$\text{ideal\_slot} = \text{hash} \ \& \ (C - 1)$$
- **Slot Structure**: Each slot is either empty or occupied. An occupied slot stores:
  - `hash`: 64-bit integer (`u64`). Cached in the slot to enable fast candidate rejection without string comparisons and to eliminate re-hashing during table resizing.
  - `psl`: 32-bit integer (`u32`). Probe Sequence Length: the distance in slots from the entry's ideal bucket to its current location.
  - `key`: `Box<[u8]>`.
  - `value`: `V`.

---

## 3. Robin Hood Probing and Displacement

Robin Hood hashing equalizes probe sequence lengths across all keys ("take from the rich, give to the poor"):
- When inserting a new element with probe distance `curr_psl`, we probe forward linearly with wraparound.
- If we encounter an empty slot, we place the element there and increment `len`.
- If we encounter an occupied slot with an identical hash and matching key bytes, we overwrite the value and return the old value (no displacement, no length change).
- If the probed slot is occupied and has `slot_psl < curr_psl`, the new element is "poorer" than the existing occupant. We **swap** the elements: the poorer element takes the slot, and we continue inserting the displaced element forward with its new PSL.
- This displacement invariant keeps the variance of PSLs extremely low, eliminating the long search chains typical of standard linear probing.

---

## 4. Deletion: Backward-Shift (No Tombstones)

Tombstone markers degrade lookup performance and require periodic cleanup passes. Instead, we use **backward-shift deletion**:
- When a key is removed, its slot becomes empty.
- We then inspect successive slots in probing order:
  - If a neighboring slot has `psl > 0`, it was displaced from an earlier bucket. We shift it backward by one slot and decrement its `psl` by 1.
  - We repeat this process until we encounter either an empty slot or a slot with `psl == 0` (an element already sitting in its ideal slot).
- Backward-shift deletion maintains all clustering invariants cleanly and eliminates tombstone accumulation.

---

## 5. Capacity and Load Factor

- **Max Load Factor**: `7 / 8` (87.5%).
  - *Rationale*: With Robin Hood hashing, variance in probe sequence lengths is remarkably small even at 87.5% load, yielding average PSLs typically between 1 and 3. An 87.5% threshold saves significant memory compared to 50% or 75% thresholds while avoiding clustering cliffs.
- **Step 1 Fixed Capacity**: In Week 6 Step 1, the table has a fixed capacity. When an insertion would cause `len + 1 > capacity * 7 / 8`, the insert fails with `Err(TableFull)`. Overwriting an existing key never increases load and therefore never fails with `TableFull`.
- **Step 2 Evolution**: In Step 2, reaching this load threshold triggers incremental background resizing across dual tables.

---

## 6. Table Invariants

The implementation enforces and validates five core invariants (`check_invariants()`):
1. **Accurate PSL**: For every occupied slot $i$, its `psl` equals the circular distance from its ideal slot:
   $$\text{psl} = (i + C - (\text{hash} \ \& \ (C - 1))) \bmod C$$
2. **PSL Gradient (Cluster Invariant)**: For neighboring occupied slots $i$ and $(i + 1) \bmod C$, the next slot's PSL cannot exceed the previous slot's PSL plus one:
   $$\text{psl}[(i + 1) \bmod C] \le \text{psl}[i] + 1$$
3. **Empty Slot Cluster Termination**: An empty slot terminates probe chains; there are no disconnected clusters.
4. **Length Consistency**: `len` equals the exact count of occupied slots.
5. **No Duplicate Keys**: No two occupied slots contain identical key byte sequences.

---

## 7. Hash Function (`hash.rs`)

- **Design**: Fast non-cryptographic 64-bit hash function based on Wyhash / Wang Yi's multiply-and-fold (`mum`) algorithm. It uses 128-bit arithmetic (`u128`) to fold 64-bit chunks with high entropy.
- **Seeded**: Accepts a 64-bit seed. A global per-process random seed is initialized via `std::hash::RandomState::new().hash_one(...)` using `std::sync::OnceLock`. Custom fixed seeds are supported for deterministic tests and benchmarks.
- **HashDoS Notice**: This is a non-cryptographic hash designed for high-throughput memory hashing. It is protected against accidental collision clustering via per-process random seeding, but is not cryptographic.

