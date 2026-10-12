//! Benchmark load generator module for cachedb.
//!
//! Provides a configurable multi-threaded synthetic load generator,
//! command-line option parsing, workload distribution generators,
//! and fixed-size logarithmic latency histograms.

use std::fmt;
use std::io::{self, Write};
use std::net::TcpStream;
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use crate::client::Client;
use crate::proto::Value;

/// Usage and help string for the `cachedb-bench` tool.
pub const HELP_TEXT: &str = "\
Usage: cachedb-bench [OPTIONS]

Options:
  --addr HOST:PORT       Server address to connect to (default: 127.0.0.1:7878)
  --connections N        Number of concurrent client connections/threads (default: 4)
  --duration SECS        Benchmark measurement duration in seconds (default: 10)
  --warmup SECS          Warmup period in seconds before measurement (default: 2)
  --pipeline N           Number of commands pipelined per batch (default: 16)
  --keys N               Key space size (default: 100000)
  --value-size BYTES     Payload size in bytes for PUT values (default: 64)
  --workload NAME        Workload profile: read, write, mixed, bump (default: read)
  --seed N               Base random seed; must not be 0 (default: 1)
  --preload              Preload keys before measurement run
  --json                 Output results as a single JSON line
  -h, --help             Print help information";

/// Deterministic 64-bit XorShift pseudorandom number generator strictly
/// conforming to AGENTS.md rule 11.1.
#[derive(Debug, Clone)]
pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// Creates a new generator with the given non-zero seed.
    ///
    /// # Panics
    ///
    /// Panics if `seed == 0` (zero state is prohibited in XorShift).
    #[must_use]
    pub fn new(seed: u64) -> Self {
        assert_ne!(seed, 0, "XorShift64 seed must not be zero");
        Self { state: seed }
    }

    /// Emits the next pseudorandom 64-bit word.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Returns a pseudorandom integer in half-open range `[min, max)`.
    ///
    /// Extracts high-order bits (`>> 32`) and computes modulo on wide `u64`
    /// before casting to `usize`. Note: accepts `% diff` modulo bias for performance.
    ///
    /// # Panics
    ///
    /// Panics if `min >= max`.
    pub fn gen_range(&mut self, min: usize, max: usize) -> usize {
        assert!(min < max, "empty range: min must be strictly less than max");
        let diff = (max - min) as u64;
        let offset = ((self.next_u64() >> 32) % diff) as usize;
        min + offset
    }

    /// Returns a pseudorandom percentage in half-open range `[0, 100)`.
    pub fn gen_percent(&mut self) -> usize {
        ((self.next_u64() >> 32) % 100) as usize
    }

    /// Returns a pseudorandom boolean by sampling the most significant bit (MSB).
    pub fn gen_bool(&mut self) -> bool {
        (self.next_u64() >> 63) == 1
    }
}

/// Benchmarking workload profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// 95% fetch, 5% put
    Read,
    /// 100% put
    Write,
    /// 50% fetch, 40% put, 5% bump, 5% drop
    Mixed,
    /// 100% bump
    Bump,
}

impl Workload {
    /// Returns the canonical name of the workload.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Mixed => "mixed",
            Self::Bump => "bump",
        }
    }

    /// Returns a human-readable description of operation distributions.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Read => "95% fetch, 5% put",
            Self::Write => "100% put",
            Self::Mixed => "50% fetch, 40% put, 5% bump, 5% drop",
            Self::Bump => "100% bump",
        }
    }
}

impl FromStr for Workload {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "mixed" => Ok(Self::Mixed),
            "bump" => Ok(Self::Bump),
            _ => Err(format!(
                "error: unknown workload '{s}', expected read|write|mixed|bump"
            )),
        }
    }
}

