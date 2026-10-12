# SkiffDB

> **Status: Active Development (v0.1.0)**  
> _SkiffDB is currently under heavy development and is not yet production-ready. APIs, internal layouts, and wire structures are evolving according to the project roadmap._

v0.1.0 is an early learning release, not production ready. See [docs/benchmarks.md](docs/benchmarks.md) for initial benchmark numbers and the [CHANGELOG](CHANGELOG.md) for release details. Linux is needed for the server (epoll); the client and CLI also build on Windows.

---

`SkiffDB` is a high-performance, single-threaded, event-driven, in-memory key-value data store built from the ground up in Rust. Designed for predictable sub-millisecond latencies and high concurrency, it features its own custom binary wire protocol, command dispatch engine, streaming I/O buffers, and write-ahead persistence layer.

Target workloads include:

- Low-latency **caching** and **session stores**
- In-memory **work queues** with atomic operations
- Real-time **leaderboards** and counters

---

## Architecture Overview

`SkiffDB` operates on a **single-threaded, event-driven core** model: network polling, frame parsing, command execution, and memory storage execute sequentially on one main thread without locks, mutexes, or reference counting overhead. Heavy operations (disk fsync, background compaction, snapshotting) are offloaded to dedicated background threads.

```
                      +---------------------------------------+
                      |       Client (skiff-cli / SDK)        |
                      +---------------------------------------+
                                          |
                               (Custom Wire Protocol)
                                          v
+-----------------------------------------------------------------------------------+
| Network Layer    | Non-blocking TCP, epoll (Linux/WSL), EventLoop abstraction     |
| Buffer Layer     | ReadBuffer, chunked WriteQueue, dual-watermark backpressure    |
| Protocol Layer   | Binary frame parser (magic, request ID, opcode, payload)       |
| Command Engine   | Arity validation, argument dispatch, error code reporting      |
| Storage Layer    | In-memory Robin Hood hash table, TTL expiration, memory limits |
+-----------------------------------------------------------------------------------+
                                          |
                                          v
+-----------------------------------------------------------------------------------+
| Background Tasks | Snapshotting, WAL journal fsync, replication log shipping       |
+-----------------------------------------------------------------------------------+
```

---

## Project Status & Roadmap

Progress is tracked through the milestone checklist in [`skiffdb-roadmap.md`](skiffdb-roadmap.md) and detailed design decisions in [`docs/journal.md`](docs/journal.md).

| Phase       | Milestone                          |  Status  | Key Deliverables                                                                             |
| :---------- | :--------------------------------- | :------: | :------------------------------------------------------------------------------------------- |
| **Phase 0** | **Decisions & Design**             | **Done** | Vision, threading model, and frozen `v0.1` specifications.                                   |
| **Phase 1** | **Week 3: Networking Basics**      | **Done** | Linux `epoll` syscall wrapper, `EventLoop`, non-blocking echo server.                        |
| **Phase 1** | **Week 4: Buffers & Connections**  | **Done** | `ConnectionTable`, flow-control backpressure, 10,000 idle connection scale test.             |
| **Phase 1** | **Week 5: Wire Protocol**          | **Done** | Binary frame encode/decode, streaming chunk parser, HELLO negotiation, limits, fuzz testing. |
| **Phase 1** | **Week 6: Hash Table**             | **Done** | Custom Robin Hood hash table with incremental background resizing.                           |
| **Phase 1** | **Week 7: Core Commands**          | **Done** | `ping`, `put`, `fetch`, `drop`, `has`, `size`, `bump`.                                       |
| **Phase 1** | **Week 8: Tooling & v0.1 Release** | **Done** | client library, CLI, server hardening, load generator and first benchmark.                   |

---

## Repository Layout

