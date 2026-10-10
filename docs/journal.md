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

---

### Decision 16 / Lesson: Hex Examples Verification and Partial TCP Reads

- **Date**: 2026-10-10
- **Context**: The original Example 6 used dummy placeholder bytes for multi-frame chunks (`AA BB...`), which failed protocol item decoding when joined because `0xAA` is not a valid type tag.
- **Fix**: Replaced Example 6 with a real `PING` request frame arriving across two TCP reads cut mid-payload. Added Example 7 with a `PONG` response frame cut mid-header after 5 bytes. Both represent realistic non-blocking TCP socket reads where the parser must buffer partial data without error.
- **Key Lesson**: *"Hex examples must be checked by a script."* Manually written hex dumps are error-prone; running automated decoders against example bytes ensures the spec is 100% executable and correct.
- **Open Questions Recorded**: Multi-frame application-level chunking (`FLAG_MORE_FRAMES`) details are kept under question for the owner (interleaving rules, timeout policies, push applicability).

---

### Decision 17: Reservation of Multi-Frame Flag (`FLAG_MORE_FRAMES`) in Protocol v0.1

- **Date**: 2026-10-10
- **Options**:
  - Implement full application-level chunking with `FLAG_MORE_FRAMES` (`0x08`) in v0.1.
  - Mark `FLAG_MORE_FRAMES` (`0x08`) as reserved in v0.1; reject frames setting it with error `1002` (`ERR_UNSUPPORTED_FLAG`) and drop connection.
- **Choice**: Reserve `FLAG_MORE_FRAMES` (`0x08`) in v0.1.
- **Rules Established**:
  - Clients and servers MUST NOT set this bit in v0.1.
  - Receivers must reply with error `1002` (`ERR_UNSUPPORTED_FLAG`, `"unsupported flag"`) and close the connection if encountered.
  - Push messages (`request_id = 0`) will never be chunked.
  - Chunk interleaving and reassembly rules are deferred to Phase 2, Week 15.
- **Reason**: Simplifies the initial network parser and prevents complex connection-level reassembly buffer state machines. Configurable `max_frame_size` (default 64 MB, up to 128 MB+) covers large payloads in v0.1.
- **What would make us revisit it**: 100 MB value tests in Phase 2, Week 15 with latency measurements. If allocating contiguous buffers causes latency spikes or memory fragmentation, multi-frame chunking will be implemented then.

---

## 2026-10-10: Phase 0 Week 2 - Value Model and Error Catalog Decisions

### Decision 18: Key Representation (Binary vs UTF-8 Text)

- **Date**: 2026-10-10
- **Options**:
  - Arbitrary Binary (`Bytes`): Keys can contain any bytes `0x00`-`0xFF` up to key limit.
  - UTF-8 Text Only (`Text`): Keys must be valid UTF-8 strings.
- **Choice**: Arbitrary Binary (`Bytes`).
- **Rule**: Shell client (`cachedb-cli`) must display non-printable or unusual bytes in a safe escaped form.
- **Reason**: Maximum flexibility for applications storing binary UUIDs or hashed keys; zero UTF-8 validation overhead on the engine lookup hot path.
- **What would make us revisit it**: If text-only debugging across third-party tools requires strictly rejecting non-UTF-8 bytes.

---

### Decision 19: Empty Key Allowance

- **Date**: 2026-10-10
- **Options**:
  - Disallow empty keys (length $\ge 1$, error `2006` `ERR_EMPTY_KEY` on 0-byte key).
  - Allow empty keys (length 0 valid).
- **Choice**: Disallow empty keys (minimum 1 byte, error `2006`).
- **Reason**: Prevents common user application bugs where unset string variables (`key = ""`) silently overwrite the same empty-string database entry.
- **What would make us revisit it**: If compliance with an external standard or testing framework requires supporting 0-byte keys.

---

### Decision 20: Error Code Organization (Grouped Ranges vs Flat)