impl fmt::Display for Workload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Benchmark configuration parameters parsed from command-line arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchConfig {
    /// Server address `HOST:PORT`.
    pub addr: String,
    /// Number of concurrent client connections (threads).
    pub connections: usize,
    /// Measurement duration in seconds.
    pub duration: u64,
    /// Warmup period in seconds.
    pub warmup: u64,
    /// Number of commands pipelined per batch.
    pub pipeline: usize,
    /// Key space size.
    pub keys: usize,
    /// Value size in bytes for PUT commands.
    pub value_size: usize,
    /// Workload profile.
    pub workload: Workload,
    /// Base pseudorandom seed (must not be 0).
    pub seed: u64,
    /// Whether to preload keys before the benchmark run.
    pub preload: bool,
    /// Whether to output results as a single JSON line.
    pub json: bool,
    /// Whether `-h` or `--help` was requested.
    pub show_help: bool,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:7878".to_string(),
            connections: 4,
            duration: 10,
            warmup: 2,
            pipeline: 16,
            keys: 100_000,
            value_size: 64,
            workload: Workload::Read,
            seed: 1,
            preload: false,
            json: false,
            show_help: false,
        }
    }
}

fn parse_flag<'a, I>(config: &mut BenchConfig, arg: &str, iter: &mut I) -> Result<bool, String>
where
    I: Iterator<Item = &'a String>,
{
    match arg {
        "-h" | "--help" => {
            config.show_help = true;
            return Ok(true);
        }
        "--addr" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --addr".to_string())?;
            config.addr.clone_from(val);
        }
        "--connections" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --connections".to_string())?;
            let n: usize = val
                .parse()
                .map_err(|_| format!("error: invalid number for --connections: '{val}'"))?;
            if n == 0 {
                return Err("error: --connections must be greater than 0".to_string());
            }
            config.connections = n;
        }
        "--duration" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --duration".to_string())?;
            config.duration = val
                .parse()
                .map_err(|_| format!("error: invalid number for --duration: '{val}'"))?;
        }
        "--warmup" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --warmup".to_string())?;
            config.warmup = val
                .parse()
                .map_err(|_| format!("error: invalid number for --warmup: '{val}'"))?;
        }
        "--pipeline" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --pipeline".to_string())?;
            let n: usize = val
                .parse()
                .map_err(|_| format!("error: invalid number for --pipeline: '{val}'"))?;
            if n == 0 {
                return Err("error: --pipeline must be greater than 0".to_string());
            }
            config.pipeline = n;
        }
        "--keys" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --keys".to_string())?;
            let n: usize = val
                .parse()
                .map_err(|_| format!("error: invalid number for --keys: '{val}'"))?;
            if n == 0 {
                return Err("error: --keys must be greater than 0".to_string());
            }
            config.keys = n;
        }
        "--value-size" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --value-size".to_string())?;
            config.value_size = val
                .parse()
                .map_err(|_| format!("error: invalid number for --value-size: '{val}'"))?;
        }
        "--workload" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --workload".to_string())?;
            config.workload = val.parse()?;
        }
        "--seed" => {
            let val = iter
                .next()
                .ok_or_else(|| "error: missing argument for --seed".to_string())?;
            let s: u64 = val
                .parse()
                .map_err(|_| format!("error: invalid number for --seed: '{val}'"))?;
            if s == 0 {
                return Err("error: --seed must not be 0".to_string());
            }
            config.seed = s;
        }
        "--preload" => {
            config.preload = true;
        }
        "--json" => {
            config.json = true;
        }
        unknown => {
            return Err(format!("error: unrecognized option '{unknown}'"));
        }
    }
    Ok(false)
}

/// Parses command-line arguments into [`BenchConfig`].
///
/// # Errors
///
/// Returns an error message string if an argument is missing, invalid, or unrecognized.
pub fn parse_args(args: &[String]) -> Result<BenchConfig, String> {
    let mut config = BenchConfig::default();
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if parse_flag(&mut config, arg, &mut iter)? {
            break;
        }
    }

    Ok(config)
}

const SUB_BUCKET_BITS: u32 = 6;
const NUM_SUB_BUCKETS: usize = 1 << SUB_BUCKET_BITS;
const NUM_BUCKETS: usize = 3776;

