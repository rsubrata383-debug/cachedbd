# Cachedb: 1-Year Roadmap to Build an Original In-Memory Database From Scratch

> **Working codename:** Cachedb.
> **Goal:** a fast, durable, networked, in-memory data store with its own wire protocol, command language, data structures, storage formats, replication, and tooling. Nothing copied; every format and API is your own design.

---

## How to Use This Roadmap

- Each phase has **weekly tasks** as checkboxes, a **deliverable**, and **exit criteria**. Do not move on until the exit criteria pass.
- Every phase ends with a tagged release (`v0.1`, `v0.2`, ...) that runs end to end.
- Keep a **design journal** (`docs/journal.md`). For every major decision write: the options, the choice, the reason, and what would make you revisit it.
- Write the **spec before the code** for every new subsystem (protocol, journal format, replication). Specs live in `docs/spec/`.

## Guiding Principles

1. **Correct first, fast second, featureful third.**
2. **One thread owns the data.** Start with a single-threaded core so every command is atomic by construction. Add parallelism only with measurements to justify it.
3. **Everything is testable.** Design components (clock, network, disk) behind interfaces so tests can fake them.
4. **Measure before optimizing.** Keep a benchmark history file from Phase 1 onward.
5. **Durability claims must be proven by crash tests**, not by reading your own code.

## Repository Layout (suggested)

```
cachedb/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── AGENTS.md
├── docs/
│   ├── spec/
│   ├── design/
│   └── journal.md
├── src/
│   ├── lib.rs              # declares all modules
│   ├── main.rs             # thin: starts the server
│   ├── net/
│   │   ├── mod.rs
│   │   ├── event_loop.rs
│   │   ├── connection.rs
│   │   └── buffer.rs
│   ├── proto/
│   │   ├── mod.rs
│   │   ├── encode.rs
│   │   └── decode.rs
│   ├── cmd/
│   │   ├── mod.rs
│   │   ├── parse.rs
│   │   ├── dispatch.rs
│   │   └── handlers/
│   │       └── mod.rs
│   ├── store/
│   │   ├── mod.rs
│   │   ├── table.rs
│   │   ├── value.rs
│   │   ├── expiry.rs
│   │   └── eviction.rs
│   ├── types/
│   │   ├── mod.rs
│   │   ├── sequence.rs
│   │   ├── map.rs
│   │   ├── pool.rs
│   │   ├── ladder.rs
│   │   └── bitset.rs
│   ├── persist/
│   │   ├── mod.rs
│   │   ├── journal.rs
│   │   ├── snapshot.rs
│   │   └── recovery.rs
│   ├── repl/
│   │   └── mod.rs
│   ├── admin/
│   │   ├── mod.rs
│   │   ├── metrics.rs
│   │   ├── slow_log.rs
│   │   ├── config.rs
│   │   └── auth.rs
│   └── bin/
│       ├── cachedb-cli.rs      # shell client
│       └── cachedb-bench.rs    # benchmark tool
├── tests/                      # integration tests only
│   ├── common/
│   │   └── mod.rs              # shared test helpers (test harness)
│   ├── integration.rs
│   ├── property.rs
│   ├── crash.rs
│   └── soak.rs
├── benches/
│   └── store_bench.rs          # criterion benchmarks
└── fuzz/                       # made by `cargo fuzz init`
    └── fuzz_targets/
        └── decode.rs
```

## Architecture Overview

| Layer       | Responsibility                                                       |
| ----------- | -------------------------------------------------------------------- |
| Network     | Accept connections, non-blocking read/write, buffering, backpressure |
| Protocol    | Frame parsing/serialization, versioning, pipelining, request IDs     |
| Command     | Parse commands, validate arguments, dispatch to handlers             |
| Store       | Keyspace, value objects, TTL, memory accounting, eviction            |
| Types       | Rich value types with their own operations                           |
| Persistence | Write-ahead journal, snapshots, recovery, compaction                 |
| Replication | Leader/follower log shipping, resync, failover                       |
| Admin       | Auth, config, metrics, slow log, health, tooling                     |

---

# Phase 0: Decisions and Design (Weeks 1-2)

**Deliverable:** a written design doc and a repo with CI that builds and runs an empty test suite.

