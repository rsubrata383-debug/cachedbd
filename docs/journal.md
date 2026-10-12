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

---

## 2026-10-10: Phase 0 Milestone — Core Specifications v0.1 Frozen

### Milestone: Phase 0 Exit — Spec v0.1 Frozen

- **Date**: 2026-10-10
- **Summary**: Comprehensive cross-document consistency audit completed across all specification and design documents. All 29 architectural decisions have been resolved and closed. Specifications `protocol.md`, `values.md`, `errors.md`, and `commands.md` are officially frozen at version 0.1.
- **Documents Frozen**:
  - [docs/spec/protocol.md](file:///c:/Users/Subrata/Desktop/rust/cachedb/docs/spec/protocol.md) (v0.1): 14-byte header, big-endian, pure binary, 8 verified hex examples, `FLAG_MORE_FRAMES` reserved.
  - [docs/spec/values.md](file:///c:/Users/Subrata/Desktop/rust/cachedb/docs/spec/values.md) (v0.1): Binary keys, 1 KB key limit, 32 MB default value limit, no empty keys, primitive and rich data type catalog.
  - [docs/spec/errors.md](file:///c:/Users/Subrata/Desktop/rust/cachedb/docs/spec/errors.md) (v0.1): 33 unique codes across semantic 1000s ranges with connection drop semantics.
  - [docs/spec/commands.md](file:///c:/Users/Subrata/Desktop/rust/cachedb/docs/spec/commands.md) (v0.1): Strict lowercase commands, 9 Phase 1 operations (`hello`, `ping`, `put`, `fetch`, `drop`, `has`, `size`, `bump`, `quit`).
  - [docs/design/threading.md](file:///c:/Users/Subrata/Desktop/rust/cachedb/docs/design/threading.md): Single-threaded core, helper threads via channels, `Clock`/`AsyncStream`/`Disk` test traits, isolated unsafe in `src/net/sys.rs`.
- **Errata and Fixes Applied During Review**:
  - Updated `docs/spec/protocol.md` overview to explicitly note that `FLAG_MORE_FRAMES` is reserved in v0.1.
  - Added formal version and status header blocks across all four specification files.
- **Policy for Modifying Frozen Specs**:
  Any future modification to a frozen specification MUST follow this procedure:
  1. Record a formal decision entry in `docs/journal.md` documenting context, options, choice, reason, and revisit criteria.
  2. Obtain explicit approval from the project owner before making modifications.
  3. Increment the spec document version line (e.g. `v0.1.1` for errata/clarifications, `v0.2` for breaking or additive feature changes).
  4. Verify all modified hex examples against an automated validation script.
- **Phase 0 Exit Status**:
  - All Phase 0 items in `cachedb-roadmap.md` are completed (compiler sanitizers explicitly deferred to Linux pre-Week 5 per Decision 6).
  - All four cargo checks pass with 0 warnings; all 8 protocol hex examples pass validation.
  - Ready to proceed to Phase 1 (Week 3: Event Loop on Linux `epoll`).

---

## 2026-10-10: Repo Cleanup After Phase 0 Review

- **Date**: 2026-10-10
- **Summary**: Minor repository hygiene cleanup following the completion of Phase 0.
- **Changes**:
  - Renamed `README.MD` to `README.md` using two-step `git mv` for case-sensitive git tracking.
  - Wrote concise `README.md` in simple English with project overview, status ("Phase 0 done, specs v0.1 frozen. Phase 1 not started"), build/check commands, binary names, default port (`7379`), primary OS (Linux / WSL2), and doc links. Removed stray `manifest.html` link.
  - Removed non-existent `cachedb.md` entry from the repository layout tree in `cachedb-roadmap.md`.
  - Verified no dangling links to `README.MD` or `cachedb.md` exist in the repository.

---

## 2026-10-10: Phase 1 Week 3 Step 1 — Blocking TCP Echo Server & Socket Mechanics

- **Date**: 2026-10-10
- **Summary**: Implemented an educational blocking TCP echo server in `examples/echo_blocking.rs` and an integration test in `tests/integration.rs` to understand low-level socket semantics using only safe Rust and `std::net`.
- **Socket Mechanics Learned**:
  - **`bind`**: Binds a socket file descriptor to a local IP address and port (`127.0.0.1:7380`).
  - **`listen`**: Marks the socket as passive to queue pending incoming connections (connection backlog).
  - **`accept`**: Blocks until a client completes the TCP 3-way handshake; creates a brand-new socket file descriptor for that specific client connection.
  - **`read`**: Copies bytes from the kernel receive buffer to user space; in blocking mode, puts the calling thread to sleep until at least 1 byte arrives.
  - **Partial Reads**: TCP is a stream-oriented protocol without inherent message boundaries. A `read` call can return any slice length from 1 to `buf.len()`. Handlers must never assume an entire message arrives in one read.
  - **Client Disconnect (EOF)**: When a client terminates or shuts down transmission, the OS receives a TCP FIN packet; `stream.read()` returns `Ok(0)`, signaling End-Of-File.
  - **`write_all`**: Feeds bytes into the kernel transmit buffer, automatically looping until all bytes are queued.
- **The Core Problem with Blocking Sockets**:
  - **Head-of-Line Blocking**: Because a single thread handles one client at a time, calling `read` blocks the entire server if that client is slow or idle. No new clients can be accepted, and no existing clients can make progress.
  - Spawning a thread per client does not scale to tens of thousands of concurrent connections due to thread stack memory consumption (typically 2–8 MB per thread) and CPU context-switching churn.
- **Next Step Motivation**: This motivates Step 2 — switching sockets to non-blocking mode (`O_NONBLOCK`) and building a single-threaded event loop driven by Linux `epoll` notifications.

---

## 2026-10-10: Phase 1 Week 3 Step 2 — Linux Epoll Wrapper & Scoped Unsafe Isolation

- **Date**: 2026-10-10
- **Summary**: Implemented the low-level Linux `epoll` system call abstraction in `src/net/sys.rs`, added the `libc` target dependency in `Cargo.toml`, and relaxed `unsafe_code` from `"forbid"` to `"deny"` per Decision 27.
- **Implementation Details**:
  - **Scoped Unsafe Policy**: `Cargo.toml` now specifies `unsafe_code = "deny"`. The `#![allow(unsafe_code)]` attribute is strictly limited to `src/net/sys.rs`. All other modules across the codebase remain 100% safe Rust.
  - **Explicit `// SAFETY:` Comments**: Every FFI call (`epoll_create1`, `epoll_ctl`, `epoll_wait`, `close`) has a dedicated safety comment documenting why it is sound (valid pointers, allocated slices, owned file descriptors).
  - **Safe RAII Wrapper (`Epoll`)**: Automatically invokes `libc::close(fd)` on `Drop`, preventing file descriptor leaks.
  - **Event Operations**: Wraps `epoll_ctl` operations into safe methods `add`, `modify`, `delete`, and `wait` with type-safe `EpollFlags` bitflags.
  - **Cross-Platform Gating**: Gated real epoll behind `#[cfg(target_os = "linux")]`, with safe stub methods on non-Linux systems returning `io::ErrorKind::Unsupported`. This ensures `cargo check`, `cargo test`, and `cargo clippy` pass on Windows host development, while the real epoll loop runs natively under Linux/WSL2.

---

## 2026-10-10: Phase 1 Week 3 Step 3 — EventLoop Abstraction & Zero-Cost Dispatch

- **Date**: 2026-10-10
- **Summary**: Implemented the single-threaded `EventLoop` abstraction in `src/net/event_loop.rs` over `sys::Epoll`, supporting descriptor registration, zero-lock/zero-alloc event dispatch, and monotonic timer scheduling.
- **Architectural Details**:
  - **Zero Unsafe Code**: The event loop contains zero `unsafe` blocks, driving I/O exclusively through the safe `sys::Epoll` interface.
  - **Zero-Lock Handler Trait**: Instead of storing closures or `Box<dyn FnMut>` requiring `Rc<RefCell<...>>`, the event loop accepts `&mut H` where `H: Handler`. The caller maintains sole, direct ownership of database and connection state with zero synchronization overhead.
  - **Monotonic Timer Min-Heap**: Scheduled timers use `std::collections::BinaryHeap` ordered by monotonic `Instant`. Each polling tick calculates the exact sleep duration needed by the earliest pending timer, ensuring precise expiration dispatch without busy spinning.
  - **Zero-Allocation Hot Path**: The event loop pre-allocates an event buffer of 64 `Event` slots on initialization, eliminating per-tick heap allocations during high-throughput network polling.

---

## 2026-10-10: Phase 1 Week 3 Step 4 — Non-Blocking Echo Server & Partial I/O Handling

- **Date**: 2026-10-10
- **Summary**: Implemented an event-driven, non-blocking TCP echo server in `examples/echo_nonblocking.rs` driven by `EventLoop`, handling `WouldBlock` (`EAGAIN`), partial reads, partial writes, dynamic epoll interest toggling, and clean client disconnects.
- **Architectural Details**:
  - **Non-blocking Loop Semantics**: Configured listener and client streams with `set_nonblocking(true)`. When accept, read, or write exhaust available data or kernel buffers, operations return `io::ErrorKind::WouldBlock` (`EAGAIN`) without stalling the thread.
  - **Partial Writes and Send Queues**: Because TCP transmit buffers are bounded, `write()` can accept fewer bytes than requested. Unwritten bytes are buffered in per-client `VecDeque<u8>` send queues.
  - **Dynamic Interest Registration**: In level-triggered epoll, a socket with available transmit buffer space continuously triggers `WRITABLE`. To avoid 100% CPU busy-looping, the server registers `WRITABLE` only when its `send_queue` is non-empty, and drops back to `READABLE` as soon as the queue drains.
  - **Overcoming Head-of-Line Blocking**: Multiple clients connect and exchange data simultaneously on a single thread. Unlike the Step 1 blocking model, a slow or idle client no longer pauses other clients or blocks incoming connections.

---

## 2026-10-10: Phase 1 Week 3 Step 5 — Signal Handling (SIGINT/SIGTERM) & Graceful EventLoop Termination

- **Date**: 2026-10-10
- **Summary**: Implemented POSIX signal handling (`SIGINT`, `SIGTERM`) in `src/net/sys.rs` and integrated graceful shutdown mechanics into `EventLoop` and `examples/echo_nonblocking.rs`.
- **Architectural & Safety Details**:
  - **Async-Signal Safety**: In POSIX systems, signal handlers may interrupt threads at arbitrary instruction boundaries; only async-signal-safe functions may be called from a signal handler. We store the shutdown request into a global `AtomicBool` (`SHUTDOWN_REQUESTED`) using `Ordering::Relaxed`, which is guaranteed lock-free, atomic, and signal-safe.
  - **Immediate Wake-up via `EINTR` (No `SA_RESTART`)**: Signal handlers are registered with `libc::sigaction` with `sa_flags = 0` (deliberately omitting `SA_RESTART`). When a signal arrives while the process is blocked in `epoll_wait`, the kernel immediately unblocks the syscall with `-1` and sets `errno = EINTR`.
  - **Handling Interrupted Syscalls**: `Epoll::wait` translates `ErrorKind::Interrupted` into `Ok(0)` rather than propagating an unhandled error. Immediately upon return, `EventLoop::run_once` and `EventLoop::run` inspect `sys::is_shutdown_requested()`, mark `self.stop()`, and terminate the loop gracefully.
  - **Cross-Platform Compatibility**: On Linux targets, native `sigaction` traps `SIGINT` and `SIGTERM`. On non-Linux targets (such as Windows development hosts), safe stub implementations allow tests, examples, and tooling to compile and execute predictably.
  - **Completion of Week 3**: Week 3 networking basics are now complete (blocking TCP echo exploration, scoped unsafe `epoll` wrapper, single-threaded zero-cost `EventLoop`, non-blocking echo server with dynamic interest registration, and signal-driven graceful shutdown).

---

## 2026-10-10: Phase 1 Week 4 Step 1 — Connection Object, Read Buffer, & Write Queue

- **Date**: 2026-10-10
- **Summary**: Implemented the core networking data structures in `src/net/buffer.rs` (`ReadBuffer`, `WriteQueue`) and `src/net/connection.rs` (`Connection`, `ReadStatus`, `WriteStatus`), providing production-grade buffer management, partial I/O handling, and backpressure enforcement.
- **Architectural Details**:
  - **Contiguous Read Buffer (`ReadBuffer`)**: Stores streaming socket bytes in a contiguous buffer with internal `read_pos` and `write_pos` tracking. Protocol decoders inspect frames as contiguous slices (`&[u8]`) with zero allocations. Includes automatic compacting and cursor reset when drained, with an enforced maximum capacity ceiling (`max_capacity`, default 64 MB) to prevent unbounded memory growth from abusive clients.
  - **FIFO Write Queue (`WriteQueue`)**: Manages outgoing byte chunks for non-blocking socket transmission. Supports zero-copy appending of serialized frames (`enqueue_chunk(Vec<u8>)`), handles partial socket writes across chunks, and tracks total queued bytes against a high-watermark threshold (default 10 MB).
  - **Connection Lifecycle (`Connection`)**: Represents an active client session with an assigned `Token`, peer `SocketAddr`, stream, read buffer, and write queue. Tracks activity timestamps (`last_active`) for idle timeouts and supports graceful closing (draining pending writes before final socket termination).
  - **Dynamic Backpressure**: `Connection::desired_flags()` inspects `write_queue.is_backpressured()`. When outgoing buffers exceed the watermark, `READABLE` is automatically excluded from the epoll interest set, pausing client reads until outgoing buffers drain.

---

## 2026-10-10: Phase 1 Week 4 Step 2 — Growable Ring Buffer & ConnectionTable Backpressure Integration

- **Date**: 2026-10-10
- **Summary**: Implemented the circular `RingBuffer` optimization in `src/net/buffer.rs`, added the centralized `ConnectionTable` in `src/net/connection.rs`, and integrated both into `examples/echo_nonblocking.rs` with active backpressure interest toggling.
- **Architectural Details**:
  - **Growable Circular Buffer (`RingBuffer`)**: Eliminates the $O(N)$ data shifting of linear buffers by maintaining independent `head` (write) and `tail` (read) indices that wrap around modulo capacity. Supports two-slice zero-copy inspection (`as_slices() -> (&[u8], &[u8])`), on-demand linearization (`make_contiguous()`), and automated geometric growth with unrolled linear reassembly.
  - **Connection Table (`ConnectionTable`)**: Replaces ad-hoc `HashMap<Token, ClientConnection>` with a centralized connection registry supporting connection limits (`max_connections`, default 10,000), atomic token allocation, and clean lookup.
  - **Dynamic Backpressure Toggling**: Integrated with the event loop via `ConnectionTable::sync_interest()`. When a client's write queue exceeds the high watermark (10 MB), `desired_flags()` drops `READABLE`, instructing epoll to stop monitoring socket read events. Once outgoing chunks are flushed below the threshold, read interest is automatically restored.
  - **Server Refactor**: Refactored `NonBlockingEchoServer` in `examples/echo_nonblocking.rs` to run cleanly over `ConnectionTable`, unifying buffer handling, error recovery, and backpressure without manual raw socket bookkeeping.

---

## 2026-10-10: Phase 1 Week 4 Step 3 — Backpressure Hysteresis & Flow Control

- **Date**: 2026-10-10
- **Summary**: Implemented dual-watermark hysteresis backpressure ($W_{\text{high}}$ and $W_{\text{low}}$) and hard capacity ceilings in `WriteQueue`, integrated backpressure states into `Connection` and `ConnectionTable`, and validated that epoll stops reading from clients whose output queues grow too large without interest flapping.
- **Architectural Details**:
  - **Dual-Watermark Hysteresis**: Addressed interest thrashing (rapid toggle of epoll interest flags across small frame boundaries) by introducing two thresholds. Backpressure engages when pending queued bytes reach $W_{\text{high}}$ (default 10 MB), dropping `READABLE` from epoll. Backpressure disengages only after the queue drains below $W_{\text{low}}$ (default 5 MB, or $W_{\text{high}} / 2$), ensuring stable flow control and amortizing epoll modification syscalls.
  - **OOM Protection Ceiling**: Added `max_capacity` (default 64 MB) to `WriteQueue`. Enqueue operations exceeding this limit safely return `io::ErrorKind::OutOfMemory` rather than allowing unbounded memory growth from stalled, malicious, or slow-reading clients.
  - **Dynamic Epoll Synchronization**: `Connection::desired_flags()` inspects `write_queue.is_backpressured()`, automatically filtering out `READABLE` while backpressured. The server event loop calls `ConnectionTable::sync_interest()` to synchronize kernel interest after reading or writing.
  - **Verification**: Tested unit-level hysteresis mechanics using bounded mock writers (verifying that backpressure remains asserted when drained between $W_{\text{high}}$ and $W_{\text{low}}$ and deasserts below $W_{\text{low}}$) and integration tests confirming event loop interest flag transitions.

---

## 2026-10-10: Phase 1 Week 4 Step 4 — Connection Limits, Idle Timeouts, and Graceful Shutdown

- **Date**: 2026-10-10
- **Summary**: Implemented connection capacity enforcement, idle connection detection and periodic sweeping, and multi-phase graceful shutdown with drain loops across `ConnectionTable`, `EventLoop`, and `NonBlockingEchoServer`.
- **Architectural Details**:
  - **Connection Capacity Ceilings**: Added `is_at_capacity()`, `remaining_capacity()`, and dynamic limit tuning (`set_max_connections()`) to `ConnectionTable`. When the table reaches maximum capacity (default 10,000), incoming connections in the accept loop are rejected immediately, incrementing rejection counters and closing the raw socket without event loop allocation.
  - **Idle Activity Tracking & Sweeper**: Added monotonic `idle_duration()`, `is_idle()`, and `touch()` to `Connection`. Implemented periodic idle sweeps driven by the `EventLoop` timer wheel (`IDLE_SWEEP_TIMER_ID`). Inactive connections exceeding `idle_timeout` (default 60s) are closed: connections with empty write queues are purged and deregistered immediately, while connections with pending writes are set to closing and kept alive to flush. Lingering closing connections that remain stalled beyond the idle timeout are forcefully purged.
  - **Graceful Shutdown & Drain Mode**: Implemented `initiate_graceful_shutdown()` on the server, paired with `EventLoop::drain_once()`. When a shutdown signal (`SIGINT`, `SIGTERM`) arrives, the server deregisters the listening socket from epoll, pauses client read interest (`READABLE` dropped), and enters a drain loop flushing pending write queues up to a configurable deadline (`shutdown_timeout`, default 5s). Lingering connections exceeding the deadline are forcefully terminated via `force_close_all()`.
  - **Verification**: Verified with unit tests covering capacity queries, dynamic limit expansion, idle sweeps, and closing-state purges in `src/net/connection.rs`, as well as integration tests validating connection rejection, shutdown drain behavior, and clean teardown in `tests/integration.rs`.

---

## 2026-10-10: Bugfix — Event Loop Handler Dispatch & Socket RST on Linux

- **Date**: 2026-10-10
- **Summary**: Resolved `ConnectionReset` (OS error 104) failures occurring in Linux CI runs within non-blocking echo server integration tests.
- **Root Cause Analysis**:
  - Previously, `NonBlockingEchoServer` attempted to hold the `EventLoop` as a struct field while simultaneously implementing `Handler for NonBlockingEchoServer`. To bypass Rust's borrow checker when calling `self.event_loop.run(self)`, `run()` and `run_ticks()` swapped `self.event_loop` with a dummy `EventLoop::new()`.
  - Consequently, callbacks (`handle_accept`, `handle_client_read`, `handle_client_write`, `on_timeout`) called methods on the dummy `self.event_loop` rather than the active running event loop instance.
  - On Linux, newly accepted client sockets were never registered with the actual kernel epoll instance. The server thread never received read events for client messages and exited upon exhausting tick counts. Dropping the server closed client sockets while unread data remained in the socket receive buffer, causing the Linux TCP stack to immediately emit TCP `RST` packets (`ECONNRESET`).
- **Architectural Solution**:
  - **First-Class EventLoop Parameter on Handler**: Updated `Handler::on_event(&mut self, event_loop: &mut EventLoop, token: Token, flags: EpollFlags)` and `Handler::on_timeout(&mut self, event_loop: &mut EventLoop, timer_id: TimerId)`.
  - **Disjoint Struct Fields**: Decomposed `NonBlockingEchoServer` into `EchoServerHandler` (owning `listener`, `connections`, configuration) and `EventLoop`. Method calls like `self.event_loop.run(&mut self.state)` borrow disjoint fields cleanly without `mem::replace` workarounds.
  - **Zero-Allocation Ready Event Copying**: In `EventLoop::run_once_internal`, ready events and expired timers are extracted before dispatching callbacks, allowing `&mut self` to be passed directly to the handler while preserving zero-allocation performance.
- **Verification**: Verified clean compilation with `cargo check --target x86_64-unknown-linux-gnu`, `cargo clippy --target x86_64-unknown-linux-gnu --all-targets --all-features -- -D warnings`, `cargo fmt --check`, and 100% test pass rate across all suites.

---

## 2026-10-10: Phase 1 Week 4 Step 5 — Test with 10,000 Idle Connections

- **Date**: 2026-10-10
- **Summary**: Implemented scale and soak tests for 10,000 concurrent idle connections in `tests/soak.rs`, validating memory footprint, capacity enforcement, fast $\mathcal{O}(1)$ token lookup, and deterministic idle sweep performance.
- **Architectural Findings & Measurements**:
  - **Memory Footprint**:
    - Each idle connection initial allocation consists of an empty `WriteQueue` (0 heap bytes) and an initial 4,096-byte `ReadBuffer`.
    - Across 10,000 idle sessions, total allocated buffer memory is exactly $10{,}000 \times 4{,}096 = 40{,}960{,}000$ bytes ($\approx 40.96\text{ MB}$).
    - Average buffer memory per idle session is $\approx 4.09\text{ KB}$, well within the target budget of $\le 16\text{ KB}$ per idle connection.
    - *Honest Note on Memory Accounting & Mock Streams*: This test uses in-memory `MockStream` socket mocks rather than real operating system descriptors to run deterministically and portably without requiring elevated system file descriptor limits (`ulimit -n >= 25,000`). The reported $\approx 4\text{ KB}$ per connection represents user-space buffer allocations only (`ReadBuffer`); it does not include heap metadata for the `HashMap` bucket entries, the `Connection` struct itself ($\approx 250$ bytes), or operating system kernel TCP socket send/receive buffers.
  - **Limit Enforcement**:
    - `ConnectionTable` enforced the exact ceiling at 10,000 connections (`is_at_capacity() == true`, `remaining_capacity() == 0`).
    - Attempting to insert an overflow connection (10,001) returns `io::ErrorKind::ConnectionRefused` cleanly without table mutation.
  - **$\mathcal{O}(1)$ Lookup & Touch**:
    - Sampled lookups and touches across 10,000 tokens completed rapidly. Generous upper-bound timing assertions ($< 100\text{ ms}$) were added to the test suite to ensure consistent $\mathcal{O}(1)$ performance while avoiding false test flakiness under heavy CI scheduler load.
  - **Deterministic Idle Sweeper Performance**:
    - Sweeping all 10,000 connections to detect idle tokens completed rapidly. A generous timing assertion ($< 100\text{ ms}$) validates that sweeping does not stall the event loop.
    - Verified exact partitioning: with 5,000 simulated idle connections, `idle_tokens()` precisely identified 5,000 tokens, and `sweep_idle_connections()` purged them, returning table length to 5,000 and remaining capacity to 5,000.
  - **Teardown & Drain Performance**:
    - Draining and removing 10,000 connections (`test_ten_thousand_connections_drain_teardown`) verified fast table deallocation ($< 200\text{ ms}$). Multi-phase shutdown and lingering close with real TCP streams are independently verified in `tests/integration.rs`.
  - **OS Descriptor Real Socket Verification**:
    - Added real non-blocking loopback socket test verifying capacity and rejection limits with operating system descriptors.
    - Added an opt-in 10,000 real TCP connection test (`#[ignore = "requires elevated ulimit -n (>= 25,000 open file descriptors)"]`) runnable on environments with configured descriptor limits.
  - **Debug vs. Release Performance Profile**:
    - Compiling in release mode (`--release`) cuts internal data structure latencies by **~2x to ~4x** across the board:

| Metric / Operation | Debug (`[unoptimized]`) | Release (`[optimized]`) | Improvement |
| :--- | :--- | :--- | :--- |
| **1,000 Lookups & Touches** | `722.1 µs` (722 ns/op) | **`209.8 µs` (209 ns/op)** | **~3.4x faster** |
| **10,000 Idle Sweeps** | `8.22 ms` | **`2.09 ms`** | **~3.9x faster** |
| **10,000 Table Insertions** | `70.30 ms` | **`32.83 ms`** | **~2.1x faster** (304k inserts/sec) |
| **10,000 Drains & Teardowns** | `33.73 ms` | **`20.43 ms`** | **~1.6x faster** |
| **10,000 Real TCP Connections** | `~9.0 s` | **`~9.0 s`** | Flat (bottleneck is Windows kernel loopback handshake rate) |

    - *Key Architectural Takeaways*:
      1. **Sub-microsecond connection routing**: $209\text{ ns}$ per connection lookup means looking up routing tables, lease timeouts, and connection metadata consumes negligible CPU overhead even during traffic bursts.
      2. **Predictable memory envelope**: $40\text{ MB}$ across 10,000 connections confirms buffers are pre-allocated or capped at exact 4 KB slab limits without unbounded heap growth.
      3. **OS Handshake Bottleneck**: The TCP connection test remained around ~9 seconds because connecting 10,000 real sockets sequentially on loopback is throttled by Windows network stack rate limits and thread context switching, not user-space Rust code.

---

## 2026-10-11: Phase 1 Code Review Bugfixes & Networking Hardening

- **Date**: 2026-10-11
- **Summary**: Comprehensive resolution of high-, medium-, and low-priority code review findings across Phase 1 networking, buffers, event loop, and error isolation. Every fix was implemented via test-driven development (failing regression test verified first, followed by fix and passing verification).

### Fix 1 (HIGH 1): Per-Connection Error Isolation & Resilient Accept Loop
- **Problem**: In `examples/echo_nonblocking.rs`, `handle_client_read` executed `conn.read_incoming()?` and `on_event` executed `self.handle_client_read(..)?`. Any per-connection I/O error (such as a connection reset `ECONNRESET` between epoll notification and read, or a read-buffer `OutOfMemory` condition) propagated out of `EventLoop::run`, crashing the entire server. Additionally, `handle_accept` bubbled errors like `stream.set_nonblocking(true)?` and `Err(err) => return Err(err)`. On descriptor exhaustion (`EMFILE`, `ENFILE`), the server terminated abruptly instead of temporarily pausing accepts.
- **Root Cause**: Unconditional propagation of per-client I/O errors using the `?` operator up through the `Handler::on_event` trait callback to the top-level event loop.
- **Solution**:
  - `handle_client_read` catches `Err(err)`, logs the error with client token and address, removes and deregisters only that specific client, and returns `Ok(false)` so `on_event` returns `Ok(())` without interrupting the event loop.
  - `handle_accept` wraps `stream.set_nonblocking(true)` safely, logs and ignores `ECONNABORTED`, breaks out of the accept loop for the current tick on `EMFILE`/`ENFILE` without returning an error, and logs other accept errors before continuing.
  - Documented on the `Handler` trait and `on_event` in `src/net/event_loop.rs` that returning `Err` is fatal to the event loop, requiring handlers to return `Err` solely for unrecoverable subsystem failures.
- **Verification**: Created regression tests in `examples/echo_nonblocking.rs`:
  - `test_client_reset_mid_session_does_not_crash_server`: A client sets `SO_LINGER 0` via newly added `cachedb::net::sys::set_linger_zero` and drops mid-session; server isolates the error, cleans up the connection, and remains fully functional for remaining clients.
  - `test_client_buffer_overflow_disconnects_client_only`: A client overflows a small read buffer capacity limit (64 bytes); server disconnects only the offending client while other clients continue operating cleanly.

### Fix 2 (HIGH 2): Half-Close / EOF Preserving Queued Output
- **Problem**: When `read_incoming()` received EOF (`Ok(0)` from `read()`), it set `is_closed = true`. The handler immediately removed and deregistered the connection, discarding any unsent response bytes remaining in the connection's write queue. A client sending requests and calling `shutdown(Write)` experienced data loss.
- **Root Cause**: EOF was treated as an immediate hard close regardless of whether output bytes were still pending transmission in `WriteQueue`.
- **Solution**:
  - In `Connection::read_incoming`, on `Ok(0)`: if `write_queue.is_empty()`, `is_closed` is set to `true`; if `write_queue` is not empty, `is_closing` is set to `true` (leaving `is_closed = false`).
  - In `examples/echo_nonblocking.rs`: on `ReadStatus::Eof`, if the connection has pending outgoing bytes, read interest is removed while write interest is retained in epoll (`sync_interest`), and queued bytes are flushed. The connection is removed only after the write queue drains completely.
  - `handle_client_read` loops until `WouldBlock` or `Eof` to drain multi-chunk socket buffers.
- **Verification**: Created regression test `test_half_close_preserves_queued_output` in `examples/echo_nonblocking.rs`: sends a 512 KB payload (larger than default OS socket buffers), issues `shutdown(Write)`, and reads until EOF. Confirmed that 100% of echoed bytes (524,288 bytes) are received by the client before EOF.

### Fix 3 (MEDIUM 3): Failed Writes Do Not Refresh Idle Timer
- **Problem**: In `Connection::flush_outgoing`, `self.last_active = Instant::now()` ran before inspecting the result of `write_to()`. When `write_to()` returned `WouldBlock` (writing 0 bytes), `last_active` was still refreshed, allowing slow or stalled clients that never consume output to evade the idle sweeper indefinitely.
- **Root Cause**: Unconditional timestamp update upon entering flush rather than verifying actual byte progression.
- **Solution**: Updated `self.last_active = Instant::now()` in `Connection::flush_outgoing` conditionally only when `n > 0` bytes were written to the socket.
- **Verification**: Added regression test `test_failed_write_does_not_refresh_idle_timer` in `src/net/connection.rs` using a mock writer returning `WouldBlock`. The test confirmed that `conn.last_active()` remained unchanged across blocked write flushes.

### Fix 4 (MEDIUM 4): Isolated Shutdown Handles & Serialized Signal Tests
- **Problem**: Tests running concurrently in parallel test threads (`--test-threads=16`) shared the process-global atomic shutdown flag `SHUTDOWN_REQUESTED`. While one test asserted shutdown, other tests saw early returns from `run_once`, causing test failures and hangs.
- **Root Cause**: Global mutable state shared across parallel test threads.
- **Solution**:
  - Added an instance-level `shutdown_flag: Arc<AtomicBool>` to `EventLoop`, checked in `run_once_internal` and `run`. The loop checks both its instance flag and the global process flag.
  - Serialized tests that manipulate the global signal flag with a static `Mutex<()>`.
  - Added 3-second read and write timeouts (`set_read_timeout`, `set_write_timeout`) to client sockets across all integration tests, guaranteeing tests fail with timeout errors rather than hanging indefinitely if a server bug occurs.
- **Verification**: Executed `cargo test -- --test-threads=16` repeatedly across multiple runs with zero failures.

### Fix 5 (MEDIUM 5): Ceil Rounding for Epoll Wait Timeout
- **Problem**: In `EventLoop::run_once_internal`, sleep duration was computed with `dur.as_millis()`, which truncates fractions. Sub-millisecond sleep durations (e.g. 500 microseconds) truncated to 0 ms, causing `epoll_wait(0)` to busy-spin until the deadline expired.
- **Root Cause**: Integer truncation in standard duration division.
- **Solution**: Implemented `EventLoop::compute_timeout_ms(timeout: Option<Duration>) -> i32` with ceiling rounding `(micros + 999) / 1000`. Any non-zero remaining duration returns at least 1 ms, capped at `i32::MAX`.
- **Verification**: Added unit test `test_compute_timeout_ms_rounding_and_limits` in `src/net/event_loop.rs` verifying 1 ns, 500 µs, 5001 µs, exact milliseconds, zero, and `None` mapping to -1.

### Fix 6 (MEDIUM 6): Signal Wake-Up Race Prevention
- **Problem**: In `EventLoop::run_once_internal`, checking the shutdown flag followed by entering `epoll_wait(-1)` could cause the event loop to block indefinitely if a termination signal arrived between the check and the syscall when no timers were scheduled.
- **Root Cause**: Unbounded poll timeout when no timer deadlines exist.
- **Solution**: Capped maximum poll sleep duration to `MAX_POLL_TIMEOUT_MS = 100` ms. When no timers are scheduled, `epoll_wait` sleeps for at most 100 ms before waking to inspect shutdown flags.
- **Verification**: Verified that event loop terminates cleanly on shutdown signals even with no timers scheduled.

### Fix 7 (MEDIUM 7): Zero-Allocation Epoll Wait Buffer
- **Problem**: `Epoll::wait` in `src/net/sys.rs` executed `vec![libc::epoll_event; events.len()]` on every invocation, allocating heap memory on every tick and contradicting zero-allocation guarantees.
- **Root Cause**: Lack of reusable buffer storage in `Epoll`.
- **Solution**: Pre-allocated a reusable `raw_events: Vec<libc::epoll_event>` inside the `Epoll` struct upon creation, resizing on-demand only if the caller's event buffer exceeds current capacity. Guarded against `events.is_empty()` to prevent `EINVAL` from reaching `epoll_wait`.
- **Verification**: Verified zero allocations during steady-state `Epoll::wait` execution.

### Fix 8 (LOW Items): Buffer Shrink, WriteZero, Interrupted Retry, Validation, Lingering Close, & Explicit Stubs
- **ReadBuffer Shrink on Empty**: In `src/net/buffer.rs`, added shrink-on-empty in `consume()` and `clear()`. When the buffer is completely empty and its capacity exceeds 64 KB, it reallocates back to its initial 4 KB size, bounding memory retention after large frames. Tested with `test_read_buffer_shrink_on_empty`.
- **Write Returning `Ok(0)`**: In `WriteQueue::write_to`, treated `Ok(0)` as `io::ErrorKind::WriteZero` rather than silently breaking the loop, preventing perpetual `WRITABLE` spins. Tested with `test_write_queue_write_zero_error`.
- **Interrupted Syscall Retry**: In `Connection::read_incoming`, retried reads in a loop when `ErrorKind::Interrupted` (`EINTR`) occurs, avoiding lost wake-ups in edge-triggered mode.
- **Token Validation**: In `ConnectionTable::insert_connection`, rejected token 0 with `ErrorKind::InvalidInput` (reserved for listener) and duplicate tokens with `ErrorKind::AlreadyExists`.
- **Lingering Close on Shutdown**: Added `lingering_close_tcp` (`shutdown(Write)` followed by draining incoming reads up to EOF or limit) in `sweep_idle`, `initiate_shutdown`, and `force_close_all` to avoid emitting TCP `RST` packets on clients with unconsumed inbound data.
- **Hangup with Unread Data**: In `examples/echo_nonblocking.rs`, if `READABLE` and `HANGUP` arrive simultaneously, available data is processed before terminating the connection.
- **Non-Linux Epoll Stub Loudness**: Changed `Epoll::create()` on non-Linux to return `io::ErrorKind::Unsupported` explicitly so non-Linux platforms fail loudly rather than silently pretending to poll.

---

## 2026-10-11: Post-Review Fixes — Example Tests Harness, Target Dependencies, and Config

- **Date**: 2026-10-11
- **Summary**: Addressed post-review findings regarding example test execution, target dependencies, MSRV configuration, and doc comment accuracy.
- **Changes & Rationale**:
  - **Example Test Harness**: Discovered that tests written directly inside `examples/echo_nonblocking.rs` were never being executed during default `cargo test` runs because Cargo ignores tests in example files unless explicitly declared with `test = true`. Added `[[example]] name = "echo_nonblocking" test = true` to `Cargo.toml`.
  - **MSRV & Lints**: Set `rust-version = "1.85"` under `[package]` in `Cargo.toml` and removed `warnings = "deny"` from `[lints.rust]` (as CI already specifies `-D warnings`).
  - **Unix Target Dependency for libc**: Changed the dependency target from `cfg(target_os = "linux")` to `cfg(unix)` so that `libc` is available for all Unix-family operating systems (including BSD and macOS) where `#[cfg(unix)]` code compiles.
  - **Doc Comment Correction**: Removed an erroneous duplicate line referencing `remove_client` from the doc comment on `set_max_read_capacity` in `examples/echo_nonblocking.rs`.
  - **Roadmap Synchronization**: Marked Phase 1 Week 5 as "Not started" in `README.md` to accurately reflect project status before beginning wire protocol implementation.

---

## 2026-10-11: CI Enhancement — Linux 10k Real TCP Soak Test Job

- **Date**: 2026-10-11
- **Summary**: Added a dedicated `soak` job to GitHub Actions CI running on `ubuntu-latest` with elevated file descriptor limits to execute the ignored 10,000 real TCP connection test.
- **Why Added**: The `test_ten_thousand_real_tcp_idle_connections` test in `tests/soak.rs` creates 10,000 client sockets and 10,000 server sockets, requiring more than 20,000 open file descriptors. On default environments (and standard CI runs), the default soft descriptor limit is 1,024, causing the test to be marked `#[ignore]`. By raising the limit with `ulimit -n 30000` in a dedicated CI job, this test runs on every workflow run and manual trigger.
- **What It Proves**: It proves that the single-threaded networking stack and `ConnectionTable` handle 10,000 live kernel TCP sockets concurrently on real Linux loopback without descriptor leaks, stack overflow, or memory exhaustion.

---

## 2026-10-11: Linux EventLoop 10,000 Real TCP Connection Soak Test

- **Date**: 2026-10-11
- **Summary**: Added `test_ten_thousand_real_tcp_event_loop_idle_connections` to `tests/soak.rs` (`#[cfg(target_os = "linux")]`, `#[ignore = "requires ulimit -n >= 25,000"]`).
- **Context & Motivation**:
  - The earlier test (`test_ten_thousand_real_tcp_idle_connections`) only connected sockets and inserted them directly into `ConnectionTable` in a loop. It never registered the descriptors with epoll, never ran `EventLoop` ticks, and never verified that the epoll event loop could actually poll, dispatch, and route I/O across 10,000 live sockets.
  - This new test proves the complete `EventLoop` abstraction under real kernel load:
    1. Binds a non-blocking `TcpListener` registered on `LISTENER_TOKEN` with `EventLoop`.
    2. Connects 10,000 real clients in batches of 200, interleaving `event_loop.run_once` ticks to accept connections without overflowing the listen backlog.
    3. Verifies that all 10,000 connections are accepted, registered in epoll, and idle.
    4. Proves I/O dispatch under full scale: while 10,000 connections stay idle, 3 clients (indices 0, 5,000, 9,999) send messages, and `EventLoop` ticks route reads, writes, and echoes back with exact byte matches.
    5. Measures elapsed time and memory footprint (VmRSS from `/proc/self/status`): ~405 ms connect time and 43,428 kB VmRSS delta (~4.3 kB/conn) measured in CI.
    6. Drops all clients, runs drain ticks until all connections are cleaned up and deregistered from epoll, and verifies that `ConnectionTable` is completely empty.

---

## 2026-10-11: Fix CI Soak Test Timeout — Backlog Overflow and Sequential Test Execution

- **Date**: 2026-10-11
- **CI Failure & Root Cause**:
  - The soak test job in GitHub Actions failed after 135 seconds with `Os { code: 110, kind: TimedOut, message: "Connection timed out" }`.
  - Rust's standard library `TcpListener::bind` sets a hardcoded listen backlog queue of 128 connections. The test attempted to connect 200 clients per batch using blocking `TcpStream::connect` before calling `event_loop.run_once` to accept them. Once client #129 attempted to connect, the kernel listen queue was full and silently dropped incoming SYN packets. The blocking `connect()` call waited ~127 seconds for TCP SYN retransmissions before timing out.
  - Additionally, Cargo test runs ignored tests concurrently by default. Both 10k connection tests running simultaneously exceeded the 30,000 file descriptor limit.
- **The Fix**:
  1. Reduced `BATCH_SIZE` from 200 to 100 with an explicit assertion that `BATCH_SIZE < 128`, guaranteeing the kernel listen backlog never overflows.
  2. Replaced `TcpStream::connect` with `TcpStream::connect_timeout(&addr, Duration::from_secs(5))` and included the exact client index in error messages, failing fast in 5 seconds rather than minutes if a socket stalls.
  3. Added `--test-threads=1` to the CI soak test step so 10k tests execute sequentially without competing for file descriptors. Reduced job timeout to 10 minutes.
- **Note for Later**:
  - A production database server must support larger listen backlogs than the 128 provided by Rust's standard `TcpListener::bind` (e.g., using `libc::listen` with `SOMAXCONN` / configurable backlog). A future low-level listener helper should live in `src/net/sys.rs`.

---

## 2026-10-11: 10k real TCP connection test passed in CI

- **Date**: 2026-10-11
- **Environment**: GitHub Actions CI (`ubuntu-latest`, release profile, `ulimit -n 30000`, `--test-threads=1`).
- **Results**:
  - `test_ten_thousand_real_tcp_event_loop_idle_connections`:
    - Connected and accepted 10,000 real TCP clients in about 405 ms.
    - Memory: VmRSS before = 2,716 kB, after = 46,144 kB, delta = 43,428 kB (about 4.3 kB per connection).
  - `test_ten_thousand_real_tcp_idle_connections`: also passed.
  - Whole soak suite: 2 tests passed in 1.34 s.
- **Notes & Observations**:
  - VmRSS counts only the process's own memory. Kernel socket memory is not counted.
  - The test's client sockets are in the same process, so the number may be a little higher than a real server would show.
  - CI runner numbers can change from run to run. Use them as a rough guide, not an exact promise.
  - The earlier failure (batch of 200 vs listen backlog 128) is fixed. The soak job is now green.

---

## 2026-10-11: Week 5 Step 1: Wire Protocol Header

- **Date**: 2026-10-11
- **Summary**: Implemented the fixed 14-byte wire protocol frame header (`src/proto/header.rs`), error type `ProtoError`, `Opcode`, `DecodedHeader`, and round-trip / boundary unit tests validating all 8 hex examples from `docs/spec/protocol.md`.
- **What Was Added**:
  - `Header`: 14-byte frame header (`magic: [u8; 2] = [0xCA, 0xDB]`, `version = 1`, `flags: u8`, `request_id: u32`, `opcode: Opcode`, `payload_len: u32`).
  - `Opcode`: Wire frame opcodes (`Hello = 0x0001`, `Command = 0x0002`, `Push = 0x0003`). Unknown raw opcode converts to error 2001 (`ERR_UNKNOWN_COMMAND`).
  - `ProtoError`: Initial protocol error container holding numeric codes and messages matching `docs/spec/errors.md` (1001, 1002, 1003, 1004, 2001).
  - `DecodedHeader`: Enum distinguishing `NeedMore` (< 14 bytes) from `Complete(Header)`.
  - Comprehensive unit tests:
    - Round-trip encode/decode equality.
    - All 8 hex examples from `protocol.md` decoded byte-for-byte.
    - Partial header inputs (0, 1, 13 bytes) return `NeedMore`.
    - Error code validation: 1004 (invalid magic), 1001 (unsupported version), 1002 (reserved flags such as `FLAG_MORE_FRAMES`), 1003 (payload > 64 MB), 2001 (unknown opcode).
    - Payload boundary tests: exact 64 MB accepted, 64 MB + 1 rejected.
- **Spec Observations & Clarifications**:
  - In `docs/spec/protocol.md`, the 14-byte frame header opcode is a 2-byte field with values `0x0001` (HELLO), `0x0002` (COMMAND), and `0x0003` (PUSH). Specific commands (`ping`, `put`, `fetch`, etc.) are verbs transmitted inside the Array payload of a `COMMAND` frame, not separate wire frame header opcodes.
  - Initialized `ProtoError` with codes from `docs/spec/errors.md` since they were not previously implemented in `src/`.

---

## 2026-10-11: Week 5 Step 2: Varint Encoding and Decoding

- **Date**: 2026-10-11
- **Part A Check Findings**:
  1. *Unknown frame opcode error code*: `docs/spec/errors.md` does not specify a 1xxx protocol framing code for unknown frame opcodes (codes 1001–1007 cover version, flags, frame size, magic, varint, payload, and type tag). We keep 2001 (`ERR_UNKNOWN_COMMAND`) and note under Open Questions whether a 1xxx code should be assigned in future spec revisions.
  2. *Reserved flag bits 4–7*: `protocol.md` states "Bits 4-7 | 0xF0 | Reserved | Must be set to 0", and `errors.md` explicitly specifies code 1002 (`ERR_UNSUPPORTED_FLAG`) for "Client sets reserved or unsupported flag bit", which encompasses bits 3–7.
  3. *Max payload 64 MB*: Yes, `protocol.md` Section 9 explicitly specifies "Max Frame Size | 64 MB (67,108,864 bytes)".
  4. *Header check order*: The spec lists fields in physical wire sequence (magic, version, flags, request_id, opcode, payload_len), which the decode validation checks sequentially.
  5. *Canonical varints*: `protocol.md` is silent regarding non-canonical (non-shortest) varints like `0x80 0x00`. The decoder accepts standard LEB128 decodings, but spec v0.2 should clarify if non-minimal encodings must be strictly forbidden.
- **What Was Added**:
  - `src/proto/varint.rs`:
    - `encode_uvarint(value: u64, out: &mut Vec<u8>)`: Encodes unsigned 64-bit LEB128 varints (up to 10 bytes).
    - `decode_uvarint(input: &[u8]) -> Result<Decoded<(u64, usize)>, ProtoError>`: Decodes LEB128 varints, returns `Decoded::NeedMore` on truncated streams, and returns error 1005 (`ERR_MALFORMED_VARINT`) on 64-bit overflows or length > 10 bytes.
    - `zigzag_encode(n: i64) -> u64` and `zigzag_decode(n: u64) -> i64`: Exact Zigzag mappings per Section 4.1.
    - `encode_ivarint(value: i64, out: &mut Vec<u8>)` and `decode_ivarint(input: &[u8]) -> Result<Decoded<(i64, usize)>, ProtoError>`.
  - Reused generic `Decoded<T>` in `src/proto/mod.rs` across headers and varints.
  - Comprehensive unit tests:
    - Round trip across critical values (0, 1, 127, 128, 255, 16383, 16384, u32::MAX, u64::MAX, and negative/extreme i64 values).
    - Exact byte assertions for 0, 127, 128, 300, u64::MAX.
    - Truncated and empty inputs return `NeedMore`.
    - Overflow / >10 bytes returns error 1005.
    - Standard LEB128 non-canonical encodings (`0x80 0x00`) decode properly to 0.
    - Extra bytes after varint are preserved.
    - Verified against varint encodings in hex examples (2001 -> `D1 0F`, zigzag 42 -> `54`, 1002 -> `EA 07`).
    - Deterministic 1,000-iteration pseudorandom loop round-trip test.
- **Open Questions**:
  - Should an unknown frame header opcode have a dedicated 1xxx framing error (e.g. 1008 `ERR_UNKNOWN_OPCODE`) rather than reusing 2001 (`ERR_UNKNOWN_COMMAND`) when `spec v0.2` is drafted?
  - Should non-canonical varint encodings (e.g. `0x80 0x00` representing 0) be explicitly forbidden with `ERR_MALFORMED_VARINT` (1005) in spec v0.2 to prevent canonicalization issues?

---

## 2026-10-11: Week 5 Step 3a: Scalar Items

- **Date**: 2026-10-11
- **Summary**: Implemented scalar typed binary payload item serialization and deserialization in `src/proto/item.rs`.
- **What Was Added**:
  - `Value`: Owned enum supporting scalar variants `Nil`, `Int(i64)`, `Float(f64)`, `Bool(bool)`, `Text(String)`, `Bytes(Vec<u8>)`. (Array, Map, and Error are deferred to Step 3b).
  - Design note: "Value is owned for now; zero-copy borrowed decoding is a later optimization."
  - `encode_value(value: &Value, out: &mut Vec<u8>)`: Encodes item tag and payload bytes per Section 4.2.
  - `decode_value(input: &[u8]) -> Result<(Value, usize), ProtoError>`: Deserializes from a complete payload slice. If bytes run out prematurely, returns `ERR_MALFORMED_PAYLOAD` (`1006`) instead of `NeedMore`, because the frame header already declared complete payload length.
  - Robust input validation and defense against malicious input:
    - Never allocates before verifying that the declared string/bytes length does not exceed available buffer bytes.
    - Avoids wrapping `as` casts using `usize::try_from`.
    - Preserves exact IEEE-754 bit representations for `Float` (comparing via `to_bits()`), ensuring `-0.0` and `NaN` round trip cleanly.
    - Strict boolean validation: rejects any byte other than `0x00` / `0x01` with `ERR_MALFORMED_PAYLOAD`.
    - Strict UTF-8 validation for `Text`.
    - Returns `ERR_UNKNOWN_TYPE_TAG` (`1007`) on unrecognized type tags.
  - Comprehensive unit tests:
    - Round trip across all variants including edge cases (`0`, `1`, `-1`, `i64::MIN`, `i64::MAX`, `0.0`, `-0.0`, `1.5`, `f64::MAX`, `f64::MIN_POSITIVE`, `f64::INFINITY`, `f64::NAN`, empty/long text and bytes).
    - Exact byte match assertions.
    - Items from `protocol.md` hex examples (Text "ping", Text "pong", Int 42, Text "put", Text "user").
    - Error handling on bad inputs (empty, unknown tag `0x7F`, cut varints, 7-byte float, bool `0x02`, invalid UTF-8, declared length > data left, `u64::MAX` length).
    - Trailing bytes preservation check.
    - Deterministic 1,000-iteration round-trip loop for pseudorandom Int and Bytes.
    - Fuzz resilience: 2,000 pseudo-random byte inputs (lengths 0..64) fed to `decode_value` with zero panics.

---

## 2026-10-11: Week 5 Step 3b: Compound Items

- **Date**: 2026-10-11
- **Summary**: Implemented compound payload items (`Array`, `Map`, `Error`), collection nesting depth limit (8 levels), allocation amplification protections, and Section 4.3 forward-compatible unknown-tag skipping in `src/proto/item.rs`.
- **Limits Found in Specifications**:
  - *Max collection nesting depth*: 8 levels (enforced with code `3005` `ERR_MAX_DEPTH_EXCEEDED` per `errors.md` and `values.md` Section 4).
  - *Max frame size*: 64 MB default (`67,108,864` bytes per `protocol.md` Section 9).
  - *Max argument count*: 65,536 items (command argument frames).
  - *Max key size*: 1 KB default (`1,024` bytes).
  - *Max value size*: 32 MB default.
  - *Generic collection item count*: The spec defines no separate item-count limit for arbitrary Array/Map payload items (bounded only by 64 MB frame size and memory).
- **Unknown-Tag Decision (Section 4.3)**:
  - `protocol.md` Section 4.3 states: *"Every future type tag (>= 0x09) must place an unsigned varint byte length directly after the tag. Older clients that encounter an unknown type tag can read the varint length and safely skip past the unknown item."*
  - The spec prescribes *skipping* rather than keeping. Therefore, we do not store unknown items into an `Unknown` variant on `Value`. Instead:
    - Within collections (`Array` and `Map`), items with tags `>= 0x09` are safely skipped using their varint length prefix without aborting collection decoding.
    - `skip_value(input: &[u8]) -> Result<usize, ProtoError>` is provided to skip any valid item (scalar, compound, or extension tag) without allocating.
    - Direct calls to `decode_value` on a standalone unknown tag return `ERR_UNKNOWN_TYPE_TAG` (`1007`) because no `Value` variant exists for unrepresentable types.
- **Safety and Memory Defenses**:
  - Bounded initial capacity: incoming collection vectors are allocated with `min(count, bytes_left, 1024)`. If declared count exceeds remaining bytes (or remaining bytes / 2 for Map), decoding errors immediately (`ERR_MALFORMED_PAYLOAD` `1006`).
  - Strict code bounds: `Error` item code is validated to fit in `u16` without lossy `as` casts.
  - Maximum nesting depth: checked at both encode and decode time. Encoding an over-deep value returns `Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED)`.
- **Open Questions**:
  - *Memory amplification*: A 64 MB payload of Nil items could create tens of millions of `Value`s (memory amplification). We need an item-count limit or a total-items budget in spec v0.2 / server config.
  - *Map forward compatibility*: When either the key or value in a Map pair uses an unknown tag `>= 0x09`, our parser currently skips the entire pair. Spec v0.2 should formally clarify whether partial pairs are discarded or whether Map keys must always belong to a restricted scalar type set.

---

## 2026-10-11: Week 5 Step 4: Frame Encode/Decode

- **Date**: 2026-10-11
- **Summary**: Implemented binary `Frame` representation, atomic frame encoding with payload size enforcement (`encode_frame`), and streaming zero-copy frame parsing with partial-read pipelining (`decode_frame`).
- **Part A Findings**:
  1. *Unknown tags*: Top-level unknown tags return `ERR_UNKNOWN_TYPE_TAG` (`1007`) when decoded as standalone values; inside an `Array`, unknown tags are skipped using their varint length prefix per Section 4.3, resulting in an in-memory `Array` whose element count is smaller than the wire item count.
  2. *Error 1007 occurrence*: `errors.md` defines 1007 for "unrecognized type tag byte without length prefix". Because Section 4.3 stipulates that all future tags ($\ge 0x09$) include length prefixes, 1007 is returned when standalone AST deserialization encounters an unrepresentable tag.
  3. *8 Spec Examples*: Added round-trip tests for all 8 examples in `docs/spec/protocol.md` Section 10; wire bytes match hex identically and decode back to identical frames.
  4. *Map Keys*: Neither `protocol.md` nor `values.md` specifies restrictions on key types or duplicate keys.
- **Part B Step 0 Findings**:
  1. *Payload structure*: In all spec examples, the payload consists of exactly one top-level typed item whose encoded size equals `payload_len`.
  2. *Payload length 0*: Undefined in spec; fails item decoding with `ERR_MALFORMED_PAYLOAD` (`1006`) because even `Nil` requires 1 byte.
  3. *HELLO / COMMAND / PUSH payloads*: HELLO request is `Array(3)` and response is `Map`; COMMAND request is `Array` of arguments and response is a typed `Value` or `Error`; PUSH is `Array`.
  4. *Legal flag combinations*: `0x00` (Request), `0x01` (Response), `0x03` (Error Response), `0x05` (Push). Bit 3 (`FLAG_MORE_FRAMES`) and bits 4–7 are reserved.
  5. *Fatal protocol errors*: All 1xxx errors (1001–1007), replication stream failures (6002, 6003), and shutdown (7004) close the TCP connection immediately.
- **What Was Added**:
  - `src/proto/mod.rs`: `Frame { header: Header, payload: Value }`.
  - `src/proto/encode.rs`: `encode_frame` and `encode_frame_with_limit` (atomic: truncates buffer to original length on failure; enforces max payload 64 MB).
  - `src/proto/decode.rs`: `decode_frame` and `decode_frame_with_limit` (stateless, zero-copy, returns `NeedMore` on incomplete header or payload; rejects invalid headers immediately after 14 bytes; rejects trailing bytes in payload with `1006`).
  - Unit tests: pipelining, exhaustive split reads across all byte offsets, byte-by-byte streaming, trailing payload rejection, 300 random frame chunked round-trips, and 6,000 fuzz samples without panics.
- **Open Questions**:
  - Should duplicate Map keys be rejected or last-write-wins in spec v0.2?
  - Does spec v0.2 plan to define multi-item root payloads or formally restrict root payloads to exactly one item?
  - A top-level item with an unknown tag gives error 1007 (connection closes), while the same item inside Array/Map is skipped. Is this what we want for future versions?

---

## 2026-10-11: Week 5 Step 5: Fuzzing Setup

- **Date**: 2026-10-11
- **Summary**: Verified collection forward compatibility and unknown-tag dropping alignment for `Map` and `Array`, introduced standalone fuzz testing crate in `fuzz/` with 3 targets and seed corpus from all 8 spec examples, and added a 60-second CI smoke test job.
- **Part A Results (Map & Unknown Tags)**:
  - Verified `decode_map` behavior: when either the key or value of an entry has an unknown tag ($\ge 0x09$), the entire key-value pair is dropped while properly advancing the input cursor past both items. Subsequent entries remain completely aligned.
  - Verified `Array` behavior: unknown items ($\ge 0x09$) in an array are safely skipped, and the remaining known items are collected into the `Array` without index disruption or misalignment.
  - Added unit test `test_map_and_array_unknown_tags_alignment_and_dropping` in `src/proto/item.rs` validating:
    - Map with an unknown key drops the pair.
    - Map with an unknown value drops the pair.
    - Map with an unknown pair flanked by known pairs drops only the unknown pair and retains the known pairs with accurate consumed byte count.
    - Array with an unknown item in the middle skips the unknown item and retains surrounding items.
- **Part B Results (Fuzz Targets & Corpus)**:
  - Created standalone package `fuzz/` (`cachedb-fuzz`) with an empty workspace so it does not affect root crate builds or pollute normal cargo workflows.
  - Implemented 3 dedicated fuzz targets in `fuzz/fuzz_targets/`:
    - `decode_frame`: Validates crash-free decoding on arbitrary binary input, ensures `used <= data.len()`, and verifies re-encode / re-decode equality for valid frames.
    - `decode_value`: Validates crash-free item AST decoding, ensures `used <= data.len()`, and validates re-encode / re-decode round-trip equality for valid values.
    - `split_stream`: Validates streaming decode invariance by splitting arbitrary inputs into two chunks at all possible byte boundaries and confirming the emitted frame sequence matches single-buffer decoding.
  - Generated seed corpora in `fuzz/corpus/decode_frame/` and `fuzz/corpus/decode_value/` containing all 8 canonical binary frames from `docs/spec/protocol.md` Section 10 (`example_1.bin` through `example_8.bin`).
  - *Platform Note*: `libFuzzer` (`libfuzzer-sys`) requires Linux/POSIX/WSL runtime and does not build on Windows MSVC. Development on Windows relies on standard deterministic fuzzing tests in unit tests, while continuous coverage is executed in CI on Linux.
- **Part C Results (CI Smoke Test)**:
  - Added `fuzz-smoke` job to `.github/workflows/ci.yml` running on `ubuntu-latest` with nightly toolchain.
  - Installs `cargo-fuzz` and executes 60-second smoke tests across all three targets (`decode_frame`, `decode_value`, `split_stream`) with a 2 GB RSS limit.
  - Configured automated artifact upload for `fuzz/artifacts` on failure so crashes can be immediately inspected and reproduced.

---

## 2026-10-11: First Fuzz Finding

- **Date**: 2026-10-11
- **Fuzz Crash Finding**:
  - CI execution of `cargo fuzz run decode_frame` found a panic on a 29-byte frame:
    `ca db 01 00 00 00 00 0a 00 02 00 00 00 0f 06 03 0d 03 70 75 74 04 04 75 73 65 72 01 54`
  - This payload is an `Array(3)` containing an unknown extension tag item (`0x0d 0x03 70 75 74`), followed by `Text("user")` and `Int(42)`.
  - The wire header declared `payload_len = 15`.
  - The decoder correctly followed Section 4.3 and skipped the unknown-tag item, returning `Array([Text("user"), Int(42)])`.
  - When the decoded `Frame` was re-encoded, the newly encoded payload had length 10 (omitting the skipped unknown item).
  - The fuzz target previously asserted `re_decoded == Decoded::Complete((frame, encoded.len()))`, comparing `re_frame.header.payload_len` (10) against `frame.header.payload_len` (15), which panicked (`10 != 15`).
- **Resolution**:
  - The decoder behavior was 100% correct according to the specification.
  - The fuzz check was amended: `decode_frame` fuzz target now compares `flags`, `request_id`, `opcode`, and `payload` equality between the original and re-decoded frame. For the re-encoded bytes, it verifies `re_frame.header.payload_len == encoded.len() - HEADER_LEN` and `re_used == encoded.len()`.
  - Audited `split_stream.rs` (compares all-at-once vs chunked decoding on the exact same wire bytes, so wire lengths match) and `decode_value.rs` (re-encodes value and checks `re_used == encoded.len()`, unaffected by wire headers).
  - Added unit regression test `test_regression_fuzz_unknown_tag_in_array_payload_len_shrinks` in `src/proto/decode.rs`.
  - Added binary seed `regression_unknown_tag_skip.bin` to `fuzz/corpus/decode_frame/`.
  - Updated CI to run `decode_frame`, `decode_value`, and `split_stream` as isolated matrix jobs with individual artifact uploads.
- **Open Design Note**:
  - Frame.header.payload_len is the wire value. After skipping unknown items inside Array/Map, it can be larger than the length of a re-encoded payload. Consider removing payload_len from Frame (derive it when encoding) in a future cleanup.

---

## 2026-10-11: Week 5 Core Complete (Premature Milestone Note)

> **Note**: This milestone marked core wire framing and encoding/decoding completion, but Week 5 in `cachedb-roadmap.md` also requires HELLO negotiation, hard limits (max arguments), and a one-hour fuzzing run. See correction and Step 6 below.

- **Date**: 2026-10-11
- **Summary**: Core binary frame encoding and decoding completed and verified across all platforms and CI workflows.
- **What Exists**:
  - `Header`: 14-byte wire header representation (`magic`, `version`, `flags`, `request_id`, `opcode`, `payload_len`), endianness-safe encode/decode, opcode mapping, fast header validation.
  - `Varint`: Unsigned LEB128 (`encode_uvarint`, `decode_uvarint`), signed zigzag LEB128 (`encode_ivarint`, `decode_ivarint`, `zigzag_encode`, `zigzag_decode`) with overflow detection.
  - `Value`: Owned AST representation for all 9 spec types (`Nil`, `Int`, `Float`, `Bool`, `Text`, `Bytes`, `Array`, `Map`, `Error`), canonical encoding/decoding, bitwise float comparisons, and Section 4.3 extension tag skipping (`skip_value`).
  - `Frame`: Complete frame representation, atomic `encode_frame` / `encode_frame_with_limit` (truncation on error, 64 MB guard), zero-copy streaming `decode_frame` / `decode_frame_with_limit` with partial buffer handling (`Decoded::NeedMore`).
  - Unit & Integration Tests: Exact byte round-trip matching for all 8 specification hex examples from `docs/spec/protocol.md` Section 10, exhaustive byte-offset split-read streaming tests, and deterministic randomized round-trips.
- **CI Results**:
  - 3 isolated fuzz smoke jobs in GitHub Actions (`decode_frame`, `decode_value`, `split_stream`), running 60 seconds each on Linux nightly with seed corpora and regression seeds.
  - Sanitizer job added (AddressSanitizer on Linux nightly). If it fails with something that looks like a false alarm from std, record it here and do not hide it.
- **First Fuzz Finding**:
  - Fuzz check falsely failed on a 29-byte frame because skipped unknown tags inside `Array` naturally shrink the re-encoded `payload_len` relative to the wire header; the decoder was verified 100% spec-compliant and the test assertion was corrected.
- **Consolidated Open Questions for Spec v0.2**:
  1. *Unknown opcode error code*: `Header::decode` returns `ERR_UNKNOWN_COMMAND` (`2001`); should spec v0.2 define a dedicated 1xxx framing error code (e.g. `ERR_UNKNOWN_OPCODE`) for unrecognized wire opcodes?
  2. *Non-canonical varints*: The specification is silent on non-shortest LEB128 encodings (e.g. `0x80 0x00` for 0); our parser currently accepts standard LEB128 encodings. Spec v0.2 should clarify if canonical shortest-form is mandatory.
  3. *Top-level unknown tag vs collections*: A top-level item with an unknown tag gives error 1007 (connection closes), while the same item inside `Array`/`Map` is safely skipped per Section 4.3. Is this asymmetry intended for future versions?
  4. *Memory amplification / item-count limit*: A 64 MB frame containing 1-byte items (e.g. `Nil`) can allocate tens of millions of AST nodes, causing memory amplification. Should spec v0.2 introduce an item-count limit or total-items budget?
  5. *Map duplicate keys and key types*: Neither `protocol.md` nor `values.md` specifies key type restrictions or behavior for duplicate keys (reject vs last-write-wins).
  6. *`payload_len` in `Frame`*: `Frame.header.payload_len` is the wire value. After skipping unknown items inside Array/Map, it can be larger than the length of a re-encoded payload. Consider removing `payload_len` from `Frame` (derive it when encoding) in a future cleanup.

---

## 2026-10-11: Correction: Week 5 is Not Done

- **Date**: 2026-10-11
- **Reason**: The earlier entry marked Week 5 complete based on parser/serializer milestones. However, `cachedb-roadmap.md` defines three remaining deliverables for Week 5:
  1. Protocol version negotiation (`HELLO` exchange).
  2. Hard limits (`max_args` of 65,536 items on command requests, checked before reading items or allocating).
  3. Fuzzing campaign for an hour with zero crashes.
- **Action**: Phase 1 Week 5 status in `README.md` reset to *In progress*.

---

## 2026-10-11: Week 5 Step 6: HELLO, Limits, Long Fuzz

- **Date**: 2026-10-11
- **Summary**: Implemented protocol-level HELLO exchange structures, added protocol Limits with immediate argument count bounds enforcement, and created the on-demand 1-hour fuzzing workflow.
- **Deliverables**:
  - **HELLO Handshake Module (`src/proto/hello.rs`)**:
    - `HelloRequest`: represents client handshake payload (`protocol_version`, `client_name`, optional `auth_token`). `to_value` and `from_value` enforce exact Array shape (2 or 3 items), field types, non-empty client name $\le 1024$ bytes.
    - `HelloResponse`: represents server metadata Map payload (`server_name`, `server_version`, `protocol_version`, `client_id`, optional limit metadata).
    - `negotiate_version`: strictly enforces `protocol_version == 1` per v0.1 frozen spec, returning `ERR_UNSUPPORTED_PROTOCOL_VERSION` (`1001`) otherwise.
  - **Hard Limits Module (`src/proto/limits.rs`)**:
    - `Limits`: configurable framing limits (`max_frame_size = 64 MB`, `max_args = 65,536`, `max_depth = 8`). Note: key size (1 KB) and value size (32 MB) belong to command dispatch in Week 7.
    - `decode_frame_with_limits`: checks `max_args` on `Opcode::Command` requests *immediately* upon reading the Array length, before reading items and before reserving vector capacity. Returns `ERR_TOO_MANY_ARGS` (`2007`).
    - Propagates configurable `max_depth` to `decode_value_with_max_depth`.
  - **Long Fuzz Campaign Workflow (`.github/workflows/fuzz-long.yml`)**:
    - `workflow_dispatch` manual trigger with configurable duration (default 60 minutes).
    - Matrix across `decode_frame`, `decode_value`, and `split_stream`.
    - Uploads grown corpus artifacts with 14-day retention, and failure artifacts on crash.
    - Updated `fuzz/fuzz_targets/decode_value.rs` to call `HelloRequest::from_value` and `HelloResponse::from_value` to ensure zero panics on arbitrary parsed values.

---

## 2026-10-11: Week 6 Plan (Draft) — Hash Table Only

- **Date**: 2026-10-11
- **Focus**: Building a scratch-built, high-performance in-memory hash table in `src/store/` with incremental resizing, non-cryptographic hashing, and differential property testing.
- **Decisions**:
  - **Keys are Bytes**: Per `values.md` Section 1.1 and `commands.md` Section 2.5, keys are arbitrary binary byte buffers (`Vec<u8>` / `&[u8]`) between 1 byte and 1 KB.
  - **Values in v0.1**: Stored as the owned enum `Value`. Boxing or compact flat-buffer encoding will be decided after Week 6 scale benchmarks.
  - **Networking Stays in Week 7**: Event loop wiring, socket dispatch, and session state are isolated to Week 7.

### Step 1: Hash Function & Core Robin Hood Table Structure (`src/store/`)
- **What to Build**:
  - Implement fast non-cryptographic hash function (e.g. SipHash-1-3 or Wyhash/FNV variant) with per-process random 64-bit seed.
  - Implement open-addressing hash table with **Robin Hood probing**:
    - Slots store key bytes (`Vec<u8>`), `Value`, and probe sequence length (PSL).
    - Insert uses "rich take from the poor" swap rule.
    - Backward-shift deletion avoids tombstone accumulation.
- **What to Test**:
  - Collision resolution, probe length distribution, basic insert/lookup/remove correctness.

### Step 2: Incremental Resizing (Zero-Stall Rehashing)
- **What to Build**:
  - Dual-table structure (`old_table` and `new_table`) when load factor exceeds threshold (e.g., 0.85).
  - Paced migration: each operation migrates $k$ consecutive buckets (e.g. 16 buckets).
  - Lookups check `new_table`, then `old_table`. Inserts/updates target `new_table`.
  - Active tick migration: timer tick migrates buckets to ensure completion even under read-only traffic.
- **What to Test**:
  - Lookups, updates, and deletes during mid-resize state; prove zero dropped keys.

### Step 3: Differential Property Testing & Scale Benchmarks
- **What to Build**:
  - Property test comparing Robin Hood table with `std::collections::HashMap` under 100,000+ random operations.
  - Scale benchmark in `benches/store_bench.rs` measuring throughput and latency at 1M and 10M keys.

---

## Week 7 Start: Network Wiring (Draft)

- **Focus**: Connecting `src/proto` to `src/net`, managing connection session state, and dispatching core commands over live sockets.
- **Step 1: Protocol Event Loop Adapter & Session State**:
  - Drain bytes from connection `ReadBuffer` into `decode_frame_with_limits`.
  - Session state machine: `Handshake` (requires `HELLO`), `Ready`, `Closing`.
  - `HELLO` handler: validate `HelloRequest`, respond with `HelloResponse` frame.
  - `PING` command: client sends `COMMAND` frame (`opcode = 0x0002`, `flags = 0x00`) with payload `Array([Text("ping")])`; server replies with `COMMAND` frame (`opcode = 0x0002`, `flags = 0x01`) and payload `Text("pong")` (per Examples 1 & 2). *Commands are payload verbs, not header opcodes.*
- **Step 2: Protocol Error Handling and Socket Teardown**:
  - **Fatal 1xxx Framing Errors**:
    - If the header was intact enough to read `request_id` (e.g. `1001` unsupported version, `1002` unsupported flag per Example 8): send a structured error response frame (`flags = 0x03`, payload `Error { code, message }`) and *then* close the connection.
    - If framing corruption occurs before header completion (e.g. `1004` invalid magic `0xCA 0xDB`): close the TCP connection immediately with no reply.
  - **Application Errors (2xxx)**: send error frame (`flags = 0x03`) and keep the connection open.

---

## 2026-10-11: CI Fix: Formatting and Line Endings & Spec Clarifications

- **Date**: 2026-10-11
- **CI Fix**:
  - `src/proto/limits.rs`: Removed duplicate blank lines at the end of the file that triggered `cargo fmt --check` failure on Linux CI.
  - Checked line endings of all 58 files across `src/`, `tests/`, `examples/`, `fuzz/`, and `.github/`: all files already consistently use LF (0 CRLF, 0 mixed).
  - Added repository-wide `.gitattributes` enforcing `eol=lf` on `*.rs`, `*.toml`, `*.md`, `*.yml`, `*.yaml`, `*.js`, `*.lock`, and `binary` on `*.bin`.
- **Correction on Fatal Framing Errors (Codes 1001–1007)**:
  - *Correction of earlier Step 6 Part E statement*: The earlier answer stated that for 1001–1007, the server closes without reply, and erroneously described 1005 as a "broken header" where the server might send an error frame.
  - *Facts from frozen specifications*:
    1. Error 1005 (`ERR_MALFORMED_VARINT`) has **nothing** to do with the header. The 14-byte wire header has no varints at all (all header fields are fixed-width big-endian integers). Varints only appear in the payload.
    2. Codes that happen after the 14-byte header has been read successfully (so `request_id` and `opcode` are already known) are: `1001`, `1002`, `1003`, `1005`, `1006`, and `1007`. Only `1004` (`ERR_INVALID_MAGIC`) fails before a valid header is parsed.
    3. The spec explicitly specifies sending an error frame before closing for `1001` (Section 5: "Server returns an error response with code 1001 ... and closes the connection") and `1002` (Section 8 and Example 8: sends error response frame with `flags = 0x03` matching `request_id`, then closes). For `1003`, Section 9 states "Connection closed immediately". For `1004`, magic is invalid so the connection is dropped immediately without reply. For `1005`, `1006`, and `1007`, the spec states the connection is closed, but is silent on whether an error response frame is sent first.

### Fatal Errors (1001–1007) Catalog:

| Code | Name | Happens when | Can header be valid? | Error frame sent before close? |
|:---:|---|---|:---:|:---:|
| **1001** | `ERR_UNSUPPORTED_PROTOCOL_VERSION` | Client sends unsupported version in `HELLO` | Yes | Yes (explicit in Section 5) |
| **1002** | `ERR_UNSUPPORTED_FLAG` | Client sets reserved or unsupported flag bit | Yes | Yes (explicit in Section 8 & Example 8) |
| **1003** | `ERR_FRAME_TOO_LARGE` | Frame `payload_len` exceeds `max_frame_size` | Yes | No (Section 9: "Connection closed immediately") |
| **1004** | `ERR_INVALID_MAGIC` | Header does not begin with `0xCA 0xDB` | No | No (connection dropped immediately) |
| **1005** | `ERR_MALFORMED_VARINT` | Varint in payload exceeds 10 bytes or terminates early | Yes | Spec is silent |
| **1006** | `ERR_MALFORMED_PAYLOAD` | Payload ends before declared length or bad structure | Yes | Spec is silent |
| **1007** | `ERR_UNKNOWN_TYPE_TAG` | Payload contains unrecognized type tag at top-level | Yes | Spec is silent |

- **Open Question: Command Sent Before HELLO**:
  - `protocol.md` Section 5 states: *"Upon opening a TCP connection, clients should perform a HELLO exchange:"*
  - Notice the spec uses "should", not "must".
  - In `errors.md`, there is no `ERR_HELLO_REQUIRED` error code. (Code `2003` is `ERR_INVALID_ARGUMENT`).
  - If authentication is configured, code `5001` (`ERR_AUTH_REQUIRED`) is returned. However, if no authentication is required, the spec is silent on whether `HELLO` is mandatory before commands like `PING`, or if `HELLO` can be skipped.
  - Added to Open Questions for Spec v0.2: Clarify whether `HELLO` is strictly required before any command when authentication is disabled.

---

## 2026-10-11: Week 5 Complete

- **Date**: 2026-10-11
- **Status Note**: The earlier "Week 5 complete" entry was prematurely recorded before HELLO negotiation, hard limits, and the 1-hour fuzzing workflow were implemented; all remaining scope has now been fully built, verified, and closed.
- **Roadmap Week 5 Deliverables & Evidence**:
  1. *Frame Encode/Decode*: Implemented in `src/proto/encode.rs` and `src/proto/header.rs`. Verified by round-trip tests matching all 8 frozen specification hex examples byte-by-byte (`test_round_trip_all_eight_spec_examples`), plus 300 randomized deterministic round-trips.
  2. *Streaming Parser*: Implemented in `src/proto/decode.rs`. Verified by split-read tests at every possible byte boundary (`test_split_reads_every_possible_split_point`), 1-byte-at-a-time streaming (`test_one_byte_at_a_time_streaming`), back-to-back pipelined requests (`test_pipelining_two_frames_back_to_back`), and partial buffer `Decoded::NeedMore` handling.
  3. *HELLO with Version Negotiation*: Implemented in `src/proto/hello.rs`. `HelloRequest` and `HelloResponse` serialize and deserialize with full type validation; `negotiate_version` enforces `protocol_version == 1` per v0.1 spec, rejecting versions 0, 2, 255 with `ERR_UNSUPPORTED_PROTOCOL_VERSION` (1001). Full frame exact wire byte layouts verified byte-for-byte in `test_hello_exact_bytes_full_frame`.
  4. *Hard Limits*: Implemented in `src/proto/limits.rs`. Fast rejection of oversized frames (`ERR_FRAME_TOO_LARGE`, 1003) at header parse time before allocating; fast rejection of `max_args > 65,536` on `COMMAND` requests (`ERR_TOO_MANY_ARGS`, 2007) immediately upon reading the array count, before decoding items or allocating buffers; recursion depth protected with `ERR_MAX_DEPTH_EXCEEDED` (max 8 levels).
  5. *Fuzzing Campaign*: Configured 3 fuzz targets (`decode_frame`, `decode_value`, `split_stream`) in `fuzz/`. 60-second CI smoke checks run green in GitHub Actions. Extended 60-minute fuzzing workflow (`.github/workflows/fuzz-long.yml`) executed on 2026-10-11 across all 3 targets with zero crashes.

---

## 2026-10-11: Week 6 Step 1: Hash Function and Robin Hood Table Core

- **Date**: 2026-10-11
- **Scope**: Implemented non-cryptographic hash function (`src/store/hash.rs`) and core fixed-capacity Robin Hood hash table (`src/store/table.rs`) in `src/store/`.

### Step 0 Review:
1. *Key Rules*: `commands.md` and `values.md` specify keys as arbitrary binary byte sequences (`&[u8]`). Empty keys (`""`) return error 2006 (`ERR_EMPTY_KEY`) and default max key size is 1 KB (1024 bytes). The low-level table accepts any byte slice (`Box<[u8]>`), including empty and large keys; key length validation (empty key and 1 KB limit) is enforced at the command parsing/dispatch layer in Week 7, not by the storage table.
2. *Value Model*: In Phase 1 / v0.1, values are primitive types (`Value`). The table is generic `RobinHoodTable<V>`, allowing unit tests to store lightweight types (`u64`) while the storage engine stores `proto::Value`.
3. *Threading Model*: Per `docs/design/threading.md`, the table is owned and accessed strictly by a single thread (the shard owner). It contains zero mutexes, zero read-write locks, and zero atomics.
4. *Week 6 Plan*: Step 1 focuses on core fixed-capacity table with Robin Hood probing, backward-shift deletion, and invariant checking. Step 2 will add incremental resizing (dual-table migration) when load reaches 7/8. Step 3 adds property testing and scale benchmarks.

### Design Choices:
- **Hash Function**: Based on Wyhash (v4) 128-bit multiply-and-fold (`mum`) algorithm. Safe Rust, 0 lossy casts, 0 panics. Per-process seed initialized via `std::hash::RandomState`. Avalanche check achieves ~31.93 flipped bits (ideal 32) over 200 inputs; 100k keys distributed across 1024 buckets stay well within $\pm 40\%$ deviation.
- **Robin Hood Probing**: Slots store `hash (u64)`, `psl (u32)`, `key (Box<[u8]>)`, `value (V)`. Ideal slot computed via `hash & mask`. Displaces richer elements when `curr_psl > existing.psl`.
- **Backward-Shift Deletion**: Deleting an element shifts subsequent cluster elements with `psl > 0` backwards by 1 and decrements their PSL. Eliminates tombstones and preserves contiguous clusters.
- **Fixed Capacity Limit**: Fixed capacity with maximum load factor $7/8$ (87.5%). Insert returns `Err(TableFull)` when new insertion would exceed $7/8$ capacity. Overwriting an existing key never increases load and always succeeds.
- **Invariants Checked**: Validated via `check_invariants()`: accurate PSL calculation, neighbor PSL gradient $psl[i+1] \le psl[i] + 1$, cluster continuity, length consistency, and key uniqueness.
- **Stats**: Under 70% load with 183,500 keys in 262,144 slots, `max_psl` was 13 and `avg_psl` was 1.176.

### Open Questions for Spec v0.2:
- Does future spec v0.2 require typed keys (e.g. integer or text keys) in collections or command dispatch, or remain purely raw binary? (Values spec confirms raw binary `&[u8]`).

---

## 2026-10-11: Fix: Module Name

- **Date**: 2026-10-11
- **Summary**: The hash table was first created in `src/storage/` by mistake. The project layout from the roadmap uses `src/store/`. Moved to `src/store/`.

---

## 2026-10-11: Week 6 Step 2: Incremental Resizing for Robin Hood Table

- **Date**: 2026-10-11
- **Scope**: Implemented non-blocking incremental dual-table resizing for `RobinHoodTable<V>` in `src/store/table.rs`.

### Architecture & Decisions:
1. **Decision: Read-Only `get(&self)` Without Migration**:
   - `get(&self)` takes `&self` and does not trigger migration.
   - *Reason*: Reads remain cheap, non-mutating, and shareable. In an event-driven architecture, event loop timer callbacks call `step_migration` periodically, so even a 100% read-only workload steadily completes background resizing without imposing mutation overhead on reads.
   - Paced migration (`MIGRATE_BATCH = 16`) is performed only in mutating operations (`insert`, `remove`, `get_mut`) and explicit `step_migration` calls.
2. **Dual Tables & Consistent Seeding**:
   - `main_table: SingleTable<V>`: Primary active table.
   - `old_table: Option<SingleTable<V>>`: Drained table during active resizing.
   - `migrate_cursor: usize`: Monotonically increasing scan cursor in `old_table`.
   - `seed`: Both `main_table` and `old_table` strictly share the identical 64-bit seed (`self.seed`). Cached hashes are transferred directly without re-hashing.
3. **Hole-Free Migration via Backward-Shift**:
   - When cursor is at index $i$: if slot $i$ is empty, cursor advances. If slot $i$ is full, it is removed using backward-shift deletion (`remove_at_and_shift`).
   - Subsequent cluster entries shift backward into $\ge \text{cursor}$ with decremented PSL. No empty hole is ever created within a probe cluster.
   - `old_table` remains 100% valid and satisfies all Robin Hood invariants at every single step of migration.
4. **Key Preservation on Migration Failure**:
   - If inserting a migrated slot into `main_table` returns an error (e.g. `TableFull`), the slot is restored into `old_table` via `old.insert_slot(slot)` and the error is returned. Keys are never dropped or lost.
5. **Operation Rules**:
   - `get(&self)`: Checks `main_table` first, then `old_table`. 0 migration buckets touched.
   - `get_mut`: Steps migration; checks `main_table`, then `old_table` (migrating on find).
   - `insert`: Steps migration; removes from `old_table` if present; writes to `main_table`.
   - `remove`: Steps migration; removes from both `main_table` and `old_table`.
   - `len`: Returns `main_table.len + old_table.len`.
6. **Latency Bounds**:
   - `get`: 0 migration buckets touched; probes $\approx 1-2$ slots.
   - `insert` / `remove` / `get_mut`: At most 16 buckets touched in `old_table` plus one Robin Hood operation in `main_table`.

### Invariants & Testing:
- `check_invariants()` verifies:
  - Both tables satisfy PSL accuracy, neighbor gradient $psl[i+1] \le psl[i] + 1$, and cluster continuity.
  - Empty slot terminates probe: for any occupied slot with `psl > 0`, the previous slot `(i + C - 1) % C` must be occupied.
  - Slots $0 \le i < \text{migrate_cursor}$ in `old_table` are strictly empty (`None`).
  - No key byte sequence is duplicated across `main_table` and `old_table`.
  - Total length matches the sum of occupied slots across both tables.
  - `Slot` fields are scoped to `pub(crate)`.
- Verification test suite:
  - `test_old_table_backward_shift_migration_cluster_wrap_invariants_at_every_step`: Validates `old_table` invariants and lookups at every single migration step on a wrapping collision cluster.
  - `test_one_seed_for_both_tables_after_full_resize`: Proves both tables share the same seed and all stored hashes equal `hash64(key, seed)`.
  - `test_migration_error_does_not_lose_key_when_main_is_full`: Proves failed migration restores slot to `old_table` without key loss.
  - `test_invariant_empty_slot_before_psl_greater_than_zero_fails`: Intentionally breaks the probe continuity rule and asserts invariant failure.
  - `test_differential_vs_std_hashmap_100k_ops`: 100,000 randomized operations against `std::collections::HashMap` with invariant checks every 500 ops.
  - `test_insert_100k_keys_from_small_table_no_err_all_found`: 100k sequential inserts starting from capacity 8.

### Open Questions & Future Considerations:
- **Table Shrinking**: Currently, the table grows dynamically up to $2^{30}$, but does not shrink when elements are removed. Should an incremental down-sizing migration be initiated when load factor drops below $1/8$ (12.5%)? (Left for future consideration; not required in Phase 1 roadmap).

---

## 2026-10-11: Week 6 Step 2 Safety Fix: Guaranteed Key Preservation & Strict Resizing Robustness

- **Date**: 2026-10-11
- **Scope**: Hardened `RobinHoodTable<V>` in `src/store/table.rs` against silent key loss and edge-case migration failures.

### Safety Fixes & Architectural Decisions:
1. **`get_mut_with_hash` Zero-Reallocation & Fallback Restore**:
   - When key resides in `old_table`, the entire slot is extracted directly via `remove_at_and_shift` (avoiding new `Box` allocation).
   - If `main_table.insert_slot(slot)` fails (e.g. `main_table` is at max load limit), the slot is restored into `old_table` via `old.insert_slot(returned_slot)`.
   - A mutable reference `&mut V` directly into `old_table` is returned so the caller can safely mutate the value in place without dropping the key.
2. **`check_and_start_resize` Complete Migration Verification**:
   - `check_and_start_resize` completes running migration via `step_migration(usize::MAX)?`.
   - If `old_table` is still `Some` after the drain attempt (e.g. capacity limits prevent completion), it immediately returns `Err(TableFull)` without replacing or dropping `main_table` or `old_table`.
3. **`step_migration` Explicit Invariant Preservation**:
   - Restoring a slot to `old_table` on migration failure checks the restore result; if failed, the slot is safely parked in an available slot and `debug_assert!` verifies restore integrity (no `let _ =`).
   - `old_table` is deallocated (`None`) **only** when `old_table.len == 0`.
   - If `migrate_cursor >= old_table.capacity` while `old.len > 0`, the cursor is wrapped to 0 to prevent abandoning remaining keys, and `debug_assert!(false, ...)` flags the unexpected state.
   - Clarified documentation: shifted items never go below `migrate_cursor` because backward shift only fills the hole at `migrate_cursor`, and all slots prior to `migrate_cursor` are empty.
4. **`insert_with_hash` Fail-Safe Slot Restoration**:
   - When overwriting a key that resides in `old_table`, the original slot is removed and kept.
   - If `main_table.insert_with_hash` fails, the removed slot is immediately restored into `old_table` so no existing key is ever dropped on insert errors.
5. **Accurate Resizing Latency Documentation**:
   - Updated `docs/design/hash-table.md` to truthfully acknowledge that while key copying is spread out incrementally across mutating operations, allocating and initializing the new backing table is $O(\text{new capacity})$ at resize start, to be measured and benchmarked in Step 3.
6. **Rigorous 3-Phase Differential Test**:
   - Implemented `test_differential_3_phases_5_seeds`: 300,000 operations per seed across 3 phases (grow, shrink, mixed) with 20,000 keys and 5 fixed seeds.
   - 10% operations run `step_migration(1..=4)`.
   - Validates $\ge 1,000$ operations during active resizing and $\ge 10$ finished resizes, with bidirectional consistency checks against `std::collections::HashMap`.

---

## 2026-10-11: Week 6 Step 3: Full Invariants List and Pre-Migration Capacity Safeguard

- **Date**: 2026-10-11
- **Scope**: Validated table invariant checks and hardened `step_migration` against capacity boundary conditions.

### Full Invariant Rules Checked:
1. **PSL is exact**: For every occupied slot $i$, `slot.psl == (i + capacity - ideal) & mask` where `ideal = hash & mask`.
2. **PSL gradient**: For adjacent slots $i$ and $(i + 1) \pmod{\text{capacity}}$, if both are occupied, `psl[i + 1] <= psl[i] + 1`.
3. **Probe continuity (empty slot ends probe)**: For every occupied slot $i$ with `psl > 0`, the previous slot `(i - 1 + capacity) & mask` must be occupied.
4. **Len is exact**: The total count of occupied slots strictly matches `table.len`.
5. **No duplicate keys**: For every occupied slot $i$, `find_index_with_hash(slot.hash, slot.key) == Some(i)` (uniqueness and reachability).
6. **Slots before cursor are empty**: In `old_table`, all slots $0 \le i < \text{migrate_cursor}$ are strictly empty (`None`).
7. **No key in both tables**: No key present in `old_table` exists in `main_table`.

### Pre-Migration Capacity Check:
- In `step_migration`, before extracting any slot from `old_table`, `main_table.len < main_table.max_load_limit()` is checked.
- If `main_table` is at or above its load limit, `step_migration` immediately returns `Err(TableFull)` without touching `old_table`, eliminating the fallback loop.

---

## 2026-10-11: Week 6 Step 3: Property Tests, Automated Shrinker, and Store Benchmarks

- **Date**: 2026-10-11
- **Scope**: Implemented property test suite with custom shrinker in `tests/property.rs`, standalone benchmark harness in `benches/store_bench.rs`, CI workflows, and empirical documentation in `docs/benchmarks.md` and `docs/design/hash-table.md`.

### Architecture & Decisions:
1. **Zero-Dependency Property Test Harness & Shrinker**:
   - Implemented `XorShift64` PRNG and multi-seed runner (`CACHEDB_PROP_SEEDS`, default 24 seeds).
   - Shrinker algorithm: iteratively tries removing halves (2), quarters (4), and single operations until reaching minimal failing operation sequences.
   - Verified via `test_deliberately_wrong_reference_shrinker` (`#[should_panic]`): minimized a 100-operation sequence down to $\le 4$ operations.
   - Property tests cover: mixed models with differential checking against `std::collections::HashMap`, complex key shapes (empty, binary, embedded nulls, 4 KB), severe hash collisions (modulo buckets and circular wraparound), resize invariants, permutation order independence, and overwrite/remove idempotence.
   - Full property test suite runs in under 0.10 seconds in debug mode.
2. **Comprehensive Benchmark Harness (`benches/store_bench.rs`)**:
   - Head-to-head comparison between `RobinHoodTable<u64>` and `std::collections::HashMap<Box<[u8]>, u64>`.
   - Supports `--smoke` (50,000 keys for fast CI execution) and `--keys <N>` (default 1,000,000 keys).
   - Tracks insert latency per operation in `Vec<u32>` and computes $p50$, $p99$, $p99.9$, $p99.99$, and $max$.
   - Tracks top 5 slowest operations and identifies the causal state transition (resize buffer allocation vs migration batch vs collision displacements).
   - Measures table quality metrics: max PSL, average PSL, load factor, finished resizes, slot size, and process VmRSS (Linux `/proc/self/status`, `"n/a"` on Windows).
   - Memory isolation: tables are dropped sequentially to prevent cross-table memory accumulation.
3. **CI Pipeline Integration**:
   - Updated `.github/workflows/ci.yml` with `cargo test --test property` and `cargo bench --bench store_bench -- --smoke`.
   - Created `.github/workflows/bench.yml`: manual `workflow_dispatch` trigger with configurable `keys` parameter, 30-minute timeout, Markdown summary written to `$GITHUB_STEP_SUMMARY`, and `benchmark-results.txt` artifact upload.
4. **Empirical Results (1,000,000 Keys)**:
   - *Throughput*: 0.87 Mops/s insert (1150 ns), 1.41 Mops/s get hits (709 ns), 2.13 Mops/s remove (470 ns).
   - *Latency Percentiles*: $p50 = 0.50\text{ µs}$, $p99 = 2.50\text{ µs}$, $p99.9 = 37.5\text{ µs}$, $p99.99 = 226.7\text{ µs}$, $max = 64.7\text{ ms}$.
   - *Table Quality*: Max PSL = 8, Average PSL = 0.44 probes/key across 1M entries, confirming outstanding probe fairness and clustering elimination.
   - *Spike Attribution*: Allocating and zero-initializing the new backing buffer when starting resize ($O(\text{new capacity})$) is the dominant source of tail latency ($24\text{ ms} - 64\text{ ms}$ for 1M-2M slot allocations).

---

## 2026-10-11: Week 6 Step 3b: Real Property Tests and Rigorous Benchmark Methodology

- **Date**: 2026-10-11
- **Scope**: Hardened property tests in `tests/property.rs`, fixed flawed benchmarking methodology in `benches/store_bench.rs`, updated documentation in `docs/benchmarks.md` and `docs/design/hash-table.md`.

### What Was Wrong With the First Benchmark:
1. **Key Cloning Inside Timed Loops**:
   - The original benchmark invoked `table.insert(key.clone(), idx)` and `hash_map.insert(key.clone(), idx)` directly inside the timed region.
   - Every single insert measured dynamic heap allocation, copying, and `Box` deallocation overhead rather than pure table hashing, probing, and displacement logic.
2. **Lack of Warmup and Repetitions**:
   - The first benchmark executed a single cold run per phase.
   - Cold CPU caches, OS demand-zero paging, and memory allocator page faults artificially distorted measurements (e.g. get-miss appeared artificially slow at 537 ns).
3. **Absence of Floor Baselines**:
   - There were no baseline floor rows measuring loop iteration overhead and raw key hashing. It was impossible to isolate how much latency came from calling `hash64` versus traversing table probe chains.
4. **Unmeasured Speculation on Slow Operations**:
   - Latency causes for slow inserts were inferred rather than instrumented.
   - In the revised benchmark, `capacity()` is explicitly measured before and after every operation to report with certainty whether an operation triggered a backing table expansion.

### Corrective Methodology Implemented:
1. **Zero Allocations in Timed Regions**:
   - Separate pre-allocated key sets (`keys_insert`, `keys_lookup`, `miss_keys`, `keys_overwrite`, `pre_mixed_ops`) are built before timers start.
   - Insert operations move `Box<[u8]>` directly via `into_iter()` with zero heap allocations during timing.
2. **1 Warmup Run + 3 Measured Runs**:
   - Every operation phase warms up CPU caches and backing pages once, then runs 3 measured iterations, reporting the median duration with minimum and maximum bounds.
3. **Floor Baselines Added**:
   - "Floor: loop + key access only" measures pure pointer dereferencing (~2.2 ns).
   - "Floor: hash only" measures raw 64-bit hashing (~21.4 ns for `hash64`, ~25.2 ns for `RandomState`).
4. **Measured Empirical Findings (1,000,000 Keys)**:
   - **Throughput**: `RobinHoodTable` beats `std::HashMap` across nearly all phases: Insert is **1.13x faster** (404.8 ns vs 456.9 ns), Get hits is **4.61x faster** (280.5 ns vs 1291.8 ns), Get misses is **1.24x faster** (150.7 ns vs 186.2 ns), Overwrite is **1.25x faster** (246.3 ns vs 307.6 ns), Mixed is **1.31x faster** (372.9 ns vs 490.0 ns).
   - **Get-Miss Investigation**: Now fine. Average miss probe count was measured via `lookup_probe_count` to be **1.70 slots**. Raw hashing takes 21.4 ns, and reading 1.70 slots (each 40 bytes, spanning ~2 to 3 64-byte DRAM cache lines) takes ~129 ns.
   - **Tail Latency Comparison**: In `std::HashMap`, stop-the-world rehashing causes a **147.57 ms** pause at op #917,504. In `RobinHoodTable`, incremental migration avoids rehashing, yielding a **24.39 ms** worst-case spike (6x lower pause).
   - **Resize Allocation Costs**: Empty table allocation (`RobinHoodTable::new`) was measured at $2^{20}$ slots (14.22 ms), $2^{22}$ slots (71.11 ms), and $2^{24}$ slots (340.56 ms).

### Upgraded Property Tests:
- Mixed model test upgraded to run **30,000+ operations** per seed with key universe of 6,000 keys, reaching `max_len >= 5,000` keys.
- Asserted that $\ge 5$ resizes finish (consistently 10 finished) and $\ge 500$ operations run while resizing (consistently 1,100+ operations).
- Heavy collisions and key shapes tests upgraded to run **10,000+ operations** per seed.
- Multi-seed driver outputs live stats for all seeds under `--nocapture`, completing all 7 tests in **6.26 seconds** in debug mode.

---

## 2026-10-12: Week 6 Step 3c: Linux CI Benchmarks, Drop Measurements, and PRNG Audit

- **Date**: 2026-10-12
- **Scope**: Final cleanup of Week 6. Updated benchmarks documentation using official Linux CI runner results (`docs/benchmarks/ci-linux-1M.txt` and `docs/benchmarks/ci-linux-10M.txt`), conducted PRNG audit across repository per Rule 11.1, instrumented slow insert tag tracking and table drop latency, and documented architectural open items.

### 1. Empirical Results from Linux CI (`ubuntu-24.04`)
- **Main Findings**:
  - `RobinHoodTable` insert throughput is **1.60x faster** at 1M keys (179.9 ns vs 286.9 ns) and **1.37x faster** at 10M keys (276.5 ns vs 379.4 ns).
  - Get hits throughput is **1.01x faster** at 1M keys (291.7 ns vs 293.4 ns) and **1.30x faster** at 10M keys (311.4 ns vs 404.4 ns).
  - Overwrite throughput is **1.08x faster** at 1M keys (157.0 ns vs 169.0 ns) and **1.47x faster** at 10M keys (185.5 ns vs 272.8 ns).
  - Worst-case latency spike at 10M keys is **50.2 ms** for `RobinHoodTable` vs **1,174.2 ms** (1.17 seconds) for `std::HashMap` (**23x lower pause**).
  - Get miss throughput is **5.6x slower** at 1M keys (129.2 ns vs 22.9 ns) and **1.5x slower** at 10M keys (150.6 ns vs 101.1 ns).
  - Mixed workload is 0.96x at 1M keys, but scales to **1.31x faster** at 10M keys (330.8 ns vs 433.4 ns).

### 2. Explanation of Slow Inserts & Drop Measurements
- **Lifecycle Event Attribution**:
  - Tracking resizing states before and after each insert revealed that all operations slower than 1 ms fall into two categories:
    1. `resize started`: Buffer allocation of $2\times$ new capacity (e.g. Op #7,340,032 at 50.2 ms on Linux, Op #917,504 at 37.8 ms on Windows).
    2. `migration finished, old table dropped`: When migration completes and `old_table` drains to 0 elements, `old_table` is deallocated. Dropping the old table buffer of $C$ slots produces the previously "unexplained" pause (e.g. Op #8,323,072 at 15.6 ms on Linux, Op #520,192 at 4.2 ms on Windows).
  - Empty table drop latency (`drop(RobinHoodTable)`) was measured directly:
    - $2^{20}$ slots (~42 MB): 1.09 ms (Linux) / 10.05 ms (Windows)
    - $2^{22}$ slots (~168 MB): 8.53 ms (Linux) / 33.01 ms (Windows)
    - $2^{24}$ slots (~671 MB): 32.79 ms (Linux) / 141.24 ms (Windows)
  - Dropping the multi-megabyte old table buffer upon migration completion accounts fully for the latency spike. Zero operations during steady-state incremental migration (`unchanged`) exceeded 1 ms.

### 3. Open Items for Future Optimizations
1. **Get-Miss Optimization (Compact Tag Array)**:
   - `std::collections::HashMap` (Swiss Table) evaluates a compact 1-byte control tag array using SIMD within a single 64-byte cache line fetch. `RobinHoodTable` inspects 40-byte `Slot` structs, requiring multiple DRAM cache line traversals.
   - *Candidate fix*: Introduce a separate compact 1-byte or 2-byte hash fingerprint array to evaluate lookups and terminate misses within a single cache line before accessing slot contents.
2. **Resize-Start Pause Mitigation**:
   - At 10M keys, allocating and zeroing the 16M-slot backing buffer (~671 MB) pauses execution for ~50 ms on Linux (~310 ms on Windows).
   - *Candidate fixes*:
     - Allocate the new table backing buffer lazily in chunks/segments upon first probe.
     - Shard the key space across multiple smaller tables (e.g. 16 or 64 independent tables), keeping per-table allocation pauses under 1 ms.
     - Offload the deallocation of `old_table` to a background worker thread so the mutating request that finishes migration does not block on `drop()`.
3. **Random Generator Audit**:
   - Audited all PRNGs in `src/store/table.rs`, `tests/property.rs`, `benches/store_bench.rs`, and consolidated shared implementation in `tests/common/mod.rs`.
   - All generators strictly enforce non-zero seed panics and sample bits/ranges using high-order bits (`>> 32`) on wide `u64` integers before casting, conforming to Rule 11.1 in `AGENTS.md`.

---

## 2026-10-12: Week 6 Step 3d: Fix Benchmark Drop Latency Data and Consolidate Background Migration

- **Date**: 2026-10-12
- **Scope**:
  1. Correction of erroneous "Linux CI Drop Latency" values in `docs/benchmarks.md` and `docs/design/hash-table.md`.
  2. Integration of fresh Linux CI v2 benchmark logs (`ci-linux-1M-v2.txt` and `ci-linux-10M-v2.txt`).
  3. Mathematical verification of the migration completion formula.
  4. Consolidation of background migration into a unified `background_migrate` helper in `src/store/table.rs`.

### 1. Root Cause of Wrong Drop Latency Values
In the previous documentation update (Step 3c), the values in the "Linux CI Drop Latency" column (1.09 ms, 8.53 ms, 32.79 ms) were erroneously copied from the table buffer *creation* (`RobinHoodTable::new`) latency table instead of the buffer deallocation (`drop(RobinHoodTable)`) latency table.

### 2. Correction and Real Numbers
The real numbers were extracted directly from the GitHub Linux CI runner logs (`ci-linux-1M-v2.txt` and `ci-linux-10M-v2.txt`):

**Creation and drop time, Linux CI (`ubuntu-24.04`)**:

| Table Capacity | Allocation Size | Creation Latency (1M run) | Drop Latency (1M run) | Creation Latency (10M run) | Drop Latency (10M run) |
| :--- | :---: | :---: | :---: | :---: | :---: |
| 2^20 (1,048,576 slots) | ~42 MB | 1.53 ms | 1.57 ms | 1.94 ms | 1.67 ms |
| 2^22 (4,194,304 slots) | ~168 MB | 12.34 ms | 6.82 ms | 13.10 ms | 7.32 ms |
| 2^24 (16,777,216 slots) | ~671 MB | 45.94 ms | 26.86 ms | 48.28 ms | 27.88 ms |

### 3. Slow Inserts Attribution and Migration Completion Formula
- **Tag Distribution for Inserts > 1 ms**:
  - 1,000,000 keys: 4 `resize started`, 1 `migration finished, old table dropped`, 0 `unchanged` (5 total).
  - 10,000,000 keys: 9 `resize started`, 5 `migration finished, old table dropped`, 0 `unchanged` (14 total).
  - 100% of pauses > 1 ms are accounted for by backing buffer allocation and deallocation; 0 pauses occurred during steady-state migration.
- **Migration Completion Formula**:
  $$\text{op\_end} = \text{op\_start} + \frac{C_{\text{old}} + N_{\text{old}}}{16}$$
  - 1M run: $458,752 + (524,288 + 458,752) / 16 = 520,192$, exactly matching measured Op #520,192 (1.19 ms).
  - 10M run (resize 1): $3,670,016 + (4,194,304 + 3,670,016) / 16 = 4,161,536$, exactly matching measured Op #4,161,536 (7.00 ms).
  - 10M run (resize 2): $7,340,032 + (8,388,608 + 7,340,032) / 16 = 8,323,072$, exactly matching measured Op #8,323,072 (13.88 ms).

### 4. Consolidated Background Migration in `src/store/table.rs`
- Implemented `background_migrate(&mut self)` private helper invoking `step_migration(MIGRATE_BATCH)` and handling errors centrally.
- Documented invariant: a failed background migration step is not fatal, both tables remain fully valid and consistent, and the next operation retries.
- Used `background_migrate` across `get_mut`, `remove`, and `insert_with_hash`.
- In `insert_with_hash`, a failed background migration step when `main_table` is at maximum load limit no longer aborts an overwrite of an existing key in `main_table`. Overwriting does not increase load, avoiding spurious `TableFull` errors.
- Added comprehensive unit test `test_overwrite_succeeds_when_main_table_at_load_limit_and_old_table_not_empty` asserting successful overwrite, full invariant retention (`check_invariants()`), and zero key loss.

---

## 2026-10-12: Week 7 Step 1: Command Layer Design and Dispatch Architecture

- **Date**: 2026-10-12
- **Scope**: Implementation of the decoupled command evaluation engine (`src/cmd/`) and the key-value store wrapper (`src/store/store.rs`) executing `ping`, `put`, `fetch`, `drop`, `has`, and `size`.

### 1. Specification Audit Summary
Prior to implementation, an audit of the frozen specifications was conducted:
1. `docs/spec/commands.md`:
   - Commands on the wire are transmitted inside standard `Value::Array` payload frames.
   - Command names must be strictly lowercase (`put`, `fetch`, etc.). Server uses exact byte matching; uppercase variants (`PUT`, `Ping`) are rejected with `2001` (`ERR_UNKNOWN_COMMAND`).
   - Defined dispatch flags: `READ`, `WRITE`, `NO_AUTH`, `ADMIN`.
   - Arity is strictly verified prior to handler dispatch, returning `2002` (`ERR_WRONG_ARG_COUNT`).
   - Multi-key `drop` returns an `Int` count of keys removed and must validate all keys atomically before dropping any key.
2. `docs/spec/errors.md`:
   - Cataloged exact error codes and message strings.
   - Key command errors: 2001 (unknown command), 2002 (wrong arg count), 2003 (invalid arg), 2004 (key too large), 2005 (value too large), 2006 (empty key), 2007 (too many args), 2008 (syntax error), 3001 (wrong type), 3003 (integer overflow), 3004 (not an integer), 4003 (out of memory / table full), 5001 (auth required), 6001 (readonly replica), 7001 (internal error).
   - In all these cases, `close` is `false`.
3. `docs/spec/values.md` & `protocol.md`:
   - Value types supported in Phase 1: `Nil` (`0x00`), `Int` (`0x01`), `Float` (`0x02`), `Bool` (`0x03`), `Text` (`0x04`), `Bytes` (`0x05`), `Array` (`0x06`), `Map` (`0x07`).
   - `Value::Error` (`0x08`) encodes code + message and is strictly a protocol reply object; it must never be stored as a keyspace value.
   - Wire limits: Max key size default 1,024 bytes (1 KB), max value size 32 MiB, max arguments 65,536.

### 2. Architecture Decisions & Implementation
1. **Store Abstraction (`src/store/store.rs`)**:
   - `Store` wraps `RobinHoodTable<Value>`, keeping the storage layer completely decoupled from protocol parsing and error codes.
   - Supports conditional insertion via `PutCondition` (`Always`, `OnlyIfAbsent` / `nx`, `OnlyIfPresent` / `xx`).
   - `fetch` returns a clone of the stored `Value`.
2. **Order of Checks Pipeline**:
   The engine enforces a strict 6-step evaluation pipeline:
   - Step 1: Parse request (`2008` for non-array/empty/non-string verb, `2007` for `args.len() > max_args`).
   - Step 2: Unknown command lookup (`2001`).
   - Step 3: Authentication verification (`5001` if `require_auth` active and command lacks `NO_AUTH`).
   - Step 4: Arity verification (`2002`).
   - Step 5: Read-only verification (`6001` if `read_only` active and command carries `WRITE`).
   - Step 6: Handler execution.
3. **No-Panic Guarantee**:
   All bad inputs are mapped to structured `CmdError` and returned as `Value::Error`. No handler panics on untrusted input.

### 3. Open Specification Questions & Observations
1. **Error Code for Malformed Command Frame (`2008` vs `1006`)**:
   - When a client sends a payload that is not a non-empty `Value::Array` or where the verb is not `Text` or `Bytes`, we chose to return `2008` (`ERR_SYNTAX_ERROR`) without closing the connection.
   - *Question for spec*: Should a malformed top-level command frame be treated as a syntax error (`2008`, keep connection open) or a fatal payload decoding error (`1006` `ERR_MALFORMED_PAYLOAD`, close connection)? We chose `2008` for graceful client recovery.
2. **Ambiguity of Storing `Nil` (`0x00`)**:
   - The specification states that `put` accepts "any typed item", which includes `Nil`.
   - Storing `Nil` creates an ambiguity: `has <key>` returns `true`, but `fetch <key>` returns `Nil`, which is identical to the reply for a non-existent key (cache miss).
   - *Question for spec*: Should storing `Nil` be disallowed, or should a missing key return a dedicated error / distinct representation in strict mode?
3. **Value Size Definition**:
   - The specification states that values cannot exceed 32 MB, but does not define how collection sizes are calculated.
   - *Adopted definition*: The size of a `Value` is computed recursively as the sum of payload bytes in its item tree (`Text` and `Bytes` byte lengths, 8 bytes for `Int` and `Float`, 1 byte for `Bool`, 0 bytes for `Nil`, and the sum of children for `Array` and `Map`).
4. **Typo in `docs/spec/values.md` Section 5.2**:
   - In Section 5.2 ("Wrong Type Errors"), the hex example for error code `3001` shows:
     `08 D9 17 38 6F ...`
   - In LEB128 uvarint encoding:
     `3001` (hex `0x0BB9`) encodes as `B9 17` (`0xB9 0x17`), NOT `D9 17`.
     The byte sequence `D9 17` decodes to decimal `3033`.
     Furthermore, the message length byte is `0x38` (56 bytes), but the frame payload length header shows `00 00 00 3C`.
5. **Memory Copy Overhead on `fetch`**:
   - `fetch` currently returns `self.get(key).cloned().unwrap_or(Value::Nil)`.
   - Cloning duplicates multi-megabyte payloads in heap memory. In future phases, this can be optimized with reference-counted shared byte buffers (`Arc<[u8]>` or `bytes::Bytes`) or zero-copy buffer slicing.

---

## 2026-10-12: Week 7, Step 2 - Atomic Bumping Semantics and Engine Field Encapsulation

### 1. Decision on `bump` Type Coercion & Error Handling

Section 3.8 of `docs/spec/commands.md` specifies that `bump` operates atomically on 64-bit signed integers and states:
> "If the key exists but holds a value that is not an integer, it returns a type error (3004 `ERR_NOT_AN_INTEGER` for non-integer strings/scalars, or 3001 `ERR_WRONG_TYPE` for non-scalar collections)."

We formalized the exact coercion and error mapping as follows:
1. **Existing `Text` and `Bytes`**:
   - Parsed with strict base-10 rules (`parse_strict_i64`):
     - Optional single leading `'-'` (no `'+'`).
     - ASCII digits only (`0-9`), no spaces.
     - No leading zeros except `"0"` itself (`"-0"`, `"01"`, `"-05"` are strictly rejected).
     - Must fit within `[i64::MIN, i64::MAX]`.
   - If parsing succeeds:
     - Value is stepped via `checked_add(step)`.
     - On overflow/underflow: returns `3003` (`ERR_INTEGER_OVERFLOW`) and stored value remains unchanged.
     - On success: stored value is updated in-place to `Value::Int(new_n)` (converting from Text/Bytes to Int) and returns `Value::Int(new_n)`.
   - If parsing fails: returns `3004` (`ERR_NOT_AN_INTEGER`) and stored value remains unchanged.
2. **Existing `Float`, `Bool`, `Nil`**:
   - Classified as non-integer scalars. Return `3004` (`ERR_NOT_AN_INTEGER`). The stored value is unchanged.
3. **Existing `Array` and `Map`**:
   - Classified as rich non-scalar collections. Return `3001` (`ERR_WRONG_TYPE`). The stored value is unchanged.
4. **Missing Key**:
   - Initialized at base `0` and steps by `step` (`0 + step`), storing `Value::Int(step)` in the keyspace. If the table is full, returns `4003` (`ERR_OUT_OF_MEMORY`).

### 2. Allocation & Lookup Optimizations
1. **`Store::put` Overwrites & `xx` Condition**:
   - `PutCondition::Always` checks `self.table.get_mut(key)` first. If the key exists, it mutates the value in-place, avoiding heap allocation of `Box::from(key)`.
   - `PutCondition::OnlyIfPresent` (`xx`) performs a single `self.table.get_mut(key)` lookup, eliminating the previous double lookup (`contains_key` followed by `insert`).
2. **`handle_drop` Fast Path**:
   - Single-key drops (`args.len() == 1`) validate the key and invoke `ctx.store.remove(key)` directly, completely bypassing `Vec` and `HashSet` allocations.
3. **`Engine` Encapsulation**:
   - Fields `store` and `config` are made private, with accessor methods `store()`, `store_mut()`, `config()`, and `config_mut()`. Existing integration tests have been transitioned to these accessors.
4. **Spec-Driven Error Constants Verification**:
   - Replaced hand-typed expected constant lists with direct compile-time inspection of `docs/spec/errors.md` via `include_str!`, dynamically validating codes, names, messages, and close flags.

---

## 2026-10-12: Week 7, Step 3 - Network & Command Layer Integration

- **Date**: 2026-10-12
- **Scope**: Connecting command dispatch (`src/cmd/`, `Engine`) to the networking layer (`src/net/`, `src/proto/`), introducing `Server` handler, tick hooks, `hello` and `quit` lifecycle commands, and CLI entry point in `src/main.rs`.

### 1. Architectural Decisions

1. **Commands Allowed Without Mandatory `hello`**:
   - *Investigation*: Audited `docs/spec/commands.md` and `docs/spec/errors.md`. The frozen specification describes `hello` as the initial greeting/negotiation command and provides `NO_AUTH` commands (`ping`, `hello`, `quit`), but does NOT mandate that a client must issue `hello` before executing any other command. There is no `ERR_HELLO_REQUIRED` error in the catalog.
   - *Decision*: Clients are permitted to execute commands without first sending `hello`. If authentication is configured on the engine (`require_auth = true`), commands requiring authentication will return `5001` (`ERR_AUTH_REQUIRED`) unless authenticated or flagged `NO_AUTH`. If authentication is not required, commands execute immediately upon connection.

2. **EventLoop `on_tick` and `wants_tick` Hooks**:
   - *Investigation*: Under the single-threaded epoll model, an idle server blocks in `epoll_wait` indefinitely (or until timer expiry). If a table resize migration is ongoing, background migration steps must proceed without waiting for new network I/O events, but an idle server with no ongoing migration must not spin the CPU.
   - *Decision*: Added default methods to `src/net::Handler`:
     ```rust
     fn wants_tick(&self) -> bool { false }
     fn on_tick(&mut self, _event_loop: &mut EventLoop) -> io::Result<()> { Ok(()) }
     ```
   - In `EventLoop::run_once_internal`, if `handler.wants_tick()` returns `true`, the epoll timeout is clamped to 1 ms (`Duration::from_millis(1)`). If `false`, the standard timer/infinite wait timeout is retained.
   - `on_tick` is called once per loop iteration after timer callbacks and I/O readiness dispatches.
   - Backward compatibility: The existing echo servers and tests implement `Handler` without modifications and continue to pass cleanly.

3. **Max Work Per Tick Limit**:
   - `Server::on_tick` checks `self.engine.store().table().is_resizing()`.
   - If resizing is active, it calls `self.engine.store_mut().step_migration(16)` exactly once per tick (moving 16 slots).
   - It never loops to completion in a single tick, ensuring low latency and fairness with network I/O.

4. **Connection Close & Error Framing Rules**:
   - Framing and decode errors in the 1000 range (`1001` to `1007`) are fatal: `FrameOutcome` sets `close = true`. For decodable headers, an error response frame is transmitted before closing the socket.
   - Semantic command errors (`2001` to `7001`) return structured `Value::Error` but leave the connection open (`close = false`).
   - The `quit` command replies with `Value::Text("ok")` and sets `close = true` for graceful shutdown.

5. **Pipelining & FIFO Guarantee**:
   - Multiple frames received in a single TCP read buffer are decoded sequentially.
   - Responses are produced and enqueued into `WriteQueue` in identical arrival order without reordering.

---

## 2026-10-12: Week 7, Step 4 - Review Hardening, Auth Rules, and TCP Verification

- **Date**: 2026-10-12
- **Scope**: Hardening server accept loop, consolidating hello responses and authentication semantics, error suppression during migration ticks, serialization failure fallbacks, and comprehensive end-to-end TCP integration testing.

### 1. Fixes from Review

1. **Non-blocking Server Listener**:
   - `Server::with_engine` now explicitly invokes `listener.set_nonblocking(true)?` and returns `io::Result<Self>`. This ensures the accept loop in `handle_accept` cannot block indefinitely when called upon epoll readiness notifications. `src/main.rs` exits with code 1 if initialization fails.
2. **Explicit Authentication Requirement & Postponed Backend**:
   - Both the `hello` command handler and the HELLO frame path now strictly enforce:
     - When `config.require_auth == false`: `session.authenticated = true`.
     - When `config.require_auth == true`: authentication backend comes in a later phase. The server keeps `session.authenticated = false` and returns `5001` (`ERR_AUTH_REQUIRED`) whether a token is missing or provided. Unauthenticated clients can continue executing `NO_AUTH` commands (`ping`, `hello`, `quit`).
3. **Unified Hello Response Builder**:
   - Created `build_hello_response(config: &CmdConfig, session: &Session) -> HelloResponse` used by both `handle_hello` and `handle_hello_frame`.
   - Software version is extracted directly from `env!("CARGO_PKG_VERSION")`.
   - Maximum frame size is sourced from `crate::proto::header::DEFAULT_MAX_PAYLOAD_LEN`.
4. **Tick Handling and Migration Error Suppression**:
   - `Server::on_tick` directly calls `self.step_migration_tick()`.
   - If `step_migration` returns an error (such as `TableFull` on tables configured with a strict capacity ceiling), the server records the error in `migration_errors`, outputs a single warning to `eprintln!`, and suppresses further ticks (`wants_tick() == false`) until the next successful insert or capacity change.
5. **Reply Serialization Fallback**:
   - If `encode_frame` fails when serializing an outgoing reply frame (for example, if a payload exceeds maximum wire dimensions or collection nesting exceeds 8 levels), the server catches the failure and transmits a fallback error frame containing `ERR_INTERNAL_ERROR` (`7001`) with the client's original `request_id`.
6. **Host Architecture Verification**:
   - `src/main.rs` checks for `target_os = "linux"`. On non-Linux platforms (e.g. Windows), it prints `"cachedbd needs Linux (epoll)"` and exits with code 1 without printing a misleading "listening" message.

### 2. Architecture Items Deferred to Later Phases

1. **Unbounded Write Queue (Slow Reader Backpressure)**:
   - Outgoing frame chunks currently append directly to each connection's `WriteQueue`. If a client stops reading from the TCP socket, the write queue can grow without bound. A future phase should introduce write queue memory ceilings and disconnect or suspend readers when limits are breached.
2. **Per-Event Read Fairness Cap**:
   - `handle_client_read` currently drains all available frames from the connection's read buffer in a loop. A malicious or high-throughput client sending continuous pipelined traffic could starve other active sockets. A maximum frames-per-read budget should be scheduled in future phases.
3. **Graceful Shutdown & OS Signal Handling**:
   - SIGINT/SIGTERM handlers and graceful drain sequences across all active client sockets are deferred to Phase 2 operations.
4. **Connection Idle Timeout**:
   - Connections currently remain open indefinitely until explicit EOF or fatal protocol violation. Heartbeat pings and configurable idle socket timeouts are scheduled for future connection lifecycle milestones.

---

## 2026-10-12: Test 16 Failure Investigation and Event Loop Architecture

- **Date**: 2026-10-12
- **Scope**: Root cause analysis of `test_16_two_clients_isolation` CI failure, event loop runtime decision, and connection idle timeout tracking.

### 1. Root Cause Analysis: `test_16_two_clients_isolation` Failure (Cause A)

- **Diagnosis**: In `test_16_two_clients_isolation`, the test client `c_bad` transmitted 8 arbitrary bytes (`[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33]`).
- **Decoder Contract**: In `src/proto/decode.rs`, `decode_frame_with_limits` checks `if buf.len() < HEADER_LEN { return Ok(Decoded::NeedMore); }`. Since `HEADER_LEN` is 14 bytes, the streaming decoder correctly treats 8 bytes as an incomplete header and waits for remaining bytes. It does not inspect magic or report framing errors until at least 14 bytes arrive.
- **Cause**: Cause (a) — Garbage is shorter than 14 bytes: the server correctly waits for more bytes. The TEST was wrong in expecting immediate socket disconnection on an incomplete 8-byte header fragment without an idle timeout.
- **Resolution**:
  - Updated Part 1 of the test to send >= 14 bytes of invalid header data (`[0xDE, 0xAD, ...]`), verifying immediate EOF disconnection upon parsing `ERR_INVALID_MAGIC` (`1004`).
  - Added Part 2 sending fewer than 14 bytes (8 bytes), asserting that the connection remains open (no EOF within 300 ms) and that concurrent clients continue operating smoothly.

### 2. Decision: No Tokio — Single-Threaded Epoll Event Loop

- **Decision**: No tokio. Single-threaded epoll event loop.
- **Reasons**:
  - Atomic commands without locks: Single-threaded execution guarantees command serializability without mutex overhead or locking bugs.
  - Incremental resize needs one thread: Paced table migration (`RobinHoodTable::step_migration`) mutates internal slot clusters safely without inter-thread data races.
  - Learning goal: Complete zero-dependency mastery of Linux kernel event polling primitives.
  - No new crates: Keeps dependency graph minimal, auditable, and fast to compile.
- **Future speed plan**: Sharding with one loop per core (shared-nothing architecture), or `io_uring` for next-generation kernel asynchronous I/O.

### 3. Idle Timeout for Half-Sent Frames

- "Idle timeout for half-sent frames is deferred." Sockets that send partial frame fragments currently remain open indefinitely until explicit EOF or new data arriving completes the frame header.

---

## 2026-10-12: Week 8 Step 1 — Synchronous Client Library Implementation

- **Date**: 2026-10-12
- **Scope**: Zero-dependency synchronous client library (`cachedb::client::Client`), fault injection testing, and pipelined throughput validation.

### 1. Synchronous Client Design (`cachedb::client`)

- **Transport**: Standard blocking `std::net::TcpStream` with configurable `connect_timeout`, `read_timeout`, and `write_timeout` via `ClientConfig`.
- **Handshake & Metadata**: Automated `HELLO` protocol negotiation (version 1) storing advertised server limits in `ServerInfo`.
- **Execution & Pipelining**:
  - `Client::command`: Dispatches single array commands with wrapping `u32` `request_id` counter, validating `FLAG_RESPONSE`.
  - `Client::pipeline`: Batches multiple commands in a single socket write (`write_all`), matching and verifying responses in strict FIFO order.
- **Typed Helpers**: Ergonomic abstractions for `ping`, `put`, `put_nx`, `put_xx`, `fetch`, `drop_keys`, `has`, `size`, `bump`, and `quit`.

### 2. Design Decisions Not Explicit in Spec

1. **Stored Nil Distinction**: Calling `fetch` on a key holding `Value::Nil` returns `Ok(None)` identical to a cache miss. Clients needing strict presence verification call `has(key)`.
2. **Empty Drop Optimization**: `drop_keys` on an empty slice returns `Ok(0)` immediately without a network round-trip.
3. **Fatal Error State Machine**: Fatal transport errors (`Io`, `Protocol`, `ConnectionClosed`, `ReplyTooLarge`) transition the client into `is_broken = true`. Subsequent calls return `ConnectionClosed` immediately. Non-fatal application errors (`ClientError::Server`) keep the connection healthy.
4. **Cross-Platform Disconnect Mapping**: Windows socket aborts (`WSAECONNABORTED`, 10053) and peer resets (`ConnectionReset`, `BrokenPipe`, `UnexpectedEof`) are normalized to `ClientError::ConnectionClosed`.
5. **Deferred Features**: Automatic reconnect, connection pooling, TLS encryption, and async runtimes are intentionally omitted in v0.1.

---

## 2026-10-12: Week 8 Step 2 — Client Hardening & CLI Implementation

- **Date**: 2026-10-12
- **Scope**: Pipeline chunking deadlock prevention, cursor-based read buffering, `cachedb-cli` utility implementation, interactive REPL, and E2E testing.

### 1. Client Hardening (`src/client.rs`)

- **Pipeline Chunking**:
  - Bound batched writes to `PIPELINE_CHUNK_COMMANDS = 256` and `PIPELINE_CHUNK_BYTES = 262,144` (256 KB). Commands exceeding 256 KB form isolated chunks.
  - Mitigates TCP socket buffer deadlocks where a server blocks on writes before draining client input buffers.
  - Zero value cloning: commands are consumed into output buffers directly.
- **Unified Reply Validation**:
  - Factored out `validate_reply` verifying `FLAG_RESPONSE`, `expected_id`, `Opcode::Command`, and mapping `FLAG_ERROR` / `Value::Error`. Applied to both `command()` and `pipeline()`.
- **Cursor Read Buffer**:
  - Replaced per-frame `drain(..used)` with `read_cursor: usize` and 64 KB block socket reads.
  - Buffer is compacted (shifting unconsumed bytes to the front) only when `read_cursor > buf.len() / 2` or empty, reducing buffer shift overhead to $O(1)$ amortized.

### 2. Command-Line Interface (`cachedb-cli`)

- **Core Engine (`src/cli.rs`)**:
  - `parse_line`: Whitespace splitting respecting double quotes and escapes (`\"`, `\\`, `\n`, `\t`, `\xHH`).
  - `build_command`: Forces command verb and known key parameters (`put`, `fetch`, `drop`, `has`, `bump` at pos 1) to `Value::Text`. Parses strict integers matching `bump` grammar, booleans, nil, floats, and `hex:...` byte literals.
  - `format_value`: Redis-cli style formatting (`(nil)`, `(integer)`, `(float)`, `(bool)`, quoted text with escapes, UTF-8/hex bytes, indented nested arrays with 3-space alignment, maps, `(error CODE) MSG`, and >1 MB safety truncation).
  - `run`: Handles one-shot commands (exiting 0 on success, 1 on server errors, 2 on usage/network failure) and interactive REPL mode.
- **Interactive REPL**:
  - Automatically identifies terminal sessions (`std::io::IsTerminal`), emitting banner and `cachedb> ` prompt.
  - Builtins: `help`, `exit`, `quit` (transmitting `quit` to server before graceful termination).
  - Resilient session loop: Syntax errors and server errors (`ClientError::Server`) log to `stderr` and continue; socket drops terminate with exit code 2.

### 3. Edge Cases & Discoveries

1. **Key Type Preservation**: Commands like `put 123 456` or `bump 100 5` must treat the key as `Value::Text("123")` rather than `Value::Int(123)` to align with server storage and dictionary lookup rules.
2. **Server Verb Return Values**: Server handlers return lowercase string values (e.g. `ping` returns `"pong"` rather than `"PONG"`, `quit` returns `"ok"`).
3. **Deadlock in Pipeline Batches**: Without chunking, large pipeline bursts easily exceed OS socket send/receive buffer sizes (typically 64KB–256KB), stalling both client and server when socket writes block synchronously. Chunking guarantees safe forward progress.

---

## 2026-10-12: Week 8 Step 3 — Server Hardening, Backpressure, Fairness & Client Refinements

1. **CLI Key Parsing**: Updated `cli::build_command` to treat all arguments after `drop` as `Value::Text` keys, matching multi-key semantics.
2. **Client Buffer Reuse**: Replaced 64 KB per-frame allocations in `Client::read_frame` with reusable heap-allocated scratch buffer `read_scratch`.
3. **Pipeline Chunking Optimization**: Cached oversized serialized frames via `pending_encoded` across chunk boundaries, avoiding duplicate encoding.
4. **Level-Triggered Epoll**: Verified `src/net/` operates in level-triggered mode, necessitating explicit `epoll_ctl(EPOLL_CTL_MOD)` to drop `READABLE`.
5. **Config & Watermarks**: Added `ServerConfig` with 4 MiB high and 1 MiB low write watermarks, 256 frames / 256 KiB fairness limits, and 300 s idle timeout.
6. **Backpressure**: Pauses client reading and drops `READABLE` interest once write queue exceeds 4 MiB, exceeding by at most one reply frame.
7. **Fairness**: Enforced per-event frame/read limits with `pending_work` queue and `wants_tick()`, preventing large pipelines from starving pings.
8. **Idle Timeout**: Monitored per-connection `last_active`, sweeping inactive connections via 1-second event loop timer and closing them cleanly.
9. **Telemetry**: Introduced `ServerStats` with relaxed atomic counters for connections, backpressure pauses, peak queue sizes, and processed frames.
10. **Testing**: Implemented Linux hardening E2E suite covering slow readers, isolation, fairness, idle sweeps, and abrupt disconnects.

---

## 2026-10-12: Week 8 Step 4 — Linux Hardening Post-Review, Half-Close Fix & Big-Reply Backpressure

1. **Root Cause Analysis (Tests Never Ran on Windows)**: Tests marked `#[cfg(target_os = "linux")]` were skipped on local Windows builds; real execution in Linux GitHub Actions CI revealed 3 backpressure test failures and half-close edge cases.
2. **Backpressure Buffer Sizing (Ping Replies Too Small)**: Ping responses (~20 B) are completely absorbed by modern OS kernel socket buffers (128 KiB–2 MiB), preventing server write queues from ever exceeding 64 KiB backpressure watermarks.
3. **Big-Reply Testing**: Fixed tests by putting 256 KiB byte patterns (`put bigkey <pattern>`) followed by pipelined `fetch bigkey` requests (~75 MB total), reliably filling kernel socket buffers and deterministically triggering user-space backpressure.
4. **Half-Close Premature Disconnect Bug**: In `handle_client_read`, observing EOF on socket reads with an empty write queue previously removed connections immediately, losing completed frames still buffered in `ReadBuffer`.
5. **Connection Half-Close State (`read_eof`)**: Added `read_eof: bool` to `Connection`. When a client half-closes via `shutdown(Write)`, `read_eof` is set and `desired_flags()` immediately drops `READABLE` to prevent level-triggered epoll busy-spinning.
6. **Triple-Condition Removal**: Connections are now removed only when (a) EOF was seen, (b) no complete frame remains in `ReadBuffer` (`!has_complete_frame`), and (c) `WriteQueue` has been fully flushed (`write_queue.is_empty()`).
7. **O(1) Work Scheduling**: Replaced $O(n)$ `pending_work.contains(&token)` lookups with an $O(1)$ `pending_work_set: HashSet<Token>` tracking queue membership.
8. **Telemetry Extension**: Added `ServerStats.connections_closed` (`AtomicU64`) tracking all terminated connections, verified within 2s on disconnect.
9. **Thread Safety & Clean Teardown**: Updated all test writer threads to set write timeouts and explicitly join at test completion.
10. **Half-Close E2E Test Suite**: Added 4 Linux tests covering pipelined pings with half-close, big fetches with backpressure and half-close, partial frame drops, and single-shot request half-closes.

---

## 2026-10-12: Week 8 Step 3c — Test Handshake Payload Fix & Strict Reply Verification

Week 8 Step 3c: tests used a wrong HELLO payload; server was right; fixed helpers; all raw tests now verify the reply flags.

1. **Root Cause Confirmation**: E2E test failures on Linux were caused by tests crafting malformed HELLO frames with payload `["hello", 1, client_name]` instead of `[1, client_name]`. The server correctly responded with `ERR_MALFORMED_PAYLOAD` (code 1006) and closed the connection.
2. **Robust Test Helpers**: Implemented `raw_handshake`, `read_one_frame`, and `expect_ok_reply` in `tests/common/mod.rs` to validate all reply headers, flags (`FLAG_RESPONSE`, not `FLAG_ERROR`), and opcodes across test suites.
3. **Pipelining & Accumulation**: Preserved unconsumed bytes in a shared accumulator across both handshake and command reply drains, eliminating race conditions and lost frames.
4. **Regression Testing**: Added `test_malformed_hello_payload_rejected_with_1006_and_closed` ensuring invalid HELLO payloads elicit code 1006 followed by clean server-initiated closure, leaving the server responsive for subsequent clients.

---

## 2026-10-12: Week 8 Step 4 — Benchmark Load Generator (`cachedb-bench`)

- **Scope**: Multi-threaded load generator implementation (`src/bench.rs` and `src/bin/cachedb-bench.rs`), workload distributions, fixed-size latency histogram, Linux E2E test, and CI workflow.

### Architecture Decisions & Trade-Offs

1. **Decision: Per-Batch Latency Measurement**:
   - Pipelined protocol execution sends multiple commands in a batch and reads all replies together to amortize system call overhead and TCP packet framing.
   - Recording latency per batch (`Instant::now()` before write to after reading full batch replies) reflects real-world pipelined client round-trip times. Per-command latency within a single pipeline batch cannot be measured non-intrusively from the client perspective without serializing round-trips.
   - Both the CLI output and documentation explicitly label latency metrics as batch round-trip latency.

2. **Decision: Distinct Counter Key Space for Mixed Workload**:
   - In `mixed` workloads (50% fetch, 40% put, 5% bump, 5% drop), `put` commands write `Value::Bytes` or `Value::Text` payloads.
   - If a subsequent `bump` command targeted the same key holding string/byte data, the server handler would return error 3004 (`ERR_NOT_AN_INTEGER`).
   - Rather than masking or ignoring 3004 errors, we isolate counters into `"cnt:<idx>"` while general key-value operations use `"key:<idx>"`. This guarantees zero protocol errors during continuous execution while maintaining realistic multi-operation distributions.

3. **Decision: GitHub Actions Benchmark via `workflow_dispatch` Only**:
   - The multi-workload benchmark suite takes >60 seconds of sustained CPU execution per run across 5 workloads.
   - Running full multi-connection benchmarks on every push or PR would create severe CI queue delays and waste runner minutes.
   - Triggering via manual `workflow_dispatch` enables on-demand profiling and performance regression testing while keeping standard PR CI fast and responsive.

---

## 2026-10-12: Week 8 Step 4c — CI Benchmark Stall Diagnosis & Server TCP_NODELAY Fix

### 1. First CI Benchmark Results
The first run of the network benchmark suite on GitHub Actions CI revealed a severe performance anomaly between unpipelined and pipelined workloads:
- **Read (pipeline 1)**: **58,150 ops/s**, batch latency p50 **0.054 ms**.
- **Read (pipeline 16)**: **1,560 ops/s**, batch latency p50 **41 ms** per batch (consistent ~41 ms on every batch).

### 2. Root Cause Analysis
The ~41 ms latency per batch is the classic interaction between **Nagle's algorithm** and the TCP **delayed-ACK timer** (~40 ms on Linux):
1. The client library already sets `TCP_NODELAY` on its socket (`src/client.rs` line 295).
2. However, the server never called `set_nodelay(true)` on accepted client sockets (`src/server.rs` had no `set_nodelay` calls).
3. When processing a pipelined batch, the server writes the first reply frame into the socket. The kernel TCP stack transmits this small segment immediately.
4. For the remaining 15 reply frames, because unacknowledged data is already in flight and each remaining segment is smaller than the maximum segment size (MSS), Nagle's algorithm holds the packets in the server's kernel socket buffer.
5. Meanwhile, the Linux client TCP stack holds its acknowledgment, waiting for the delayed-ACK timer (~40 ms) to expire before sending the ACK.
6. Only after the 40 ms ACK arrives does the server kernel release the next buffered segment, causing every pipelined batch to stall for ~41 ms.

### 3. The Fix
- In `Server::handle_accept` (`src/server.rs`), immediately after accepting a client connection, call `stream.set_nodelay(true)`.
- If `set_nodelay(true)` fails, the server records the event in `ServerStats.nodelay_failures` (`AtomicU64`, `Ordering::Relaxed`) and continues servicing the connection without crashing.
- Added a dedicated Linux end-to-end regression test (`test_pipelined_pings_no_nagle_delayed_ack_stall`) in `tests/hardening_e2e.rs` running 200 batches of 16 pipelined `ping` commands with a strict <3s time bound.
- A second CI benchmark run on GitHub Actions is required to obtain real, unstalled pipeline-16 benchmark numbers.

### 4. Code Inspection: Reply Write Mechanics
- In `src/server.rs`, `handle_client_read` processes all pipelined frames received in the read buffer, enqueuing serialized reply frames into `WriteQueue` via `conn.send_chunk(out)`.
- The server performs one flush per event-loop turn by calling `self.handle_client_write(token, event_loop)` once after processing the batch.
- However, inside `WriteQueue::write_to` (`src/net/buffer.rs`), a `while let Some(front) = self.chunks.front()` loop issues a separate `writer.write()` syscall for every individual frame chunk in the queue rather than using vectored I/O (`writev`) or a coalescing write buffer. While `TCP_NODELAY` prevents Nagle stalls, issuing $N$ individual syscalls per batch incurs avoidable context-switch overhead that could be streamlined in a future optimization phase.
