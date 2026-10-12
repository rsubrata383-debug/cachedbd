# SkiffDB Error Code Catalog (v0.1)

This document catalogs all numeric error codes, error names, human-readable messages, trigger conditions, and connection behavior in SkiffDB.

> **Version:** 0.1 (Frozen)  
> **Status:** Frozen on 2026-10-10 (Phase 0 Week 2)  
> **Change Policy:** Frozen specifications cannot be modified without a formal decision entry in `docs/journal.md`, documented rationale, and a version increment (e.g. v0.1.1 or v0.2).

---

## 1. Organization Principle

Error codes are organized into semantic 1000s ranges with generous gaps. This allows client SDKs and drivers to inspect error ranges (e.g., `1000..=1999` indicates a fatal framing error requiring connection reset, while `2000..=4999` indicates command/application-level errors where the connection remains open).

---

## 2. Codes Previously Established in `protocol.md`

The following codes were established in [`protocol.md`](protocol.md) and are preserved with their exact numbers, names, and messages in the catalog below:
- `1001`: `ERR_UNSUPPORTED_PROTOCOL_VERSION` (Protocol version mismatch)
- `1002`: `ERR_UNSUPPORTED_FLAG` (Reserved flag bit received)
- `1008`: `ERR_UNKNOWN_OPCODE` (Unrecognized frame opcode received)
- `2001`: `ERR_UNKNOWN_COMMAND` (Unregistered command name)

---

## 3. Comprehensive Error Catalog by Range

### 3.1 Range 1000–1999: Protocol and Framing Errors
*These represent low-level wire framing corruptions or protocol violations. The server immediately closes the connection.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **1001** | `ERR_UNSUPPORTED_PROTOCOL_VERSION` | `"unsupported protocol version"` | Client sends unsupported version in HELLO | **Yes** |
| **1002** | `ERR_UNSUPPORTED_FLAG` | `"unsupported flag"` | Client sets reserved or unsupported flag bit | **Yes** |
| **1003** | `ERR_FRAME_TOO_LARGE` | `"frame exceeds maximum size"` | Frame `payload_len` exceeds `max_frame_size` (default 64 MB) | **Yes** |
| **1004** | `ERR_INVALID_MAGIC` | `"invalid protocol magic header"` | Header does not begin with `0xCA 0xDB` | **Yes** |
| **1005** | `ERR_MALFORMED_VARINT` | `"malformed varint encoding"` | Varint continuation exceeds 10 bytes (64 bits) or stream terminates early | **Yes** |
| **1006** | `ERR_MALFORMED_PAYLOAD` | `"payload decoding failed"` | Payload byte stream ends before declared item length | **Yes** |
| **1007** | `ERR_UNKNOWN_TYPE_TAG` | `"unknown type tag"` | Payload contains unrecognized type tag byte without length prefix | **Yes** |
| **1008** | `ERR_UNKNOWN_OPCODE` | `"unknown frame opcode"` | Header specifies an unrecognized frame opcode | **Yes** |
| *1009–1099* | *(Reserved for future framing errors)* | — | — | — |

---

### 3.2 Range 2000–2999: Command Syntax and Argument Errors
*These represent invalid command usage or argument bounds violations. The connection remains open.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **2001** | `ERR_UNKNOWN_COMMAND` | `"unknown command"` | Command verb does not exist | **No** |
| **2002** | `ERR_WRONG_ARG_COUNT` | `"wrong number of arguments for command"` | Command receives fewer or more arguments than required | **No** |
| **2003** | `ERR_INVALID_ARGUMENT` | `"invalid argument syntax"` | Argument cannot be parsed into expected target type (e.g. invalid integer) | **No** |
| **2004** | `ERR_KEY_TOO_LARGE` | `"key exceeds maximum size"` | Key length exceeds `max_key_size` (default 1 KB, configurable) | **No** |
| **2005** | `ERR_VALUE_TOO_LARGE` | `"value exceeds maximum size"` | Value payload exceeds 32 MB limit | **No** |
| **2006** | `ERR_EMPTY_KEY` | `"empty key is not allowed"` | Command supplied a 0-length key | **No** |
| **2007** | `ERR_TOO_MANY_ARGS` | `"too many arguments"` | Number of arguments exceeds 65,536 limit | **No** |
| **2008** | `ERR_SYNTAX_ERROR` | `"syntax error"` | Generic argument syntax error (e.g. unknown sub-command flag) | **No** |
| *2009–2099* | *(Reserved for future command errors)* | — | — | — |

