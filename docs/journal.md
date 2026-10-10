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
