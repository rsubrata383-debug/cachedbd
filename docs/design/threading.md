# SkiffDB Threading and Concurrency Model

This document defines the threading model, component boundaries, testability abstractions, and platform strategy for SkiffDB.

---

## 1. Core Rule: One Thread Owns the Data

SkiffDB follows a strict single-threaded core architecture:
- **One thread owns the entire in-memory keyspace.**
- **No shared mutexes or locks on stored data**: Commands execute sequentially on the main thread.
- **Atomicity by construction**: Every command is atomic because no other thread can read or modify the keyspace while a command is running.
- **No race conditions or deadlocks**: Eliminates lock contention, priority inversions, and complex concurrent debugging.

---

## 2. Main Thread Responsibilities

The main server thread runs a continuous non-blocking event loop:

1. **Network I/O**: Polls non-blocking sockets for readable and writable events using Linux `epoll`.
2. **Protocol Framing**: Streams bytes into connection buffers and parses binary request frames.
3. **Command Execution**: Dispatches and executes commands sequentially against the hash table.
4. **Time & TTL Expiry**: Evaluates lazy expiry on key lookup, and runs active TTL sweeps on timer ticks.
5. **Memory Accounting & Eviction**: Tracks allocated memory and executes eviction policies (LRU, LFU, random) when limits are approached.
6. **Response Serialization**: Writes response frames into client output queues and flushes to sockets.

---

## 3. Helper Threads and Communication

To ensure the main thread never stalls on heavy work, specific tasks may be offloaded to background helper threads:

### 3.1 Allowed Helper Threads (Future Phases)
- **Snapshot Worker (Phase 4, Weeks 30-32)**: Serializes point-in-time dataset snapshots to disk (`.cdbs`) without blocking client requests.
- **Journal Flush Worker (Phase 4, Weeks 27-28)**: Executes background `fsync` operations to ensure durability without stalling memory operations.
- **Async Value Deletion (Phase 2, Week 15)**: Frees large value allocations (e.g. 100 MB objects or massive collections) on a background thread so memory reclamation does not cause latency spikes.

### 3.2 Thread Communication Rule
- **Channels only**: Helper threads communicate with the main thread strictly through bounded message-passing channels (e.g. MPSC / SPSC channels).
- **Never shared locks**: Helper threads **never** acquire locks (`Mutex` or `RwLock`) on keyspace hash tables. Data ownership is transferred via message passing.

---

## 4. Path to Multi-Core Scaling (Phase 6, Option B)

The single-threaded core is designed to scale to multi-core machines without rewriting the database engine:
- In Phase 6 (Option B), the keyspace will be partitioned into $N$ shards (e.g. 16,384 hash slots).
- Each CPU core runs an independent instance of this exact single-threaded engine, owning its slice of memory and running its own event loop.
- A lightweight router forwards client commands to the worker thread that owns the target key slot.
- Because the v0.1 engine assumes zero shared state, running $N$ partitioned instances requires no locks inside the core table.

---

## 5. Testability Interfaces (Traits)

Per our guiding principles (*"Everything is testable"*), hardware and OS dependencies are placed behind Rust interfaces (traits) so unit and property tests can run without real network sockets or clocks:

### 5.1 Clock Trait (`Clock`)
- Provides current monotonic time (`now()`).
- Production uses the real OS monotonic clock.
- Tests use a `FakeClock` that can be advanced manually by milliseconds or seconds. This makes TTL expiry tests fast, reliable, and completely deterministic (no `thread::sleep`).

### 5.2 Network Stream Trait (`AsyncStream`)
- Abstracts non-blocking socket reads and writes.
- Production uses raw non-blocking TCP socket file descriptors.
- Tests use in-memory mock buffers (`MockStream`) to test partial reads, split frames, and pipelining without opening real TCP ports.

### 5.3 Storage Disk Trait (`Disk`)
- Abstracts journal appending and snapshot writing.
- Production writes to real files on disk.
- Tests use a `MemoryDisk` or `FaultyDisk` that can simulate power loss, torn writes, and disk-full errors deterministically.

