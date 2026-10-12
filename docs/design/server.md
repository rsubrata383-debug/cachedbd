# Server Architecture & Request Flow (Week 7, Step 3)

## Plan (Max 10 Lines)
1. Register `hello` and `quit` in `REGISTRY` (`NO_AUTH` flag; `quit` sets `Outcome.close = true`).
2. Implement pure `handle_request(engine, session, frame) -> FrameOutcome` without socket I/O.
3. Support opcode 1 (HELLO) and opcode 2 (COMMAND); reject invalid opcodes with fatal errors.
4. Reply frames mirror `request_id`, setting `FLAG_RESPONSE` (0x01) and `FLAG_ERROR` (0x02) on errors.
5. Enforce connection closure on fatal errors (1001-1007) and when `Outcome.close` is true.
6. Maintain 1 `Session` per connection, dropped upon socket termination.
7. Support pipelining: process frames sequentially in FIFO order without reordering.
8. Add minimal `on_tick` hook to `src/net::Handler` so idle/active migration runs without blocking.
9. Implement `cachedbd` CLI in `src/main.rs` with `--bind`, `--port`, `--help` (exit code 2 on error).

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
     - Returns `HelloResponse` map encoded with opcode `0x0001`, `flags = 0x01`.
   - **Opcode `0x0002` (COMMAND)**:
     - Validates that payload is an `Array`.
     - Invokes `Engine::execute(session, payload)`.
     - Constructs response frame with opcode `0x0002`, `request_id = frame.header.request_id`.
     - Flags set to `0x01` (`FLAG_RESPONSE`), plus `0x02` (`FLAG_ERROR`) if reply is `Value::Error`.
   - **Unknown Opcode**:
     - Returns error `1001` or `2001` with `close = true`.

4. **Pipelining**:
   - If multiple frames are present in `ReadBuffer`, they are decoded and processed in strict arrival order.
   - Responses are enqueued into the connection's `WriteQueue` in identical order.

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

## 3. Migration Stepping & Event Loop Timer

- During hash table resizes, incremental migration steps (`step_migration(16)`) must execute continuously without stalling user commands.
- The network event loop executes an `on_tick` callback on every loop iteration.
- If `engine.store().table().is_resizing()` is true, a small bounded chunk (e.g., 16 slots) is migrated per tick.
- If no migration is active, the tick performs a zero-cost early return.
