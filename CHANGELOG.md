# Changelog

All notable changes to `cachedb` will be documented in this file.

## v0.1.0 - 2026-10-12

### Added
- **Wire Protocol**: Custom binary framing with 14-byte headers (`magic: 0x43444231`, `request_id`, `opcode`, `payload_len`), streaming frame decoder, LEB128 varint encoding, AST value model (`Nil`, `Bool`, `Int`, `Float`, `Text`, `Bytes`, `Array`, `Map`, `Error`), and handshake negotiation via `HELLO` frames.
- **Storage Engine**: In-memory Robin Hood hash table with backward-shift deletion, linear probing with PSL (probe sequence length) tracking, and incremental resize migration spreading rehash work across subsequent operations.
- **Command Engine**: Layer supporting commands:
  - `ping [msg]`: Heartbeat and connection testing.
  - `put <key> <val> [nx|xx]`: Key-value storage with conditional insert semantics.
  - `fetch <key>`: Key lookup returning stored values.
  - `drop <key...>`: Multi-key deletion returning the count of removed keys.
  - `has <key>`: Key existence check returning boolean.
  - `size`: Key space cardinality count.
  - `bump <key> [amt]`: Atomic integer increment/decrement.
  - `hello [version name [token]]`: Protocol negotiation and client handshake.
  - `quit`: Clean connection termination.
- **Structured Errors**: Categorized error codes across classes (protocol `1xxx`, command `2xxx`, type/value `3xxx`, storage `4xxx`, server `5xxx`) with human-readable error messages.
- **Server Architecture**: Single-threaded event-driven server (`cachedbd`) on Linux `epoll` with:
  - Backpressure flow control using dual watermarks (`PAUSE_WRITE_WATERMARK = 64 KiB`, `RESUME_WRITE_WATERMARK = 16 KiB`).
  - Fairness limits per turn (`MAX_FRAMES_PER_READ = 64`, `MAX_CHUNK_BYTES_PER_TURN = 256 KiB`).
  - Idle timeout tracking via wheel timer / timer lists.
  - Clean half-close socket handling (draining pending replies before closing).
  - Socket tuning with `TCP_NODELAY` to prevent Nagle and delayed-ACK deadlocks.
  - Reply coalescing (`COALESCE_LIMIT = 64 KiB`) in `WriteQueue` merging small pipelined reply buffers into single write syscalls.
- **Client Library**: Synchronous client API (`Client`) in `src/client.rs` with automatic `HELLO` negotiation, strongly-typed command methods, pipelined batch execution (`pipeline`), and zero third-party dependencies.
- **CLI Shell**: `cachedb-cli` utility supporting both one-shot commands and interactive REPL mode with syntax highlighting/formatting, escaping, and structured exit codes.
- **Load Generator & Benchmarking**: `cachedb-bench` tool with configurable synthetic workloads (`read`, `write`, `mixed`, `bump`), concurrent connection pools, configurable pipeline depth, latency percentile tracking, and JSON output formatting.
- **Testing & Fuzzing**: Comprehensive test suites including `cargo-fuzz` targets (`fuzz_frame_decode`, `fuzz_item_decode`), CI workflows with AddressSanitizer (`ASan`), and soak tests validating up to 10,000 concurrent idle connections.

### Performance
- **Network Throughput**: Sustained ~487k ops/s for pipelined read workloads and ~513k ops/s for atomic counters in 4-vCPU Linux CI runners with batch round-trip p50 latencies around 0.1 ms.
- **In-Memory Store**: Robin Hood hash table achieves ~4.9 Mops/s insert throughput and ~4.0 Mops/s lookup throughput at 1,000,000 keys, with worst-case resize pauses bounded to 47.3 ms at 10,000,000 keys (compared to 1,055 ms for `std::HashMap`).

### Known Limits
- **No Persistence**: Purely in-memory; no disk WAL (write-ahead log) or snapshotting.
- **No TLS**: Network transport is unencrypted plain-text TCP.
- **No Authentication Backend**: `HELLO` frame accepts token field, but server does not validate against a credential store.
- **No Maximum Memory Limit**: Memory growth is unbounded; eviction policies are not enforced.
- **No Graceful Shutdown / Signal Handling**: Daemon terminates immediately on process termination signals.
- **No Per-IP Rate Limits**: Connection counts are global without per-IP rate limiting or connection throttles.
- **Single-Threaded Server**: Event loop and storage core run on a single thread; cannot utilize multiple CPU cores for request execution.
- **Linear Command Lookup**: Dispatch engine uses linear scan over registered commands.
- **Value Cloning on Fetch**: `fetch` clones values from storage during reply frame construction.
- **Project Naming**: Package and binary names (`cachedb`/`cachedbd`) are subject to change prior to public registry publication.