/// Fixed-size logarithmic latency histogram with zero allocation in recording paths.
#[derive(Debug, Clone)]
pub struct Histogram {
    buckets: Box<[u64]>,
    count: u64,
    total_nanos: u128,
    max_nanos: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    /// Constructs a new empty histogram with preallocated fixed-size buckets.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: vec![0u64; NUM_BUCKETS].into_boxed_slice(),
            count: 0,
            total_nanos: 0,
            max_nanos: 0,
        }
    }

    /// Records a latency duration into the histogram without dynamic allocation.
    pub fn record(&mut self, duration: Duration) {
        let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.record_nanos(nanos);
    }

    /// Records a nanosecond value into the histogram.
    pub fn record_nanos(&mut self, nanos: u64) {
        let b = Self::bucket_for_nanos(nanos);
        self.buckets[b] += 1;
        self.count += 1;
        self.total_nanos = self.total_nanos.saturating_add(u128::from(nanos));
        if nanos > self.max_nanos {
            self.max_nanos = nanos;
        }
    }

    /// Returns the total number of recorded observations.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// Returns the maximum recorded latency.
    #[must_use]
    pub const fn max(&self) -> Duration {
        Duration::from_nanos(self.max_nanos)
    }

    /// Returns the p50 (median) latency duration.
    #[must_use]
    pub fn p50(&self) -> Duration {
        self.percentile(50.0)
    }

    /// Returns the p99 latency duration.
    #[must_use]
    pub fn p99(&self) -> Duration {
        self.percentile(99.0)
    }

    /// Returns the p99.9 latency duration.
    #[must_use]
    pub fn p99_9(&self) -> Duration {
        self.percentile(99.9)
    }

    /// Returns the requested percentile duration between 0.0 and 100.0.
    #[must_use]
    pub fn percentile(&self, p: f64) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }
        let p_clamped = p.clamp(0.0, 100.0);
        let target = ((p_clamped / 100.0) * self.count as f64).ceil() as u64;
        let target = target.max(1).min(self.count);

        let mut cumulative = 0u64;
        for (i, &cnt) in self.buckets.iter().enumerate() {
            cumulative = cumulative.saturating_add(cnt);
            if cumulative >= target {
                let ns = Self::nanos_for_bucket(i);
                let clamped_ns = ns.min(self.max_nanos);
                return Duration::from_nanos(clamped_ns);
            }
        }
        Duration::from_nanos(self.max_nanos)
    }

    /// Merges observations from another histogram into this one.
    pub fn merge(&mut self, other: &Self) {
        self.count += other.count;
        self.total_nanos = self.total_nanos.saturating_add(other.total_nanos);
        if other.max_nanos > self.max_nanos {
            self.max_nanos = other.max_nanos;
        }
        for (a, b) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *a += *b;
        }
    }

    fn bucket_for_nanos(ns: u64) -> usize {
        if ns < NUM_SUB_BUCKETS as u64 {
            ns as usize
        } else {
            let leading = ns.leading_zeros();
            let exp = 63 - leading;
            let mantissa =
                ((ns >> (exp - SUB_BUCKET_BITS)) & (NUM_SUB_BUCKETS as u64 - 1)) as usize;
            let bucket = ((exp - SUB_BUCKET_BITS + 1) as usize) * NUM_SUB_BUCKETS + mantissa;
            bucket.min(NUM_BUCKETS - 1)
        }
    }

    fn nanos_for_bucket(bucket: usize) -> u64 {
        if bucket < NUM_SUB_BUCKETS {
            bucket as u64
        } else {
            let offset = bucket - NUM_SUB_BUCKETS;
            let exp = (offset / NUM_SUB_BUCKETS) as u32 + SUB_BUCKET_BITS;
            let mantissa = (offset % NUM_SUB_BUCKETS) as u64;
            let lower = (1u64 << exp) | (mantissa << (exp - SUB_BUCKET_BITS));
            if exp > SUB_BUCKET_BITS {
                let half_step = 1u64 << (exp - SUB_BUCKET_BITS - 1);
                lower.saturating_add(half_step)
            } else {
                lower
            }
        }
    }
}

