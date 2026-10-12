//! Benchmark suite comparing [`skiffdb::store::RobinHoodTable`] against [`std::collections::HashMap`].
//!
//! Measures throughput, latency percentiles, latency spikes, table quality metrics,
//! and resident set size (VmRSS) across synthetic key-value workloads.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::BuildHasher;
use std::time::{Duration, Instant};

use skiffdb::store::{RobinHoodTable, hash64, slot_size};

/// Deterministic 64-bit XorShift generator strictly conforming to AGENTS.md rules.
#[derive(Debug, Clone)]
struct BenchRng {
    state: u64,
}

impl BenchRng {
    fn new(seed: u64) -> Self {
        assert_ne!(seed, 0, "seed must not be zero");
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Generates integer in `[min, max)` with wide arithmetic and high-order bits before casting.
    fn gen_range(&mut self, min: usize, max: usize) -> usize {
        assert!(min < max, "min must be strictly less than max");
        let diff = (max - min) as u64;
        let offset = ((self.next_u64() >> 32) % diff) as usize;
        min + offset
    }

    /// Generates boolean using MSB.
    #[expect(dead_code, reason = "Utility method implementing AGENTS.md rule 11.1")]
    fn gen_bool(&mut self) -> bool {
        (self.next_u64() >> 63) == 1
    }
}

/// Reads Resident Set Size (`VmRSS`) in kilobytes from `/proc/self/status` on Linux.
///
/// Returns `None` on Windows or platforms where `/proc` is unavailable.
fn get_vm_rss_kb() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return parts[1].parse::<usize>().ok();
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Formats VmRSS measurement or returns `"n/a"` if unavailable.
fn format_vm_rss(rss_kb: Option<usize>) -> String {
    match rss_kb {
        Some(kb) => format!("{:.2} MB", (kb as f64) / 1024.0),
        None => "n/a".to_string(),
    }
}

/// Record of an individual high-latency insert operation.
#[derive(Debug, Clone)]
struct SlowOp {
    index: usize,
    latency_us: f64,
    tag: &'static str,
    description: String,
}

/// Multi-run benchmark results recording median, min, max, and throughput.
#[derive(Debug, Clone)]
struct MultiRunResult {
    name: &'static str,
    median_ns: f64,
    min_ns: f64,
    max_ns: f64,
    mops_per_sec: f64,
}

impl MultiRunResult {
    fn from_durations(name: &'static str, ops: usize, mut durations: Vec<Duration>) -> Self {
        assert!(!durations.is_empty(), "durations must not be empty");
        durations.sort();
        let mid = durations.len() / 2;
        let median_d = durations[mid];
        let min_d = durations[0];
        let max_d = durations[durations.len() - 1];

        let median_ns = (median_d.as_nanos() as f64) / (ops as f64);
        let min_ns = (min_d.as_nanos() as f64) / (ops as f64);
        let max_ns = (max_d.as_nanos() as f64) / (ops as f64);
        let mops_per_sec = (ops as f64) / (median_d.as_secs_f64() * 1_000_000.0);

        Self {
            name,
            median_ns,
            min_ns,
            max_ns,
            mops_per_sec,
        }
    }
}

/// Latency distribution statistics across operation executions.
#[derive(Debug, Clone)]
struct LatencyProfile {
    p50_us: f64,
    p99_us: f64,
    p99_9_us: f64,
    p99_99_us: f64,
    max_us: f64,
    slow_over_1ms_count: usize,
    count_over_1ms_resize_started: usize,
    count_over_1ms_migration_finished: usize,
    count_over_1ms_unchanged: usize,
    slowest_5: Vec<SlowOp>,
}

/// Configuration parameters for the benchmark run.
struct BenchConfig {
    keys: usize,
    mixed_ops: usize,
    is_smoke: bool,
}

impl BenchConfig {
    fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let is_smoke = args.iter().any(|a| a == "--smoke");

        let mut keys = if is_smoke { 50_000 } else { 1_000_000 };

        let mut idx = 0;
        while idx < args.len() {
            if args[idx] == "--keys" && idx + 1 < args.len() {
                if let Ok(parsed) = args[idx + 1].parse::<usize>() {
                    keys = parsed;
                }
                idx += 1;
            }
            idx += 1;
        }

        let mixed_ops = if is_smoke {
            50_000
        } else {
            1_000_000.min(keys)
        };

        Self {
            keys,
            mixed_ops,
            is_smoke,
        }
    }
}

/// Pre-allocated operation payload for mixed benchmarks (zero allocation in timed region).
enum PreMixedOp {
    Get(usize),
    Insert(Box<[u8]>, u64),
    Remove(usize),
}

