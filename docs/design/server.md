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