### Week 1: Choices and setup

- [x] Choose the implementation language (Rust) and write down why.
- [x] Set up the repo, build system, formatter, linter, and a CI pipeline (build + tests on every push).
- [x] Set up sanitizers (address, undefined behavior, thread) or the equivalent for your language.
- [x] Create the `docs/journal.md` and `docs/spec/` folders.
- [x] Write the project vision: target workloads (cache, session store, queue, leaderboard), non-goals, and performance targets (for example 200k ops/sec on one core, p99 latency under 1 ms).
- [x] Decide naming: project, binary names (`cachedbd` server, `cachedb-cli` client), file extensions, and default port.

### Week 2: Core specs

- [x] Draft the **wire protocol spec** (see Appendix A for a starting sketch).
- [x] Draft the **command language spec** (see Appendix B).
- [x] Draft the **value model**: which types exist, their limits (max key size, max value size), and how errors are reported.
- [x] Write the **threading model** decision (single-threaded core plus optional helper threads for I/O and background work).
- [x] Define the error code catalog (numbers plus messages).
- [x] Review everything once, fix contradictions, and freeze `spec v0.1`.

**Exit criteria:** specs reviewed; CI green; you can explain the whole architecture in 10 minutes without notes.

---

# Phase 1: Network, Protocol, and Core Store (Weeks 3-8)

**Deliverable:** `v0.1`: a server and a shell client that can store and fetch text values over your own protocol.

### Week 3: Networking basics

- [x] Write a blocking TCP echo server to learn the socket API.
- [x] Rewrite it as a non-blocking server using `epoll` (Linux) / `kqueue` (BSD/macOS) / IOCP (Windows). Pick the primary OS and abstract the rest.
- [x] Implement an event loop abstraction: register fd, readable/writable callbacks, timers.
- [x] Handle partial reads/writes, `EAGAIN`, connection close, and signals.

### Week 4: Buffers and connections

- [x] Implement a connection object with a read buffer and a write queue.
- [x] Implement a growable ring buffer or chunked buffer with zero-copy where practical.
- [x] Add backpressure: stop reading from a client whose output queue is too large.
- [x] Add connection limits, idle timeouts, and graceful shutdown.
- [x] Test with 10,000 idle connections.

### Week 5: Wire protocol

- [x] Implement frame encode/decode per your spec (magic, version, length, request ID, opcode, payload).
- [x] Implement a streaming parser that handles frames split across reads and multiple frames in one read.
- [x] Add protocol version negotiation (a `HELLO` exchange).
- [x] Add hard limits (max frame size, max argument count) to prevent abuse.
- [x] Fuzz the parser for an hour with no crashes.

### Week 6: Hash table

- [x] Design and implement your own hash table (choose: open addressing with robin hood probing, or chained buckets).
- [x] Choose a hash function (implement your own, for example a variant of a well-known non-cryptographic design), and add per-process random seeding.
- [x] Implement **incremental resizing** (migrate a few buckets per operation) so no operation causes a long pause.
- [x] Write property tests comparing it to a trivial reference map under random operations.
- [x] Benchmark insert/lookup/delete at 1M, 10M keys; record numbers.

### Week 7: Commands, part 1

- [x] Implement the command dispatcher (table of name to handler, with arity and flags).
- [x] Implement `ping`, `put`, `fetch`, `drop`, `has`, `size` (see Appendix B).
- [x] Implement `bump` (atomic integer increment/decrement) with overflow checks.
- [x] Implement `put` options: only-if-absent, only-if-present.
- [x] Implement structured errors with codes.

### Week 8: Client and tooling

- [x] Build `cachedb-cli`, an interactive client with history, pipelined script mode, and pretty printing.
- [ ] Build a small client library (one language) with connection pooling and pipelining.
- [ ] Build a basic load generator (`cachedb-bench`) with configurable key size, value size, read/write ratio, and connection count.
- [ ] Write the first benchmark report and commit it.
- [ ] Tag `v0.1`.

**Exit criteria:** 100k+ ops/sec on one core with pipelining; no memory leaks under a 1-hour soak test; parser fuzzing clean.

---

# Phase 2: Cache Behavior (Weeks 9-16)

