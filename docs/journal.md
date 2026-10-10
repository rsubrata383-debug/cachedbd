# Cachedb Design Journal

Record of major architecture decisions, alternatives considered, and trade-offs.

---

## 2026-10-10: Phase 0 Setup and Architecture Decisions

### Decision 1: Implementation Language

- **Options**:
  - C: Very fast, but manual memory management can cause crashes, security bugs, and buffer overflows.
  - Go: Simple concurrency and fast builds, but garbage collection pauses hurt predictable low latency.
  - Rust: Fast execution, zero-cost abstractions, memory safety at compile time, and no garbage collection.
- **Choice**: Rust (Edition 2024).
- **Reason**: Rust gives high performance without unpredictable garbage collector pauses, while the compiler prevents memory corruption.
- **What would make us revisit it**: Nothing in practice, unless standard Rust tooling becomes unusable for our target operating systems.

---

### Decision 2: Project Layout and Package Structure

- **Options**:
  - Cargo Workspace: Multiple smaller crates inside one repository.
  - Single Crate with `src/lib.rs` and binaries in `src/bin/`: One crate containing library code and separate binary entry points.
- **Choice**: Single crate with `src/lib.rs`, `src/main.rs`, and helper binaries in `src/bin/`.
- **Reason**: Simpler build configuration, unified dependencies, faster compile times at this stage, and matches the roadmap repository layout.
- **What would make us revisit it**: If compile times grow too long or we need to publish independent client libraries to crates.io.

---

### Decision 3: Core Concurrency Model

- **Options**:
  - Multi-threaded shared memory with fine-grained mutexes or locks.
  - Single-threaded event-driven core where one thread owns all stored data.
- **Choice**: Single-threaded event-driven core for command processing, with optional background helper threads for disk I/O and persistence.
- **Reason**: Commands are atomic by default without locking bugs or lock contention. It is simpler to reason about, test, and maintain.
- **What would make us revisit it**: If single-core CPU throughput cannot reach target goals and real benchmark measurements prove multi-threading is necessary.

---

### Decision 4: Server Binary Name

- **Options**:
  - `cachedb`: Matches crate package name.
  - `cachedbd`: Trailing `d` indicates daemon/server process.
- **Choice**: `cachedbd` (mapped to `src/main.rs` via `[[bin]]` in `Cargo.toml`).
- **Reason**: Distinguishes the database daemon server cleanly from client and tooling binaries.
- **What would make us revisit it**: If users expect the package name to match the server binary name directly without confusion.

---

### Decision 5: Client Shell Binary Name

- **Options**:
  - `cachedb-shell`: Emphasizes interactive REPL shell.
  - `cachedb-cli`: Standard industry convention (e.g. `redis-cli`) and matches file layout.
- **Choice**: `cachedb-cli` (`src/bin/cachedb-cli.rs`).
- **Reason**: Familiar naming convention for command-line database clients and matches repository layout.
- **What would make us revisit it**: If the tool becomes solely an interactive TUI/REPL and needs distinct naming.

---

### Decision 6: Benchmark Binary Name

- **Options**:
  - `cachedb-load`: Load generation tool.
  - `cachedb-bench`: Matches repository layout (`src/bin/cachedb-bench.rs`).
- **Choice**: `cachedb-bench`.
- **Reason**: Standard and descriptive name for database benchmarking tools.
- **What would make us revisit it**: If separate stress testing tools are split off.

---

### Decision 7: Storage File Extensions

- **Options**:
  - Journal (WAL): `.wal`, `.journal`, or `.cdbj`.
  - Snapshot: `.snap`, `.snapshot`, or `.cdbs`.
- **Choice**: `.cdbj` for journals and `.cdbs` for snapshots.
- **Reason**: Unique branded file extensions prevent accidental conflicts with other database tools on disk.
- **What would make us revisit it**: If standard tooling or cloud backup tools require generic file extensions.

---

### Decision 8: Configuration File Format

- **Options**:
  - `cachedb.conf`: Custom key-value text format.
  - `cachedb.toml`: Standard TOML format.
