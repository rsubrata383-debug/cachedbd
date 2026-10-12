# SkiffDB Command Language Specification (v0.1)

This document defines the core command language and dispatching rules for SkiffDB v0.1.

> **Version:** 0.1 (Frozen)  
> **Status:** Frozen on 2026-10-10 (Phase 0 Week 2)  
> **Change Policy:** Frozen specifications cannot be modified without a formal decision entry in `docs/journal.md`, documented rationale, and a version increment (e.g. v0.1.1 or v0.2).

---

## 1. Design Choices and Conventions

1. **Strict Lowercase Commands**: Command names over the wire MUST be strictly lowercase. The server performs exact byte matching without case conversion. Any uppercase letters trigger error `2001` (`ERR_UNKNOWN_COMMAND`). The CLI client (`skiff-cli`) automatically normalizes the first word to lowercase before transmission.
2. **Missing Key on Fetch**: `fetch` returns `Nil` (`0x00`) on a missing key (standard cache miss behavior).
3. **Drop Return Type**: `drop` returns an `Int` count of keys that were removed.
4. **Put Return Type**: `put` returns `Bool` (`true` on successful store, `false` if `nx` or `xx` condition prevented write).
5. **Options Style**: Options are lowercase keyword strings (e.g. `"nx"`, `"xx"`).

---

## 2. General Command Rules

### 2.1 Wire Transmission
Every command is transmitted inside a standard wire protocol frame (opcode `0x0002` = `COMMAND`, `flags = 0x00`).
The frame payload contains an **Array** (`0x06`) of typed items:
- **First Item**: Command verb (`Text` or `Bytes`, strictly lowercase, e.g. `"put"`).
- **Subsequent Items**: Command arguments.

### 2.2 Strict Lowercase Command Names
Command verbs over the wire MUST be strictly lowercase. If a client transmits a command name containing uppercase letters (such as `"PUT"` or `"Ping"`), the server treats it as unknown and returns error **`2001`** (`ERR_UNKNOWN_COMMAND`).
**Client Rule (`skiff-cli`)**: The interactive CLI client automatically converts the first word of user input to lowercase before encoding the wire frame. Key names and argument values are never modified and remain strictly byte-sensitive.

### 2.3 Dispatcher Table and Flags
The server dispatches commands using an internal registry table:

| Flag Name | Meaning |
|---|---|
| `READ` | Command only reads keyspace data (can run on replicas). |
| `WRITE` | Command mutates keyspace data (rejected on read-only replicas with error `6001`). |
| `NO_AUTH` | Command is permitted before client authentication (`hello`, `ping`, `quit`). |
| `ADMIN` | Command requires elevated administrative privilege. |

### 2.4 Argument Arity Checking
Each command in the dispatcher declares its arity rules:
- Fixed arity: Exactly $N$ arguments.
- Variable arity: Between $N$ and $M$ arguments.
If a client provides too few or too many arguments, execution halts before handler invocation and returns error **`2002`** (`ERR_WRONG_ARG_COUNT`).

### 2.5 Keyspace Validation
Before invoking keyspace operations:
- **Empty Key**: If key length is 0, returns error **`2006`** (`ERR_EMPTY_KEY`).
- **Key Too Large**: If key length exceeds `max_key_size` (default 1 KB), returns error **`2004`** (`ERR_KEY_TOO_LARGE`).
- **Wrong Type**: If a command targets a key holding a different value type, returns error **`3001`** (`ERR_WRONG_TYPE`).

---

## 3. Phase 1 Command Specifications

---

### 3.1 `hello`
Performs connection handshake, protocol negotiation, and optional authentication.

- **Dispatch Flags**: `NO_AUTH`, `READ`
- **Arity**: 2 to 3 arguments
- **Arguments**:
  1. `protocol_version` (`Int`): Client's protocol version (must be `1` in v0.1).
  2. `client_name` (`Text`): Identifying name and version of the client library.
  3. `auth_token` (`Text` | `Bytes` | `Nil`, optional): Authentication credentials.
