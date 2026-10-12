# In-Memory Store Benchmarks

Comparative benchmark suite evaluating [`cachedb::store::RobinHoodTable`] against [`std::collections::HashMap`] under synthetic key-value workloads.

Benchmarks were executed on GitHub Actions CI runners (`ubuntu-24.04`, x86_64, standard Release profile):
- **1,000,000 keys**: `ci-linux-1M-v2.txt`
- **10,000,000 keys**: `ci-linux-10M-v2.txt`
- **Methodology**:
  - **Zero Allocations Inside Timed Regions**: All keys, values, and permutation indices are pre-constructed before timers start.
  - **Repetitions & Warmup**: Every phase executes 1 warmup iteration followed by 3 measured runs, reporting the median duration `[min - max]`.
  - **Baseline Floors**: Measures loop overhead and raw hash function execution to isolate the true cost of table algorithms.

---

## Executive Performance Summary

- **Where RobinHoodTable is faster**:
  - **Insert throughput**: **1.14x faster** at 1M keys (204.3 ns vs 232.9 ns) and **1.22x faster** at 10M keys (260.0 ns vs 315.9 ns).
  - **Get hits throughput**: **1.08x faster** at 1M keys (246.3 ns vs 265.5 ns) and **1.31x faster** at 10M keys (283.7 ns vs 370.7 ns).
  - **Overwrite throughput**: **1.38x faster** at 10M keys (175.2 ns vs 241.5 ns).
  - **Mixed workload throughput at scale**: **1.02x faster** at 1M keys (257.2 ns vs 261.3 ns) and **1.17x faster** at 10M keys (313.2 ns vs 366.7 ns).
  - **Remove throughput at scale**: **1.39x faster** at 10M keys (171.3 ns vs 238.8 ns).
  - **Worst-case latency spikes**: **47.3 ms** at 10M keys against **1,055.2 ms** (1.05 seconds) for `std::HashMap` (**22x lower worst-case pause**).
- **Where RobinHoodTable is slower**:
  - **Get misses**: **3.4x slower** at 1M keys (118.9 ns vs 35.2 ns, ratio 0.30x) and **1.76x slower** at 10M keys (147.1 ns vs 83.6 ns, ratio 0.57x).
  - **Overwrite at 1M keys**: **0.98x** (152.0 ns vs 148.8 ns).
  - **Remove at 1M keys**: **0.86x** (170.9 ns vs 147.1 ns).

---

## 1. Linux CI Benchmark: 1,000,000 Keys (`ci-linux-1M-v2.txt`)

### Throughput & Performance Comparison (Median of 3 Runs)

| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Floor: loop + key access only | 0.6 ns [0.4 - 0.7] | 1563.65 Mops/s | 0.6 ns [0.4 - 0.7] | 1563.65 Mops/s | **1.00x** |
| Floor: hash only (hash64) | 16.3 ns [16.3 - 16.5] | 61.42 Mops/s | 19.6 ns [19.4 - 20.2] | 50.96 Mops/s | **1.21x** |
| Insert (N keys from empty) | 204.3 ns [203.3 - 308.5] | 4.90 Mops/s | 232.9 ns [230.3 - 309.3] | 4.29 Mops/s | **1.14x** |
| Get hits (random order) | 246.3 ns [246.2 - 246.6] | 4.06 Mops/s | 265.5 ns [265.0 - 266.4] | 3.77 Mops/s | **1.08x** |
| Get misses | 118.9 ns [118.5 - 119.5] | 8.41 Mops/s | 35.2 ns [34.7 - 35.5] | 28.44 Mops/s | **0.30x** |
| Overwrite (all keys) | 152.0 ns [151.7 - 152.2] | 6.58 Mops/s | 148.8 ns [148.8 - 151.0] | 6.72 Mops/s | **0.98x** |
| Mixed (80% get / 15% ins / 5% rem) | 257.2 ns [255.6 - 260.3] | 3.89 Mops/s | 261.3 ns [260.4 - 262.0] | 3.83 Mops/s | **1.02x** |
| Remove (all keys) | 170.9 ns [170.8 - 171.7] | 5.85 Mops/s | 147.1 ns [146.6 - 147.3] | 6.80 Mops/s | **0.86x** |