- **Date**: 2026-10-10
- **Options**:
  - Grouped Ranges by Subsystem (1000s = Protocol, 2000s = Command, 3000s = Types, 4000s = Storage, 5000s = Auth, 6000s = Replication, 7000s = Server).
  - Flat sequential numbering (1, 2, 3...).
- **Choice**: Grouped ranges with gaps.
- **Reason**: Allows client libraries to inspect error ranges (e.g. `1000..=1999` means disconnect immediately) and leaves ample gaps for future feature additions.
- **What would make us revisit it**: If compact single-byte status codes are prioritized over extensible numeric catalogs.

---

### Decision 21: Default Max Key Size Limit (1 KB Default, Configurable)

- **Date**: 2026-10-10
- **Options**:
  - 64 KB: Permissive limit, but allows massive keys that cause memory bloat and hash bucket inefficiency.
  - 1 KB (1,024 bytes): Default limit, plenty of space for real-world hierarchical keys (e.g. `tenant:user:session:uuid`) while keeping index overhead bounded. Configurable via `max_key_size` in `cachedb.toml`.
  - 256 bytes: Very strict, might reject long URLs or compound keys.
- **Choice**: 1 KB (1,024 bytes default, configurable).
- **Reason**: Protects against memory bloat from oversized keys while providing ample room for standard naming conventions. Operators with special requirements can raise `max_key_size` in `cachedb.toml`.
- **What would make us revisit it**: If major target workloads require embedding large serialized payloads or multi-field tokens directly within key strings.

---

## 2026-10-10: Phase 0 Week 2 - Command Language Specification Decisions

### Decision 22: Command Name Case Sensitivity

- **Date**: 2026-10-10
- **Options**:
  - Case-Insensitive ASCII: Server converts command names to lowercase.
  - Strictly Lowercase: Server requires exact lowercase bytes over the wire.
- **Choice**: Strictly lowercase command names over the wire.
- **Rule**: If a wire frame contains uppercase letters in the command name, the server returns error `2001` (`ERR_UNKNOWN_COMMAND`). The interactive client (`cachedb-cli`) converts the first word to lowercase before sending.
- **Reason**: Maximum server dispatch speed via direct byte comparison with zero case-folding overhead on the network loop.
- **What would make us revisit it**: If raw socket clients report significant user friction from case sensitivity errors.

---

### Decision 23: Result of `fetch` on Missing Key

- **Date**: 2026-10-10
- **Options**:
  - Return `Nil` (`0x00`): Standard cache miss representation.
  - Return Error `4001` (`ERR_KEY_NOT_FOUND`): Explicit error reply.
- **Choice**: Return `Nil` (`0x00`).
- **Reason**: Cache misses are standard expected states; returning errors forces expensive exception handling in client SDKs.
- **What would make us revisit it**: If a future strict relational or document layer requires hard error propagation on missing items.

---

### Decision 24: Return Type of `drop`

- **Date**: 2026-10-10
- **Options**:
  - `Int` count: Number of keys deleted (`1` if deleted, `0` if not present).
  - `Bool`: `true` if deleted, `false` if not found.
- **Choice**: `Int` count of dropped keys.
- **Reason**: Consistent return shape that scales seamlessly from single-key deletion to multi-key deletion (`drop key1 key2 key3`).
- **What would make us revisit it**: If saving a single byte on single-key deletions becomes critical.

---

### Decision 25: Return Type of `put`

- **Date**: 2026-10-10
- **Options**:
  - `Bool`: `true` on store, `false` on conditional failure (`nx`/`xx`).
  - `Text("ok")` on success, `Nil` on conditional failure.
- **Choice**: `Bool` (`true` on success, `false` if `nx` or `xx` condition prevented write).
- **Reason**: Unambiguous boolean success indicator; homogeneous return type regardless of whether conditions are used.
- **What would make us revisit it**: If drop-in Redis wire compatibility (`"OK"`) is requested by users.

---

### Decision 26: Command Options Parsing Style