---

## 6. Keeping the Event Loop Fast (Latency Protection)

Because one thread runs all commands sequentially, a slow command blocks the entire server:
- If a client runs a huge `drop` command that deallocates millions of keys or a massive data structure, the main thread halts event processing and all other connected clients experience latency spikes.
- **What we do now**: Enforce strict size limits (keys $\le 1$ KB, default values $\le 32$ MB).
- **What we will do later (Phase 2, Week 15)**: Implement **lazy free**. When dropping large keys, the main thread unlinks the pointer from the keyspace in $O(1)$ time and sends the allocation across a channel to a background helper thread to reclaim memory without stalling the event loop.
- **Incremental Resizing**: Hash table bucket expansion is spread incrementally across individual operations, avoiding multi-millisecond stop-the-world pauses.
- **Slow Log**: Any command taking longer than a configured threshold (e.g. 10 ms) is recorded in the slow command log for operator inspection.

---

## 7. Target Operating System Strategy

- **Primary Operating System**: Linux (or WSL2 on Windows) is the first-class deployment and benchmarking target. The real networking event loop directly targets the Linux kernel `epoll` API.
- **Testing on Other Systems**: Because networking, clocks, and disk I/O are hidden behind Rust traits (`AsyncStream`, `Clock`, `Disk`), unit tests, protocol tests, and command engine tests compile and pass deterministically on any platform (Windows native, macOS, Linux, CI). Native Windows and macOS developers can run the entire test suite without Linux.
- **Future Native Ports**: Clean trait abstractions mean native Windows (IOCP) or macOS (`kqueue`) event loop backends can be added in later phases if needed without modifying the core engine.

---

## 8. Final Architectural Decisions

The following architectural choices govern SkiffDB implementation:

### Decision 1: Handling `unsafe` and Linux `epoll` Syscalls
- **Choice**: Encapsulated `libc` FFI inside a dedicated `src/net/sys.rs` module.
- **Rules**:
  1. All `unsafe` syscall invocations (`epoll_create1`, `epoll_ctl`, `epoll_wait`) are strictly isolated inside `src/net/sys.rs`.
  2. `src/net/sys.rs` presents a 100% safe Rust abstraction to `src/net/event_loop.rs` and the rest of the engine.
  3. `src/net/event_loop.rs` contains zero `unsafe` code.
  4. Use the `libc` crate only on Linux targets (`#[cfg(target_os = "linux")]`).
  5. **No changes now**: Keep `unsafe_code = "forbid"` in `Cargo.toml` during Phase 0. Do not add `libc` yet.
  6. **Phase 1 Week 3 TODO**: Update `Cargo.toml` lint from `forbid` to `deny`, and permit `unsafe` solely within `src/net/sys.rs`.
  7. Every `unsafe` block in `src/net/sys.rs` must have a clear `// SAFETY:` comment documenting why it is sound.

### Decision 2: Threading Model for v0.1
- **Choice**: Pure single thread for everything in v0.1.
- **Rules**:
  1. The main thread runs the event loop, socket polling, frame decoding, command dispatch, keyspace storage, and TTL checks.
  2. No helper threads or background pools are spawned in v0.1.
  3. Background threads will be introduced only when required in future phases:
     - Lazy freeing of oversized keys in Phase 2, Week 15.
     - Background snapshot worker and journal `fsync` in Phase 4 (Weeks 27-32).
  4. Helper threads will communicate strictly via bounded message-passing channels, never with shared mutexes.

### Decision 3: Event Loop Implementation
- **Choice**: Custom hand-written event loop on Linux `epoll`. No `tokio`.
- **Rules**:
  1. The networking engine is built directly on non-blocking TCP sockets and Linux `epoll` notifications.
  2. Provides complete control over memory allocations, buffer growth, and scheduling latency.
  3. Avoids opaque thread pool overhead and preserves the project's educational and scratch-built principles.
