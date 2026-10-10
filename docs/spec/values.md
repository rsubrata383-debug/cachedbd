# Cachedb Value Model Specification (v0.1)

This document defines the value types, memory model, limits, naming conventions, and key rules for Cachedb.

> **Version:** 0.1 (Frozen)  
> **Status:** Frozen on 2026-10-10 (Phase 0 Week 2)  
> **Change Policy:** Frozen specifications cannot be modified without a formal decision entry in `docs/journal.md`, documented rationale, and a version increment (e.g. v0.1.1 or v0.2).

---

## 1. Key Principles and Rules

1. **Byte Content**: Keys are treated as arbitrary binary byte sequences (`&[u8]`). Any byte value from `0x00` to `0xFF` is allowed.
2. **Safe Escaping in Shell Client**: Because keys can contain arbitrary binary, control characters, or non-printable bytes, the interactive client (`cachedb-cli`) must display non-printable bytes in a safe escaped form (e.g. `\xNN` or quoted strings).
3. **Empty Keys**: Keys of length 0 are disallowed. Operations attempting to use `""` as a key return error `2006` (`ERR_EMPTY_KEY`). Keys must be at least 1 byte long.
4. **Case Sensitivity**: Keys are strictly byte-sensitive. `"myKey"` and `"mykey"` are distinct keys.
5. **Namespaces**: Cachedb does not enforce schema. Applications may use delimiters such as colons (`:`) or slashes (`/`) by convention (e.g. `"user:1001:profile"`).

---

## 2. Value Types Catalog

Cachedb values are divided into **Primitive Types** (Phase 1 / v0.1) and **Rich Data Structures** (Phase 3 / v0.3).

### 2.1 Primitive Types (Available in Phase 1 / v0.1)

| Type | Type Name (`TYPE` reply) | Description | Protocol Tag |
|---|---|---|---|
| **Nil** | `"nil"` | Represents an absent key or null value. | `0x00` |
| **Int** | `"int"` | Signed 64-bit integer (`-9,223,372,036,854,775,808` to `9,223,372,036,854,775,807`). | `0x01` |
| **Float** | `"float"` | 64-bit IEEE-754 double-precision floating point number. | `0x02` |
| **Bool** | `"bool"` | Boolean true or false value. | `0x03` |
| **Text** | `"text"` | UTF-8 encoded string. | `0x04` |
| **Bytes** | `"bytes"` | Arbitrary binary byte buffer. | `0x05` |
| **Array** | `"array"` | Ordered list of heterogeneous typed items. | `0x06` |
| **Map** | `"map"` | Key-value associative mapping of typed items. | `0x07` |

### 2.2 Rich Data Structures (Planned for Phase 3 / v0.3)

These specialized data structures have dedicated commands and memory-optimized dual encodings:

| Type | Type Name (`TYPE` reply) | Planned Phase | Description |
|---|---|---|---|
| **Sequence** | `"seq"` | Phase 3 (Weeks 18-19) | Ordered list / double-ended queue. Supports fast push/pop at both ends, range indexing, and blocking pops. |
| **Table** | `"tab"` | Phase 3 (Week 20) | Field-to-value map (hash table). Compact packed array for small tables, hash table for large tables. |
| **Pool** | `"pool"` | Phase 3 (Week 21) | Unique element set. Integer array for integer sets, hash set for general elements. Supports union/inter/diff. |
| **Ladder** | `"lad"` | Phase 3 (Weeks 22-23) | Ranked set / sorted score index. Skip list paired with hash table for score and rank lookups ($O(\log N)$). |
| **Bitset** | `"bit"` | Phase 3 (Week 24) | Compact bitmap supporting bitwise operations (`AND`, `OR`, `XOR`), popcounts, and bit testing. |
| **Stream** | `"stream"` | Phase 3 (Week 25, Stretch) | Append-only log with monotonic entry IDs and consumer groups. |

---

## 3. Storage and Value Object Model

In the core key-value engine, every entry in the keyspace consists of:

```text
+-------------------------------------------------------------+
| Key: byte buffer (1 byte to 1 KB default)                   |
+-------------------------------------------------------------+
| Value Object:                                               |
|   - type_tag: u8 (primitive or rich data structure)         |
|   - encoding_tag: u8 (e.g. compact vs raw vs general)       |
|   - expiry_deadline: Option<u64> (monotonic timestamp, ms)  |
|   - lru_lfu_metadata: u32 (access time or frequency count)  |
|   - payload: type-specific data buffer / heap allocation    |
+-------------------------------------------------------------+
```

---

## 4. System Limits

The limits in the value model align with [docs/spec/protocol.md](file:///c:/Users/Subrata/Desktop/rust/cachedb/docs/spec/protocol.md):

| Limit Name | Limit Value | Scope | Enforcement |
|---|---|---|---|
| **Max Key Size** | 1 KB default (1,024 bytes, configurable) | Single key | Rejects with error `2004` (`ERR_KEY_TOO_LARGE`) |
| **Min Key Size** | 1 byte | Single key | Rejects with error `2006` (`ERR_EMPTY_KEY`) |
| **Max Value Size** | 32 MB default (configurable) | Single value payload | Rejects with error `2005` (`ERR_VALUE_TOO_LARGE`) |
| **Max Frame Size** | 64 MB default (configurable) | Wire protocol frame | Drops connection with error `1003` (`ERR_FRAME_TOO_LARGE`) |
| **Max Argument Count** | 65,536 arguments | Command frame | Rejects with error `2007` (`ERR_TOO_MANY_ARGS`) |
| **Max Array / Collection Size**| 4,294,967,295 ($2^{32} - 1$) | Elements in list/set/table | Bounded by memory and max value size |
| **Max Nesting Depth** | 8 levels | Nested collections (Array/Map)| Rejects with error `3005` (`ERR_MAX_DEPTH_EXCEEDED`) |

> **Testing Large Values**: To test 100 MB values, raise both `max_value_size` and `max_frame_size` in `cachedb.toml`.

---

## 5. Type Inspection and "Wrong Type" Semantics

### 5.1 Inspecting Types (`TYPE` command)
The `type <key>` command returns a `Text` item with the type's canonical name:
- If key exists: returns `"text"`, `"bytes"`, `"int"`, `"float"`, `"bool"`, `"seq"`, `"tab"`, `"pool"`, `"lad"`, `"bit"`.
- If key does not exist: returns `"nil"`.

### 5.2 "Wrong Type" Errors
When a command is executed against a key holding an incompatible data type, the server returns a structured error:
- **Error Code**: `3001` (`ERR_WRONG_TYPE`)
- **Message**: `"operation against a key holding the wrong kind of value"`
- **Connection**: Remains open (non-fatal error).

**Example:**
- Key `"counter"` holds `Int(42)`.
- Client sends `seq-push-back counter "item"`.
- Server responds with error `3001`:
  ```text
  CA DB 01 03 00 00 00 07 00 02 00 00 00 3C
  08 D9 17 38 6F 70 65 72 61 74 69 6F 6E 20 61 67 61 69 6E 73 74 ...
  -- Error Code 3001 (0xD9 0x17), message: "operation against a key holding the wrong kind of value"
  ```