/// Generates a batch of commands for the specified workload.
///
/// In the [`Workload::Mixed`] profile, bump operations target an isolated `"cnt:<idx>"`
/// key space while fetch, put, and drop target `"key:<idx>"`. This separation guarantees
/// that `bump` never encounters stored string or byte data, avoiding `ERR_NOT_AN_INTEGER` (3004).
#[must_use]
pub fn generate_batch(
    rng: &mut XorShift64,
    workload: Workload,
    batch_size: usize,
    keys: usize,
    value_payload: &[u8],
) -> Vec<Vec<Value>> {
    let mut batch = Vec::with_capacity(batch_size);

    for _ in 0..batch_size {
        let key_idx = rng.gen_range(0, keys);
        match workload {
            Workload::Read => {
                // 95% fetch, 5% put
                if rng.gen_percent() < 95 {
                    batch.push(vec![
                        Value::Text("fetch".to_string()),
                        Value::Text(format!("key:{key_idx}")),
                    ]);
                } else {
                    batch.push(vec![
                        Value::Text("put".to_string()),
                        Value::Text(format!("key:{key_idx}")),
                        Value::Bytes(value_payload.to_vec()),
                    ]);
                }
            }
            Workload::Write => {
                // 100% put
                batch.push(vec![
                    Value::Text("put".to_string()),
                    Value::Text(format!("key:{key_idx}")),
                    Value::Bytes(value_payload.to_vec()),
                ]);
            }
            Workload::Mixed => {
                // 50% fetch, 40% put, 5% bump, 5% drop
                let roll = rng.gen_percent();
                if roll < 50 {
                    batch.push(vec![
                        Value::Text("fetch".to_string()),
                        Value::Text(format!("key:{key_idx}")),
                    ]);
                } else if roll < 90 {
                    batch.push(vec![
                        Value::Text("put".to_string()),
                        Value::Text(format!("key:{key_idx}")),
                        Value::Bytes(value_payload.to_vec()),
                    ]);
                } else if roll < 95 {
                    // Separate counter key space avoids ERR_NOT_AN_INTEGER (3004)
                    batch.push(vec![
                        Value::Text("bump".to_string()),
                        Value::Text(format!("cnt:{key_idx}")),
                        Value::Int(1),
                    ]);
                } else {
                    batch.push(vec![
                        Value::Text("drop".to_string()),
                        Value::Text(format!("key:{key_idx}")),
                    ]);
                }
            }
            Workload::Bump => {
                // 100% bump
                batch.push(vec![
                    Value::Text("bump".to_string()),
                    Value::Text(format!("cnt:{key_idx}")),
                    Value::Int(1),
                ]);
            }
        }
    }

    batch
}

/// Aggregated benchmark execution report.
#[derive(Debug, Clone)]
pub struct BenchReport {
    /// Configuration options used for this run.
    pub config: BenchConfig,
    /// Total operations successfully submitted across all threads.
    pub total_ops: u64,
    /// Total logical command error replies received.
    pub total_errors: u64,
    /// Actual wall-clock measurement duration.
    pub duration: Duration,
    /// Calculated aggregate operations per second.
    pub ops_per_sec: f64,
    /// Combined latency distribution across all worker threads.
    pub histogram: Histogram,
    /// Thread error messages if any worker crashed or disconnected.
    pub thread_errors: Vec<String>,
}

