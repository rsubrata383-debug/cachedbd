# Cachedb Project Vision

This document describes what Cachedb is, what it is for, and what it is not.

## Target Workloads

Cachedb is made for four common workloads:

1. **Cache**
   - Fast key-value reads and writes in memory.
   - Automatic expiry with Time-To-Live (TTL).
   - Memory limits with eviction policies (LRU, LFU, random).

2. **Session Store**
   - Fast lookups and updates for user logins, tokens, and temporary session state.
   - High write throughput and predictable low latency.
   - Expiration on inactivity.

3. **Queue**
   - Fast ordered list and deque operations.
   - Push and pop operations for job workers and background tasks.

4. **Leaderboard**
   - Fast sorted ranking by score.
   - Efficient range queries and rank lookups.

## Non-Goals

Cachedb does not try to do everything. These are non-goals:

- **Not a relational database**: No SQL parser, no table joins, and no multi-table ACID transactions.
- **Not a document store**: No deep JSON path queries, search indexes, or nested document engines.
- **Not a distributed consensus cluster**: The focus is a fast single-node core first, followed by simple leader-follower replication.
- **No plugin runtime**: No embedded Lua or Wasm runtimes in the core engine. The core remains small, safe, and predictable.

## Performance Targets

These targets guide our design and benchmarks:

- **Throughput**: 200,000 operations per second on one CPU core with command pipelining.
- **Latency**: 99th percentile (p99) response time under 1 millisecond.
- **Startup & Recovery**: Fast replay of journals on restart with minimal downtime.

## Why We Chose Rust

We chose Rust for four simple reasons:

1. **No Garbage Collection**: There are no GC pauses, so latency stays predictable under load.
2. **Memory Safety**: The borrow checker prevents buffer overflows, use-after-free bugs, and data races without runtime cost.
3. **High Performance**: Zero-cost abstractions let us write clean high-level code that runs as fast as C.
4. **Great Tooling**: The compiler, package manager (`cargo`), linter (`clippy`), and formatter (`rustfmt`) work together out of the box.

