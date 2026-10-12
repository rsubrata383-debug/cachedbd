# Server Architecture & Request Flow (Week 7, Step 4)

## Plan (Max 10 Lines)
1. Register `hello` and `quit` in `REGISTRY` (`NO_AUTH` flag; `quit` sets `Outcome.close = true`).
2. Implement pure `handle_request(engine, session, frame) -> FrameOutcome` without socket I/O.
3. Configure non-blocking listener in `Server::with_engine` to prevent blocking the accept loop.
4. Support opcode 1 (HELLO) and opcode 2 (COMMAND); reject invalid opcodes with fatal errors.
5. Standardize hello responses via unified `build_hello_response` and enforce strict auth rules.
6. Enforce connection closure on fatal errors (1001-1007) and when `Outcome.close` is true.
7. Support pipelining: process frames sequentially in FIFO arrival order without reordering.
8. Fallback to `ERR_INTERNAL_ERROR` (7001) if reply frame serialization fails.
9. Drive incremental migration via `step_migration_tick`, suppressing ticks on table errors.
10. Exit with informative notice on non-Linux hosts ("cachedbd needs Linux (epoll)").

---

## 1. Flow of One Frame

1. **Reception & Ingestion**:
   - The OS epoll readiness event triggers `read_incoming()` on the client's `Connection`.
   - Inbound bytes append to the connection's streaming `ReadBuffer`.

2. **Frame Framing & Decoding**:
   - `decode_frame(buf)` parses the 14-byte binary header (`0xCA 0xDB`, version 1, flags, request_id, opcode, payload_len).
   - If `Decoded::NeedMore`, the loop pauses until more bytes arrive without consuming or reallocating.
   - If a framing error occurs (invalid magic, reserved flag, frame too large), a fatal protocol error is triggered.
   - On `Decoded::Complete((frame, used))`, exactly `used` bytes are consumed from the buffer.

3. **Request Dispatching (`handle_request`)**:
   - **Opcode `0x0001` (HELLO)**:
     - Validates payload format (`[version, client_name, auth_token?]`).
     - Negotiates protocol version (must be 1 in v0.1).
     - If `require_auth` is enabled, returns `5001` (`ERR_AUTH_REQUIRED`) without authenticating session.
     - If `require_auth` is disabled, marks `session.authenticated = true`.
     - Returns `HelloResponse` map built via `build_hello_response`.
   - **Opcode `0x0002` (COMMAND)**:
     - Validates that payload is an `Array`.
     - Invokes `Engine::execute(session, payload)`.
     - Constructs response frame with opcode `0x0002`, `request_id = frame.header.request_id`.
     - Flags set to `0x01` (`FLAG_RESPONSE`), plus `0x02` (`FLAG_ERROR`) if reply is `Value::Error`.
   - **Unknown Opcode**:
     - Returns error `1001` or `2001` with `close = true`.

4. **Pipelining & Encoding Fallback**:
   - If multiple frames are present in `ReadBuffer`, they are decoded and processed in strict arrival order.
   - Responses are serialized via `encode_frame`. If serialization fails (e.g. frame exceeded maximum size limit), the server falls back to transmitting `ERR_INTERNAL_ERROR` (`7001`) with the client's original `request_id`.
   - Responses are enqueued into the connection's `WriteQueue` in identical arrival order.

---

## 2. Close Rules

| Situation | Reply Frame Sent? | Connection Closed? | Reason |
| :--- | :---: | :---: | :--- |
| `quit` command | Yes (`Text("ok")`) | Yes | Graceful client termination |
| `1001` (Unsupported Version) | Yes (`Error { code: 1001 }`) | Yes | Handshake incompatibility |
| `1002` (Unsupported Flag) | Yes (`Error { code: 1002 }`) | Yes | Wire protocol violation |
| `1003` (Frame Too Large) | No | Yes | Security ceiling breached |
| `1004` (Invalid Magic) | No | Yes | Stream desynchronization |
| `1005`–`1007` (Payload Corruption) | Yes | Yes | Unrecoverable frame malformation |
| `2001`–`6001` (Command/Data Errors) | Yes (`Error { code, msg }`) | No | Application error; connection preserved |

---

## 3. Migration Stepping & Error Suppression

- During hash table resizes, incremental migration steps (`step_migration(16)`) execute continuously without stalling user commands.
- The network event loop executes an `on_tick` callback on every loop iteration, calling `step_migration_tick`.
- If `step_migration` returns an error (e.g. `TableFull`), the error is recorded, logged once, and `wants_tick` is suppressed (`false`) so the event loop does not spin until the next successful insert or resize start.
- If no migration is active or migration error is suppressed, the tick performs a zero-cost early return and epoll sleeps normally.

---

## 4. Epoll Trigger Mode & Backpressure Implications

1. **Trigger Mode**: The epoll polling subsystem in `src/net/` is strictly **level-triggered**.
2. **Code Verification**: In `src/net/sys.rs`, `to_raw_flags()` only sets `libc::EPOLLET` if `EpollFlags::EDGE_TRIGGERED` is passed; in `src/net/connection.rs`, `desired_flags()` sets only `READABLE` and `WRITABLE`, never `EDGE_TRIGGERED`; and `main.rs` registers sockets without `EDGE_TRIGGERED`.
3. **Backpressure Implication**: In level-triggered mode, as long as unread bytes remain in the socket receive buffer, epoll will continually report `EPOLLIN` on every wait iteration. To prevent 100% CPU busy-spinning while a connection is paused, the server MUST explicitly remove `READABLE` interest via `epoll_ctl(EPOLL_CTL_MOD)`.
4. **Resumption Implication**: Once write queues drain below the low watermark, restoring `READABLE` via `EPOLL_CTL_MOD` causes the kernel to immediately signal readiness if socket bytes remain.
5. **Buffered Data**: Because unread data may already reside in user-space read buffers when paused, resuming also requires actively queuing the connection in `pending_work` to avoid waiting indefinitely for new socket traffic.