impl BenchReport {
    /// Returns true if any worker thread experienced a connection or transport failure.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        !self.thread_errors.is_empty()
    }

    /// Prints a human-readable results table to the given writer.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if writing fails.
    pub fn print_table<W: Write>(&self, out: &mut W) -> io::Result<()> {
        writeln!(out, "# Cachedb Benchmark Report")?;
        writeln!(out)?;
        writeln!(out, "Target:            {}", self.config.addr)?;
        writeln!(
            out,
            "Workload:          {} ({})",
            self.config.workload.name(),
            self.config.workload.description()
        )?;
        writeln!(out, "Connections:       {}", self.config.connections)?;
        writeln!(
            out,
            "Pipeline batch:    {} commands/batch",
            self.config.pipeline
        )?;
        writeln!(out, "Keyspace:          {} keys", self.config.keys)?;
        writeln!(out, "Value size:        {} bytes", self.config.value_size)?;
        writeln!(
            out,
            "Duration:          {:.2}s (warmup: {:.2}s)",
            self.duration.as_secs_f64(),
            self.config.warmup
        )?;
        writeln!(out)?;
        writeln!(out, "| Metric | Value |")?;
        writeln!(out, "| :--- | :--- |")?;
        writeln!(out, "| Total Operations | {} ops |", self.total_ops)?;
        writeln!(out, "| Throughput | {:.2} ops/sec |", self.ops_per_sec)?;
        writeln!(out, "| Command Errors | {} |", self.total_errors)?;
        writeln!(
            out,
            "| Latency Measurement | Batch round-trip ({} cmds/batch) |",
            self.config.pipeline
        )?;
        writeln!(
            out,
            "| Latency p50 | {:.3} ms |",
            self.histogram.p50().as_secs_f64() * 1000.0
        )?;
        writeln!(
            out,
            "| Latency p99 | {:.3} ms |",
            self.histogram.p99().as_secs_f64() * 1000.0
        )?;
        writeln!(
            out,
            "| Latency p99.9 | {:.3} ms |",
            self.histogram.p99_9().as_secs_f64() * 1000.0
        )?;
        writeln!(
            out,
            "| Latency max | {:.3} ms |",
            self.histogram.max().as_secs_f64() * 1000.0
        )?;

        if !self.thread_errors.is_empty() {
            writeln!(out)?;
            writeln!(out, "Worker Thread Errors ({}):", self.thread_errors.len())?;
            for err in &self.thread_errors {
                writeln!(out, "  - {err}")?;
            }
        }

        Ok(())
    }

    /// Serializes key benchmark metrics into a single JSON line.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\"workload\":\"{}\",\"connections\":{},\"pipeline\":{},\"keys\":{},\
            \"value_size\":{},\"duration_s\":{:.2},\"warmup_s\":{:.2},\"total_ops\":{},\
            \"ops_per_sec\":{:.2},\"errors\":{},\"p50_ms\":{:.4},\"p99_ms\":{:.4},\
            \"p99_9_ms\":{:.4},\"max_ms\":{:.4},\"thread_failures\":{}}}",
            self.config.workload.name(),
            self.config.connections,
            self.config.pipeline,
            self.config.keys,
            self.config.value_size,
            self.duration.as_secs_f64(),
            self.config.warmup,
            self.total_ops,
            self.ops_per_sec,
            self.total_errors,
            self.histogram.p50().as_secs_f64() * 1000.0,
            self.histogram.p99().as_secs_f64() * 1000.0,
            self.histogram.p99_9().as_secs_f64() * 1000.0,
            self.histogram.max().as_secs_f64() * 1000.0,
            self.thread_errors.len()
        )
    }
}

struct WorkerResult {
    ops: u64,
    errors: u64,
    histogram: Histogram,
    error: Option<String>,
}

fn preload_keys(config: &BenchConfig, value_payload: &[u8]) -> Result<(), String> {
    let mut client = Client::connect(&config.addr)
        .map_err(|e| format!("preload connection to {} failed: {e}", config.addr))?;

    let batch_size = config.pipeline.max(1);
    let mut batch = Vec::with_capacity(batch_size);

    for idx in 0..config.keys {
        batch.push(vec![
            Value::Text("put".to_string()),
            Value::Text(format!("key:{idx}")),
            Value::Bytes(value_payload.to_vec()),
        ]);

        if batch.len() >= batch_size {
            let replies = client
                .pipeline(std::mem::take(&mut batch))
                .map_err(|e| format!("preload pipeline write failed: {e}"))?;
            for reply in replies {
                if let Err(e) = reply {
                    return Err(format!("preload command rejected: {e}"));
                }
            }
        }
    }

    if !batch.is_empty() {
        let replies = client
            .pipeline(batch)
            .map_err(|e| format!("preload final pipeline failed: {e}"))?;
        for reply in replies {
            if let Err(e) = reply {
                return Err(format!("preload command rejected: {e}"));
            }
        }
    }

    Ok(())
}

