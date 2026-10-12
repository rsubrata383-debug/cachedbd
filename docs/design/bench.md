# Load Generator Architecture and Design (`cachedb-bench`)

Architecture overview of the multi-threaded synthetic load generator `cachedb-bench` for evaluating Cachedb throughput and latency profiles.

---

## 1. Thread and Concurrency Model

- **Architecture**: M-client threads connecting over independent TCP streams to the server's single-threaded non-blocking event loop.
- **Worker Isolation**:
  - Each thread instantiates a dedicated [`cachedb::client::Client`].
  - Thread seeds are derived deterministically:
    $$\text{thread\_seed} = (\text{base\_seed} \times \text{0x517c\_c1b7\_2722\_0a95}) + (\text{thread\_idx} + 1) \times 2 + 1$$
    This mathematical transformation guarantees non-zero, distinct seed states for every thread without shared mutable synchronization.
  - Zero inter-thread locking during measurement: threads allocate local results and histograms on the stack/heap, merging them into an aggregate report upon thread join.
- **Warmup and Measurement Phases**:
  - Optional warmup period ($T_{\text{warmup}}$) executes first to prime TCP buffers and in-memory hash table structures. Warmup metrics are completely discarded.
  - Measurement period ($T_{\text{measure}}$) records batch latency and operation counts until wall-clock expiry.

---

## 2. Workload Profiles and Key Space Separation

- **Workload Distributions**:
  - `read`: 95% `fetch`, 5% `put`
  - `write`: 100% `put`
  - `mixed`: 50% `fetch`, 40% `put`, 5% `bump`, 5% `drop`
  - `bump`: 100% `bump`
- **Counter Key Space Separation**:
  - In `mixed` workloads, `put` stores `Value::Bytes` or `Value::Text`. If a subsequent `bump` command targeted the same key, the server would return error code 3004 (`ERR_NOT_AN_INTEGER`).
  - To maintain zero error rates during continuous mixed benchmarks, operations are isolated into two distinct namespaces:
    - Primary key space: `"key:<idx>"` for `fetch`, `put`, and `drop`.
    - Counter key space: `"cnt:<idx>"` exclusively for `bump`.
- **Preloading**:
  - Optional `--preload` flag populates all keys in chunks prior to measurement, ensuring reads hit existing values rather than missing keys (`Ok(None)`).

---

## 3. Fixed-Size Logarithmic Histogram Design

- **Zero Allocation in Hot Path**:
  - Histogram allocates a single boxed array of 3,776 64-bit integer counters upon initialization.
  - Recording latency (`record(Duration)`) computes bucket indices using bitwise shift operations (`leading_zeros`) and increments the counter in $O(1)$ time with zero heap allocations.
- **Sub-Bucket Logarithmic Resolution**:
  - 64 sub-buckets per power of two (6 bits of mantissa).
  - Nanosecond dynamic range: tracks latencies from 1 ns to $2^{64}-1$ ns.
  - Maximum observed latency is tracked exactly alongside cumulative bucket distribution.
- **Percentile Calculation**:
  - Supports arbitrary percentiles ($P \in [0.0, 100.0]$), calculating p50, p99, and p99.9 via linear prefix scans over bucket counters.
  - Returns durations bounded by the exact maximum observed latency.