### Latency Profiles: RobinHoodTable vs std::collections::HashMap

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| p50 | 0.160 µs | 0.090 µs |
| p99 | 0.821 µs | 0.371 µs |
| p99.9 | 1.693 µs | 0.601 µs |
| p99.99 | 10.961 µs | 10.340 µs |
| max | 6315.823 µs | 104871.793 µs |
| Ops slower than 1 ms | 5 | 6 |

### Top 5 Slowest Inserts: RobinHoodTable

| Rank | Op Index | Latency (µs) | Tag | Capacity Details |
| :---: | :---: | :---: | :---: | :--- |
| 1 | 917504 | 6315.82 µs | resize started | capacity changed: 1048576 -> 2097152 |
| 2 | 458752 | 3610.03 µs | resize started | capacity changed: 524288 -> 1048576 |
| 3 | 229376 | 1476.15 µs | resize started | capacity changed: 262144 -> 524288 |
| 4 | 520192 | 1193.93 µs | migration finished, old table dropped | capacity unchanged: 1048576 |
| 5 | 28672 | 1001.25 µs | resize started | capacity changed: 32768 -> 65536 |

### Inserts Slower Than 1 ms by Tag: RobinHoodTable (1M Keys)

| Tag | Count | Percentage |
| :--- | :---: | :---: |
| resize started | 4 | 80.0% |
| migration finished, old table dropped | 1 | 20.0% |
| unchanged | 0 | 0.0% |
| **Total (> 1 ms)** | **5** | **100.0%** |

### Top 5 Slowest Inserts: std::collections::HashMap

| Rank | Op Index | Latency (µs) | Event Description |
| :---: | :---: | :---: | :--- |
| 1 | 917504 | 104871.79 µs | std::HashMap insert operation #917504 |
| 2 | 458752 | 38266.18 µs | std::HashMap insert operation #458752 |
| 3 | 229376 | 9271.91 µs | std::HashMap insert operation #229376 |
| 4 | 114688 | 4478.35 µs | std::HashMap insert operation #114688 |
| 5 | 57344 | 2401.76 µs | std::HashMap insert operation #57344 |

### Resize-Start Allocation Cost (RobinHoodTable::new)

| Table Capacity | Allocation Size | Creation Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots, ~42 MB) | ~42 MB | 1.53 ms |
| 2^22 (4,194,304 slots, ~168 MB) | ~168 MB | 12.34 ms |
| 2^24 (16,777,216 slots, ~671 MB) | ~671 MB | 45.94 ms |

### Empty Table Drop Latency (drop(RobinHoodTable))

| Table Capacity | Allocation Size | Drop Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots, ~42 MB) | ~42 MB | 1.57 ms |
| 2^22 (4,194,304 slots, ~168 MB) | ~168 MB | 6.82 ms |
| 2^24 (16,777,216 slots, ~671 MB) | ~671 MB | 26.86 ms |

### Hash Table Quality & Memory Metrics

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| Max PSL | 11 | n/a |
| Average PSL | 0.46 | n/a |
| Measured Average Probes per Miss | 1.70 slots | n/a |
| Final Load Factor | 47.7% | n/a |
| Table Slot Size | 40 bytes | n/a |
| Est. Memory / Key | 83.9 bytes | n/a |
| Process VmRSS | 231.04 MB | 197.23 MB |

---

## 2. Linux CI Benchmark: 10,000,000 Keys (`ci-linux-10M-v2.txt`)

### Throughput & Performance Comparison (Median of 3 Runs)

| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Floor: loop + key access only | 0.8 ns [0.7 - 0.8] | 1300.59 Mops/s | 0.8 ns [0.7 - 0.8] | 1300.59 Mops/s | **1.00x** |
| Floor: hash only (hash64) | 16.3 ns [16.3 - 16.5] | 61.43 Mops/s | 19.8 ns [19.8 - 20.0] | 50.41 Mops/s | **1.22x** |
| Insert (N keys from empty) | 260.0 ns [259.2 - 362.8] | 3.85 Mops/s | 315.9 ns [313.6 - 412.1] | 3.17 Mops/s | **1.22x** |
| Get hits (random order) | 283.7 ns [283.5 - 286.2] | 3.53 Mops/s | 370.7 ns [370.6 - 375.1] | 2.70 Mops/s | **1.31x** |
| Get misses | 147.1 ns [147.0 - 148.3] | 6.80 Mops/s | 83.6 ns [82.8 - 92.1] | 11.96 Mops/s | **0.57x** |
| Overwrite (all keys) | 175.2 ns [171.7 - 175.4] | 5.71 Mops/s | 241.5 ns [235.4 - 243.9] | 4.14 Mops/s | **1.38x** |
| Mixed (80% get / 15% ins / 5% rem) | 313.2 ns [310.7 - 314.5] | 3.19 Mops/s | 366.7 ns [360.3 - 373.4] | 2.73 Mops/s | **1.17x** |
| Remove (all keys) | 171.3 ns [169.6 - 171.8] | 5.84 Mops/s | 238.8 ns [232.2 - 240.7] | 4.19 Mops/s | **1.39x** |

### Latency Profiles: RobinHoodTable vs std::collections::HashMap

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| p50 | 0.191 µs | 0.161 µs |
| p99 | 0.902 µs | 0.532 µs |
| p99.9 | 1.823 µs | 0.801 µs |
| p99.99 | 11.081 µs | 10.780 µs |
| max | 47328.061 µs (~47.3 ms) | 1055231.885 µs (~1,055.2 ms) |
| Ops slower than 1 ms | 14 | 8 |

### Top 5 Slowest Inserts: RobinHoodTable

| Rank | Op Index | Latency (µs) | Tag | Capacity Details |
| :---: | :---: | :---: | :---: | :--- |
| 1 | 7340032 | 47328.06 µs | resize started | capacity changed: 8388608 -> 16777216 |
| 2 | 3670016 | 24621.72 µs | resize started | capacity changed: 4194304 -> 8388608 |
| 3 | 8323072 | 13882.64 µs | migration finished, old table dropped | capacity unchanged: 16777216 |
| 4 | 1835008 | 12608.30 µs | resize started | capacity changed: 2097152 -> 4194304 |
| 5 | 4161536 | 6995.86 µs | migration finished, old table dropped | capacity unchanged: 8388608 |

### Inserts Slower Than 1 ms by Tag: RobinHoodTable (10M Keys)

| Tag | Count | Percentage |
| :--- | :---: | :---: |
| resize started | 9 | 64.3% |
| migration finished, old table dropped | 5 | 35.7% |
| unchanged | 0 | 0.0% |
| **Total (> 1 ms)** | **14** | **100.0%** |

### Top 5 Slowest Inserts: std::collections::HashMap

| Rank | Op Index | Latency (µs) | Event Description |
| :---: | :---: | :---: | :--- |
| 1 | 7340032 | 1055231.89 µs | std::HashMap insert operation #7340032 |
| 2 | 3670016 | 499540.37 µs | std::HashMap insert operation #3670016 |
| 3 | 1835008 | 243242.18 µs | std::HashMap insert operation #1835008 |
| 4 | 917504 | 109528.11 µs | std::HashMap insert operation #917504 |
| 5 | 458752 | 43339.27 µs | std::HashMap insert operation #458752 |

### Resize-Start Allocation Cost (RobinHoodTable::new)

| Table Capacity | Allocation Size | Creation Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots, ~42 MB) | ~42 MB | 1.94 ms |
| 2^22 (4,194,304 slots, ~168 MB) | ~168 MB | 13.10 ms |
| 2^24 (16,777,216 slots, ~671 MB) | ~671 MB | 48.28 ms |

### Empty Table Drop Latency (drop(RobinHoodTable))

| Table Capacity | Allocation Size | Drop Latency (Median) |
| :--- | :---: | :---: |
| 2^20 (1,048,576 slots, ~42 MB) | ~42 MB | 1.67 ms |
| 2^22 (4,194,304 slots, ~168 MB) | ~168 MB | 7.32 ms |
| 2^24 (16,777,216 slots, ~671 MB) | ~671 MB | 27.88 ms |

### Hash Table Quality & Memory Metrics