- **Success Reply**: `Map` (`0x07`) containing server configuration and metadata:
  - `"server_name"`: `"skiffd"` (`Text`)
  - `"server_version"`: `"0.1.0"` (`Text`)
  - `"protocol_version"`: `1` (`Int`)
  - `"client_id"`: assigned connection ID (`Int`)
  - `"max_frame_size"`: maximum frame bytes (`Int`, e.g. `67108864`)
  - `"max_key_size"`: maximum key bytes (`Int`, e.g. `1024`)
  - `"max_value_size"`: maximum value bytes (`Int`, e.g. `33554432`)
- **Errors**:
  - `1001` (`ERR_UNSUPPORTED_PROTOCOL_VERSION`): Client requested version other than 1. Closes connection.
  - `2002` (`ERR_WRONG_ARG_COUNT`): Missing version or client name.
  - `5002` (`ERR_INVALID_PASSWORD`): Authentication token failed verification.
- **Example**:
  - Request: `["hello", 1, "skiff-cli 0.1.0"]`
  - Reply: `{"server_name": "skiffd", "server_version": "0.1.0", "protocol_version": 1, "client_id": 1, "max_frame_size": 67108864, "max_key_size": 1024, "max_value_size": 33554432}`

---

### 3.2 `ping`
Tests connection liveness or echoes a message.

- **Dispatch Flags**: `NO_AUTH`, `READ`
- **Arity**: 0 to 1 arguments
- **Arguments**:
  1. `message` (`Text` | `Bytes`, optional): Custom message to echo.
- **Success Reply**:
  - If no argument given: returns `Text("pong")`.
  - If argument given: returns the provided message item unmodified.
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): More than 1 argument provided.
- **Example**:
  - Request: `["ping"]` -> Reply: `"pong"`
  - Request: `["ping", "heartbeat"]` -> Reply: `"heartbeat"`

---

### 3.3 `put`
Stores a value at a key, with optional conditional flags.

- **Dispatch Flags**: `WRITE`
- **Arity**: 2 to 3 arguments
- **Arguments**:
  1. `key` (`Bytes` | `Text`): Target key name (1 B to 1 KB).
  2. `value` (`Any` typed item): Payload to store (up to 32 MB).
  3. `condition` (`Text`, optional):
     - `"nx"`: Only set the key if it does NOT already exist (only-if-absent).
     - `"xx"`: Only set the key if it DOES already exist (only-if-present).
- **Success Reply**:
  - `Bool(true)`: Value was successfully stored.
  - `Bool(false)`: Value was not stored because condition `nx` or `xx` was not satisfied.
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): Fewer than 2 or more than 3 arguments.
  - `2003` (`ERR_INVALID_ARGUMENT`): Third argument is not `"nx"` or `"xx"`.
  - `2004` (`ERR_KEY_TOO_LARGE`): Key exceeds 1 KB limit.
  - `2005` (`ERR_VALUE_TOO_LARGE`): Value exceeds 32 MB limit.
  - `2006` (`ERR_EMPTY_KEY`): Key is empty (0 bytes).
  - `4003` (`ERR_OUT_OF_MEMORY`): Max memory reached and eviction cannot free space.
  - `5001` (`ERR_AUTH_REQUIRED`): Server requires authentication.
  - `6001` (`ERR_READONLY_REPLICA`): Server is a read-only replica.
- **Example**:
  - Request: `["put", "user:1", "alice"]` -> Reply: `true`
  - Request: `["put", "user:1", "bob", "nx"]` -> Reply: `false`
  - Request: `["put", "user:1", "bob", "xx"]` -> Reply: `true`

---

### 3.4 `fetch`
Retrieves the value stored at a key.

- **Dispatch Flags**: `READ`
- **Arity**: Exactly 1 argument
- **Arguments**:
  1. `key` (`Bytes` | `Text`): Target key name.
