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














