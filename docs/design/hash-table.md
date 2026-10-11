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
  - *Empirical PSL at 1,000,000 Keys*: Measured with `store_bench`, the table achieved a **maximum PSL of 8** and an **average PSL of 0.44** probes per key, confirming near-optimal probe efficiency where the overwhelming majority of keys reside in their ideal hash bucket.
- **Incremental Resizing**: When an insertion causes `main_table.len + 1 > main_table.capacity * 7 / 8`, the table initiates incremental resizing by doubling capacity.
- **Max Capacity**: $2^{30}$ slots (~1.07 billion buckets). If the table is already at maximum capacity and cannot double, insertions that exceed the 87.5% threshold return `Err(TableFull)`. Overwriting an existing key never increases load and always succeeds.

---

## 6. Table Invariants

The implementation enforces and validates core invariants (`check_invariants()`):
1. **Accurate PSL**: For every occupied slot $i$, its `psl` equals the circular distance from its ideal slot:
   $$\text{psl} = (i + C - (\text{hash} \ \& \ (C - 1))) \bmod C$$
2. **PSL Gradient (Cluster Invariant)**: For neighboring occupied slots $i$ and $(i + 1) \bmod C$, the next slot's PSL cannot exceed the previous slot's PSL plus one:
   $$\text{psl}[(i + 1) \bmod C] \le \text{psl}[i] + 1$$
3. **Empty Slot Cluster Termination**: An empty slot terminates probe chains. For any slot with `psl > 0`, the immediately preceding slot `(i + C - 1) % C` must be occupied.
4. **Length Consistency**: `len` equals the exact count of occupied slots across `main_table` and `old_table`.
5. **No Duplicate Keys**: No key byte sequence exists in both `main_table` and `old_table`, nor in multiple slots within either table.
6. **Migrated Region Emptiness**: When resizing is active, all slots $0 \le i < \text{migrate\_cursor}$ in `old_table` are guaranteed empty (`None`).

---

## 7. Hash Function (`hash.rs`)

- **Design**: Fast non-cryptographic 64-bit hash function based on Wyhash / Wang Yi's multiply-and-fold (`mum`) algorithm. It uses 128-bit arithmetic (`u128`) to fold 64-bit chunks with high entropy.
- **Seeded**: Accepts a 64-bit seed. A global per-process random seed is initialized via `std::hash::RandomState::new().hash_one(...)` using `std::sync::OnceLock`. Both `main_table` and `old_table` always share the exact same seed.
- **HashDoS Notice**: This is a non-cryptographic hash designed for high-throughput memory hashing. It is protected against accidental collision clustering via per-process random seeding, but is not cryptographic.

---

## 8. Incremental Resizing Architecture (Step 2)

### 8.1 Why Incremental Resizing?
In an in-memory database targeting a strict $< 1\text{ ms}$ p99 latency SLA and 200k ops/sec throughput, stop-the-world reallocation is unacceptable. Reallocating and copying a hash table with 10M keys can pause execution for 50–200 ms. Incremental resizing spreads key copying work over mutating operations, bounding latency.

However, while key migration is spread out incrementally across mutating operations (16 buckets at a time), allocating and zero-initializing the new backing table is $O(\text{new capacity})$ at the moment a resize is initiated. Empirical measurements for `RobinHoodTable::new` creation latency on Linux CI (`ubuntu-24.04`):
- **$2^{20}$ slots (~42 MB)**: `1.09 ms` (1M run) / `2.26 ms` (10M run)
- **$2^{22}$ slots (~168 MB)**: `8.53 ms` (1M run) / `13.13 ms` (10M run)
- **$2^{24}$ slots (~671 MB)**: `32.79 ms` (1M run) / `59.05 ms` (10M run)

While this allocation cost causes a transient latency spike at power-of-two boundaries ($p99.99$ tail latency), it avoids copying all keys at once, reducing worst-case latency spikes by **23x to 30x** compared to standard library rehashing.

### 8.2 Dual-Table Structure
- `main_table: SingleTable<V>`: The primary active table receiving new insertions.
- `old_table: Option<SingleTable<V>>`: The previous table being drained during an active resize.
- `migrate_cursor: usize`: Index of the next bucket in `old_table` to inspect and migrate.
- `MIGRATE_BATCH = 16`: Number of `old_table` buckets migrated on mutating operations.

### 8.3 Resizing Lifecycle
1. **Trigger**: When `main_table.len + 1 > main_table.capacity * 7 / 8` on an insert:
   - If a resize is already running, the table finishes migrating all remaining buckets from `old_table` first before beginning the next resize.
   - If `main_table.capacity >= max_capacity` ($2^{30}$), insertion fails with `Err(TableFull)`.
   - Otherwise, `main_table` is moved to `old_table`, a new `main_table` with $2\times$ capacity is allocated using the exact same seed, and `migrate_cursor` is reset to 0.
2. **Paced Incremental Migration**:
   - Mutating operations (`insert`, `remove`, `get_mut`) and periodic event-loop calls (`step_migration`) migrate up to `MIGRATE_BATCH = 16` buckets from `old_table`.
   - Read-only lookups (`get(&self)`) **do not migrate** and remain cheap and shareable.
   - **Hole-Free Migration via Backward-Shift**: If `old_table[migrate_cursor]` is empty, the cursor advances. If it is occupied, it is removed using backward-shift deletion (`remove_at_and_shift`). Removal only fills the hole at `migrate_cursor`, and every slot before `migrate_cursor` is empty, so shifted items never go below `migrate_cursor`. No holes are created in `old_table`, ensuring lookups in `old_table` remain fully valid at every step.
   - **Key Preservation**: If inserting into `main_table` returns an error, the item is restored into `old_table` so keys are never lost.
   - Keys are **never re-hashed** during migration; cached 64-bit hashes are directly transferred.