---

## 5. Server Hardening Architecture (Week 8, Step 3)

### ServerConfig
- `idle_timeout`: `Option<Duration>` (default: `300s`, `None` = disabled). Configurable via `--idle-timeout <secs>` CLI flag (`0` = never).
- `write_queue_high_watermark`: `usize` (default: `4 MiB`). Threshold where client socket reads are paused.
- `write_queue_low_watermark`: `usize` (default: `1 MiB`). Threshold where paused socket reads are resumed.
- `max_frames_per_event`: `usize` (default: `256`). Maximum complete protocol frames dispatched per event.
- `max_read_bytes_per_event`: `usize` (default: `256 KiB`). Maximum socket bytes read per event.

### Backpressure (Slow Reader Protection)
- When a response is enqueued and the connection's write queue exceeds `write_queue_high_watermark`, frame decoding immediately halts for that client and `READABLE` interest is removed from epoll.
- The write queue exceeds the high watermark by at most one serialized reply frame, preventing unbounded memory growth.
- During pause, the socket send buffer flushes when writable. If writes make progress, the connection is not considered idle.
- When the write queue drains to or below `write_queue_low_watermark`, `READABLE` interest is restored in epoll, and any unconsumed frames remaining in the user-space read buffer are enqueued into `pending_work`.
- If a client disconnects while paused, socket writes fail with `EPIPE` / `ECONNRESET`, immediately cleaning up the connection without leaks or panics.

### Fairness & Deferred Work Processing
- In `handle_client_read`, at most `max_read_bytes_per_event` bytes are read from the socket, and at most `max_frames_per_event` frames are executed.
- If buffered unconsumed bytes remain after the fairness limit is reached, the connection token is enqueued into `pending_work: VecDeque<Token>`.
- `Handler::wants_tick()` returns `true` whenever `!pending_work.is_empty()`, capping event loop polling sleep to 1 ms.
- In `on_tick()`, deferred connections are drained in round-robin fashion, ensuring large pipelines cannot starve concurrent clients.

### Idle Timeout Tracking & Periodic Sweeps
- Each connection tracks `last_active: Instant`, reset whenever incoming bytes are read or outgoing bytes make flush progress.
- A periodic timer (`IDLE_CHECK_TIMER_ID`) runs every `idle_check_interval` (default: 1 s, or `min(1s, idle_timeout / 2)` for sub-second test timeouts).
- `schedule_initial_timers(&mut event_loop)` MUST be called before starting `event_loop.run(&mut server)` (and is also guarded upon first accept), ensuring the first client is monitored even if no subsequent accept events occur.
- Connections idle longer than `idle_timeout` are closed and removed, incrementing `ServerStats.connections_closed_idle`.

### Half-Close Rule & Level-Triggered Epoll Handling
- When a client issues a TCP half-close (`shutdown(Shutdown::Write)`), the kernel signals `EPOLLIN` / `READABLE`. `Connection::read_incoming()` detects `0` bytes read and sets `Connection.read_eof = true`.
- To prevent 100% CPU busy-spinning in level-triggered epoll, `desired_flags()` immediately drops `READABLE` interest once `read_eof` is set.
- Completed frames already buffered in the connection's user-space `ReadBuffer` continue to be processed across subsequent event loop ticks (`pending_work`).
- The connection is closed and removed if and only if all three conditions hold:
  1. EOF was seen on the read half (`read_eof` is true).
  2. No complete frame remains in the user-space read buffer (`has_complete_frame` is false; leftover partial frames are dropped).
  3. The outbound `WriteQueue` has been fully flushed (`write_queue.is_empty()`).

### Big-Reply Test Methodology & Kernel Socket Buffers
- Kernel TCP socket buffers (typically 128 KiB to 2 MiB on modern Linux kernels) easily absorb thousands of tiny ping responses (~20 bytes each), allowing the server's user-space write queue to drain completely without ever reaching user-space backpressure thresholds.
- To rigorously test user-space backpressure and watermark hysteresis, tests use the "big-reply" methodology:
  1. Store a large payload (e.g. 256 KiB byte pattern via `put bigkey <pattern>`).
  2. Pipeline multiple `fetch bigkey` requests (e.g. 300 fetches $\approx$ 75 MB of replies) without reading immediately.
  3. The large responses rapidly saturate OS kernel socket buffers, forcing replies to accumulate in the server's `WriteQueue` and deterministically triggering user-space backpressure pauses (`backpressure_pauses >= 1`).
  4. The client then reads all responses in order, verifying request IDs and payloads, and joins background sender threads.

### Telemetry Counters & `connections_closed` / `nodelay_failures`
- `ServerStats.connections_closed`: Monotonically increasing `AtomicU64` counter incremented whenever a client connection is terminated and removed from the server for any reason (clean client quit, EOF completion, protocol error, disconnect, or idle sweep).
- `ServerStats.nodelay_failures`: Monotonically increasing `AtomicU64` counter incremented whenever setting `TCP_NODELAY` on an accepted client connection fails (the connection continues without crashing).
