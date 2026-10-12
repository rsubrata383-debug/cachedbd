# SkiffDB Wire Protocol Specification (v0.1)

This document defines the binary wire protocol used for communication between SkiffDB clients and the `skiffd` server.

> **Version:** 0.1 (Frozen)  
> **Status:** Frozen on 2026-10-10 (Phase 0 Week 2)  
> **Change Policy:** Frozen specifications cannot be modified without a formal decision entry in `docs/journal.md`, documented rationale, and a version increment (e.g. v0.1.1 or v0.2).

---

## 1. Protocol Overview

The SkiffDB wire protocol is a framed, pure-binary protocol designed for high throughput, low latency, and efficient pipelining.

Key design attributes:
- **Encoding**: Pure binary. Payloads use typed binary items without text parsing overhead.
- **Byte Order**: Big-Endian (Network Byte Order) for all multi-byte header fields and numbers.
- **Framing**: Fixed 14-byte header containing a 4-byte (`u32`) payload length prefix.
- **Multiplexed Pipelining**: 32-bit `request_id` pairs asynchronous requests and responses.
- **Push Notifications**: Unsolicited messages (pub/sub, replication) use `FLAG_PUSH` and `request_id = 0`.
- **Streaming Chunks (Reserved in v0.1)**: `FLAG_MORE_FRAMES` is reserved in v0.1; chunking design is deferred to Phase 2, Week 15.


---

## 2. Frame Header Format

Every frame begins with a fixed 14-byte header:

