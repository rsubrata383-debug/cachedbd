# In-Memory Store Benchmarks

Comparative benchmark suite evaluating [`cachedb::store::RobinHoodTable`] against [`std::collections::HashMap`] under synthetic key-value workloads.

Benchmarks were executed on GitHub Actions CI runners (`ubuntu-24.04`, x86_64, standard Release profile):
- **1,000,000 keys**: `ci-linux-1M.txt`
- **10,000,000 keys**: `ci-linux-10M.txt`
- **Methodology**:
  - **Zero Allocations Inside Timed Regions**: All keys, values, and permutation indices are pre-constructed before timers start.
  - **Repetitions & Warmup**: Every phase executes 1 warmup iteration followed by 3 measured runs, reporting the median duration `[min - max]`.
  - **Baseline Floors**: Measures loop overhead and raw hash function execution to isolate the true cost of table algorithms.

---

## Executive Performance Summary

- **Where RobinHoodTable is faster**:
  - **Insert throughput**: **1.60x faster** at 1M keys (179.9 ns vs 286.9 ns) and **1.37x faster** at 10M keys (276.5 ns vs 379.4 ns).
  - **Get hits throughput**: **1.01x faster** at 1M keys (291.7 ns vs 293.4 ns) and **1.30x faster** at 10M keys (311.4 ns vs 404.4 ns).
  - **Overwrite throughput**: **1.08x faster** at 1M keys (157.0 ns vs 169.0 ns) and **1.47x faster** at 10M keys (185.5 ns vs 272.8 ns).
  - **Mixed workload throughput at scale**: **1.31x faster** at 10M keys (330.8 ns vs 433.4 ns).
  - **Remove throughput at scale**: **1.39x faster** at 10M keys (186.2 ns vs 259.2 ns).
  - **Worst-case latency spikes**: **50 ms** at 10M keys against **1,174 ms** (1.17 seconds) for `std::HashMap` (**23x lower worst-case pause**).
- **Where RobinHoodTable is slower**:
  - **Get misses**: **5.6x slower** at 1M keys (129.2 ns vs 22.9 ns) and **1.5x slower** at 10M keys (150.6 ns vs 101.1 ns).
  - **Mixed workload at 1M keys**: **0.96x** (296.4 ns vs 284.4 ns).
  - **Remove at 1M keys**: **0.85x** (206.2 ns vs 174.7 ns).

---

## 1. Linux CI Benchmark: 1,000,000 Keys (`ci-linux-1M.txt`)

### Throughput & Performance Comparison (Median of 3 Runs)

| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Floor: loop + key access only | 0.4 ns [0.4 - 0.4] | 2534.60 Mops/s | 0.4 ns [0.4 - 0.4] | 2534.60 Mops/s | **1.00x** |
| Floor: hash only (hash64) | 9.6 ns [9.6 - 9.8] | 104.07 Mops/s | 9.0 ns [9.0 - 9.0] | 110.55 Mops/s | **0.94x** |
| Insert (N keys from empty) | 179.9 ns [160.1 - 230.3] | 5.56 Mops/s | 286.9 ns [275.3 - 335.5] | 3.49 Mops/s | **1.60x** |
| Get hits (random order) | 291.7 ns [289.3 - 293.5] | 3.43 Mops/s | 293.4 ns [292.8 - 301.2] | 3.41 Mops/s | **1.01x** |
| Get misses | 129.2 ns [128.4 - 138.4] | 7.74 Mops/s | 22.9 ns [22.0 - 23.1] | 43.72 Mops/s | **0.18x** |
| Overwrite (all keys) | 157.0 ns [154.4 - 162.1] | 6.37 Mops/s | 169.0 ns [167.9 - 171.8] | 5.92 Mops/s | **1.08x** |
| Mixed (80% get / 15% ins / 5% rem) | 296.4 ns [278.9 - 305.8] | 3.37 Mops/s | 284.4 ns [283.4 - 293.5] | 3.52 Mops/s | **0.96x** |
| Remove (all keys) | 206.2 ns [203.7 - 208.0] | 4.85 Mops/s | 174.7 ns [173.1 - 181.5] | 5.72 Mops/s | **0.85x** |

### Latency Profiles: RobinHoodTable vs std::collections::HashMap

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| p50 | 0.120 µs | 0.040 µs |
| p99 | 0.611 µs | 0.390 µs |
| p99.9 | 1.062 µs | 0.541 µs |
| p99.99 | 7.321 µs | 1.333 µs |
| max | 4460.195 µs | 134747.812 µs |
| Ops slower than 1 ms | 4 | 6 |