**Deliverable:** `v0.2`: a real cache with expiry, memory limits, and eviction.

### Week 9: Memory accounting

- [ ] Define a per-key overhead model and track total memory used (including allocator overhead).
- [ ] Implement a custom allocator layer (or wrap the system one) with usage counters.
- [ ] Add a `memory` command that reports per-key and total usage.
- [ ] Investigate fragmentation: write a test that churns keys of varying sizes and measure waste.

### Week 10: Expiry (TTL)

- [ ] Store expiry timestamps with each key (monotonic clock for deadlines; wall clock only for display).
- [ ] Implement **lazy expiry** (check on access).
- [ ] Implement **active expiry** with your own scheme: choose between a timer wheel, a min-heap, or probabilistic sampling, and justify the choice.
- [ ] Add `expire`, `expire-at`, `ttl`, `persist`.
- [ ] Add a fake clock interface so expiry can be tested deterministically.

### Week 11: Eviction, part 1

- [ ] Define eviction policy configuration: `none`, `random`, `lru`, `lfu`, `ttl-first`.
- [ ] Implement exact LRU (hash table plus intrusive linked list) as a baseline.
- [ ] Implement a `max-memory` setting and the check-before-write path.
- [ ] Return a clear error when memory is full and eviction is `none`.

### Week 12: Eviction, part 2

- [ ] Implement an approximate LRU (sampling-based or clock-based) to cut per-key overhead.
- [ ] Implement LFU with counters that decay over time.
- [ ] Implement an admission policy experiment (a TinyLFU-style frequency sketch).
- [ ] Build a **cache simulator** that replays access traces and reports hit rate for each policy.
- [ ] Write up a comparison in the journal: hit rate vs memory overhead vs CPU cost.

### Week 13: Keyspace tools

- [ ] Implement `scan`: cursor-based iteration that never blocks the server and tolerates resizing.
- [ ] Implement pattern matching for key names (your own glob matcher).
- [ ] Implement `rename`, `copy`, `type`, `random-key`.
- [ ] Implement namespaces (multiple logical keyspaces) with `use <name>`.

### Week 14: Server hygiene

- [ ] Config system: config file plus command-line flags plus runtime `config get/set`.
- [ ] Structured logging with levels.
- [ ] Metrics: ops/sec per command, hit/miss, evictions, expired keys, memory, connected clients.
- [ ] `stats` command and a plain-text metrics endpoint.
- [ ] A slow command log with a configurable threshold.

### Week 15: Large-value handling

- [ ] Add max value size and reject oversized values cleanly.
- [ ] Add incremental freeing: deleting a huge value must not stall the loop (free in chunks or hand off to a helper thread).
- [ ] Add optional value compression (implement a simple LZ-style compressor of your own, behind a flag).
- [ ] Measure latency spikes with 100 MB values.

### Week 16: Stabilize and release

- [ ] Run a 24-hour soak test with mixed workload; track memory and latency over time.
- [ ] Fix every leak and latency spike found.
- [ ] Write the Phase 2 benchmark report.
- [ ] Tag `v0.2`.

**Exit criteria:** server stays at its memory limit under sustained writes with no growth; p99 latency stays stable during eviction; deterministic TTL tests pass.

---

# Phase 3: Data Types (Weeks 17-26)

**Deliverable:** `v0.3`: rich value types, each with tests and docs.

Each type follows the same routine: spec the commands, implement the structure, add the commands, add property tests against a simple reference model, benchmark, document.

### Week 17: Type system foundation

- [ ] Define the value object layout: type tag, encoding tag, expiry, eviction metadata, payload.
- [ ] Implement type checking and a uniform "wrong type" error.
- [ ] Implement **dual encodings**: a compact encoding for small collections and a general one for large ones, with automatic conversion at thresholds.

### Weeks 18-19: Sequences (ordered list / deque)

- [ ] Structure: a chunked array deque or a linked list of compact blocks.
- [ ] Commands: push/pop at both ends, get by index, range, length, trim, insert before/after, remove by value.
- [ ] Blocking pop with timeout (wake waiting clients when data arrives).
- [ ] Tests: random operations vs a reference deque.

### Week 20: Tables (field-to-value maps)