```text
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Magic (2B)           |  Version (1B) |   Flags (1B)  |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Request ID (4B)                      |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Opcode (2B)          |       Payload Length (4B)     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+                               +
|                           (continued)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Header Fields:

| Field | Size | Type | Byte Order | Description |
|---|---|---|---|---|
| `magic` | 2 bytes | Bytes | — | Fixed protocol identifier: `0xCA 0xDB` ("CADB"). Rejects non-protocol traffic immediately. |
| `version` | 1 byte | u8 | — | Protocol version. Initial version is `1` (`0x01`). |
| `flags` | 1 byte | Bitfield | — | Message control flags (see Flags section). |
| `request_id` | 4 bytes | u32 | Big-Endian | Identifier chosen by client to pair responses. Must be `0` for push frames. |
| `opcode` | 2 bytes | u16 | Big-Endian | Operation identifier (`0x0001` = HELLO, `0x0002` = COMMAND, `0x0003` = PUSH). Unrecognized opcodes return `1008 ERR_UNKNOWN_OPCODE` and close the TCP connection. |
| `payload_len`| 4 bytes | u32 | Big-Endian | Byte size of the payload following this header. |

---

## 3. Flags Field (1 byte)

The `flags` byte controls message routing and framing behavior:

| Bit | Hex | Name | Meaning |
|---|---|---|---|
| Bit 0 | `0x01` | `FLAG_RESPONSE` | `0` = Request from client; `1` = Response from server. |
| Bit 1 | `0x02` | `FLAG_ERROR` | `1` = Payload contains a structured error response. |
| Bit 2 | `0x04` | `FLAG_PUSH` | `1` = Unsolicited server push message. `request_id` must be `0`. |
| Bit 3 | `0x08` | `FLAG_MORE_FRAMES` | Reserved in v0.1. Clients and servers MUST NOT set this bit in v0.1. |
| Bits 4-7 | `0xF0` | Reserved | Must be set to `0`. Reserved for future protocol revisions. |

---

## 4. Payload Items and Encoding

The payload begins immediately after the 14-byte header and consists of typed items.

### 4.1 Varint and Zigzag Encoding
Lengths and integers use variable-length encoding:
- **Unsigned Varint (LEB128)**: 7 data bits per byte with MSB (bit 7) as continuation flag.
- **Signed Varint (Zigzag)**: Maps signed values to positive integers before LEB128:
  $$\text{zigzag}(n) = (n \ll 1) \oplus (n \gg 63)$$
  For example, `0 -> 0`, `-1 -> 1`, `1 -> 2`, `42 -> 84` (`0x54`).

### 4.2 Item Type Tags

| Tag | Name | Format | Description |
|---|---|---|---|
| `0x00` | `Nil` | Tag only (1 byte) | Represents missing or null value. |
| `0x01` | `Int` | Tag + zigzag varint | 64-bit signed integer. |
| `0x02` | `Float` | Tag + 8 bytes (Big-Endian) | IEEE-754 64-bit floating point number. |
| `0x03` | `Bool` | Tag + 1 byte | `0x00` for false, `0x01` for true. |
| `0x04` | `Text` | Tag + varint byte length + UTF-8 bytes | UTF-8 encoded string. |
| `0x05` | `Bytes` | Tag + varint byte length + raw bytes | Arbitrary binary data. |
| `0x06` | `Array` | Tag + varint count + sequence of typed items | Ordered list of items. |
| `0x07` | `Map` | Tag + varint count + key-value pairs | Ordered pairs of typed items. |
| `0x08` | `Error` | Tag + varint code + varint length + UTF-8 string | Structured error object. |

### 4.3 Extensibility: Forward Compatibility for New Types
To add new data types without breaking existing clients:
- Every future type tag ($\ge 0x09$) must place an unsigned varint byte length directly after the tag.
- Older clients that encounter an unknown type tag can read the varint length and safely skip past the unknown item.

---

## 5. Handshake and Version Negotiation (HELLO)

Upon opening a TCP connection, clients should perform a `HELLO` exchange:

1. **Client Request**:
   - `opcode = 0x0001` (HELLO), `flags = 0x00` (Request).
   - Payload: Array of 3 items:
     - Protocol version (`Int`: `1`)
     - Client name (`Text`: e.g. `"skiff-cli 0.1.0"`)
     - Auth token / credentials (`Text` or `Nil`)
2. **Server Response**:
   - `opcode = 0x0001`, `flags = 0x01` (FLAG_RESPONSE).
   - Payload: Map containing server information:
     - `"server_version"`: `"0.1.0"`
     - `"protocol_version"`: `1`
     - `"client_id"`: assigned connection ID (`Int`)
3. **Incompatible Version**:
   - Server returns an error response with code `1001` (`ERR_UNSUPPORTED_PROTOCOL_VERSION`) and closes the connection.

---

## 6. Error Replies

When an operation fails:
- Header has `flags = FLAG_RESPONSE | FLAG_ERROR` (`0x03`).
- `request_id` matches the failed request.
- Payload contains an `Error` item (`0x08`):
  - Varint: numeric error code (e.g. `2001` for unknown command).
  - Varint: UTF-8 message byte length.
  - Bytes: human-readable error string.

---

## 7. Push Messages vs Reply Matching

- **Standard Reply**: `flags` has bit 0 set (`FLAG_RESPONSE = 0x01`), bit 2 is 0. The client uses `request_id` to route the response to the waiting request future/callback.
- **Push Message**: `flags` has bit 2 set (`FLAG_PUSH = 0x04`), `flags = 0x05`. `request_id` is always `0x00000000`. The client routes the payload directly to pub/sub listeners or replication event handlers.

---

## 8. Multi-Frame Chunking (Reserved in v0.1)

In v0.1, application-level multi-frame chunking is **reserved**:
- Clients and servers **MUST NOT** set the `FLAG_MORE_FRAMES` (`0x08`) flag in v0.1.
- If a receiver sees a frame with `FLAG_MORE_FRAMES` (`0x08`) set, it MUST send a protocol error (`code = 1002`, `ERR_UNSUPPORTED_FLAG`, message `"unsupported flag"`) and immediately close the connection.
- **Push messages (`request_id = 0`) will never be chunked.**
- Detailed rules for interleaving chunks across different request IDs and chunk reassembly will be formally specified in **Phase 2, Week 15**, guided by real latency measurements.

---

## 9. Limits and Configuration

To prevent denial-of-service and buffer exhaustion:

| Limit | Default Value | Configurable | Behavior on Breach |
|---|---|---|---|
| **Max Frame Size** | 64 MB (67,108,864 bytes) | Yes (`max_frame_size` in `skiff.toml`) | Connection closed immediately |
| **Max Argument Count** | 65,536 items | Yes (`max_arguments`) | Error reply returned |
| **Max Key Size** | 1 KB (1,024 bytes default) | Yes (`max_key_size` in `skiff.toml`) | Error reply returned |
| **Max Value Size** | 32 MB (default) | Yes (`max_value_size`) | Error reply returned |

> **Note on Large Values**: In v0.1, large values up to 100 MB (tested in Phase 2 Week 15 benchmarks) are supported by raising `max_frame_size = 134217728` (128 MB) in `skiff.toml`. Multi-frame chunking remains reserved.

---

## 10. Concrete Hex Examples (Byte-by-Byte)

All multi-byte integers are Big-Endian.

### Example 1: Request (`PING`)
Client sends `PING` with `request_id = 1`:

```text
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
00                -- Flags: 0x00 (Request, 1 byte)
00 00 00 01       -- Request ID: 1 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 08       -- Payload length: 8 bytes (4 bytes)
-- Header total: 14 bytes