fn build_key_list(count: usize) -> Vec<Box<[u8]>> {
    (0..count)
        .map(|i| format!("key_{i:010}").into_bytes().into_boxed_slice())
        .collect()
}

fn build_miss_keys(count: usize) -> Vec<Box<[u8]>> {
    (0..count)
        .map(|i| format!("miss_{i:010}").into_bytes().into_boxed_slice())
        .collect()
}

#[allow(clippy::too_many_lines, clippy::similar_names)]
fn main() {
    let config = BenchConfig::parse();

    println!("# SkiffDB Store Benchmark: RobinHoodTable vs std::collections::HashMap");
    println!();
    println!(
        "- **Configuration**: {} keys, {} mixed operations{}",
        config.keys,
        config.mixed_ops,
        if config.is_smoke { " (smoke mode)" } else { "" }
    );
    println!("- **OS**: {}", std::env::consts::OS);
    println!("- **Architecture**: {}", std::env::consts::ARCH);
    println!("- **Repetitions**: 1 warmup run + 3 measured runs (reporting median [min - max])");
    println!("- **Allocation Policy**: Strictly 0 allocations inside timed regions");
    println!();

    // -------------------------------------------------------------------------
    // 0. Pre-generate datasets outside timed regions
    // -------------------------------------------------------------------------
    println!(
        "Pre-generating {} keys and permutation indices (outside timed regions)...",
        config.keys
    );

    // Primary read-only key list for lookups and baselines
    let keys_lookup = build_key_list(config.keys);
    let miss_keys = build_miss_keys(config.keys);

    // Shuffled index list for random get hits
    let mut hit_indices: Vec<usize> = (0..config.keys).collect();
    let mut rng = BenchRng::new(0x853c_49e6_748f_ea9b_u64);
    for i in (1..config.keys).rev() {
        let j = rng.gen_range(0, i + 1);
        hit_indices.swap(i, j);
    }

    // Pre-generate mixed operations for 1 warmup + 3 measured runs
    let generate_mixed_ops = |ops_count: usize, rng: &mut BenchRng| -> Vec<PreMixedOp> {
        let mut ops = Vec::with_capacity(ops_count);
        for i in 0..ops_count {
            let roll = rng.gen_range(0, 100);
            if roll < 80 {
                let k_idx = rng.gen_range(0, config.keys);
                ops.push(PreMixedOp::Get(k_idx));
            } else if roll < 95 {
                let extra_key = format!("extra_{i:010}").into_bytes().into_boxed_slice();
                ops.push(PreMixedOp::Insert(extra_key, rng.next_u64()));
            } else {
                let k_idx = rng.gen_range(0, config.keys);
                ops.push(PreMixedOp::Remove(k_idx));
            }
        }
        ops
    };

    println!("Datasets ready.\n");

    // =========================================================================
    // 1. Benchmark Floor Baselines (Loop Overhead & Hashing)
    // =========================================================================
    println!("Measuring baseline floor costs (no table operations)...");

    // Floor 1: Loop + Key Access Only
    let rh_floor_loop = {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        for k in &keys_lookup {
            std::hint::black_box(k.as_ref());
        }
        // 3 measured runs
        for _ in 0..3 {
            let t0 = Instant::now();
            for k in &keys_lookup {
                std::hint::black_box(k.as_ref());
            }
            runs.push(t0.elapsed());
        }
        MultiRunResult::from_durations("Floor: loop + key access only", config.keys, runs)
    };
    let hm_floor_loop = rh_floor_loop.clone();

    // Floor 2: Hash Only
    let rh_seed = 0x853c_49e6_748f_ea9b_u64;
    let rh_floor_hash = {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        for k in &keys_lookup {
            let h = hash64(k, rh_seed);
            std::hint::black_box(h);
        }
        // 3 measured runs
        for _ in 0..3 {
            let t0 = Instant::now();
            for k in &keys_lookup {
                let h = hash64(k, rh_seed);
                std::hint::black_box(h);
            }
            runs.push(t0.elapsed());
        }
        MultiRunResult::from_durations("Floor: hash only (hash64)", config.keys, runs)
    };

    let std_hasher = std::hash::RandomState::new();
    let hm_floor_hash = {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        for k in &keys_lookup {
            let h = std_hasher.hash_one(k);
            std::hint::black_box(h);
        }
        // 3 measured runs
        for _ in 0..3 {
            let t0 = Instant::now();
            for k in &keys_lookup {
                let h = std_hasher.hash_one(k);
                std::hint::black_box(h);
            }
            runs.push(t0.elapsed());
        }
        MultiRunResult::from_durations("Floor: hash only (RandomState)", config.keys, runs)
    };

    // =========================================================================
    // 2. Benchmark RobinHoodTable
    // =========================================================================
    println!("Benchmarking RobinHoodTable<u64> across 6 operations (warmup + 3 runs)...");

    let mut rh_results = Vec::new();
    rh_results.push(rh_floor_loop);
    rh_results.push(rh_floor_hash);

    // Latency tracking during measured insert
    let mut rh_insert_latencies = Vec::with_capacity(config.keys);
    let mut rh_slow_heap: BinaryHeap<Reverse<(u32, usize, &'static str, String)>> =
        BinaryHeap::new();
    let mut rh_slow_over_1ms = 0usize;
    let mut rh_over_1ms_resize_started = 0usize;
    let mut rh_over_1ms_migration_finished = 0usize;
    let mut rh_over_1ms_unchanged = 0usize;

    // Phase 1: Insert (N keys from empty)
    {
        let mut runs = Vec::with_capacity(3);
        // Warmup run
        {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);
            let warmup_keys = build_key_list(config.keys);
            for (idx, key) in warmup_keys.into_iter().enumerate() {
                let _ = table.insert(key, idx as u64);
            }
        }

        // 3 measured runs
        for run_idx in 0..3 {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);
            let insert_keys = build_key_list(config.keys);

            let t0 = Instant::now();
            if run_idx == 0 {
                // Record fine-grained latencies on run 0
                for (idx, key) in insert_keys.into_iter().enumerate() {
                    let res_before = table.is_resizing();
                    let cap_before = table.capacity();
                    let op_t0 = Instant::now();
                    let _ = table.insert(key, idx as u64);
                    let op_elapsed_ns = op_t0.elapsed().as_nanos().min(u128::from(u32::MAX)) as u32;
                    let res_after = table.is_resizing();
                    let cap_after = table.capacity();

                    let tag = if !res_before && res_after {
                        "resize started"
                    } else if res_before && !res_after {
                        "migration finished, old table dropped"
                    } else {
                        "unchanged"
                    };

                    rh_insert_latencies.push(op_elapsed_ns);
                    if op_elapsed_ns >= 1_000_000 {
                        rh_slow_over_1ms = rh_slow_over_1ms.saturating_add(1);
                        match tag {
                            "resize started" => {
                                rh_over_1ms_resize_started =
                                    rh_over_1ms_resize_started.saturating_add(1);
                            }
                            "migration finished, old table dropped" => {
                                rh_over_1ms_migration_finished =
                                    rh_over_1ms_migration_finished.saturating_add(1);
                            }
                            _ => {
                                rh_over_1ms_unchanged = rh_over_1ms_unchanged.saturating_add(1);
                            }
                        }
                    }

                    let desc = if cap_after == cap_before {
                        format!("capacity unchanged: {cap_before}")
                    } else {
                        format!("capacity changed: {cap_before} -> {cap_after}")
                    };

                    if rh_slow_heap.len() < 5
                        || op_elapsed_ns > rh_slow_heap.peek().map_or(0, |r| r.0.0)
                    {
                        rh_slow_heap.push(Reverse((op_elapsed_ns, idx, tag, desc)));
                        if rh_slow_heap.len() > 5 {
                            rh_slow_heap.pop();
                        }
                    }
                }
            } else {
                for (idx, key) in insert_keys.into_iter().enumerate() {
                    let _ = table.insert(key, idx as u64);
                }
            }
            runs.push(t0.elapsed());
        }
        rh_results.push(MultiRunResult::from_durations(
            "Insert (N keys from empty)",
            config.keys,
            runs,
        ));
    }

    // Build stable populated table for read-only and mutate benchmarks
    let mut rh_table = RobinHoodTable::<u64>::with_default_seed(8);
    for (idx, key) in build_key_list(config.keys).into_iter().enumerate() {
        let _ = rh_table.insert(key, idx as u64);
    }
    // Step any residual migration so table is fully stable
    let _ = rh_table.step_migration(usize::MAX);

    let rh_stats = rh_table.stats();
    let rh_rss = get_vm_rss_kb();
    let rh_slot_bytes = slot_size::<u64>();
    let rh_est_bytes_per_key =
        ((rh_stats.capacity as f64) * (rh_slot_bytes as f64)) / (config.keys as f64);

    // Phase 2: Get hits (random order)
    {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        for &idx in &hit_indices {
            let v = rh_table.get(&keys_lookup[idx]);
            std::hint::black_box(v);
        }
        // 3 measured runs
        for _ in 0..3 {
            let t0 = Instant::now();
            for &idx in &hit_indices {
                let v = rh_table.get(&keys_lookup[idx]);
                std::hint::black_box(v);
            }
            runs.push(t0.elapsed());
        }
        rh_results.push(MultiRunResult::from_durations(
            "Get hits (random order)",
            config.keys,
            runs,
        ));
    }

    // Phase 3: Get misses
    {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        for k in &miss_keys {
            let v = rh_table.get(k);
            std::hint::black_box(v);
        }
        // 3 measured runs
        for _ in 0..3 {
            let t0 = Instant::now();
            for k in &miss_keys {
                let v = rh_table.get(k);
                std::hint::black_box(v);
            }
            runs.push(t0.elapsed());
        }
        rh_results.push(MultiRunResult::from_durations(
            "Get misses",
            config.keys,
            runs,
        ));
    }

    // Measure miss probe count evidence directly
    let mut total_miss_probes = 0usize;
    let sample_misses = config.keys.min(100_000);
    for k in miss_keys.iter().take(sample_misses) {
        total_miss_probes = total_miss_probes.saturating_add(rh_table.lookup_probe_count(k));
    }
    let avg_miss_probes = (total_miss_probes as f64) / (sample_misses as f64);

    // Phase 4: Overwrite
    {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        let warmup_keys = build_key_list(config.keys);
        for (idx, key) in warmup_keys.into_iter().enumerate() {
            let _ = rh_table.insert(key, (idx as u64).wrapping_mul(3));
        }
        // 3 measured runs
        for run_idx in 0..3 {
            let run_keys = build_key_list(config.keys);
            let val = (run_idx as u64).wrapping_add(10);
            let t0 = Instant::now();
            for key in run_keys {
                let _ = rh_table.insert(key, val);
            }
            runs.push(t0.elapsed());
        }
        rh_results.push(MultiRunResult::from_durations(
            "Overwrite (all keys)",
            config.keys,
            runs,
        ));
    }

    // Phase 5: Mixed (80/15/5)
    {
        let mut runs = Vec::with_capacity(3);
        let mut mixed_rng = BenchRng::new(0xdead_beef_cafe_babe_u64);
        let warmup_ops = generate_mixed_ops(config.mixed_ops, &mut mixed_rng);
        for op in warmup_ops {
            match op {
                PreMixedOp::Get(idx) => {
                    let v = rh_table.get(&keys_lookup[idx]);
                    std::hint::black_box(v);
                }
                PreMixedOp::Insert(k, v) => {
                    let _ = rh_table.insert(k, v);
                }
                PreMixedOp::Remove(idx) => {
                    let v = rh_table.remove(&keys_lookup[idx]);
                    std::hint::black_box(v);
                }
            }
        }

        for _ in 0..3 {
            let measured_ops = generate_mixed_ops(config.mixed_ops, &mut mixed_rng);
            let t0 = Instant::now();
            for op in measured_ops {
                match op {
                    PreMixedOp::Get(idx) => {
                        let v = rh_table.get(&keys_lookup[idx]);
                        std::hint::black_box(v);
                    }
                    PreMixedOp::Insert(k, v) => {
                        let _ = rh_table.insert(k, v);
                    }
                    PreMixedOp::Remove(idx) => {
                        let v = rh_table.remove(&keys_lookup[idx]);
                        std::hint::black_box(v);
                    }
                }
            }
            runs.push(t0.elapsed());
        }
        rh_results.push(MultiRunResult::from_durations(
            "Mixed (80% get / 15% ins / 5% rem)",
            config.mixed_ops,
            runs,
        ));
    }

    // Phase 6: Remove (all keys)
    {
        let mut runs = Vec::with_capacity(3);
        // Warmup
        let mut warmup_table = RobinHoodTable::<u64>::with_default_seed(8);
        for (idx, k) in build_key_list(config.keys).into_iter().enumerate() {
            let _ = warmup_table.insert(k, idx as u64);
        }
        for k in &keys_lookup {
            let v = warmup_table.remove(k);
            std::hint::black_box(v);
        }

        // 3 measured runs
        for _ in 0..3 {
            let mut test_table = RobinHoodTable::<u64>::with_default_seed(8);
            for (idx, k) in build_key_list(config.keys).into_iter().enumerate() {
                let _ = test_table.insert(k, idx as u64);
            }
            let t0 = Instant::now();
            for k in &keys_lookup {
                let v = test_table.remove(k);
                std::hint::black_box(v);
            }
            runs.push(t0.elapsed());
        }
        rh_results.push(MultiRunResult::from_durations(
            "Remove (all keys)",
            config.keys,
            runs,
        ));
    }

    // Compute RobinHoodTable latency profile
    rh_insert_latencies.sort_unstable();
    let n_lat = rh_insert_latencies.len();
    let rh_lat_profile = LatencyProfile {
        p50_us: f64::from(rh_insert_latencies[(n_lat * 50) / 100]) / 1000.0,
        p99_us: f64::from(rh_insert_latencies[(n_lat * 99) / 100]) / 1000.0,
        p99_9_us: f64::from(rh_insert_latencies[(n_lat * 999) / 1000]) / 1000.0,
        p99_99_us: f64::from(rh_insert_latencies[(n_lat * 9999) / 10000]) / 1000.0,
        max_us: f64::from(rh_insert_latencies[n_lat.saturating_sub(1)]) / 1000.0,
        slow_over_1ms_count: rh_slow_over_1ms,
        count_over_1ms_resize_started: rh_over_1ms_resize_started,
        count_over_1ms_migration_finished: rh_over_1ms_migration_finished,
        count_over_1ms_unchanged: rh_over_1ms_unchanged,
        slowest_5: {
            let mut ops: Vec<SlowOp> = rh_slow_heap
                .into_iter()
                .map(|Reverse((ns, idx, tag, desc))| SlowOp {
                    index: idx,
                    latency_us: f64::from(ns) / 1000.0,
                    tag,
                    description: desc,
                })
                .collect();
            ops.sort_by(|a, b| {
                b.latency_us
                    .partial_cmp(&a.latency_us)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            ops
        },
    };
    drop(rh_insert_latencies);
    drop(rh_table);

    // =========================================================================
    // 3. Benchmark std::collections::HashMap
    // =========================================================================
    println!("Benchmarking std::collections::HashMap across 6 operations (warmup + 3 runs)...");

    let mut hm_results = Vec::new();
    hm_results.push(hm_floor_loop);
    hm_results.push(hm_floor_hash);

    let mut hm_insert_latencies = Vec::with_capacity(config.keys);
    let mut hm_slow_heap: BinaryHeap<Reverse<(u32, usize, String)>> = BinaryHeap::new();
    let mut hm_slow_over_1ms = 0usize;

    // Phase 1: Insert (N keys from empty)
    {
        let mut runs = Vec::with_capacity(3);
        // Warmup run
        {
            let mut map = HashMap::<Box<[u8]>, u64>::new();
            for (idx, key) in build_key_list(config.keys).into_iter().enumerate() {
                map.insert(key, idx as u64);
            }
            std::hint::black_box(&map);
        }

        // 3 measured runs
        for run_idx in 0..3 {
            let mut map = HashMap::<Box<[u8]>, u64>::new();
            let insert_keys = build_key_list(config.keys);

            let t0 = Instant::now();
            if run_idx == 0 {
                for (idx, key) in insert_keys.into_iter().enumerate() {
                    let op_t0 = Instant::now();
                    map.insert(key, idx as u64);
                    let op_elapsed_ns = op_t0.elapsed().as_nanos().min(u128::from(u32::MAX)) as u32;

                    hm_insert_latencies.push(op_elapsed_ns);
                    if op_elapsed_ns >= 1_000_000 {
                        hm_slow_over_1ms = hm_slow_over_1ms.saturating_add(1);
                    }

                    if hm_slow_heap.len() < 5
                        || op_elapsed_ns > hm_slow_heap.peek().map_or(0, |r| r.0.0)
                    {
                        hm_slow_heap.push(Reverse((
                            op_elapsed_ns,
                            idx,
                            format!("std::HashMap insert operation #{idx}"),
                        )));
                        if hm_slow_heap.len() > 5 {
                            hm_slow_heap.pop();
                        }
                    }
                }
            } else {
                for (idx, key) in insert_keys.into_iter().enumerate() {
                    map.insert(key, idx as u64);
                }
            }
            let elapsed = t0.elapsed();
            std::hint::black_box(&map);
            runs.push(elapsed);
        }
        hm_results.push(MultiRunResult::from_durations(
            "Insert (N keys from empty)",
            config.keys,
            runs,
        ));
    }

    // Populate stable HashMap
    let mut hash_map = HashMap::<Box<[u8]>, u64>::new();
    for (idx, key) in build_key_list(config.keys).into_iter().enumerate() {
        hash_map.insert(key, idx as u64);
    }
    let hm_rss = get_vm_rss_kb();

    // Phase 2: Get hits (random order)
    {
        let mut runs = Vec::with_capacity(3);
        for &idx in &hit_indices {
            let v = hash_map.get(&keys_lookup[idx]);
            std::hint::black_box(v);
        }
        for _ in 0..3 {
            let t0 = Instant::now();
            for &idx in &hit_indices {
                let v = hash_map.get(&keys_lookup[idx]);
                std::hint::black_box(v);
            }
            runs.push(t0.elapsed());
        }
        hm_results.push(MultiRunResult::from_durations(
            "Get hits (random order)",
            config.keys,
            runs,
        ));
    }

    // Phase 3: Get misses
    {
        let mut runs = Vec::with_capacity(3);
        for k in &miss_keys {
            let v = hash_map.get(k);
            std::hint::black_box(v);
        }
        for _ in 0..3 {
            let t0 = Instant::now();
            for k in &miss_keys {
                let v = hash_map.get(k);
                std::hint::black_box(v);
            }
            runs.push(t0.elapsed());
        }
        hm_results.push(MultiRunResult::from_durations(
            "Get misses",
            config.keys,
            runs,
        ));
    }

    // Phase 4: Overwrite
    {
        let mut runs = Vec::with_capacity(3);
        let warmup_keys = build_key_list(config.keys);
        for (idx, key) in warmup_keys.into_iter().enumerate() {
            hash_map.insert(key, (idx as u64).wrapping_mul(3));
        }
        for run_idx in 0..3 {
            let run_keys = build_key_list(config.keys);
            let val = (run_idx as u64).wrapping_add(10);
            let t0 = Instant::now();
            for key in run_keys {
                hash_map.insert(key, val);
            }
            runs.push(t0.elapsed());
        }
        hm_results.push(MultiRunResult::from_durations(
            "Overwrite (all keys)",
            config.keys,
            runs,
        ));
    }

    // Phase 5: Mixed (80/15/5)
    {
        let mut runs = Vec::with_capacity(3);
        let mut mixed_rng = BenchRng::new(0xdead_beef_cafe_babe_u64);
        let warmup_ops = generate_mixed_ops(config.mixed_ops, &mut mixed_rng);
        for op in warmup_ops {
            match op {
                PreMixedOp::Get(idx) => {
                    let v = hash_map.get(&keys_lookup[idx]);
                    std::hint::black_box(v);
                }
                PreMixedOp::Insert(k, v) => {
                    hash_map.insert(k, v);
                }
                PreMixedOp::Remove(idx) => {
                    let v = hash_map.remove(&keys_lookup[idx]);
                    std::hint::black_box(v);
                }
            }
        }

        for _ in 0..3 {
            let measured_ops = generate_mixed_ops(config.mixed_ops, &mut mixed_rng);
            let t0 = Instant::now();
            for op in measured_ops {
                match op {
                    PreMixedOp::Get(idx) => {
                        let v = hash_map.get(&keys_lookup[idx]);
                        std::hint::black_box(v);
                    }
                    PreMixedOp::Insert(k, v) => {
                        hash_map.insert(k, v);
                    }
                    PreMixedOp::Remove(idx) => {
                        let v = hash_map.remove(&keys_lookup[idx]);
                        std::hint::black_box(v);
                    }
                }
            }
            runs.push(t0.elapsed());
        }
        hm_results.push(MultiRunResult::from_durations(
            "Mixed (80% get / 15% ins / 5% rem)",
            config.mixed_ops,
            runs,
        ));
    }

    // Phase 6: Remove (all keys)
    {
        let mut runs = Vec::with_capacity(3);
        let mut warmup_map = HashMap::<Box<[u8]>, u64>::new();
        for (idx, k) in build_key_list(config.keys).into_iter().enumerate() {
            warmup_map.insert(k, idx as u64);
        }
        for k in &keys_lookup {
            let v = warmup_map.remove(k);
            std::hint::black_box(v);
        }

        for _ in 0..3 {
            let mut test_map = HashMap::<Box<[u8]>, u64>::new();
            for (idx, k) in build_key_list(config.keys).into_iter().enumerate() {
                test_map.insert(k, idx as u64);
            }
            let t0 = Instant::now();
            for k in &keys_lookup {
                let v = test_map.remove(k);
                std::hint::black_box(v);
            }
            runs.push(t0.elapsed());
        }
        hm_results.push(MultiRunResult::from_durations(
            "Remove (all keys)",
            config.keys,
            runs,
        ));
    }

    hm_insert_latencies.sort_unstable();
    let n_lat_hm = hm_insert_latencies.len();
    let hm_lat_profile = LatencyProfile {
        p50_us: f64::from(hm_insert_latencies[(n_lat_hm * 50) / 100]) / 1000.0,
        p99_us: f64::from(hm_insert_latencies[(n_lat_hm * 99) / 100]) / 1000.0,
        p99_9_us: f64::from(hm_insert_latencies[(n_lat_hm * 999) / 1000]) / 1000.0,
        p99_99_us: f64::from(hm_insert_latencies[(n_lat_hm * 9999) / 10000]) / 1000.0,
        max_us: f64::from(hm_insert_latencies[n_lat_hm.saturating_sub(1)]) / 1000.0,
        slow_over_1ms_count: hm_slow_over_1ms,
        count_over_1ms_resize_started: 0,
        count_over_1ms_migration_finished: 0,
        count_over_1ms_unchanged: hm_slow_over_1ms,
        slowest_5: {
            let mut ops: Vec<SlowOp> = hm_slow_heap
                .into_iter()
                .map(|Reverse((ns, idx, desc))| SlowOp {
                    index: idx,
                    latency_us: f64::from(ns) / 1000.0,
                    tag: "n/a",
                    description: desc,
                })
                .collect();
            ops.sort_by(|a, b| {
                b.latency_us
                    .partial_cmp(&a.latency_us)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            ops
        },
    };
    drop(hm_insert_latencies);
    drop(hash_map);

    // =========================================================================
    // 4. Benchmark Resize-Start Allocation Cost and Table Drop Latency
    // =========================================================================
    println!("Measuring table allocation resize-start cost...");
    let resize_sizes: Vec<(usize, &'static str, bool)> = vec![
        (1 << 20, "2^20 (1,048,576 slots, ~42 MB)", false),
        (1 << 22, "2^22 (4,194,304 slots, ~168 MB)", false),
        (1 << 24, "2^24 (16,777,216 slots, ~671 MB)", config.is_smoke),
    ];

    let mut resize_costs = Vec::new();
    for (cap, label, skip) in &resize_sizes {
        if *skip {
            resize_costs.push((*label, "skipped in --smoke".to_string()));
            continue;
        }
        let mut runs = Vec::with_capacity(3);
        // 1 warmup run
        {
            let table = RobinHoodTable::<u64>::new(*cap, rh_seed);
            drop(table);
        }
        for _ in 0..3 {
            let t0 = Instant::now();
            let table = RobinHoodTable::<u64>::new(*cap, rh_seed);
            let elapsed = t0.elapsed();
            std::hint::black_box(&table);
            runs.push(elapsed);
            drop(table);
        }
        runs.sort();
        let median_ms = runs[1].as_secs_f64() * 1000.0;
        resize_costs.push((*label, format!("{median_ms:.2} ms")));
    }

    println!("Measuring empty table drop latency...");
    let mut drop_costs = Vec::new();
    for (cap, label, skip) in &resize_sizes {
        if *skip {
            drop_costs.push((*label, "skipped in --smoke".to_string()));
            continue;
        }
        let mut runs = Vec::with_capacity(3);
        // 1 warmup run
        {
            let table = RobinHoodTable::<u64>::new(*cap, rh_seed);
            drop(table);
        }
        for _ in 0..3 {
            let table = RobinHoodTable::<u64>::new(*cap, rh_seed);
            let t0 = Instant::now();
            drop(table);
            let elapsed = t0.elapsed();
            runs.push(elapsed);
        }
        runs.sort();
        let median_ms = runs[1].as_secs_f64() * 1000.0;
        drop_costs.push((*label, format!("{median_ms:.2} ms")));
    }

    // =========================================================================
    // 5. Output Markdown Results
    // =========================================================================
    println!();
    println!("### Throughput & Performance Comparison (Median of 3 Runs)");
    println!();
    println!(
        "| Operation | RobinHoodTable (median) | RobinHoodTable (Mops/s) | std::HashMap (median) | std::HashMap (Mops/s) | Ratio |"
    );
    println!("| :--- | :---: | :---: | :---: | :---: | :---: |");
    for i in 0..rh_results.len() {
        let rh = &rh_results[i];
        let hm = &hm_results[i];
        let ratio = hm.median_ns / rh.median_ns;
        println!(
            "| {} | {:.1} ns [{:.1} - {:.1}] | {:.2} Mops/s | {:.1} ns [{:.1} - {:.1}] | {:.2} Mops/s | **{:.2}x** |",
            rh.name,
            rh.median_ns,
            rh.min_ns,
            rh.max_ns,
            rh.mops_per_sec,
            hm.median_ns,
            hm.min_ns,
            hm.max_ns,
            hm.mops_per_sec,
            ratio
        );
    }
    println!();

    println!("### Latency Profiles: RobinHoodTable vs std::collections::HashMap");
    println!();
    println!("| Metric | RobinHoodTable | std::collections::HashMap |");
    println!("| :--- | :---: | :---: |");
    println!(
        "| p50 | {:.3} µs | {:.3} µs |",
        rh_lat_profile.p50_us, hm_lat_profile.p50_us
    );
    println!(
        "| p99 | {:.3} µs | {:.3} µs |",
        rh_lat_profile.p99_us, hm_lat_profile.p99_us
    );
    println!(
        "| p99.9 | {:.3} µs | {:.3} µs |",
        rh_lat_profile.p99_9_us, hm_lat_profile.p99_9_us
    );
    println!(
        "| p99.99 | {:.3} µs | {:.3} µs |",
        rh_lat_profile.p99_99_us, hm_lat_profile.p99_99_us
    );
    println!(
        "| max | {:.3} µs | {:.3} µs |",
        rh_lat_profile.max_us, hm_lat_profile.max_us
    );
    println!(
        "| Ops slower than 1 ms | {} | {} |",
        rh_lat_profile.slow_over_1ms_count, hm_lat_profile.slow_over_1ms_count
    );
    println!();

    println!("### Top 5 Slowest Inserts: RobinHoodTable");
    println!();
    println!("| Rank | Op Index | Latency (µs) | Tag | Capacity Details |");
    println!("| :---: | :---: | :---: | :---: | :--- |");
    for (rank, op) in rh_lat_profile.slowest_5.iter().enumerate() {
        println!(
            "| {} | {} | {:.2} µs | {} | {} |",
            rank + 1,
            op.index,
            op.latency_us,
            op.tag,
            op.description
        );
    }
    println!();

    println!("### Inserts Slower Than 1 ms by Tag: RobinHoodTable");
    println!();
    println!("| Tag | Count |");
    println!("| :--- | :---: |");
    println!(
        "| resize started | {} |",
        rh_lat_profile.count_over_1ms_resize_started
    );
    println!(
        "| migration finished, old table dropped | {} |",
        rh_lat_profile.count_over_1ms_migration_finished
    );
    println!(
        "| unchanged | {} |",
        rh_lat_profile.count_over_1ms_unchanged
    );
    println!();

    println!("### Top 5 Slowest Inserts: std::collections::HashMap");
    println!();
    println!("| Rank | Op Index | Latency (µs) | Event Description |");
    println!("| :---: | :---: | :---: | :--- |");
    for (rank, op) in hm_lat_profile.slowest_5.iter().enumerate() {
        println!(
            "| {} | {} | {:.2} µs | {} |",
            rank + 1,
            op.index,
            op.latency_us,
            op.description
        );
    }
    println!();

    println!("### Resize-Start Allocation Cost (RobinHoodTable::new)");
    println!();
    println!("| Table Capacity | Allocation Size | Creation Latency (Median) |");
    println!("| :--- | :---: | :---: |");
    for (label, cost) in resize_costs {
        println!("| {label} | {cost} |");
    }
    println!();

    println!("### Empty Table Drop Latency (drop(RobinHoodTable))");
    println!();
    println!("| Table Capacity | Allocation Size | Drop Latency (Median) |");
    println!("| :--- | :---: | :---: |");
    for (label, cost) in drop_costs {
        println!("| {label} | {cost} |");
    }
    println!();

    println!("### Hash Table Quality & Memory Metrics");
    println!();
    println!("| Metric | RobinHoodTable | std::collections::HashMap |");
    println!("| :--- | :---: | :---: |");
    println!("| Max PSL | {} | n/a |", rh_stats.max_psl);
    println!("| Average PSL | {:.2} | n/a |", rh_stats.average_psl);
    println!("| Measured Average Probes per Miss | {avg_miss_probes:.2} slots | n/a |");
    println!(
        "| Final Load Factor | {:.1}% | n/a |",
        rh_stats.load * 100.0
    );
    println!("| Table Slot Size | {rh_slot_bytes} bytes | n/a |");
    println!("| Est. Memory / Key | {rh_est_bytes_per_key:.1} bytes | n/a |");
    println!(
        "| Process VmRSS | {} | {} |",
        format_vm_rss(rh_rss),
        format_vm_rss(hm_rss)
    );
    println!();
}