- [ ] Structure: compact packed array for small tables; hash table for large ones.
- [ ] Commands: set field, get field, delete field, get all, field count, increment field, has field.
- [ ] Optional: per-field expiry (decide in the journal whether to include it).

### Week 21: Pools (unique sets)

- [ ] Structure: compact sorted integer array when all members are integers; hash set otherwise.
- [ ] Commands: add, remove, is-member, count, random member, pop.
- [ ] Set algebra: union, intersection, difference, with size-based algorithm choice.

### Weeks 22-23: Ladders (ranked sets, ordered by score)

- [ ] Implement a **skip list** from scratch (with span counts so rank lookup is fast).
- [ ] Pair it with a hash table for member-to-score lookup.
- [ ] Commands: add/update, remove, score of member, rank of member, range by rank, range by score, count in range, pop min/max.
- [ ] Alternative experiment: implement an order-statistic balanced tree and compare.
- [ ] Heavy property testing; this is the most bug-prone structure.

### Week 24: Bit arrays and counters

- [ ] Bit array: set/get bit, count set bits, bitwise operations across keys, find first set bit.
- [ ] Probabilistic counter for unique-counting (implement a HyperLogLog-style estimator from the original paper).
- [ ] Optional: a Bloom-filter type with configurable false-positive rate.

### Week 25: Streams / append-only logs (optional but valuable)

- [ ] Structure: a sequence of entries with monotonically increasing IDs, stored in compact blocks.
- [ ] Commands: append, read range, read from ID, trim by length or age.
- [ ] Consumer groups with acknowledgements (stretch).

### Week 26: Type hardening and release

- [ ] Cross-type tests (keys changing type, rename across types, expiry on every type).
- [ ] Memory accounting audit for every type.
- [ ] Benchmark each type; record complexity vs measured behavior.
- [ ] Write the data type reference docs.
- [ ] Tag `v0.3`.

**Exit criteria:** every type passes property tests against its reference model; no operation on any type is worse than its documented complexity.

---

# Phase 4: Durability (Weeks 27-34)

**Deliverable:** `v0.4`: data survives crashes and restarts, with measured guarantees.

### Week 27: Journal design

- [ ] Write the **journal spec**: record format (length, type, payload, checksum), segment files, sequence numbers, versioning.
- [ ] Choose a checksum (implement CRC32C or similar yourself).
- [ ] Define what is logged: deterministic command effects (not raw commands that depend on time or randomness).
- [ ] Define the sync policies: `always`, `every-N-ms`, `never`.

### Week 28: Journal writer

- [ ] Implement append with a buffered writer and the chosen sync policy.
- [ ] Implement **group commit**: batch multiple clients' writes into one sync.
- [ ] Implement segment rotation by size.
- [ ] Measure throughput and latency for each sync policy.

### Week 29: Recovery

- [ ] Implement journal replay at startup.
- [ ] Handle a torn final record (truncate cleanly) and detect corruption mid-file (stop and report).
- [ ] Make replay deterministic (expiry times stored as absolute timestamps in the record).
- [ ] Add a `verify-journal` tool.

### Week 30: Snapshot design

- [ ] Write the **snapshot format spec**: header, type-tagged entries, per-section checksums, footer.
- [ ] Decide the snapshot method: fork with copy-on-write, or a consistent incremental iterator that yields to the event loop. Document the trade-offs.
- [ ] Define how a snapshot and journal position relate (snapshot records the journal sequence number it covers).

### Week 31: Snapshot implementation

- [ ] Implement snapshot writing in the background without blocking clients.
- [ ] Write to a temp file, `fsync`, then atomically rename.
- [ ] Implement snapshot loading, and fast loading with pre-sized tables.
- [ ] Add scheduled snapshots and a manual `snapshot` command.

### Week 32: Compaction and retention

- [ ] After a snapshot, delete journal segments it fully covers.
- [ ] Implement journal compaction (rewrite from current state when the journal grows too large).
- [ ] Add disk-space checks and a clear failure mode when the disk is full.

### Week 33: Crash testing

- [ ] Build a **fault-injection harness**: kill the process at random points, truncate files, flip bits, fail `fsync`.
- [ ] Build a crash-consistency test: run a workload, kill at random times, restart, and verify the recovered state matches an acknowledged-writes model.
- [ ] Run thousands of iterations; fix every inconsistency.