- **Choice**: `cachedb.toml`.
- **Reason**: Standard, structured, well-supported in Rust ecosystem, and easy to read and edit.
- **What would make us revisit it**: If minimal footprint requirements preclude TOML parsing in embedded deployments.

---

### Decision 9: Default Network Port

- **Options**:
  - `4888`, `5555`, `7379`.
- **Choice**: `7379`.
- **Reason**: Memorable, distinct from standard Redis (`6379`), and avoids commonly assigned system ports.
- **What would make us revisit it**: If IANA port conflicts arise.

---

### Decision 10: Primary Operating System

- **Options**:
  - Windows native.
  - macOS.
  - Linux.
- **Choice**: Linux (development in WSL2 / Ubuntu).
- **Reason**: Production databases overwhelmingly run on Linux. Networking primitives (`epoll`, non-blocking I/O) are Linux-native.
- **What would make us revisit it**: If cross-platform production deployments on native Windows or macOS become a primary requirement.

---

### Decision 11: Sanitizers Setup (Address, Thread, Memory)

- **Options**:
  - Set up sanitizers immediately.
  - Defer sanitizers until needed.
- **Choice**: Deferred for now. We will add a Linux sanitizer CI workflow before Week 5 (when parser fuzzing begins).
- **Reason**: Sanitizers require nightly toolchains and Linux runners. At this stage, all code is safe Rust with no unsafe blocks. Week 5 fuzzing is the appropriate milestone to enforce sanitizers.
- **What would make us revisit it**: If any unsafe block or C-FFI is introduced prior to Week 5.

---

## 2026-10-10: Phase 0 Week 2 - Wire Protocol Specification Decisions

### Decision 12: Byte Order (Endianness)

- **Date**: 2026-10-10
- **Options**:
  - Big-Endian (Network Byte Order): Standard for network protocols; reads naturally in Wireshark and hex dumps.
  - Little-Endian: Matches native CPU architecture on x86 and ARM64.
- **Choice**: Big-Endian.
- **Reason**: Standard internet conventions make packet captures easier to inspect, and endian conversion cost is negligible.
- **What would make us revisit it**: If profiling shows byte-swapping overhead on packet headers is significant under 200k ops/sec.

---

### Decision 13: Protocol Encoding Format

- **Date**: 2026-10-10
- **Options**:
  - Pure Binary (Header + typed payload items): Fast zero-copy parsing, compact, no string escape overhead.
  - Text-based (RESP or HTTP-like): Human-readable over raw netcat, but slower and requires string parsing.
- **Choice**: Pure Binary.
- **Reason**: Primary goals are extreme speed and low latency (200k ops/sec, < 1ms p99). `cachedb-cli` provides human interaction.
- **What would make us revisit it**: If manual debugging with raw TCP tools without our CLI is deemed mandatory.

---

### Decision 14: Payload Length Field Size and Large Value Handling

- **Date**: 2026-10-10
- **Options**:
  - 4 bytes (`u32`): Fixed header offset, handles payloads up to limits (32 MB / 64 MB default).
  - 2 bytes (`u16`): Saves 2 bytes per frame header, but limits single frame payloads to 64 KB.
- **Choice**: 4 bytes `u32` length field, with a configurable `max_frame_size` (default 64 MB).
- **Reason**: Simplifies frame handling so multi-frame chunking is only needed for massive payloads. The 64 MB limit can be raised in `cachedb.toml`.
- **What would make us revisit it**: We may revisit chunking with the `more-frames` flag in Phase 2, Week 15 when testing 100 MB values to prevent huge contiguous memory allocations.

---

### Decision 15: Message Framing and Boundaries

- **Date**: 2026-10-10
- **Options**:
  - Length-prefixed (`payload_len` in 14-byte header): Explicit buffer sizing, zero delimiter scanning or escaping.
  - Delimiter-terminated (`\r\n` or sentinel byte): Allows unbounded streaming without knowing length upfront.
- **Choice**: Length-prefixed header.
- **Reason**: Safe against delimiter injection attacks and eliminates byte-by-byte scanning in the hot path.
- **What would make us revisit it**: If streaming generators cannot calculate payload length in advance (multi-frame chunking solves this).
