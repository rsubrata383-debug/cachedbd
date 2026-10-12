# Synchronous Client Architecture (v0.1)

## Implementation Plan
1. Add ClientConfig with connect/read/write timeouts, client_name, and max_reply_size.
2. Implement Client::connect and Client::connect_with performing standard HELLO handshake.
3. Parse and retain ServerInfo containing negotiated limits and connection metadata.
4. Implement Client::command with wrapping u32 request_id and response/error flag validation.
5. Implement Client::pipeline sending batched requests in one write_all and reading replies in order.
6. Provide typed helpers for ping, put, put_nx, put_xx, fetch, drop_keys, has, size, bump, quit.
7. Implement ClientError enum with Display and Error mapping Io, Protocol, Server, and Closed states.
8. Enforce max_reply_size and stream buffer draining via decode_frame_with_limit.
9. Mark client as broken on fatal errors to immediately fail subsequent operations.
10. Verify cross-platform client with fake blocking server, bad server tests, Linux E2E, and model test.

---

## 1. Architectural Overview

The synchronous client library (`skiffdb::client::Client`) provides a pure `std`-only, blocking interface to `skiffd`. It is portable across both Windows and Linux, utilizing `std::net::TcpStream` with configurable socket timeouts.

Communication is strictly framed using the 14-byte wire header specified in `docs/spec/protocol.md` and typed payloads (`skiffdb::proto::Value`).

## 2. Configuration & Connection Lifecycle

- `ClientConfig`:
  - `connect_timeout: Option<Duration>`: Maximum duration to establish TCP connection (default `None`).
  - `read_timeout: Option<Duration>`: Socket read timeout (default `None`).
  - `write_timeout: Option<Duration>`: Socket write timeout (default `None`).
  - `client_name: String`: Identification string transmitted during handshake (default `"skiff-client/0.1.0"`).
  - `max_reply_size: usize`: Maximum allowed reply payload size in bytes (default `64 MB`, matching server limit).
- Handshake:
  - On `Client::connect` or `Client::connect_with`, the client immediately opens a TCP stream, configures timeouts, and sends an `Opcode::Hello` frame (`protocol_version = 1`).
  - Upon receiving the server's reply map, it constructs and caches `ServerInfo` (`server_name`, `server_version`, `client_id`, `max_key_size`, `max_value_size`, `max_frame_size`).
  - If the server rejects the handshake or disconnects, the error is immediately returned and no client instance is created.

## 3. Command Execution & Pipelining

- `request_id`: A 32-bit unsigned integer counter initialized to `1` (for `HELLO`) and incremented with `wrapping_add(1)` for each subsequent command.
- Single Commands:
  - `Client::command(&mut self, args: Vec<Value>) -> Result<Value, ClientError>` builds a `Value::Array`, writes the frame, awaits the response, and verifies `FLAG_RESPONSE` and `request_id`.
  - Logical server errors (`FLAG_ERROR` set with `Value::Error { code, message }`) return `Err(ClientError::Server { code, message })`. Logical errors do **not** break the transport connection.
- Pipelining & Chunking:
  - `Client::pipeline(&mut self, commands: Vec<Vec<Value>>) -> Result<Vec<Result<Value, ClientError>>, ClientError>` batches commands into bounded chunks to prevent socket buffer deadlocks when the peer stops consuming writes while replies remain unread.
  - Chunk limits: `PIPELINE_CHUNK_COMMANDS = 256` and `PIPELINE_CHUNK_BYTES = 256 * 1024` (256 KB). If a single command exceeds 256 KB, it forms its own independent chunk.
  - For each chunk: the client writes the chunk's frames, reads and validates all corresponding replies, and stores them before proceeding to write subsequent chunks.
  - Zero cloning: command value vectors are consumed directly during encoding.
  - Validation: Both single commands and pipeline chunks invoke shared `validate_reply` which verifies `FLAG_RESPONSE`, `request_id`, opcode matches `Opcode::Command`, and maps structured server errors.
- Read Buffer with Cursor:
  - Instead of draining the buffer on every frame (`drain(..used)`), a `read_cursor` tracks the consumption point.
  - Socket data is read in 64 KB blocks (`[0u8; 64 * 1024]`).
  - Compaction shifts unconsumed bytes to the front only when `read_cursor > buf.len() / 2` or when the buffer is empty, amortizing memory movement to $O(1)$ per byte.

## 4. Error Mapping & Broken Connection Handling

- `ClientError` Taxonomy:
  - `Io(std::io::Error)`: Socket read/write or timeout errors.
  - `Protocol(String)`: Framing violations, invalid magic, missing flags, or mismatched `request_id`.
  - `Server { code: u16, message: String }`: Structured application error returned by `skiffd`.
  - `UnexpectedReply(String)`: Typed helper received a `Value` variant incompatible with its contract.
  - `ConnectionClosed`: Socket closed prematurely by the peer (unexpected EOF).
  - `ReplyTooLarge`: Server header declared a `payload_len` exceeding `max_reply_size`.
- Fatal Error Invariant:
  - Once a fatal error occurs (`Io`, `Protocol`, `ConnectionClosed`, `ReplyTooLarge`), the client transitions into a broken state (`is_broken = true`).
  - Any subsequent method invocation fails immediately with `ClientError::ConnectionClosed` without attempting further network I/O.

## 5. Deferred Features (Phase 2 & Beyond)

1. **Automatic Reconnection**: The client does not retry or reconnect automatically upon network failure. Callers must re-instantiate `Client::connect`.
2. **Connection Pooling**: Multiplexed or threaded connection pooling is deferred. Callers requiring concurrency may instantiate independent `Client` instances across threads.
3. **Transport Layer Security (TLS)**: Pure plain-text TCP in v0.1. TLS encryption will be designed alongside authentication enhancements.
4. **Asynchronous Runtime**: No `tokio` or async runtime dependencies. A zero-dependency blocking client ensures maximum compatibility, minimal compile overhead, and straightforward reasoning.