### Week 34: Release

- [ ] Document the durability guarantees per sync policy in plain language.
- [ ] Benchmark recovery time vs dataset size.
- [ ] Add backup/restore tooling (`cachedb-backup`).
- [ ] Tag `v0.4`.

**Exit criteria:** 10,000 random-kill crash tests with zero acknowledged-write loss under the `always` policy and bounded loss under the others; recovery time documented.

---

# Phase 5: Transactions, Messaging, and Replication (Weeks 35-42)

**Deliverable:** `v0.5`: atomic multi-command operations, pub/sub, and a working leader/follower pair.

### Week 35: Transactions

- [ ] Design a transaction model: queued commands applied atomically, with all-or-nothing validation.
- [ ] Implement `begin`, `commit`, `discard`.
- [ ] Implement optimistic locking: `watch <key>` aborts the transaction if the key changed.
- [ ] Log transactions to the journal as a single atomic record.

### Week 36: Server-side scripting (stretch, or a simpler alternative)

- [ ] Option A: embed a small scripting engine so scripts run atomically on the server.
- [ ] Option B: implement a built-in set of **compound commands** (compare-and-set, get-and-delete, conditional increment) that cover the common need without a script engine.
- [ ] Document whichever you choose and why.

### Week 37: Publish/subscribe

- [ ] Channels, pattern subscriptions, publish, subscriber counts.
- [ ] Push messages over the same connection using your protocol's push frame type.
- [ ] Protect against slow subscribers (drop policy or disconnect when the output queue limit is exceeded).

### Week 38: Replication design

- [ ] Write the **replication spec**: handshake, roles, log sequence numbers, full sync, partial resync, heartbeat, acknowledgement.
- [ ] Decide consistency semantics: asynchronous by default, with an optional `wait-for-N-followers` command.
- [ ] Define a replication backlog (ring buffer of recent journal records).

### Week 39: Replication implementation

- [ ] Implement the follower handshake and full sync (send snapshot, then stream the log).
- [ ] Implement continuous log streaming from leader to followers.
- [ ] Implement partial resync: a reconnecting follower sends its last sequence number; the leader replays from the backlog if available.
- [ ] Implement read-only follower mode.

### Week 40: Failure handling

- [ ] Heartbeats and failure detection.
- [ ] Manual promotion: `promote` turns a follower into a leader and tells others to follow.
- [ ] Handle split-brain risk: leaders reject writes if they cannot reach a configured minimum number of followers (optional flag).
- [ ] Handle a follower that falls too far behind (force full resync).

### Week 41: Replication testing

- [ ] Build a network fault proxy (delay, drop, partition, reorder).
- [ ] Test: partitions, restarts, slow followers, leader crash mid-write.
- [ ] Write an invariant checker: every acknowledged write exists on the promoted follower within the documented bound.

### Week 42: Release

- [ ] Docs for replication topologies and operational runbooks.
- [ ] Benchmarks with 0, 1, and 2 followers.
- [ ] Tag `v0.5`.

**Exit criteria:** failover test suite passes under injected faults; documented data-loss window matches measurements.

---

# Phase 6: Security, Scale, Hardening, and Release (Weeks 43-52)

**Deliverable:** `v1.0`: a documented, tested, benchmarked release.

### Week 43: Security

- [ ] Authentication (passwords hashed with a standard slow hash from a vetted library; do not invent cryptography).
- [ ] Access control: per-user permissions on commands and key patterns.
- [ ] TLS support via a vetted library; do not implement TLS yourself.
- [ ] Bind-address safety defaults (listen on localhost unless configured).
- [ ] Rate limits and a maximum client count.

### Weeks 44-46: Pick ONE scale-out stretch goal

Choose based on your interests; do not attempt all three.

**Option A: Sharded cluster**

- [ ] Hash-slot design (for example, 16k slots) and a slot-to-node map.
- [ ] Client-side routing and redirect responses.
- [ ] Gossip or coordinator-based membership.
- [ ] Online slot migration between nodes.

**Option B: Multi-core server**