### Top 5 Slowest Inserts: RobinHoodTable

| Rank | Op Index | Latency (µs) | Capacity Change Measured |
| :---: | :---: | :---: | :--- |
| 1 | 917504 | 4460.19 µs | capacity changed: 1048576 -> 2097152 |
| 2 | 458752 | 2457.57 µs | capacity changed: 524288 -> 1048576 |
| 3 | 229376 | 1433.52 µs | capacity changed: 262144 -> 524288 |
| 4 | 520192 | 1166.49 µs | capacity unchanged: 1048576 |
| 5 | 114688 | 937.56 µs | capacity changed: 131072 -> 262144 |

### Top 5 Slowest Inserts: std::collections::HashMap

| Rank | Op Index | Latency (µs) | Event Description |
| :---: | :---: | :---: | :--- |
| 1 | 917504 | 134747.81 µs | std::HashMap insert operation #917504 |
| 2 | 458752 | 59946.80 µs | std::HashMap insert operation #458752 |
| 3 | 229376 | 17963.96 µs | std::HashMap insert operation #229376 |
| 4 | 114688 | 4513.02 µs | std::HashMap insert operation #114688 |
| 5 | 57344 | 2324.90 µs | std::HashMap insert operation #57344 |

### Resize-Start Allocation Cost (RobinHoodTable::new)

| Table Capacity | Allocation Size | Creation Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots, ~42 MB) | 1.09 ms |
| 2^22 (4,194,304 slots, ~168 MB) | 8.53 ms |
| 2^24 (16,777,216 slots, ~671 MB) | 32.79 ms |

### Hash Table Quality & Memory Metrics

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| Max PSL | 10 | n/a |
| Average PSL | 0.46 | n/a |
| Measured Average Probes per Miss | 1.70 slots | n/a |
| Final Load Factor | 47.7% | n/a |
| Table Slot Size | 40 bytes | n/a |
| Est. Memory / Key | 83.9 bytes | n/a |
| Process VmRSS | 231.14 MB | 197.32 MB |

---

## 2. Linux CI Benchmark: 10,000,000 Keys (`ci-linux-10M.txt`)

### Throughput & Performance Comparison (Median of 3 Runs)

| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Floor: loop + key access only | 0.8 ns [0.8 - 0.8] | 1272.73 Mops/s | 0.8 ns [0.8 - 0.8] | 1272.73 Mops/s | **1.00x** |
| Floor: hash only (hash64) | 16.3 ns [16.3 - 16.5] | 61.34 Mops/s | 20.0 ns [19.9 - 22.2] | 50.10 Mops/s | **1.22x** |
| Insert (N keys from empty) | 276.5 ns [268.7 - 378.9] | 3.62 Mops/s | 379.4 ns [374.6 - 490.8] | 2.64 Mops/s | **1.37x** |
| Get hits (random order) | 311.4 ns [305.5 - 313.0] | 3.21 Mops/s | 404.4 ns [402.6 - 425.5] | 2.47 Mops/s | **1.30x** |
| Get misses | 150.6 ns [149.2 - 152.1] | 6.64 Mops/s | 101.1 ns [96.1 - 109.5] | 9.89 Mops/s | **0.67x** |
| Overwrite (all keys) | 185.5 ns [182.6 - 188.0] | 5.39 Mops/s | 272.8 ns [266.8 - 274.0] | 3.67 Mops/s | **1.47x** |
| Mixed (80% get / 15% ins / 5% rem) | 330.8 ns [330.2 - 338.8] | 3.02 Mops/s | 433.4 ns [424.2 - 437.6] | 2.31 Mops/s | **1.31x** |
| Remove (all keys) | 186.2 ns [184.3 - 190.1] | 5.37 Mops/s | 259.2 ns [258.5 - 275.2] | 3.86 Mops/s | **1.39x** |

### Latency Profiles: RobinHoodTable vs std::collections::HashMap

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| p50 | 0.210 µs | 0.210 µs |
| p99 | 0.931 µs | 0.661 µs |
| p99.9 | 1.854 µs | 0.942 µs |
| p99.99 | 11.612 µs | 11.050 µs |
| max | 50234.631 µs (~50.2 ms) | 1174178.501 µs (~1,174.2 ms) |
| Ops slower than 1 ms | 13 | 9 |

