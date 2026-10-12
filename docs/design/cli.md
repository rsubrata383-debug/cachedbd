# Command-Line Interface (`skiff-cli`) Design

## 1. Architectural Overview

`skiff-cli` is the official command-line client utility for SkiffDB. It provides two operational modes:
1. **One-shot Mode**: Executes a single command passed via positional command-line arguments, writes the formatted result, and immediately terminates.
2. **Interactive REPL Mode**: Enters an interactive line-based read-eval-print loop with human-friendly output formatting, connection banner, and built-in control commands.

The CLI is implemented strictly using standard library facilities (`std::io`, `std::net`, `std::env`, `std::process`) without external dependencies.

---

## 2. Command-Line Options & Usage

```text
skiff-cli [OPTIONS] [COMMAND [ARGS...]]

A command-line interface for SkiffDB.

Options:
  --host <addr>    Server host address (default: 127.0.0.1)
  --port <port>    Server port number (default: 7878)
  -h, --help       Print this help message
  -V, --version    Print version information
```

### Exit Codes
- **`0`**: Success (command succeeded, `--help`/`--version` rendered, or REPL exited cleanly via `exit`/`quit`/EOF).
- **`1`**: Command execution failure (server returned a structured error response, e.g. `ERR_UNKNOWN_COMMAND` (2001) or `ERR_NOT_AN_INTEGER` (3004), during one-shot invocation).
- **`2`**: Usage or connection failure (unknown option flags, malformed port number, network connect failure, unexpected socket EOF/reset).

---

## 3. Lexical Line & Literal Parsing

Input lines are parsed by `parse_line(&str) -> Result<Vec<Token>, CliError>`:
- **Whitespace delimitation**: Separates tokens on `' '`, `'\t'`, `'\r'`, `'\n'`.
- **Double quotation**: Quoted strings (`"hello world"`) are treated as a single token with `is_quoted: true`.
- **Backslash escapes** (supported both within and outside quotes):
  - `\"` -> Literal double quote (`"`)
  - `\\` -> Literal backslash (`\`)
  - `\n` -> Newline
  - `\t` -> Tab
  - `\xHH` -> 8-bit byte matching the two hexadecimal digits `HH`.
- Unterminated quotes, trailing backslashes, or invalid escape sequences return `CliError::Parse`.

### Command Typing (`build_command`)
Tokens are converted into wire-protocol `Value` items:
1. **Verb**: Position 0 is always serialized as `Value::Text`.
2. **Key Positions**: For known key commands (`put`, `fetch`, `drop`, `has`, `bump`), argument at index 1 is forced to `Value::Text` to preserve textual key semantics regardless of numeric appearance (e.g. `put 123 val`).
3. **Quoted Tokens**: Explicitly quoted tokens are preserved as `Value::Text`.
4. **Unquoted Literals**:
   - Strict `i64` integer matching `bump` syntax (optional leading `+` or `-`, no leading zeros except `0`, no `-0`/`+0`) -> `Value::Int`.
   - `"true"` / `"false"` -> `Value::Bool`.
   - `"nil"` -> `Value::Nil`.
   - Finite decimal or scientific exponential notation containing `.` or `e`/`E` -> `Value::Float`.
   - Prefix `"hex:"` followed by an even count of hex characters -> `Value::Bytes`.
   - Fallback -> `Value::Text`.

---

## 4. Human-Readable Result Formatting (`format_value`)

Matching standard Redis-cli conventions:
- **Nil**: `(nil)`
- **Integer**: `(integer) <n>`
- **Float**: `(float) <f>`
- **Boolean**: `(bool) true` / `(bool) false`
- **Text**: Double-quoted with escape sequences: `"hello \"world\"\n"`
- **Bytes**: Valid UTF-8 formatted as text (`"hello"`), non-UTF-8 formatted as `hex:010203...`
- **Array**: Numbered 1-indexed list:
  ```text
  1) "first"
  2) (integer) 2
  3) (nil)
  ```
  Nested arrays indent child items by 3 spaces per level, aligning with the parent index:
  ```text
  1) "first"
  2) 1) "sub1"
     2) "sub2"
  3) (nil)
  ```
- **Map**: Key-value mappings rendered line-by-line:
  ```text
  "key1" => (integer) 1
  "key2" => "val2"
  ```
- **Error**: `(error <CODE>) <MESSAGE>`
- **Safety Truncation**: Text or Bytes values exceeding 1 MB (1,048,576 bytes) are truncated to the first 1 KB (1,024 bytes) followed by `... (<N> bytes total)`.

---

## 5. REPL Behavior & Terminal Integration

- **Interactive Detection**: Prompt (`skiff> `) and banner (`connected to <name> <version> at <addr>`) are displayed only when standard input is an interactive terminal (`std::io::IsTerminal`).
- **Piped Inputs**: Automated pipelines or redirected standard input bypass banners and prompts, writing only results to `stdout` and errors to `stderr`.
- **Built-in Commands**:
  - `help`: Emits detailed command usage and flag syntax locally.
  - `exit` / `quit`: Sends a `quit` frame to the server (best effort) and breaks out of the loop with exit code 0.
- **Fault Tolerance**: Syntax errors and server errors (`ClientError::Server`) log error diagnostics to `stderr` and continue the session loop. Transport connection drops immediately break the loop with exit code 2.

---

## 6. Deferred Features (Phase 2)

- Advanced line-editing, history navigation, and tab-completion via `rustyline`.
- ANSI syntax highlighting and colored output for terminals with color support.
- Multi-server connection bookmarks and TLS transport negotiation.