06                -- Type: Array (1 byte)
01                -- Array count: 1 element (1 byte)
04                -- Type: Text (1 byte)
04                -- Text length: 4 bytes (1 byte)
70 69 6E 67       -- UTF-8: "ping" (4 bytes)
-- Payload total: 8 bytes
```

---

### Example 2: Reply (`PONG`)
Server replies to request ID 1:

```text
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
01                -- Flags: 0x01 (FLAG_RESPONSE, 1 byte)
00 00 00 01       -- Request ID: 1 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 06       -- Payload length: 6 bytes (4 bytes)
-- Header total: 14 bytes

04                -- Type: Text (1 byte)
04                -- Text length: 4 bytes (1 byte)
70 6F 6E 67       -- UTF-8: "pong" (4 bytes)
-- Payload total: 6 bytes
```

---

### Example 3: Error Reply (Unknown Command)
Server returns error for request ID 2:

```text
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
03                -- Flags: 0x03 (FLAG_RESPONSE | FLAG_ERROR, 1 byte)
00 00 00 02       -- Request ID: 2 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 13       -- Payload length: 19 bytes (4 bytes, 0x13 = 19)
-- Header total: 14 bytes

08                -- Type: Error (1 byte)
D1 0F             -- Error Code: 2001 (2 bytes, LEB128 varint)
0F                -- Message length: 15 bytes (1 byte)
75 6E 6B 6E 6F 77 -- UTF-8: "unknow" (6 bytes)
6E 20 63 6F 6D 6D -- UTF-8: "n comm" (6 bytes)
61 6E 64          -- UTF-8: "and" (3 bytes)
-- Payload total: 1 + 2 + 1 + 15 = 19 bytes
```

---

### Example 4: Push Message (Pub/Sub Event)
Server pushes message to subscriber (`request_id = 0`):

```text
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
05                -- Flags: 0x05 (FLAG_RESPONSE | FLAG_PUSH, 1 byte)
00 00 00 00       -- Request ID: 0 (4 bytes, Big-Endian)
00 03             -- Opcode: 0x0003 (PUSH_MESSAGE, 2 bytes)
00 00 00 0F       -- Payload length: 15 bytes (4 bytes, 0x0F = 15)
-- Header total: 14 bytes

06                -- Type: Array (1 byte)
02                -- Array count: 2 elements (1 byte)
04 04 6E 65 77 73 -- Item 1: Text ("news", 6 bytes: tag + len + 4 chars)
04 05 68 65 6C 6C -- Item 2: Text ("hello", 7 bytes: tag + len + 5 chars)
6F
-- Payload total: 1 + 1 + 6 + 7 = 15 bytes
```

---

### Example 5: Pipelined Pair (Two Requests Back-to-Back)
Client sends `PUT user 42` (ID 10) followed by `FETCH user` (ID 11):

```text
-- Frame 1: PUT user 42 (Request ID 10)
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
00                -- Flags: 0x00 (Request, 1 byte)
00 00 00 0A       -- Request ID: 10 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 0F       -- Payload length: 15 bytes (4 bytes, 0x0F = 15)
-- Header 1: 14 bytes
06 03             -- Array of 3 items (2 bytes)
04 03 70 75 74    -- Item 1: Text "put" (5 bytes: tag + len + 3 chars)
04 04 75 73 65 72 -- Item 2: Text "user" (6 bytes: tag + len + 4 chars)
01 54             -- Item 3: Int 42 (2 bytes: tag + zigzag varint 84 = 0x54)
-- Payload 1: 2 + 5 + 6 + 2 = 15 bytes