fn run_worker(
    cfg: &BenchConfig,
    payload: &[u8],
    thread_idx: usize,
    warmup_duration: Duration,
    measure_duration: Duration,
) -> WorkerResult {
    let thread_seed = cfg
        .seed
        .wrapping_mul(0x517c_c1b7_2722_0a95)
        .wrapping_add(((thread_idx as u64) + 1) * 2 + 1);

    let mut rng = XorShift64::new(thread_seed);

    let mut client = match Client::connect(&cfg.addr) {
        Ok(c) => c,
        Err(e) => {
            return WorkerResult {
                ops: 0,
                errors: 0,
                histogram: Histogram::new(),
                error: Some(format!("worker {thread_idx} connect error: {e}")),
            };
        }
    };

    if !warmup_duration.is_zero() {
        let warmup_start = Instant::now();
        while warmup_start.elapsed() < warmup_duration {
            let batch = generate_batch(&mut rng, cfg.workload, cfg.pipeline, cfg.keys, payload);
            if client.pipeline(batch).is_err() {
                break;
            }
        }
    }

    let mut histogram = Histogram::new();
    let mut ops = 0u64;
    let mut errors = 0u64;
    let mut thread_error = None;
    let measure_start = Instant::now();

    while measure_start.elapsed() < measure_duration {
        let batch = generate_batch(&mut rng, cfg.workload, cfg.pipeline, cfg.keys, payload);
        let batch_len = batch.len();
        let batch_start = Instant::now();

        match client.pipeline(batch) {
            Ok(replies) => {
                let batch_duration = batch_start.elapsed();
                histogram.record(batch_duration);
                ops += batch_len as u64;

                for reply in replies {
                    if reply.is_err() {
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                thread_error = Some(format!("worker {thread_idx} pipeline error: {e}"));
                break;
            }
        }
    }

    WorkerResult {
        ops,
        errors,
        histogram,
        error: thread_error,
    }
}

/// Executes the full benchmark run across concurrent worker threads.
///
/// # Errors
///
/// Returns an error string if initial server connection fails or preloading errors out.
pub fn run_benchmark(config: &BenchConfig) -> Result<BenchReport, String> {
    TcpStream::connect(&config.addr)
        .map_err(|e| format!("cannot connect to server at {}: {e}", config.addr))?;

    let value_payload: Vec<u8> = (0..config.value_size).map(|i| (i % 251) as u8).collect();

    if config.preload {
        preload_keys(config, &value_payload)?;
    }

    let warmup_duration = Duration::from_secs(config.warmup);
    let measure_duration = Duration::from_secs(config.duration);

    let mut handles = Vec::with_capacity(config.connections);

    for thread_idx in 0..config.connections {
        let cfg = config.clone();
        let payload = value_payload.clone();

        let handle = thread::spawn(move || {
            run_worker(
                &cfg,
                &payload,
                thread_idx,
                warmup_duration,
                measure_duration,
            )
        });

        handles.push(handle);
    }

    let run_start = Instant::now();
    let mut total_ops = 0u64;
    let mut total_errors = 0u64;
    let mut combined_histogram = Histogram::new();
    let mut thread_errors = Vec::new();

    for handle in handles {
        match handle.join() {
            Ok(result) => {
                total_ops += result.ops;
                total_errors += result.errors;
                combined_histogram.merge(&result.histogram);
                if let Some(err) = result.error {
                    thread_errors.push(err);
                }
            }
            Err(_) => {
                thread_errors.push("worker thread panicked".to_string());
            }
        }
    }

    let actual_duration = run_start.elapsed().max(measure_duration);
    let ops_per_sec = if actual_duration.as_secs_f64() > 0.0 {
        total_ops as f64 / actual_duration.as_secs_f64()
    } else {
        0.0
    };

    Ok(BenchReport {
        config: config.clone(),
        total_ops,
        total_errors,
        duration: actual_duration,
        ops_per_sec,
        histogram: combined_histogram,
        thread_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_defaults() {
        let args: Vec<String> = Vec::new();
        let config = parse_args(&args).expect("parse empty args");

        assert_eq!(config.addr, "127.0.0.1:7878");
        assert_eq!(config.connections, 4);
        assert_eq!(config.duration, 10);
        assert_eq!(config.warmup, 2);
        assert_eq!(config.pipeline, 16);
        assert_eq!(config.keys, 100_000);
        assert_eq!(config.value_size, 64);
        assert_eq!(config.workload, Workload::Read);
        assert_eq!(config.seed, 1);
        assert!(!config.preload);
        assert!(!config.json);
        assert!(!config.show_help);
    }

    #[test]
    fn test_parse_args_every_option() {
        let args = vec![
            "--addr".to_string(),
            "10.0.0.1:9999".to_string(),
            "--connections".to_string(),
            "8".to_string(),
            "--duration".to_string(),
            "30".to_string(),
            "--warmup".to_string(),
            "5".to_string(),
            "--pipeline".to_string(),
            "32".to_string(),
            "--keys".to_string(),
            "50000".to_string(),
            "--value-size".to_string(),
            "128".to_string(),
            "--workload".to_string(),
            "mixed".to_string(),
            "--seed".to_string(),
            "42".to_string(),
            "--preload".to_string(),
            "--json".to_string(),
        ];
        let config = parse_args(&args).expect("parse custom args");

        assert_eq!(config.addr, "10.0.0.1:9999");
        assert_eq!(config.connections, 8);
        assert_eq!(config.duration, 30);
        assert_eq!(config.warmup, 5);
        assert_eq!(config.pipeline, 32);
        assert_eq!(config.keys, 50_000);
        assert_eq!(config.value_size, 128);
        assert_eq!(config.workload, Workload::Mixed);
        assert_eq!(config.seed, 42);
        assert!(config.preload);
        assert!(config.json);
    }

    #[test]
    fn test_parse_args_bad_numbers() {
        let args = vec!["--connections".to_string(), "abc".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec!["--connections".to_string(), "0".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec!["--pipeline".to_string(), "0".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec!["--keys".to_string(), "0".to_string()];
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn test_parse_args_seed_zero_rejected() {
        let args = vec!["--seed".to_string(), "0".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert!(err.contains("must not be 0"));
    }

    #[test]
    fn test_parse_args_unknown_option_and_workload() {
        let args = vec!["--unknown".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec!["--workload".to_string(), "invalid".to_string()];
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn test_histogram_empty() {
        let hist = Histogram::new();
        assert_eq!(hist.count(), 0);
        assert_eq!(hist.max(), Duration::ZERO);
        assert_eq!(hist.p50(), Duration::ZERO);
        assert_eq!(hist.p99(), Duration::ZERO);
        assert_eq!(hist.p99_9(), Duration::ZERO);
    }

    #[test]
    fn test_histogram_single_value() {
        let mut hist = Histogram::new();
        hist.record(Duration::from_millis(10));
        assert_eq!(hist.count(), 1);
        assert_eq!(hist.max(), Duration::from_millis(10));

        let p50_ms = hist.p50().as_secs_f64() * 1000.0;
        assert!((p50_ms - 10.0).abs() < 0.5);
    }

    #[test]
    fn test_histogram_many_values() {
        let mut hist = Histogram::new();
        for i in 1..=100 {
            hist.record(Duration::from_millis(i));
        }
        assert_eq!(hist.count(), 100);
        assert_eq!(hist.max(), Duration::from_millis(100));

        let p50_ms = hist.p50().as_secs_f64() * 1000.0;
        let p99_ms = hist.p99().as_secs_f64() * 1000.0;
        assert!((p50_ms - 50.0).abs() < 2.0);
        assert!((p99_ms - 99.0).abs() < 2.0);
    }

    #[test]
    fn test_histogram_bucket_edges() {
        let mut hist = Histogram::new();
        let edges = [63, 64, 65, 127, 128, 129, 1023, 1024, 1025];
        for &ns in &edges {
            hist.record_nanos(ns);
        }
        assert_eq!(hist.count(), edges.len() as u64);
        assert_eq!(hist.max(), Duration::from_nanos(1025));
    }

    #[test]
    fn test_histogram_merge() {
        let mut h1 = Histogram::new();
        let mut h2 = Histogram::new();

        for i in 1..=50 {
            h1.record(Duration::from_millis(i));
        }
        for i in 51..=100 {
            h2.record(Duration::from_millis(i));
        }

        h1.merge(&h2);
        assert_eq!(h1.count(), 100);
        assert_eq!(h1.max(), Duration::from_millis(100));

        let p50_ms = h1.p50().as_secs_f64() * 1000.0;
        assert!((p50_ms - 50.0).abs() < 2.0);
    }

    #[test]
    fn test_workload_distribution_read() {
        let mut rng = XorShift64::new(42);
        let batch = generate_batch(&mut rng, Workload::Read, 10_000, 1_000, &[0u8; 16]);
        let fetches = batch
            .iter()
            .filter(|cmd| cmd[0] == Value::Text("fetch".to_string()))
            .count() as u32;
        let puts = batch
            .iter()
            .filter(|cmd| cmd[0] == Value::Text("put".to_string()))
            .count() as u32;

        let fetch_pct = f64::from(fetches) / 100.0;
        let put_pct = f64::from(puts) / 100.0;

        assert!((fetch_pct - 95.0).abs() < 1.5, "fetch_pct: {fetch_pct}");
        assert!((put_pct - 5.0).abs() < 1.5, "put_pct: {put_pct}");
    }

    #[test]
    fn test_workload_distribution_write() {
        let mut rng = XorShift64::new(42);
        let batch = generate_batch(&mut rng, Workload::Write, 1_000, 1_000, &[0u8; 16]);
        assert!(
            batch
                .iter()
                .all(|cmd| cmd[0] == Value::Text("put".to_string()))
        );
    }

    #[test]
    fn test_workload_distribution_bump() {
        let mut rng = XorShift64::new(42);
        let batch = generate_batch(&mut rng, Workload::Bump, 1_000, 1_000, &[0u8; 16]);
        assert!(
            batch
                .iter()
                .all(|cmd| cmd[0] == Value::Text("bump".to_string()))
        );
    }

    #[test]
    fn test_workload_distribution_mixed() {
        let mut rng = XorShift64::new(12345);
        let batch = generate_batch(&mut rng, Workload::Mixed, 20_000, 1_000, &[0u8; 16]);

        let mut fetches: u32 = 0;
        let mut puts: u32 = 0;
        let mut bumps: u32 = 0;
        let mut drops: u32 = 0;

        for cmd in &batch {
            match &cmd[0] {
                Value::Text(t) if t == "fetch" => fetches += 1,
                Value::Text(t) if t == "put" => puts += 1,
                Value::Text(t) if t == "bump" => {
                    bumps += 1;
                    match &cmd[1] {
                        Value::Text(k) => assert!(k.starts_with("cnt:"), "key was {k}"),
                        _ => panic!("unexpected key type"),
                    }
                }
                Value::Text(t) if t == "drop" => drops += 1,
                other => panic!("unexpected command verb: {other:?}"),
            }
        }

        let fetch_pct = f64::from(fetches) / 200.0;
        let put_pct = f64::from(puts) / 200.0;
        let bump_pct = f64::from(bumps) / 200.0;
        let drop_pct = f64::from(drops) / 200.0;

        assert!((fetch_pct - 50.0).abs() < 1.5, "fetch_pct was {fetch_pct}");
        assert!((put_pct - 40.0).abs() < 1.5, "put_pct was {put_pct}");
        assert!((bump_pct - 5.0).abs() < 1.0, "bump_pct was {bump_pct}");
        assert!((drop_pct - 5.0).abs() < 1.0, "drop_pct was {drop_pct}");
    }
}