| Metric | RobinHoodTable | std::collections::HashMap |
| :--- | :---: | :---: |
| Max PSL | 14 | n/a |
| Average PSL | 0.74 | n/a |
| Measured Average Probes per Miss | 2.04 slots | n/a |
| Final Load Factor | 59.6% | n/a |
| Table Slot Size | 40 bytes | n/a |
| Est. Memory / Key | 67.1 bytes | n/a |
| Process VmRSS | 1977.39 MB | 1939.85 MB |

---

## 3. Analysis of Slow Inserts & Drop Measurements

### Measured Root Causes of Slow Inserts

Tracking slow inserts (> 1 ms) by tag demonstrates that all latency spikes belong to two distinct memory lifecycle operations:
1. **Resize Started (`capacity changed`)**:
   Allocating and zero-initializing the new backing buffer of size $2\times C$ is an $\mathcal{O}(\text{new capacity})$ operation.
   - At 1M keys, allocating $2^{21}$ slots (~84 MB) takes **6.32 ms** (Op #917,504).
   - At 10M keys, allocating $2^{24}$ slots (~671 MB) takes **47.33 ms** (Op #7,340,032).
2. **Migration Finished, Old Table Dropped (`capacity unchanged`)**:
   When `old_table.len == 0`, `self.old_table` is set to `None`, invoking `drop()` on the drained old table buffer. Deallocating a multi-megabyte buffer produces a measurable pause:
   - At 1M keys, Op #520,192 is the exact insert where migration finishes; dropping the $2^{19}$ slots (~21 MB) buffer takes **1.19 ms** on Linux (and **4.19 ms** on Windows).
   - At 10M keys, Op #8,323,072 takes **13.88 ms** dropping the $2^{23}$ slots (~335 MB) buffer, and Op #4,161,536 takes **7.00 ms** dropping the $2^{22}$ slots (~168 MB) buffer.
3. **Steady-State Operations (`unchanged`)**:
   Zero operations during ongoing incremental migration without buffer allocation or deallocation exceeded 1 ms (0 in 1M run, 0 in 10M run).

### Mathematical Verification of Migration Completion

In incremental migration, each mutating insert operation steps migration by up to `MIGRATE_BATCH = 16` units of work in `old_table`.
Each loop iteration in `step_migration` either inspects and skips an empty slot (`migrate_cursor` advances, 1 unit of work) or removes an occupied slot via backward-shift and inserts it into `main_table` (1 unit of work).

Over the entire migration from index 0 to $C_{\text{old}}$, the loop encounters all $C_{\text{old}} - N_{\text{old}}$ empty slots and all $N_{\text{old}}$ occupied slots (and the holes created by their backward shifts, totaling $C_{\text{old}} + N_{\text{old}}$ iterations).
Therefore, the number of mutating operations required to complete migration is exactly:
$$\Delta\text{ops} = \frac{C_{\text{old}} + N_{\text{old}}}{16}$$
And migration finishes at operation index:
$$\text{op\_end} = \text{op\_start} + \frac{C_{\text{old}} + N_{\text{old}}}{16}$$

Verifying against benchmark logs:
- **1,000,000 Keys**:
  Resize started at Op #458,752 ($C_{\text{old}} = 524,288$, $N_{\text{old}} = 458,752$):
  $$\text{op\_end} = 458,752 + \frac{524,288 + 458,752}{16} = 458,752 + \frac{983,040}{16} = 458,752 + 61,440 = \mathbf{520,192}$$
  The measured slow insert at Op #520,192 (`1,193.93 µs`, `migration finished, old table dropped`) matches this formula exactly.
- **10,000,000 Keys (Resize 1)**:
  Resize started at Op #3,670,016 ($C_{\text{old}} = 4,194,304$, $N_{\text{old}} = 3,670,016$):
  $$\text{op\_end} = 3,670,016 + \frac{4,194,304 + 3,670,016}{16} = 3,670,016 + \frac{7,864,320}{16} = 3,670,016 + 491,520 = \mathbf{4,161,536}$$
  The measured slow insert at Op #4,161,536 (`6,995.86 µs`, `migration finished, old table dropped`) matches this formula exactly.
- **10,000,000 Keys (Resize 2)**:
  Resize started at Op #7,340,032 ($C_{\text{old}} = 8,388,608$, $N_{\text{old}} = 7,340,032$):
  $$\text{op\_end} = 7,340,032 + \frac{8,388,608 + 7,340,032}{16} = 7,340,032 + \frac{15,728,640}{16} = 7,340,032 + 983,040 = \mathbf{8,323,072}$$
  The measured slow insert at Op #8,323,072 (`13,882.64 µs`, `migration finished, old table dropped`) matches this formula exactly.

### Creation and drop time, Linux CI

Empirical median latency for buffer creation (`RobinHoodTable::new`) and deallocation (`drop(RobinHoodTable)`) measured on Linux CI (`ubuntu-24.04`), copied directly from `ci-linux-1M-v2.txt` and `ci-linux-10M-v2.txt`:

| Table Capacity | Allocation Size | Creation Latency (1M run) | Drop Latency (1M run) | Creation Latency (10M run) | Drop Latency (10M run) |
| :--- | :---: | :---: | :---: | :---: | :---: |
| 2^20 (1,048,576 slots) | ~42 MB | 1.53 ms | 1.57 ms | 1.94 ms | 1.67 ms |
| 2^22 (4,194,304 slots) | ~168 MB | 12.34 ms | 6.82 ms | 13.10 ms | 7.32 ms |
| 2^24 (16,777,216 slots) | ~671 MB | 45.94 ms | 26.86 ms | 48.28 ms | 27.88 ms |

### Drop Latency Comparison: Linux CI vs Local Windows

| Table Capacity | Allocation Size | Linux CI Drop Latency (1M run) | Local Windows Drop Latency |
| :--- | :---: | :---: | :---: |
| 2^20 (1,048,576 slots) | ~42 MB | 1.57 ms | 10.05 ms |
| 2^22 (4,194,304 slots) | ~168 MB | 6.82 ms | 33.01 ms |
| 2^24 (16,777,216 slots) | ~671 MB | 26.86 ms | 141.24 ms |

The measured drop latency matches the observed pause at migration completion within measurement variance. The deallocation of the drained `old_table` is the confirmed cause of the "capacity unchanged" pause. Zero operations during steady-state incremental migration (`"unchanged"`) exceed 1 ms.

---

## 4. Analysis: Why Get-Miss is Slower

At 1,000,000 keys, `RobinHoodTable` get-miss latency is **118.9 ns** compared to `std::HashMap`'s **35.2 ns** (**0.30x**, ~3.4x slower). At 10,000,000 keys, the gap is **147.1 ns** vs **83.6 ns** (**0.57x**, ~1.76x slower).

**Analysis**:
- `std::collections::HashMap` uses Swiss Table (hashbrown), which stores 1-byte control tags in a compact metadata array. It checks 16 buckets simultaneously using SIMD instructions within a single 64-byte cache line fetch before accessing any key or payload.
- `RobinHoodTable` stores each `Slot<K, V>` as an inline 40-byte struct containing `Box<K>`, `Box<V>`, stored `hash: u64`, and `psl: u32`. Even though an average miss only probes **1.70 to 2.04 slots**, reading 40-byte slots sequentially requires multiple DRAM cache line fetches across a large address space.
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

---

## Network Load Generator Benchmarks (`cachedb-bench`)

The `cachedb-bench` tool generates synthetic multi-connection client workloads over standard TCP sockets using the native client protocol implementation.

### Running Benchmarks Locally (Script-Free)

To run the benchmark suite manually without external scripts:

1. **Build Release Binaries**:
   ```bash
   cargo build --release --bin cachedbd --bin cachedb-bench
   ```

2. **Start the Database Daemon**:
   In a separate terminal (or in the background):
   ```bash
   ./target/release/cachedbd --bind 127.0.0.1 --port 7878
   ```

3. **Run Workloads with `cachedb-bench`**:

   - **Read-Heavy Workload (95% Fetch, 5% Put, Pipeline 16)**:
     ```bash
     ./target/release/cachedb-bench --addr 127.0.0.1:7878 --workload read --connections 4 --duration 10 --warmup 2 --pipeline 16 --preload
     ```

   - **Read-Heavy Workload (Unpipelined, Pipeline 1)**:
     ```bash
     ./target/release/cachedb-bench --addr 127.0.0.1:7878 --workload read --connections 4 --duration 10 --warmup 2 --pipeline 1
     ```

   - **Write-Only Workload (100% Put, Pipeline 16)**:
     ```bash
     ./target/release/cachedb-bench --addr 127.0.0.1:7878 --workload write --connections 4 --duration 10 --warmup 2 --pipeline 16
     ```

   - **Mixed Workload (50% Fetch, 40% Put, 5% Bump, 5% Drop, Pipeline 16)**:
     ```bash
     ./target/release/cachedb-bench --addr 127.0.0.1:7878 --workload mixed --connections 4 --duration 10 --warmup 2 --pipeline 16 --preload
     ```

   - **Counter Workload (100% Bump, Pipeline 16)**:
     ```bash
     ./target/release/cachedb-bench --addr 127.0.0.1:7878 --workload bump --connections 4 --duration 10 --warmup 2 --pipeline 16
     ```

### Honest Limits

When evaluating network benchmark results, consider the following environmental and structural constraints:

1. **Co-located Client and Server**: Benchmarks execute client threads and the database daemon on the same physical or virtual host. Loopback TCP avoids physical network interface and switch latency, but client and server compete for shared CPU cores and cache hierarchy.
2. **Shared CI Runner Noise**: GitHub Actions runners run on shared virtualized cloud hardware (`ubuntu-latest`). Noisy neighbor effects, CPU throttling, and scheduling jitter can affect percentile distributions between separate runs.
3. **Per-Batch Latency Measurement**: Pipelined batches send multiple commands in a single frame sequence. The recorded latency represents the round-trip duration for the entire batch (e.g. 16 commands), not individual command execution times inside the server engine.
4. **Single-Threaded Event Loop Core**: The database server core processes all commands on a single non-blocking event loop thread. Increasing client connection counts beyond CPU saturation tests queueing behavior rather than parallel execution.
5. **Small Synthetic Values**: Standard benchmarks utilize 64-byte payloads. Workloads with multi-megabyte payloads will be bottlenecked by memory copying and TCP socket buffer watermarks rather than command dispatch throughput.
6. **In-Memory Operation (No Persistence)**: Measurements reflect pure in-memory execution without WAL disk writes or background snapshot creation enabled.

### Lessons: The 40 ms Nagle + Delayed-ACK Stall

During initial CI benchmark profiling, unpipelined reads (`pipeline 1`) achieved 58,150 ops/s with p50 latency of 0.054 ms, whereas pipelined reads (`pipeline 16`) collapsed to 1,560 ops/s with batch p50 latency of 41 ms (every batch consistently stalled for ~41 ms). This demonstrated the classic deadlock between Nagle's algorithm and delayed ACKs: while the client enabled `TCP_NODELAY`, the server omitted `set_nodelay(true)` on accepted sockets. Pipelined reply streams were buffered in the kernel waiting for delayed ACKs (~40 ms). Enabling `TCP_NODELAY` on accepted sockets in `Server::handle_accept` eliminates this stall. The next CI run will show the effect of reply coalescing.

### Network Benchmark Results (GitHub Actions CI)

The table below reflects automated measurements gathered via `.github/workflows/network-bench.yml` (the in-memory store benchmark is run via `.github/workflows/bench.yml`):

| Workload | Pipeline | Connections | Throughput (ops/sec) | Batch Latency p50 | Batch Latency p99 | Batch Latency p99.9 | Batch Latency max |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| Read (95/5) | 16 | 4 | *to be filled from the CI artifact* | *to be filled* | *to be filled* | *to be filled* | *to be filled* |
| Read (95/5) | 1 | 4 | *to be filled from the CI artifact* | *to be filled* | *to be filled* | *to be filled* | *to be filled* |
| Write (100% put) | 16 | 4 | *to be filled from the CI artifact* | *to be filled* | *to be filled* | *to be filled* | *to be filled* |
| Mixed | 16 | 4 | *to be filled from the CI artifact* | *to be filled* | *to be filled* | *to be filled* | *to be filled* |
| Bump (100% bump) | 16 | 4 | *to be filled from the CI artifact* | *to be filled* | *to be filled* | *to be filled* | *to be filled* |