3. **Completion**:
   - When `old_table.len == 0`, `old_table` is deallocated (`None`) and `migrate_cursor` is reset to 0.
   - Resizing can also be stepped manually or finished in one shot via `table.step_migration(usize::MAX)`.

### 8.4 Operation Rules
- **Read Lookup (`get(&self)`)**: Read-only (`&self`). Probes `main_table` first, then `old_table` (if resizing). Touches 0 migration buckets.
- **Mutable Lookup (`get_mut(&mut self)`)**: Migrates up to 16 buckets. If present in `old_table`, removes and transfers to `main_table`.
- **Insert (`insert`)**: Migrates up to 16 buckets. Always inserts into `main_table`. If present in `old_table`, removes from `old_table` so keys are never duplicated.
- **Remove (`remove`)**: Migrates up to 16 buckets. Removes from `main_table` and `old_table`, returning the removed value if found.
- **Length (`len()`)**: Exactly `main_table.len + old_table.len`.
- **Worst-Case Cost**:
  - `get`: 0 migration buckets touched. Probes $\approx 1-2$ slots in `main_table` (and `old_table` on miss).
  - `insert` / `remove` / `get_mut`: At most $16$ buckets touched in `old_table` plus $1$ Robin Hood operation in `main_table`. Every operation remains strictly $\mathcal{O}(1)$ and runs in sub-microsecond time.

---

## 9. Benchmark Profile and Latency Spikes (Week 6 Step 3c)

Empirical performance measured on Linux CI runners (`ubuntu-24.04`, x86_64) across 1,000,000 and 10,000,000 keys:

### 9.1 Insert Latency Distribution
- **At 1,000,000 keys**:
  - **Median ($p50$)**: `0.120 µs` (120 ns) vs `0.040 µs` in `std::HashMap`
  - **99th percentile ($p99$)**: `0.611 µs` vs `0.390 µs` in `std::HashMap`
  - **99.9th percentile ($p99.9$)**: `1.062 µs` vs `0.541 µs` in `std::HashMap`
  - **99.99th percentile ($p99.99$)**: `7.321 µs` vs `1.333 µs` in `std::HashMap`
  - **Maximum latency ($max$)**: `4,460.195 µs` (~4.5 ms) vs `134,747.812 µs` (~134.7 ms) in `std::HashMap` (**30x lower maximum pause**)
- **At 10,000,000 keys**:
  - **Median ($p50$)**: `0.210 µs` vs `0.210 µs` in `std::HashMap`
  - **99th percentile ($p99$)**: `0.931 µs` vs `0.661 µs` in `std::HashMap`
  - **99.9th percentile ($p99.9$)**: `1.854 µs` vs `0.942 µs` in `std::HashMap`
  - **99.99th percentile ($p99.99$)**: `11.612 µs` vs `11.050 µs` in `std::HashMap`
  - **Maximum latency ($max$)**: `50,234.631 µs` (~50.2 ms) vs `1,174,178.501 µs` (~1,174.2 ms) in `std::HashMap` (**23x lower maximum pause**)

### 9.2 Measured Root Causes of Latency Spikes
By measuring `capacity()` and resizing states before and after every operation, the top slowest inserts showed:
1. **Resize Started (Buffer Allocation)**:
   - Op #7,340,032 (`50.23 ms`): capacity changed `8388608 -> 16777216` (~671 MB zero-fill allocation).
   - Op #3,670,016 (`24.70 ms`): capacity changed `4194304 -> 8388608` (~335 MB zero-fill allocation).
   - In `std::HashMap`, the equivalent op #7,340,032 took `1,174.18 ms` (1.17 s) because it had to synchronously reallocate, rehash, and copy 7.3M keys. In `RobinHoodTable`, incremental migration avoids stop-the-world copying, reducing pause by **23x**.
2. **Migration Finished (Old Table Deallocation)**:
   - Op #8,323,072 (`15.56 ms`): capacity unchanged (`16777216`), tag `migration finished, old table dropped`. When migration completes and `old_table` reaches 0 elements, dropping the old 8M-slot table deallocates ~335 MB of memory.
   - Op #4,161,536 (`8.58 ms`): dropping the 4M-slot old table (~168 MB) upon migration completion.
   - Measured drop latency of empty tables on Linux confirms this: $2^{20}$ slots = 1.09 ms, $2^{22}$ slots = 8.53 ms, $2^{24}$ slots = 32.79 ms.
3. **Steady-State Operations**:
   - Zero operations during normal incremental migration without allocation or deallocation exceeded 1 ms.
   - Average PSL remains small (0.46 at 1M keys, 0.74 at 10M keys), ensuring steady-state operations remain sub-microsecond.
4. **Get-Miss Latency & Tag Arrays**:
   - Get misses are slower than `std::HashMap` (129.2 ns vs 22.9 ns at 1M, 150.6 ns vs 101.1 ns at 10M). The likely reason is that `std::HashMap` (Swiss Table) checks a compact 1-byte control tag array via SIMD within a single 64-byte cache line fetch, whereas `RobinHoodTable` inspects 40-byte slots across memory. A separate compact hash/tag array is a candidate optimization.