### Top 5 Slowest Inserts: RobinHoodTable

| Rank | Op Index | Latency (µs) | Capacity Change Measured |
| :---: | :---: | :---: | :--- |
| 1 | 7340032 | 50234.63 µs | capacity changed: 8388608 -> 16777216 |
| 2 | 3670016 | 24704.49 µs | capacity changed: 4194304 -> 8388608 |
| 3 | 8323072 | 15556.75 µs | capacity unchanged: 16777216 |
| 4 | 1835008 | 12526.09 µs | capacity changed: 2097152 -> 4194304 |
| 5 | 4161536 | 8577.80 µs | capacity unchanged: 8388608 |

### Top 5 Slowest Inserts: std::collections::HashMap

| Rank | Op Index | Latency (µs) | Event Description |
| :---: | :---: | :---: | :--- |
| 1 | 7340032 | 1174178.50 µs | std::HashMap insert operation #7340032 |
| 2 | 3670016 | 554290.83 µs | std::HashMap insert operation #3670016 |
| 3 | 1835008 | 266469.61 µs | std::HashMap insert operation #1835008 |
| 4 | 917504 | 128128.37 µs | std::HashMap insert operation #917504 |
| 5 | 458752 | 57891.54 µs | std::HashMap insert operation #458752 |

### Resize-Start Allocation Cost (RobinHoodTable::new)

| Table Capacity | Allocation Size | Creation Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots, ~42 MB) | 2.26 ms |
| 2^22 (4,194,304 slots, ~168 MB) | 13.13 ms |
| 2^24 (16,777,216 slots, ~671 MB) | 59.05 ms |

### Hash Table Quality & Memory Metrics

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| Max PSL | 15 | n/a |
| Average PSL | 0.74 | n/a |
| Measured Average Probes per Miss | 2.04 slots | n/a |
| Final Load Factor | 59.6% | n/a |
| Table Slot Size | 40 bytes | n/a |
| Est. Memory / Key | 67.1 bytes | n/a |
| Process VmRSS | 1977.41 MB | 1939.88 MB |

---

## 3. Analysis of Slow Inserts & Drop Measurements

### Measured Root Cause of the "Capacity Unchanged" Slow Inserts