```text
skiffdb/
├── Cargo.toml                  # Project metadata, lints, dependencies
├── README.md                   # Project overview and developer guide
├── LICENSE-MIT                 # MIT license
├── LICENSE-APACHE              # Apache 2.0 license
├── AGENTS.md                   # Rust engineering guidelines and coding rules
├── skiffdb-roadmap.md          # Multi-phase project roadmap and checklist
├── docs/
│   ├── journal.md              # Architectural decisions and design log
│   ├── design/                 # High-level architecture decisions
│   │   ├── vision.md           # Workloads, targets, and non-goals
│   │   ├── naming.md           # Naming conventions and defaults
│   │   ├── threading.md        # Concurrency and threading model
│   │   └── hash-table.md       # Robin Hood hash table architecture
│   └── spec/                   # Frozen v0.1 specifications
│       ├── protocol.md         # Binary wire protocol format
│       ├── values.md           # Value model, encoding, and limits
│       ├── commands.md         # Command language specification
│       └── errors.md           # Structured error catalog
├── src/
│   ├── lib.rs                  # Core library entry point
│   ├── main.rs                 # Server daemon binary (skiffd)
│   ├── bin/
│   │   ├── skiff-cli.rs        # Interactive terminal client
│   │   └── skiff-bench.rs      # Benchmarking and load generation harness
│   ├── net/                    # Networking and event loop abstractions
│   │   ├── buffer.rs           # ReadBuffer, WriteQueue, RingBuffer
│   │   ├── connection.rs       # Connection and ConnectionTable registry
│   │   ├── event_loop.rs       # Single-threaded EventLoop and timers
│   │   └── sys.rs              # Linux epoll abstraction (safe RAII)
│   ├── proto/                  # Wire protocol parser and serializer
│   │   ├── decode.rs           # Zero-copy streaming frame decoder
│   │   ├── encode.rs           # Atomic frame serializer
│   │   ├── header.rs           # 14-byte wire header definition
│   │   ├── hello.rs            # HELLO handshake and version negotiation
│   │   ├── item.rs             # Value AST encoder and decoder
│   │   ├── limits.rs           # Framing limits (max_frame_size, max_args)
│   │   └── varint.rs           # LEB128 unsigned and zigzag varints
│   └── store/                  # In-memory store engine
│       ├── eviction.rs         # Memory eviction policies
│       ├── expiry.rs           # TTL expiration tracking
│       ├── hash.rs             # 64-bit seeded non-cryptographic hash function
│       ├── table.rs            # Robin Hood hash table core
│       ├── value.rs            # Value object representation
│       └── mod.rs              # Store module definitions
├── fuzz/                       # Cargo-fuzz test suite
│   ├── Cargo.toml              # Fuzz targets configuration
│   └── fuzz_targets/           # libFuzzer fuzz harnesses
├── examples/
│   ├── echo_blocking.rs        # Educational synchronous echo server
│   └── echo_nonblocking.rs     # Production event-driven non-blocking echo server
├── tests/
│   ├── integration.rs          # End-to-end integration test suite
│   ├── soak.rs                 # Scale and soak tests (10k idle connections)
│   ├── crash.rs                # Crash recovery and durability tests
│   └── property.rs             # Property-based invariants tests
└── benches/
    └── store_bench.rs          # Microbenchmarks (Criterion)
```

---

## Binaries & Default Settings

- **Server Daemon**: `skiffd`
- **Interactive Shell Client**: `skiff-cli`
- **Load Generator & Benchmark**: `skiff-bench`
- **Default Port**: `7878`
- **Primary OS Target**: Linux (kernel `epoll`). Use WSL2 on Windows.

---

## Quick Start

### 1. Build

```bash
cargo build --release
```

### 2. Run Server

Start the server daemon (requires Linux / WSL2 for epoll):

```bash
./target/release/skiffd --bind 127.0.0.1 --port 7878
```

### 3. Use the CLI (`skiff-cli`)

One-shot commands:

```bash
./target/release/skiff-cli put mykey "hello world"
./target/release/skiff-cli fetch mykey
./target/release/skiff-cli bump counter 1
```

Interactive REPL mode:

```bash
./target/release/skiff-cli --host 127.0.0.1 --port 7878
```

```text
skiff> put mykey "hello world"
"hello world"
skiff> fetch mykey
"hello world"
skiff> bump counter 1
(integer) 1
skiff> quit
```

### 4. Run the Load Generator (`skiff-bench`)

Run synthetic client workloads using `skiff-bench`:

```bash
./target/release/skiff-bench --addr 127.0.0.1:7878 --workload read --connections 4 --duration 10 --warmup 2 --pipeline 16 --preload
```

---

## Getting Started

### Prerequisites

- **Rust**: Latest stable compiler (Edition 2024, MSRV 1.85+)
- **OS**: Linux or Windows Subsystem for Linux (WSL2) recommended for native epoll networking.

### Building

```bash
# Build all library targets, binaries, and examples in debug mode
cargo build --all-targets

# Build release binaries optimized for performance
cargo build --release --bins
```

### Running Tests

```bash
# Run unit and integration tests
cargo test --all-features

# Run documentation doctests
cargo test --doc

# Run scale and soak tests (including 10,000 idle connection checks)
cargo test --test soak
```

### Quality Assurance & Linting

All contributions and agent changes must pass strict zero-warning tooling checks:

```bash
# 1. Code formatting check
cargo fmt --check

# 2. Strict Clippy checks
cargo clippy --all-targets --all-features -- -D warnings

# 3. Cross-platform Linux target check (when building from non-Linux host)
cargo clippy --target x86_64-unknown-linux-gnu --all-targets --all-features -- -D warnings

# 4. Documentation generation without broken links
cargo doc --no-deps
```

---

## Documentation Index

- **Roadmap & Tracking**: [`skiffdb-roadmap.md`](skiffdb-roadmap.md)
- **Design Journal**: [`docs/journal.md`](docs/journal.md)
- **Frozen Specifications (v0.1)**:
  - [Wire Protocol Specification](docs/spec/protocol.md)
  - [Command Language Specification](docs/spec/commands.md)
  - [Value Model & Serialization](docs/spec/values.md)
  - [Error Code Catalog](docs/spec/errors.md)
- **Engineering Guidelines**: [`AGENTS.md`](AGENTS.md)

---

## License

Dual-licensed under either:

- **MIT License** ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)

at your option.