- **Date**: 2026-10-10
- **Options**:
  - Keyword string arguments (e.g. `"nx"`, `"xx"` passed as trailing tokens).
  - Positional arguments (fixed index in array).
- **Choice**: Lowercase keyword string arguments (`"nx"`, `"xx"`).
- **Reason**: Clean readability and straightforward extensibility for upcoming options (e.g. expiry TTL).
- **What would make us revisit it**: If option parsing in the command dispatcher is measured to be a bottleneck.

---

## 2026-10-10: Phase 0 Week 2 - Threading Model and Architecture Decisions

### Decision 27: Handling `unsafe` and OS Syscalls for the `epoll` Event Loop

- **Date**: 2026-10-10
- **Options**:
  - Option A: Use `libc` directly with narrowly scoped `unsafe` in `src/net/sys.rs` (safe API to rest of crate).
  - Option B: Use a safe wrapper crate like `mio`, keeping `unsafe_code = "forbid"`.
  - Option C: Use standard library `std::net` non-blocking sockets with polling in pure safe Rust.
- **Choice**: Option A with encapsulation in `src/net/sys.rs`.
- **Implementation Rules**:
  - Put all `unsafe` code in one small module, `src/net/sys.rs`, exposing a 100% safe API to the rest of the code.
  - Keep `src/net/event_loop.rs` completely free of `unsafe` code.
  - Use the `libc` crate, only on Linux (`#[cfg(target_os = "linux")]`).
  - Do not add `libc` or change `Cargo.toml` now; keep `unsafe_code = "forbid"` until Phase 1 Week 3.
  - **Phase 1 Week 3 To-Do**: Change `unsafe_code` lint in `Cargo.toml` from `"forbid"` to `"deny"`, and allow `unsafe` only in `net/sys.rs`.
  - Every `unsafe` block must have a `// SAFETY:` comment explaining why it is sound.
- **Reason**: Keeps `unsafe` strictly isolated to a single dedicated system-call wrapper module while building our own event loop from scratch per the roadmap; the rest of the codebase (including `event_loop.rs`) remains 100% safe Rust.
- **What would make us revisit it**: If maintaining custom `sys.rs` bindings across multiple OS kernels becomes excessive and warrants adopting an external abstraction crate like `mio`.

---

### Decision 28: v0.1 Server Threading Model

- **Date**: 2026-10-10
- **Options**:
  - Option A: Pure single thread for everything in v0.1 (event loop, network parsing, command execution, expiry).
  - Option B: Main thread + dedicated network I/O helper thread from day 1.
- **Choice**: Option A (one thread for everything in v0.1).
- **Reason**: Guarantees atomicity by construction without locks, avoids race conditions, simplifies debugging and testing, and provides optimal cache locality for key/value operations.
- **Rules for Later Phases**:
  - Phase 2, Week 15: Add background helper thread for lazy freeing of oversized keys/collections to avoid latency spikes.
  - Phase 4 (Weeks 27-32): Add background helper threads for snapshots (`.cdbs`) and journal `fsync`.
  - Helper threads communicate with the main thread strictly via message-passing channels, never shared locks on the keyspace.
- **What would make us revisit it**: If single-threaded execution cannot meet latency requirements for mixed workloads, or if large operations block the loop before Phase 2 Week 15.

---

### Decision 29: Event Loop Implementation: Custom vs Async Runtime (`tokio`)

- **Date**: 2026-10-10
- **Options**:
  - Option A: Custom hand-written event loop on Linux `epoll`.
  - Option B: Third-party async runtime (`tokio`).
- **Choice**: Option A (custom hand-written event loop on Linux `epoll`, no `tokio`).
- **Reason**: Gives exact control over memory allocations and network buffers, predictable sub-millisecond p99 latency without opaque thread-pool scheduling, and follows the project's educational guiding principle to understand systems programming from scratch.
- **What would make us revisit it**: If the custom event loop proves fragile under soak testing or cannot maintain 10,000 connections compared to a standard runtime.