- [ ] Shard the keyspace across N worker threads, each owning its partition and running its own event loop.
- [ ] Route commands to the owning thread; implement cross-shard multi-key commands carefully (or restrict them).
- [ ] Benchmark scaling from 1 to N cores.

**Option C: Disk-backed storage tier**

- [ ] Implement an LSM-tree or B-tree engine for values larger than memory.
- [ ] Keep hot data in memory with the existing eviction policy as the cache layer.
- [ ] Define read/write paths and consistency with the journal.

### Week 47: Performance engineering

- [ ] Profile with a sampling profiler; produce flame graphs for read-heavy, write-heavy, and mixed loads.
- [ ] Fix the top 5 bottlenecks (common ones: allocation, copying, syscalls, cache misses).
- [ ] Try batching syscalls (`writev`, vectored I/O) and the newest async I/O interface your OS offers.
- [ ] Re-run all benchmarks and update the history file.

### Week 48: Hardening

- [ ] Fuzz the protocol, command arguments, journal reader, and snapshot reader for long runs.
- [ ] Run sanitizers on the full integration suite.
- [ ] Add resource-exhaustion tests (huge keys, huge pipelines, many connections, slow clients).
- [ ] Review every `TODO` and known-issue note; fix or document.

### Week 49: Operations

- [ ] Health and readiness checks.
- [ ] Rolling-restart procedure and zero-downtime upgrade notes.
- [ ] Config validation with helpful messages.
- [ ] Packaging: static binary, container image, service-manager unit file.

### Week 50: Documentation

- [ ] User guide (quick start, command reference, data types, persistence, replication).
- [ ] Operator guide (config, monitoring, backup, failure scenarios).
- [ ] Internals guide (architecture, formats, algorithms; this doubles as your portfolio piece).
- [ ] Protocol and file-format specs published and versioned.

### Week 51: Client ecosystem

- [ ] Polish the first client library; add a second in another language.
- [ ] Publish a client conformance test-suite so anyone can verify a client.
- [ ] Example applications: session store, rate limiter, job queue, leaderboard.

### Week 52: Release

- [ ] Final full regression: unit, property, integration, crash, replication fault, 7-day soak.
- [ ] Final benchmark report with methodology.
- [ ] Write the retrospective: what worked, what you would change, what comes next.
- [ ] Tag `v1.0`, publish, and announce.

**Exit criteria:** all test suites green; 7-day soak with no leaks or latency drift; docs complete.

---

# Appendices

## Appendix A: Wire Protocol Sketch (your own design; refine in Week 2)

```
Frame header (fixed size):
  magic        2 bytes   protocol identifier
  version      1 byte
  flags        1 byte    (request / response / push / error / more-frames)
  request_id   4 bytes   lets clients match responses and pipeline freely
  opcode       2 bytes
  payload_len  4 bytes

Payload: a sequence of typed items
  type byte + length (varint) + data
  types: nil, int, float, bool, text, bytes, array, map, error
```

Design questions to settle in the spec: byte order, maximum sizes, how errors carry codes, how push messages are distinguished, how to extend with new types without breaking old clients.

## Appendix B: Starting Command Set (your own naming)

| Group        | Commands                                                                                               |
| ------------ | ------------------------------------------------------------------------------------------------------ |
| Connection   | `hello`, `ping`, `auth`, `use`, `quit`                                                                 |
| Values       | `put`, `fetch`, `drop`, `has`, `size`, `bump`, `append`, `swap`                                        |
| Expiry       | `expire`, `expire-at`, `ttl`, `persist`                                                                |
| Keyspace     | `scan`, `rename`, `copy`, `type`, `random-key`, `wipe`                                                 |
| Sequences    | `seq-push-front`, `seq-push-back`, `seq-pop-front`, `seq-pop-back`, `seq-range`, `seq-len`, `seq-trim` |
| Tables       | `tab-set`, `tab-get`, `tab-drop`, `tab-all`, `tab-len`, `tab-bump`                                     |
| Pools        | `pool-add`, `pool-drop`, `pool-has`, `pool-len`, `pool-union`, `pool-inter`, `pool-diff`               |
| Ladders      | `lad-add`, `lad-drop`, `lad-score`, `lad-rank`, `lad-range`, `lad-count`, `lad-pop-min`, `lad-pop-max` |
| Bits         | `bit-set`, `bit-get`, `bit-count`, `bit-op`                                                            |
| Messaging    | `subscribe`, `unsubscribe`, `publish`                                                                  |
| Transactions | `begin`, `commit`, `discard`, `watch`                                                                  |
| Admin        | `config-get`, `config-set`, `stats`, `slowlog`, `snapshot`, `memory`                                   |
| Replication  | `follow`, `promote`, `repl-status`, `wait-for`                                                         |