- **Success Reply**:
  - If key exists: returns the stored value (typed item).
  - If key does not exist: returns `Nil` (`0x00`).
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): Not exactly 1 argument.
  - `2004` (`ERR_KEY_TOO_LARGE`): Key exceeds 1 KB limit.
  - `2006` (`ERR_EMPTY_KEY`): Key is empty (0 bytes).
  - `5001` (`ERR_AUTH_REQUIRED`): Server requires authentication.
- **Example**:
  - Request: `["fetch", "user:1"]` -> Reply: `"alice"`
  - Request: `["fetch", "user:999"]` -> Reply: `nil`

---

### 3.5 `drop`
Deletes one or more keys from the keyspace.

- **Dispatch Flags**: `WRITE`
- **Arity**: 1 or more arguments
- **Arguments**:
  1. `key` (`Bytes` | `Text`): Key to delete. Multiple keys may be specified (`key1`, `key2`, ...).
- **Success Reply**: `Int` count of keys that were removed.
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): 0 arguments provided.
  - `2004` (`ERR_KEY_TOO_LARGE`): Any key exceeds 1 KB limit.
  - `2006` (`ERR_EMPTY_KEY`): Any key is empty (0 bytes).
  - `5001` (`ERR_AUTH_REQUIRED`): Server requires authentication.
  - `6001` (`ERR_READONLY_REPLICA`): Server is a read-only replica.
- **Example**:
  - Request: `["drop", "user:1"]` -> Reply: `1`
  - Request: `["drop", "user:1"]` -> Reply: `0`
  - Request: `["drop", "k1", "k2", "k3"]` -> Reply: `2` (if k1, k2 existed, k3 did not)

---

### 3.6 `has`
Tests whether a key exists in the keyspace.

- **Dispatch Flags**: `READ`
- **Arity**: Exactly 1 argument
- **Arguments**:
  1. `key` (`Bytes` | `Text`): Target key name.
- **Success Reply**: `Bool` (`true` if key exists, `false` otherwise).
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): Not exactly 1 argument.
  - `2004` (`ERR_KEY_TOO_LARGE`): Key exceeds 1 KB limit.
  - `2006` (`ERR_EMPTY_KEY`): Key is empty (0 bytes).
  - `5001` (`ERR_AUTH_REQUIRED`): Server requires authentication.
- **Example**:
  - Request: `["has", "user:1"]` -> Reply: `true`
  - Request: `["has", "user:999"]` -> Reply: `false`

---

### 3.7 `size`
Returns the total number of keys currently stored in the database.

- **Dispatch Flags**: `READ`
- **Arity**: Exactly 0 arguments
- **Arguments**: None.
- **Success Reply**: `Int` representing total keys count.
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): Arguments were provided.
  - `5001` (`ERR_AUTH_REQUIRED`): Server requires authentication.
- **Example**:
  - Request: `["size"]` -> Reply: `42`

---

### 3.8 `bump`
Atomically increments or decrements a 64-bit integer value stored at a key.

- **Dispatch Flags**: `WRITE`
- **Arity**: 1 to 2 arguments
- **Arguments**:
  1. `key` (`Bytes` | `Text`): Target key name.
- **Special Semantics**:
  - **Missing Key**: A missing key starts at `0` and the key is created with the stepped value (e.g. `["bump", "views", 5]` creates `"views"` with value `5`, and `["bump", "views"]` creates `"views"` with value `1`).
  - **Step Size**: The step can be positive or negative (to decrement), and the default step is `1`.
  - **Overflow & Underflow**: On overflow or underflow beyond signed 64-bit integer limits (`-9,223,372,036,854,775,808` to `9,223,372,036,854,775,807`), it returns error **`3003`** (`ERR_INTEGER_OVERFLOW`) and the stored value does not change.
  - **Non-Integer Value**: If the key exists but holds a value that is not an integer, it returns a type error (**`3004`** `ERR_NOT_AN_INTEGER` for non-integer strings/scalars, or **`3001`** `ERR_WRONG_TYPE` for non-scalar collections).
