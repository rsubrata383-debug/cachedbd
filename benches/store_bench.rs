//! Benchmark suite comparing [`cachedb::store::RobinHoodTable`] against [`std::collections::HashMap`].
//!
//! Measures throughput, latency percentiles, latency spikes, table quality metrics,
//! and resident set size (VmRSS) across synthetic key-value workloads.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::time::Instant;

use cachedb::store::{RobinHoodTable, slot_size};

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
    description: String,
}

/// Summary metrics for a single measured benchmark phase.
#[derive(Debug, Clone)]
struct BenchResult {
    name: &'static str,
    ns_per_op: f64,
    mops_per_sec: f64,
}

impl BenchResult {
    fn new(name: &'static str, ops: usize, elapsed: std::time::Duration) -> Self {
        let duration_secs = elapsed.as_secs_f64();
        let ns_per_op = (elapsed.as_nanos() as f64) / (ops as f64);
        let mops_per_sec = (ops as f64) / (duration_secs * 1_000_000.0);
        Self {
            name,
            ns_per_op,
            mops_per_sec,
        }
    }
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

/// Tag describing a synthetic mixed workload operation.
enum MixedOpTag {
    Get(usize),
    Insert(usize, u64),
    Remove(usize),
}

#[allow(clippy::too_many_lines, clippy::similar_names)]
fn main() {
    let config = BenchConfig::parse();

    println!("# Cachedb Store Benchmark: RobinHoodTable vs std::collections::HashMap");
    println!();
    println!(
        "- **Configuration**: {} keys, {} mixed operations{}",
        config.keys,
        config.mixed_ops,
        if config.is_smoke { " (smoke mode)" } else { "" }
    );
    println!("- **OS**: {}", std::env::consts::OS);
    println!("- **Architecture**: {}", std::env::consts::ARCH);
    println!();

    // -------------------------------------------------------------------------
    // 0. Pre-generate keys and datasets outside timed regions
    // -------------------------------------------------------------------------
    println!(
        "Pre-generating {} benchmark keys and permutation indices...",
        config.keys
    );

    let keys: Vec<Box<[u8]>> = (0..config.keys)
        .map(|i| format!("key_{i:010}").into_bytes().into_boxed_slice())
        .collect();

    let overwrite_vals: Vec<u64> = (0..config.keys)
        .map(|i| (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .collect();

    let miss_keys: Vec<Box<[u8]>> = (0..config.keys)
        .map(|i| format!("miss_{i:010}").into_bytes().into_boxed_slice())
        .collect();

    // Pseudo-random permutation of indices for random get hits
    let mut hit_indices: Vec<usize> = (0..config.keys).collect();
    let mut rng = 0x853c_49e6_748f_ea9b_u64;
    for i in (1..config.keys).rev() {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let j = (rng as usize) % (i + 1);
        hit_indices.swap(i, j);
    }

    // Pre-generate mixed workload operations (80% get hits, 15% insert, 5% remove)
    let extra_keys_count = (config.mixed_ops * 20) / 100 + 100;
    let extra_keys: Vec<Box<[u8]>> = (0..extra_keys_count)
        .map(|i| format!("extra_{i:010}").into_bytes().into_boxed_slice())
        .collect();

    let mut mixed_ops: Vec<MixedOpTag> = Vec::with_capacity(config.mixed_ops);
    let mut extra_idx = 0usize;
    for _ in 0..config.mixed_ops {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let roll = (rng as usize) % 100;
        if roll < 80 {
            let key_idx = (rng as usize) % config.keys;
            mixed_ops.push(MixedOpTag::Get(key_idx));
        } else if roll < 95 {
            let key_idx = extra_idx % extra_keys.len();
            extra_idx = extra_idx.saturating_add(1);
            mixed_ops.push(MixedOpTag::Insert(key_idx, rng));
        } else {
            let key_idx = (rng as usize) % config.keys;
            mixed_ops.push(MixedOpTag::Remove(key_idx));
        }
    }

    println!("Dataset prepared. Starting RobinHoodTable benchmarks...\n");

    // -------------------------------------------------------------------------
    // 1. Benchmark RobinHoodTable<u64>
    // -------------------------------------------------------------------------
    let mut rh_results = Vec::new();
    let mut insert_latencies_ns = Vec::with_capacity(config.keys);
    let mut top_slowest_heap: BinaryHeap<Reverse<(u32, usize, String)>> = BinaryHeap::new();
    let mut finished_resizes = 0usize;

    let mut rh_table = RobinHoodTable::<u64>::with_default_seed(8);

    // Operation 1: Insert from empty
    {
        let t0 = Instant::now();
        for (idx, key) in keys.iter().enumerate() {
            let cap_before = rh_table.capacity();
            let resizing_before = rh_table.is_resizing();

            let op_start = Instant::now();
            let _ = rh_table.insert(key.clone(), idx as u64);
            let op_elapsed_ns = op_start.elapsed().as_nanos().min(u128::from(u32::MAX)) as u32;

            insert_latencies_ns.push(op_elapsed_ns);

            let cap_after = rh_table.capacity();
            let resizing_after = rh_table.is_resizing();

            if resizing_before && !resizing_after {
                finished_resizes = finished_resizes.saturating_add(1);
            }

            // Track top 5 slowest inserts with detailed context
            if top_slowest_heap.len() < 5
                || op_elapsed_ns > top_slowest_heap.peek().map_or(0, |r| r.0.0)
            {
                let note = if cap_after > cap_before {
                    format!("Resize started: capacity expanded {cap_before} -> {cap_after}")
                } else if resizing_before && !resizing_after {
                    format!("Resize completed: old table fully drained at cap {cap_after}")
                } else if resizing_after {
                    "Incremental migration batch active (migrated up to 16 buckets)".to_string()
                } else {
                    "Normal Robin Hood probe collision chain displacement".to_string()
                };

                top_slowest_heap.push(Reverse((op_elapsed_ns, idx, note)));
                if top_slowest_heap.len() > 5 {
                    top_slowest_heap.pop();
                }
            }
        }
        let elapsed = t0.elapsed();
        rh_results.push(BenchResult::new(
            "Insert (N keys from empty)",
            config.keys,
            elapsed,
        ));
    }

    // Capture quality stats and memory while populated
    let rh_stats = rh_table.stats();
    let rh_rss = get_vm_rss_kb();
    let rh_slot_bytes = slot_size::<u64>();
    let rh_est_bytes_per_key =
        ((rh_stats.capacity as f64) * (rh_slot_bytes as f64)) / (config.keys as f64);

    // Operation 2: Get hits (random order)
    {
        let t0 = Instant::now();
        for &idx in &hit_indices {
            let val = rh_table.get(&keys[idx]);
            std::hint::black_box(val);
        }
        let elapsed = t0.elapsed();
        rh_results.push(BenchResult::new(
            "Get hits (random order)",
            config.keys,
            elapsed,
        ));
    }

    // Operation 3: Get misses
    {
        let t0 = Instant::now();
        for miss_key in &miss_keys {
            let val = rh_table.get(miss_key);
            std::hint::black_box(val);
        }
        let elapsed = t0.elapsed();
        rh_results.push(BenchResult::new("Get misses", config.keys, elapsed));
    }

    // Operation 4: Overwrite
    {
        let t0 = Instant::now();
        for (idx, key) in keys.iter().enumerate() {
            let _ = rh_table.insert(key.clone(), overwrite_vals[idx]);
        }
        let elapsed = t0.elapsed();
        rh_results.push(BenchResult::new(
            "Overwrite (all keys)",
            config.keys,
            elapsed,
        ));
    }

    // Operation 6: Mixed (80/15/5) on existing table
    {
        let t0 = Instant::now();
        for op in &mixed_ops {
            match op {
                MixedOpTag::Get(k_idx) => {
                    let v = rh_table.get(&keys[*k_idx]);
                    std::hint::black_box(v);
                }
                MixedOpTag::Insert(extra_k_idx, val) => {
                    let _ = rh_table.insert(extra_keys[*extra_k_idx].clone(), *val);
                }
                MixedOpTag::Remove(k_idx) => {
                    let v = rh_table.remove(&keys[*k_idx]);
                    std::hint::black_box(v);
                }
            }
        }
        let elapsed = t0.elapsed();
        rh_results.push(BenchResult::new(
            "Mixed (80% get / 15% ins / 5% rem)",
            config.mixed_ops,
            elapsed,
        ));
    }

    // Operation 5: Remove all keys
    {
        let t0 = Instant::now();
        for key in &keys {
            let v = rh_table.remove(key);
            std::hint::black_box(v);
        }
        for extra in &extra_keys {
            let v = rh_table.remove(extra);
            std::hint::black_box(v);
        }
        let elapsed = t0.elapsed();
        rh_results.push(BenchResult::new("Remove (all keys)", config.keys, elapsed));
    }

    // Drop RobinHoodTable before running HashMap to prevent memory accumulation
    drop(rh_table);

    // Extract and sort top 5 slowest ops
    let mut top_slowest_ops: Vec<SlowOp> = top_slowest_heap
        .into_iter()
        .map(|Reverse((ns, idx, desc))| SlowOp {
            index: idx,
            latency_us: f64::from(ns) / 1000.0,
            description: desc,
        })
        .collect();
    top_slowest_ops.sort_by(|a, b| {
        b.latency_us
            .partial_cmp(&a.latency_us)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Compute latency percentiles
    insert_latencies_ns.sort_unstable();
    let n_lat = insert_latencies_ns.len();
    let p50_us = f64::from(insert_latencies_ns[(n_lat * 50) / 100]) / 1000.0;
    let p99_us = f64::from(insert_latencies_ns[(n_lat * 99) / 100]) / 1000.0;
    let p99_9_us = f64::from(insert_latencies_ns[(n_lat * 999) / 1000]) / 1000.0;
    let p99_99_us = f64::from(insert_latencies_ns[(n_lat * 9999) / 10000]) / 1000.0;
    let max_us = f64::from(insert_latencies_ns[n_lat.saturating_sub(1)]) / 1000.0;
    drop(insert_latencies_ns);

    println!("RobinHoodTable benchmarks completed. Starting std::collections::HashMap...\n");

    // -------------------------------------------------------------------------
    // 2. Benchmark std::collections::HashMap<Box<[u8]>, u64>
    // -------------------------------------------------------------------------
    let mut hm_results = Vec::new();
    let mut hash_map = HashMap::<Box<[u8]>, u64>::new();

    // Operation 1: Insert from empty
    {
        let t0 = Instant::now();
        for (idx, key) in keys.iter().enumerate() {
            hash_map.insert(key.clone(), idx as u64);
        }
        let elapsed = t0.elapsed();
        hm_results.push(BenchResult::new(
            "Insert (N keys from empty)",
            config.keys,
            elapsed,
        ));
    }

    // Capture HashMap memory while populated
    let hm_rss = get_vm_rss_kb();

    // Operation 2: Get hits (random order)
    {
        let t0 = Instant::now();
        for &idx in &hit_indices {
            let val = hash_map.get(&keys[idx]);
            std::hint::black_box(val);
        }
        let elapsed = t0.elapsed();
        hm_results.push(BenchResult::new(
            "Get hits (random order)",
            config.keys,
            elapsed,
        ));
    }

    // Operation 3: Get misses
    {
        let t0 = Instant::now();
        for miss_key in &miss_keys {
            let val = hash_map.get(miss_key);
            std::hint::black_box(val);
        }
        let elapsed = t0.elapsed();
        hm_results.push(BenchResult::new("Get misses", config.keys, elapsed));
    }

    // Operation 4: Overwrite
    {
        let t0 = Instant::now();
        for (idx, key) in keys.iter().enumerate() {
            hash_map.insert(key.clone(), overwrite_vals[idx]);
        }
        let elapsed = t0.elapsed();
        hm_results.push(BenchResult::new(
            "Overwrite (all keys)",
            config.keys,
            elapsed,
        ));
    }

    // Operation 6: Mixed (80/15/5)
    {
        let t0 = Instant::now();
        for op in &mixed_ops {
            match op {
                MixedOpTag::Get(k_idx) => {
                    let v = hash_map.get(&keys[*k_idx]);
                    std::hint::black_box(v);
                }
                MixedOpTag::Insert(extra_k_idx, val) => {
                    hash_map.insert(extra_keys[*extra_k_idx].clone(), *val);
                }
                MixedOpTag::Remove(k_idx) => {
                    let v = hash_map.remove(&keys[*k_idx]);
                    std::hint::black_box(v);
                }
            }
        }
        let elapsed = t0.elapsed();
        hm_results.push(BenchResult::new(
            "Mixed (80% get / 15% ins / 5% rem)",
            config.mixed_ops,
            elapsed,
        ));
    }

    // Operation 5: Remove all keys
    {
        let t0 = Instant::now();
        for key in &keys {
            let v = hash_map.remove(key);
            std::hint::black_box(v);
        }
        for extra in &extra_keys {
            let v = hash_map.remove(extra);
            std::hint::black_box(v);
        }
        let elapsed = t0.elapsed();
        hm_results.push(BenchResult::new("Remove (all keys)", config.keys, elapsed));
    }

    drop(hash_map);

    // -------------------------------------------------------------------------
    // 3. Print Benchmark Markdown Tables
    // -------------------------------------------------------------------------
    println!("### Throughput & Performance Comparison");
    println!();
    println!(
        "| Operation | RobinHoodTable (ns/op) | RobinHoodTable (Mops/s) | std::HashMap (ns/op) | std::HashMap (Mops/s) | Speedup Ratio |"
    );
    println!("| :--- | :---: | :---: | :---: | :---: | :---: |");
    for i in 0..rh_results.len() {
        let rh = &rh_results[i];
        let hm = &hm_results[i];
        let speedup = hm.ns_per_op / rh.ns_per_op;
        println!(
            "| {} | {:.1} ns | {:.2} Mops/s | {:.1} ns | {:.2} Mops/s | **{:.2}x** |",
            rh.name, rh.ns_per_op, rh.mops_per_sec, hm.ns_per_op, hm.mops_per_sec, speedup
        );
    }
    println!();

    println!(
        "### RobinHoodTable Insert Latency Distribution ({} keys)",
        config.keys
    );
    println!();
    println!("| Percentile | Latency (µs) |");
    println!("| :--- | :---: |");
    println!("| p50 | {p50_us:.3} µs |");
    println!("| p99 | {p99_us:.3} µs |");
    println!("| p99.9 | {p99_9_us:.3} µs |");
    println!("| p99.99 | {p99_99_us:.3} µs |");
    println!("| max | {max_us:.3} µs |");
    println!();

    println!("### Top 5 Slowest Inserts (RobinHoodTable)");
    println!();
    println!("| Rank | Op Index | Latency (µs) | Triggering Event / State |");
    println!("| :---: | :---: | :---: | :--- |");
    for (rank, op) in top_slowest_ops.iter().enumerate() {
        println!(
            "| {} | {} | {:.2} µs | {} |",
            rank + 1,
            op.index,
            op.latency_us,
            op.description
        );
    }
    println!();

    println!("### Hash Table Quality & Memory Metrics");
    println!();
    println!("| Metric | RobinHoodTable | std::collections::HashMap |");
    println!("| :--- | :---: | :---: |");
    println!("| Max PSL | {} | n/a |", rh_stats.max_psl);
    println!("| Average PSL | {:.2} | n/a |", rh_stats.average_psl);
    println!(
        "| Final Load Factor | {:.1}% | n/a |",
        rh_stats.load * 100.0
    );
    println!("| Finished Resizes | {finished_resizes} | n/a |");
    println!("| Table Slot Size | {rh_slot_bytes} bytes | n/a |");
    println!("| Est. Memory / Key | {rh_est_bytes_per_key:.1} bytes | n/a |");
    println!(
        "| Process VmRSS | {} | {} |",
        format_vm_rss(rh_rss),
        format_vm_rss(hm_rss)
    );
    println!();
}