## Appendix C: Testing Strategy

| Layer           | Method                                                             |
| --------------- | ------------------------------------------------------------------ |
| Unit            | Every data structure and parser function                           |
| Property-based  | Random operation sequences compared to a trivial reference model   |
| Integration     | Real server, real client, scripted scenarios                       |
| Fuzzing         | Protocol parser, command arguments, journal and snapshot readers   |
| Crash           | Random kills, torn writes, bit flips, failed syncs                 |
| Fault injection | Network partitions, delays, reordering (replication)               |
| Soak            | 24-hour and 7-day mixed workloads with memory and latency tracking |
| Benchmark       | Fixed scenarios; results appended to a history file every phase    |

## Appendix D: Benchmark Scenarios

1. Read-only, small values, 50 connections, pipelined and non-pipelined.
2. Write-only, small values, with each journal sync policy.
3. 90/10 read/write mix, 1 KB values.
4. Large values (100 KB, 10 MB) and their effect on tail latency.
5. Eviction under memory pressure (hit rate and latency).
6. Each data type's hot operations at 1k, 100k, and 10M elements.
7. Recovery time vs dataset size.
8. Replication lag under write load.

Record for each: throughput, p50/p99/p99.9 latency, CPU use, memory use.

## Appendix E: Risk Register

| Risk                                     | Where it hits | Mitigation                                            |
| ---------------------------------------- | ------------- | ----------------------------------------------------- |
| Scope creep                              | All phases    | Treat optional items as optional; cut them first      |
| Hard bugs in ranked sets and persistence | Phases 3-4    | Property tests and crash harness before features      |
| Burnout or lost momentum                 | Months 4-8    | Keep each week's goal small; ship a tag every phase   |
| Falling behind schedule                  | Any           | Drop stretch goals, never drop testing or persistence |
| Premature optimization                   | Phases 1-3    | Benchmarks decide, not intuition                      |
| Data loss claims you can't back          | Phase 4       | Only document guarantees your crash tests prove       |

## Appendix F: Reading List (concepts, not code to copy)

- _Database Internals_ by Alex Petrov: storage engines and distributed systems
- _Designing Data-Intensive Applications_ by Martin Kleppmann: replication, consistency, trade-offs
- William Pugh's original skip list paper
- The HyperLogLog paper (Flajolet et al.)
- The TinyLFU admission policy paper
- The Raft consensus paper (if you extend replication to automatic election)
- _The Linux Programming Interface_ by Michael Kerrisk: sockets, I/O multiplexing, `fork`, file sync semantics
- _Systems Performance_ by Brendan Gregg: profiling and measurement

## Appendix G: Weekly Rhythm

| Day      | Focus                                            |
| -------- | ------------------------------------------------ |
| Day 1    | Write or update the spec for this week's work    |
| Days 2-4 | Implement, with tests written alongside          |
| Day 5    | Benchmark, profile, and fix regressions          |
| Day 6    | Write the journal entry; tidy the code           |
| Day 7    | Rest, or read one chapter or paper from the list |

## Appendix H: Milestone Summary

| Tag    | Week | What you can show                                     |
| ------ | ---- | ----------------------------------------------------- |
| `v0.1` | 8    | Own protocol, store, shell client, benchmarks         |
| `v0.2` | 16   | Cache with expiry, memory limit, eviction, metrics    |
| `v0.3` | 26   | Sequences, tables, pools, ranked sets, bits, counters |
| `v0.4` | 34   | Journal, snapshots, recovery, crash-tested durability |
| `v0.5` | 42   | Transactions, pub/sub, replication and failover       |
| `v1.0` | 52   | Secured, scaled, hardened, documented release         |
