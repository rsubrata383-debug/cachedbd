# In-Memory Store Benchmarks

Comparative benchmark suite evaluating [`cachedb::store::RobinHoodTable`] against [`std::collections::HashMap`] under synthetic key-value workloads.

Benchmark executed via `cargo bench --bench store_bench -- --keys 1000000`:
- **Keys**: 1,000,000
- **Mixed Operations**: 1,000,000 (80% get hits, 15% insert, 5% remove)
- **Environment**: x86_64, Windows (Release profile, LTO, unoptimized debug flags stripped)
- **Methodology**:
  - **Zero Allocations Inside Timed Regions**: All keys (`keys_insert`, `keys_lookup`, `miss_keys`), values, and random permutation index lists are pre-allocated and constructed before timers start.
  - **Repetitions & Warmup**: Every phase executes 1 warmup iteration followed by 3 measured runs; reports the median duration with minimum and maximum bounds.
  - **Baseline Floors**: Measures loop overhead and raw hash function execution to isolate the true cost of table algorithms.

---

## 1. Throughput & Performance Comparison (Median of 3 Runs)

| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Floor: loop + key access only | 2.2 ns [1.6 - 4.6] | 454.44 Mops/s | 2.2 ns [1.6 - 4.6] | 454.44 Mops/s | **1.00x** |
| Floor: hash only (hash64 / RandomState) | 21.4 ns [20.6 - 22.8] | 46.65 Mops/s | 25.2 ns [24.7 - 25.5] | 39.72 Mops/s | **1.17x** |
| Insert (1M keys from empty) | 404.8 ns [386.3 - 670.3] | 2.47 Mops/s | 456.9 ns [418.5 - 536.8] | 2.19 Mops/s | **1.13x** |
| Get hits (random order) | 280.5 ns [276.1 - 309.3] | 3.57 Mops/s | 1291.8 ns [480.4 - 1292.1] | 0.77 Mops/s | **4.61x** |
| Get misses | 150.7 ns [149.8 - 151.3] | 6.63 Mops/s | 186.2 ns [156.3 - 194.4] | 5.37 Mops/s | **1.24x** |
| Overwrite (all keys) | 246.3 ns [206.5 - 481.5] | 4.06 Mops/s | 307.6 ns [267.7 - 364.8] | 3.25 Mops/s | **1.25x** |
| Mixed (80% get / 15% ins / 5% rem) | 372.9 ns [365.0 - 380.7] | 2.68 Mops/s | 490.0 ns [448.3 - 501.7] | 2.04 Mops/s | **1.31x** |
| Remove (all keys) | 281.0 ns [279.5 - 281.3] | 3.56 Mops/s | 274.3 ns [253.8 - 489.4] | 3.65 Mops/s | **0.98x** |

---

## 2. Latency Profiles: RobinHoodTable vs std::collections::HashMap

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| p50 | 0.300 µs | 0.200 µs |
| p99 | 1.500 µs | 0.600 µs |
| p99.9 | 3.100 µs | 1.000 µs |
| p99.99 | 22.100 µs | 13.600 µs |
| max | 24390.300 µs (~24.4 ms) | 147570.800 µs (~147.6 ms) |
| Ops slower than 1 ms | 8 | 6 |

---

## 3. Top 5 Slowest Inserts

### RobinHoodTable
| Rank | Op Index | Latency (µs) | Capacity Change Measured |
| :---: | :---: | :---: | :--- |
| 1 | 917504 | 24390.30 µs (24.4 ms) | capacity changed: 1048576 -> 2097152 |
| 2 | 458752 | 12242.60 µs (12.2 ms) | capacity changed: 524288 -> 1048576 |
| 3 | 229376 | 6969.40 µs (7.0 ms) | capacity changed: 262144 -> 524288 |
| 4 | 520192 | 4430.10 µs (4.4 ms) | capacity unchanged: 1048576 |
| 5 | 260096 | 2883.80 µs (2.9 ms) | capacity unchanged: 524288 |

### std::collections::HashMap
| Rank | Op Index | Latency (µs) | Event Description |
| :---: | :---: | :---: | :--- |
| 1 | 917504 | 147570.80 µs (147.6 ms) | std::HashMap full stop-the-world reallocation copy |
| 2 | 458752 | 68358.60 µs (68.4 ms) | std::HashMap full stop-the-world reallocation copy |
| 3 | 229376 | 33026.80 µs (33.0 ms) | std::HashMap full stop-the-world reallocation copy |
| 4 | 114688 | 14309.10 µs (14.3 ms) | std::HashMap full stop-the-world reallocation copy |
| 5 | 57344 | 6034.50 µs (6.0 ms) | std::HashMap full stop-the-world reallocation copy |

> **Key Observation**: While both tables experience latency spikes at power-of-two resize boundaries due to memory allocator zero-fill initialization, `std::HashMap`'s worst-case spike is **6x higher** (147.6 ms vs 24.4 ms) because it reallocates and synchronously rehashes/copies all existing elements, whereas `RobinHoodTable` merely allocates the new buffer and migrates keys incrementally over subsequent operations.

---

## 4. Resize-Start Allocation Cost (`RobinHoodTable::new`)

Measured creation latency for initializing an empty backing table buffer across capacities (median of 3 runs):

| Table Capacity | Allocation Size | Creation Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots) | ~42 MB | 14.22 ms |
| 2^22 (4,194,304 slots) | ~168 MB | 71.11 ms |
| 2^24 (16,777,216 slots) | ~671 MB | 340.56 ms |

---

## 5. Hash Table Quality & Memory Metrics

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| Max PSL | 9 | n/a |
| Average PSL | 0.46 | n/a |
| Measured Average Probes per Miss | 1.70 slots | n/a |
| Final Load Factor | 47.7% | n/a |
| Table Slot Size | 40 bytes | n/a |
| Est. Memory / Key | 83.9 bytes | n/a |
| Process VmRSS | n/a (Windows) | n/a (Windows) |

> Note: Process `VmRSS` is read dynamically from `/proc/self/status` on Linux runners and reports `"n/a"` on Windows.

---

## 6. Investigation: Get-Miss Latency Analysis

Under the refined methodology with pre-allocated keys and warmup runs:
- **Status**: **Now fine**. At 1,000,000 keys, `RobinHoodTable` get-miss latency is **150.7 ns** compared to `std::HashMap`'s **186.2 ns** (**1.24x faster**).
- **Measurement Breakdown**:
  1. **Raw Hashing Cost**: `hash64` accounts for **21.4 ns** (measured via floor hash).
  2. **Probe Count**: The measured average number of slots inspected per miss is **1.70 slots** across the 1M key table.
  3. **Memory Subsystem & Cache Alignment**: Each slot is 40 bytes (`Option<Slot<u64>>`). Because 40 bytes does not divide the 64-byte CPU cache line evenly, approximately 50% of slot reads span two adjacent cache lines. In a 2,097,152 slot table (~83.9 MB, exceeding L3 cache capacity), probing 1.70 slots triggers approximately 2 to 3 main memory DRAM accesses. At ~50 ns per random DRAM access, this accounts for the remaining ~127 ns of lookup miss time.
  4. In previous un-warmed runs, get-miss appeared artificially high (537 ns) due to cold OS page faults and measuring key cloning inside the timed loop. With proper warmup and zero-allocation timed regions, miss performance matches expected hardware cache bounds.