- **Success Reply**: `Int` containing the new integer value after bumping.
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): Fewer than 1 or more than 2 arguments.
  - `2003` (`ERR_INVALID_ARGUMENT`): Step argument is not a valid integer.
  - `2004` (`ERR_KEY_TOO_LARGE`): Key exceeds 1 KB limit.
  - `2006` (`ERR_EMPTY_KEY`): Key is empty (0 bytes).
  - `3001` (`ERR_WRONG_TYPE`): Key holds a rich non-scalar data structure.
  - `3003` (`ERR_INTEGER_OVERFLOW`): Addition would overflow or underflow 64-bit signed integer.
  - `3004` (`ERR_NOT_AN_INTEGER`): Key holds string bytes that cannot be parsed as a 64-bit integer.
  - `4003` (`ERR_OUT_OF_MEMORY`): Max memory reached when creating key.
  - `5001` (`ERR_AUTH_REQUIRED`): Server requires authentication.
  - `6001` (`ERR_READONLY_REPLICA`): Server is a read-only replica.
- **Example**:
  - Request: `["bump", "page_views"]` -> Reply: `1`
  - Request: `["bump", "page_views", 10]` -> Reply: `11`
  - Request: `["bump", "page_views", -2]` -> Reply: `9`

---

### 3.9 `quit`
Asks the server to close the connection gracefully.

- **Dispatch Flags**: `NO_AUTH`, `READ`
- **Arity**: Exactly 0 arguments
- **Arguments**: None.
- **Success Reply**: `Text("ok")`. The server flushes the reply to the client and then closes the TCP connection.
- **Errors**:
  - `2002` (`ERR_WRONG_ARG_COUNT`): Arguments were provided.
- **Example**:
  - Request: `["quit"]` -> Reply: `"ok"` (connection closes)

---

## 4. Planned Commands by Phase (Names Only)

Future commands planned in the roadmap across upcoming phases:

### Phase 1 (Shipped in v0.1.0)
- Core key-value and server commands: `hello`, `ping`, `put`, `fetch`, `drop`, `has`, `size`, `bump`, `quit`.

### Phase 2: Cache Behavior & Value Mutators (Weeks 9-16)
- `expire`, `expire-at`, `ttl`, `persist`
- `scan`, `rename`, `copy`, `type`, `random-key`, `wipe`
- `stats`, `memory`, `slowlog`, `config-get`, `config-set`
- `append`, `swap`: deferred from Phase 1 to align with general value mutation and type management semantics in Phase 2.
- `auth`, `use`: deferred from Phase 1 to align with access-control and multi-database features planned alongside config/admin commands in Phase 2.

### Phase 3: Data Types (Weeks 17-26)
- **Sequences**: `seq-push-front`, `seq-push-back`, `seq-pop-front`, `seq-pop-back`, `seq-range`, `seq-len`, `seq-trim`
- **Tables**: `tab-set`, `tab-get`, `tab-drop`, `tab-all`, `tab-len`, `tab-bump`
- **Pools**: `pool-add`, `pool-drop`, `pool-has`, `pool-len`, `pool-union`, `pool-inter`, `pool-diff`
- **Ladders**: `lad-add`, `lad-drop`, `lad-score`, `lad-rank`, `lad-range`, `lad-count`, `lad-pop-min`, `lad-pop-max`
- **Bits**: `bit-set`, `bit-get`, `bit-count`, `bit-op`
- **Streams (Stretch)**: `stream-add`, `stream-read`, `stream-len`

### Phase 4: Durability (Weeks 27-34)
- `snapshot`

### Phase 5: Transactions, Messaging, and Replication (Weeks 35-42)
- **Replication**: `follow`, `promote`, `repl-status`, `wait-for`
- **Transactions**: `begin`, `commit`, `discard`, `watch`
- **Messaging (Pub/Sub)**: `subscribe`, `unsubscribe`, `publish`

### Phase 6: Security, Scale, Hardening, and Release (Weeks 43-52)
- Advanced security policies and multi-tenant authentication.