-- Frame 2: FETCH user (Request ID 11)
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
00                -- Flags: 0x00 (Request, 1 byte)
00 00 00 0B       -- Request ID: 11 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 0F       -- Payload length: 15 bytes (4 bytes, 0x0F = 15)
-- Header 2: 14 bytes
06 02             -- Array of 2 items (2 bytes)
04 05 66 65 74 63 -- Item 1: Text "fetch" (7 bytes: tag + len + 5 chars)
68
04 04 75 73 65 72 -- Item 2: Text "user" (6 bytes: tag + len + 4 chars)
-- Payload 2: 2 + 7 + 6 = 15 bytes
```

---

### Example 6: Split Frame Arriving in Two TCP Reads (Cut in Payload)
A single valid `PING` request frame (total 22 bytes from Example 1) split across two separate TCP socket reads:

```text
-- Read 1 (16 bytes): Full 14-byte header + first 2 bytes of payload
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
00                -- Flags: 0x00 (Request, 1 byte)
00 00 00 01       -- Request ID: 1 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 08       -- Payload length: 8 bytes (4 bytes)
06 01             -- Payload start: Array tag (1B) + count 1 (1B)
-- Read 1 total: 16 bytes received so far

-- Read 2 (6 bytes): Remaining payload bytes arrive in next socket read
04 04             -- Text tag (1B) + length 4 (1B)
70 69 6E 67       -- UTF-8: "ping" (4 bytes)
-- Read 2 total: 6 bytes

-- Joined Frame (22 bytes total):
CA DB 01 00 00 00 00 01 00 02 00 00 00 08 06 01 04 04 70 69 6E 67
```

> **Parser Rule:** The parser reads the 14-byte header and sees `payload_len = 8`. Having only read 2 payload bytes so far, it must keep the 16 bytes in its connection read buffer and wait for more bytes from the socket. It must not report an error or drop the connection.

---

### Example 7: Split Frame Arriving in Two TCP Reads (Cut in Header)
A single valid `PONG` response frame (total 20 bytes from Example 2) cut in the middle of the 14-byte header after only 5 bytes:

```text
-- Read 1 (5 bytes): Partial header (first 5 bytes)
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
01                -- Flags: 0x01 (FLAG_RESPONSE, 1 byte)
00                -- First byte of Request ID (1 byte)
-- Read 1 total: 5 bytes

-- Read 2 (15 bytes): Remaining 9 header bytes + full 6-byte payload
00 00 01          -- Remaining 3 bytes of Request ID (3 bytes)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 06       -- Payload length: 6 bytes (4 bytes)
04 04 70 6F 6E 67 -- Payload: Text "pong" (6 bytes)
-- Read 2 total: 15 bytes

-- Joined Frame (20 bytes total):
CA DB 01 01 00 00 00 01 00 02 00 00 00 06 04 04 70 6F 6E 67
```

> **Parser Rule:** Because the buffer contains only 5 bytes (less than the required 14-byte header), the parser cannot determine the full frame size yet. It retains the 5 bytes in the connection buffer without error and yields control until the socket is readable again.

---

### Example 8: Unsupported Reserved Flag and Server Error Reply
Client sends a request frame with reserved bit 3 (`0x08`) set; server responds with error `1002` (`ERR_UNSUPPORTED_FLAG`) and closes the connection:

```text
-- Client Request Frame (Reserved bit 3 set: flags = 0x08):
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
08                -- Flags: 0x08 (Reserved bit set, 1 byte)
00 00 00 05       -- Request ID: 5 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 08       -- Payload length: 8 bytes (4 bytes)
-- Header total: 14 bytes
06 01 04 04 70 69 6E 67 -- Payload: Array[Text("ping")] (8 bytes)

-- Server Error Response (flags = 0x03, payload_len = 20):
CA DB             -- Magic: "CADB" (2 bytes)
01                -- Version: 1 (1 byte)
03                -- Flags: 0x03 (FLAG_RESPONSE | FLAG_ERROR, 1 byte)
00 00 00 05       -- Request ID: 5 (4 bytes, Big-Endian)
00 02             -- Opcode: 0x0002 (COMMAND, 2 bytes)
00 00 00 14       -- Payload length: 20 bytes (4 bytes, 0x14 = 20)
-- Header total: 14 bytes

08                -- Type: Error (1 byte)
EA 07             -- Error Code: 1002 (2 bytes, LEB128 varint)
10                -- Message length: 16 bytes (1 byte)
75 6E 73 75 70 70 6F 72 -- UTF-8: "unsuppor" (8 bytes)
74 65 64 20 66 6C 61 67 -- UTF-8: "ted flag" (8 bytes)
-- Payload total: 1 + 2 + 1 + 16 = 20 bytes
```