---

### 3.3 Range 3000–3999: Value and Data Type Errors
*These occur when operations are executed against incompatible data types or cause numerical overflow. The connection remains open.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **3001** | `ERR_WRONG_TYPE` | `"operation against a key holding the wrong kind of value"` | Calling type-specific command on a mismatched value type | **No** |
| **3002** | `ERR_INDEX_OUT_OF_BOUNDS` | `"index out of bounds"` | Sequence or array index is out of valid range | **No** |
| **3003** | `ERR_INTEGER_OVERFLOW` | `"integer overflow"` | Atomic increment/decrement (`BUMP`) would exceed signed 64-bit integer limits | **No** |
| **3004** | `ERR_NOT_AN_INTEGER` | `"value is not an integer"` | Target string/bytes value cannot be parsed as a 64-bit integer | **No** |
| **3005** | `ERR_MAX_DEPTH_EXCEEDED` | `"maximum nesting depth exceeded"` | Collection exceeds nesting depth limit (8 levels) | **No** |
| *3006–3099* | *(Reserved for future type errors)* | — | — | — |

---

### 3.4 Range 4000–4999: Storage, Key, and Memory Errors
*These relate to keyspace lookups, conditions, and resource limits. The connection remains open.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **4001** | `ERR_KEY_NOT_FOUND` | `"key does not exist"` | Strict fetch command executed on non-existent key | **No** |
| **4002** | `ERR_KEY_EXISTS` | `"key already exists"` | Conditional write (`PUT` only-if-absent) failed because key is present | **No** |
| **4003** | `ERR_OUT_OF_MEMORY` | `"out of memory: maxmemory limit reached"` | Memory limit is reached and eviction policy is `none` or unable to free space | **No** |
| *4004–4099* | *(Reserved for future storage errors)* | — | — | — |

---

### 3.5 Range 5000–5999: Security and Authentication Errors
*These relate to credentials, permissions, and security. The connection remains open unless excessive failures occur.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **5001** | `ERR_AUTH_REQUIRED` | `"authentication required"` | Client sends command before successful authentication | **No** |
| **5002** | `ERR_INVALID_PASSWORD` | `"invalid password or credentials"` | Supplied authentication credentials do not match | **No** |
| **5003** | `ERR_PERMISSION_DENIED` | `"permission denied"` | Authenticated client lacks permission for admin command | **No** |
| *5004–5099* | *(Reserved for future security errors)* | — | — | — |

---

### 3.6 Range 6000–6999: Replication and Clustering Errors
*These relate to leader-follower replication state and cluster roles.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **6001** | `ERR_READONLY_REPLICA` | `"cannot write to read-only replica"` | Client attempts write command on a follower node | **No** |
| **6002** | `ERR_REPLICA_SYNC_FAILED` | `"replication synchronization failed"` | Follower failed to handshake or stream journal | **Yes** |
| **6003** | `ERR_REPLICATION_OFFSET_LOST` | `"replication offset pruned"` | Follower journal offset is older than leader's retained journal buffer | **Yes** |
| *6004–6099* | *(Reserved for future replication errors)* | — | — | — |

---

### 3.7 Range 7000–7999: Server and Persistence Errors
*These relate to server lifecycle, storage engines, and disk I/O.*

| Code | Error Name | Message | When it Happens | Closed? |
|:---:|---|---|---|:---:|
| **7001** | `ERR_INTERNAL_ERROR` | `"internal server error"` | Unhandled internal runtime condition | **No** |
| **7002** | `ERR_PERSISTENCE_FAILED` | `"journal sync or snapshot failed"` | Disk write, flush, or snapshot serialization failed | **No** |
| **7003** | `ERR_LOADING_DATASET` | `"server is loading dataset in background"` | Command sent while server is replaying journal on startup | **No** |
| **7004** | `ERR_SERVER_SHUTTING_DOWN` | `"server is shutting down"` | Command received during server shutdown sequence | **Yes** |
| *7005–7099* | *(Reserved for future server errors)* | — | — | — |