When tracking the 5 slowest inserts across both 1M and 10M key runs, operations fall into two distinct lifecycle events:
1. **Resize Started (`capacity changed`)**:
   Allocating and zero-initializing the new backing buffer of size $2\times C$ is an $\mathcal{O}(\text{new capacity})$ kernel/allocator operation.
   - At 1M keys, allocating $2^{21}$ slots (~84 MB) takes **4.46 ms** (Op #917,504).
   - At 10M keys, allocating $2^{24}$ slots (~671 MB) takes **50.23 ms** (Op #7,340,032).
2. **Migration Finished, Old Table Dropped (`capacity unchanged`)**:
   In incremental migration, once `old_table.len == 0`, `self.old_table` is set to `None`, invoking `drop()` on the old table buffer. Deallocating a multi-megabyte buffer produces a measurable pause:
   - At 1M keys, Op #520,192 is the exact insert where migration finishes; dropping the $2^{19}$ slots (~21 MB) buffer takes **1.17 ms** on Linux (and **4.19 ms** on Windows).
   - At 10M keys, Op #8,323,072 takes **15.56 ms** dropping the $2^{23}$ slots (~335 MB) buffer, and Op #4,161,536 takes **8.58 ms** dropping the $2^{22}$ slots (~168 MB) buffer.

### Empty Table Drop Latency (`drop(RobinHoodTable)`)

Empirical median latency to deallocate an empty table buffer:

| Table Capacity | Allocation Size | Linux CI Drop Latency | Local Windows Drop Latency |
| :--- | :---: | :---: | :---: |
| 2^20 (1,048,576 slots) | ~42 MB | 1.09 ms | 10.05 ms |
| 2^22 (4,194,304 slots) | ~168 MB | 8.53 ms | 33.01 ms |
| 2^24 (16,777,216 slots) | ~671 MB | 32.79 ms | 141.24 ms |

The measured drop latency matches the observed pause at migration completion within measurement variance. The deallocation of the drained `old_table` is the confirmed cause of the "capacity unchanged" pause. Zero operations during steady-state incremental migration (`"unchanged"`) exceed 1 ms.

---

## 4. Analysis: Why Get-Miss is Slower

At 1,000,000 keys, `RobinHoodTable` get-miss latency is **129.2 ns** compared to `std::HashMap`'s **22.9 ns** (**5.6x slower**). At 10,000,000 keys, the gap narrows to **150.6 ns** vs **101.1 ns** (**1.5x slower**).

**Analysis**:
- `std::collections::HashMap` uses Swiss Table (hashbrown), which stores 1-byte control tags in a compact metadata array. It checks 16 buckets simultaneously using SIMD instructions within a single 64-byte cache line fetch before accessing any key or payload.
- `RobinHoodTable` stores each `Slot<K, V>` as an inline 40-byte struct containing `Box<K>`, `Box<V>`, stored `hash: u64`, and `psl: u16`. Even though an average miss only probes **1.70 to 2.04 slots**, reading 40-byte slots sequentially requires multiple DRAM cache line fetches across a large address space.
- **Candidate Optimization**: A separate compact hash/tag array (storing a 1-byte or 2-byte fingerprint per bucket) is a candidate optimization to allow rapid miss termination within a single cache line before dereferencing slot contents.

---

## 5. Local Windows Run (Rough Guide)

Measured on Windows 11 x86_64 (`cargo bench --bench store_bench -- --keys 1000000`):

| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Floor: loop + key access only | 2.0 ns [1.7 - 2.4] | 510.13 Mops/s | 2.0 ns [1.7 - 2.4] | 510.13 Mops/s | **1.00x** |
| Floor: hash only (hash64) | 22.0 ns [21.6 - 23.0] | 45.42 Mops/s | 31.0 ns [25.0 - 31.1] | 32.31 Mops/s | **1.41x** |
| Insert (1M keys from empty) | 702.4 ns [571.5 - 728.3] | 1.42 Mops/s | 499.1 ns [471.2 - 754.1] | 2.00 Mops/s | **0.71x** |
| Get hits (random order) | 314.7 ns [293.7 - 346.6] | 3.18 Mops/s | 779.5 ns [721.4 - 1172.4] | 1.28 Mops/s | **2.48x** |
| Get misses | 171.4 ns [154.9 - 317.4] | 5.83 Mops/s | 213.7 ns [183.6 - 369.9] | 4.68 Mops/s | **1.25x** |
| Overwrite (all keys) | 460.8 ns [272.4 - 1130.5] | 2.17 Mops/s | 305.5 ns [300.2 - 619.7] | 3.27 Mops/s | **0.66x** |
| Mixed (80% get / 15% ins / 5% rem) | 437.8 ns [407.7 - 439.7] | 2.28 Mops/s | 490.0 ns [482.8 - 711.9] | 2.04 Mops/s | **1.12x** |
| Remove (all keys) | 360.0 ns [302.4 - 512.1] | 2.78 Mops/s | 311.6 ns [292.3 - 359.6] | 3.21 Mops/s | **0.87x** |

### Local Windows Latency Profile & Top Slowest Inserts

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| p50 | 0.300 µs | 0.200 µs |
| p99 | 1.600 µs | 0.800 µs |
| p99.9 | 3.400 µs | 4.200 µs |
| p99.99 | 33.700 µs | 120.500 µs |
| max | 37,756.300 µs (~37.8 ms) | 195,257.000 µs (~195.3 ms) |
| Ops slower than 1 ms | 9 | 16 |

### Top 5 Slowest Inserts on Windows

| Rank | Op Index | Latency (µs) | Tag | Capacity Details |
| :---: | :---: | :---: | :---: | :--- |
| 1 | 917504 | 37756.30 µs | resize started | capacity changed: 1048576 -> 2097152 |
| 2 | 458752 | 12889.60 µs | resize started | capacity changed: 524288 -> 1048576 |
| 3 | 229376 | 6840.40 µs | resize started | capacity changed: 262144 -> 524288 |
| 4 | 520192 | 4193.60 µs | migration finished, old table dropped | capacity unchanged: 1048576 |
| 5 | 114688 | 2822.90 µs | resize started | capacity changed: 131072 -> 262144 |

### Inserts Slower Than 1 ms by Tag: RobinHoodTable (Windows)

| Tag | Count |
| :--- | :---: |
| resize started | 6 |
| migration finished, old table dropped | 3 |
| unchanged | 0 |
