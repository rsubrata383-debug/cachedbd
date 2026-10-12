//! Command-line interface and parsing utilities for `skiff-cli`.

use std::fmt;
use std::io::{BufRead, Write};

use crate::client::{Client, ClientError};
use crate::proto::Value;

/// Standard help message displayed for `skiff-cli --help` or the `help` builtin.
pub const HELP_TEXT: &str = "\
skiff-cli [OPTIONS] [COMMAND [ARGS...]]

A command-line interface for SkiffDB.

Options:
  --host <addr>    Server host address (default: 127.0.0.1)
  --port <port>    Server port number (default: 7878)
  -h, --help       Print this help message
  -V, --version    Print version information

Commands:
  ping [msg]       Test connection
  put <key> <val>  Store a key-value pair
  fetch <key>      Retrieve value for a key
  drop <key...>    Delete one or more keys
  has <key>        Check if a key exists
  bump <key> [amt] Increment integer key by step
  size             Get key count
  quit             Close connection and exit";

/// A parsed token with its unescaped string contents and quotation flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The parsed and unescaped textual content of the token.
    pub text: String,
    /// Whether the token was enclosed in double quotes in the input string.
    pub is_quoted: bool,
}

/// Errors encountered during CLI argument parsing, input evaluation, or network transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// Syntax error during line parsing or value literal construction.
    Parse(String),
    /// Invalid CLI flags or usage invocation.
    Usage(String),
    /// Network transport or socket connectivity failure.
    Connection(String),
    /// Structured application error returned by the server.
    Server {
        /// Numeric error code.
        code: u16,
        /// Human-readable error message.
        message: String,
    },
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::Usage(msg) => write!(f, "usage error: {msg}"),
            Self::Connection(msg) => write!(f, "connection error: {msg}"),
            Self::Server { code, message } => write!(f, "(error {code}) {message}"),
        }
    }
}

impl std::error::Error for CliError {}

/// Configuration options for invoking the `skiff-cli` utility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    /// Host address to connect to (default: "127.0.0.1").
    pub host: String,
    /// Port number to connect to (default: 7878).
    pub port: u16,
    /// Positional command arguments if running in one-shot mode.
    /// If empty, interactive REPL mode is selected.
    pub command: Vec<String>,
    /// Whether `--help` or `-h` was requested.
    pub show_help: bool,
    /// Whether `--version` or `-V` was requested.
    pub show_version: bool,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 7878,
            command: Vec::new(),
            show_help: false,
            show_version: false,
        }
    }
}

/// Parses CLI command-line arguments into a [`CliConfig`].
///
/// # Errors
///
/// Returns [`CliError::Usage`] if unknown flags are passed or if required flag
/// values (such as `--host` or `--port`) are missing or malformed.
pub fn parse_args(args: &[String]) -> Result<CliConfig, CliError> {
    let mut config = CliConfig::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            config.show_help = true;
            i += 1;
        } else if arg == "-V" || arg == "--version" {
            config.show_version = true;
            i += 1;
        } else if arg == "--host" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Usage("missing value for --host".to_string()));
            }
            config.host.clone_from(&args[i]);
            i += 1;
        } else if arg == "--port" {
            i += 1;
            if i >= args.len() {
                return Err(CliError::Usage("missing value for --port".to_string()));
            }
            let p = args[i]
                .parse::<u16>()
                .map_err(|_| CliError::Usage(format!("invalid port number: '{}'", args[i])))?;
            if p == 0 {
                return Err(CliError::Usage("port number must not be 0".to_string()));
            }
            config.port = p;
            i += 1;
        } else if arg.starts_with('-') {
            return Err(CliError::Usage(format!("unknown option: '{arg}'")));
        } else {
            config.command.extend_from_slice(&args[i..]);
            break;
        }
    }
    Ok(config)
}

fn parse_escape_char(
    chars: &[char],
    i: &mut usize,
    byte_buf: &mut Vec<u8>,
) -> Result<(), CliError> {
    *i += 1;
    if *i >= chars.len() {
        return Err(CliError::Parse(
            "unexpected end of line after backslash".to_string(),
        ));
    }
    match chars[*i] {
        '"' => byte_buf.push(b'"'),
        '\\' => byte_buf.push(b'\\'),
        'n' => byte_buf.push(b'\n'),
        't' => byte_buf.push(b'\t'),
        'x' => {
            if *i + 2 >= chars.len() {
                return Err(CliError::Parse(
                    "invalid hex escape: expected 2 hex digits".to_string(),
                ));
            }
            let h1 = chars[*i + 1];
            let h2 = chars[*i + 2];
            if !h1.is_ascii_hexdigit() || !h2.is_ascii_hexdigit() {
                return Err(CliError::Parse(format!("invalid hex escape: \\x{h1}{h2}")));
            }
            let d1 = h1.to_digit(16).unwrap_or(0);
            let d2 = h2.to_digit(16).unwrap_or(0);
            byte_buf.push(((d1 << 4) | d2) as u8);
            *i += 2;
        }
        other => {
            return Err(CliError::Parse(format!(
                "unknown escape sequence: \\{other}"
            )));
        }
    }
    Ok(())
}

fn parse_quoted_token(chars: &[char], start: usize) -> Result<(Token, usize), CliError> {
    let mut i = start + 1;
    let mut byte_buf = Vec::new();
    let mut closed = false;

    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            closed = true;
            i += 1;
            break;
        }
        if c == '\\' {
            parse_escape_char(chars, &mut i, &mut byte_buf)?;
        } else {
            let mut char_buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut char_buf);
            byte_buf.extend_from_slice(encoded.as_bytes());
        }
        i += 1;
    }

    if !closed {
        return Err(CliError::Parse("unterminated quote".to_string()));
    }

    if i < chars.len() && !chars[i].is_ascii_whitespace() {
        return Err(CliError::Parse(format!(
            "unexpected character '{}' following closing quote",
            chars[i]
        )));
    }

    let text = String::from_utf8(byte_buf)
        .map_err(|e| CliError::Parse(format!("invalid UTF-8 in quoted token: {e}")))?;
    Ok((
        Token {
            text,
            is_quoted: true,
        },
        i,
    ))
}

fn parse_unquoted_token(chars: &[char], start: usize) -> Result<(Token, usize), CliError> {
    let mut i = start;
    let mut byte_buf = Vec::new();

    while i < chars.len() && !chars[i].is_ascii_whitespace() {
        let c = chars[i];
        if c == '"' {
            return Err(CliError::Parse(
                "unquoted token cannot contain unescaped quote; use \\\"".to_string(),
            ));
        }
        if c == '\\' {
            parse_escape_char(chars, &mut i, &mut byte_buf)?;
        } else {
            let mut char_buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut char_buf);
            byte_buf.extend_from_slice(encoded.as_bytes());
        }
        i += 1;
    }

    let text = String::from_utf8(byte_buf)
        .map_err(|e| CliError::Parse(format!("invalid UTF-8 in token: {e}")))?;
    Ok((
        Token {
            text,
            is_quoted: false,
        },
        i,
    ))
}

/// Parses a command line string into a sequence of [`Token`]s.
///
/// Splits on whitespace (`' '`, `'\t'`, `'\r'`, `'\n'`). Respects double quotes
/// and supports standard backslash escapes (`\"`, `\\`, `\n`, `\t`, `\xHH`).
///
/// # Errors
///
/// Returns [`CliError::Parse`] if quotes are unterminated, escape sequences are invalid,
/// or non-UTF-8 bytes are generated.
pub fn parse_line(line: &str) -> Result<Vec<Token>, CliError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        let (token, next_i) = if chars[i] == '"' {
            parse_quoted_token(&chars, i)?
        } else {
            parse_unquoted_token(&chars, i)?
        };
        tokens.push(token);
        i = next_i;
    }

    Ok(tokens)
}

/// Parses a byte slice as a strict base-10 64-bit signed integer with optional leading `+` or `-`.
fn parse_cli_i64(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let (is_negative, digits) = if bytes[0] == b'-' {
        (true, &bytes[1..])
    } else if bytes[0] == b'+' {
        (false, &bytes[1..])
    } else {
        (false, bytes)
    };

    if digits.is_empty() {
        return None;
    }

    // No leading zeros except "0" itself, and "-0" or "+0" is prohibited
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    if (bytes[0] == b'-' || bytes[0] == b'+') && digits == b"0" {
        return None;
    }

    let mut acc: i64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        let digit = i64::from(b - b'0');
        if is_negative {
            acc = acc.checked_mul(10)?.checked_sub(digit)?;
        } else {
            acc = acc.checked_mul(10)?.checked_add(digit)?;
        }
    }

    Some(acc)
}

/// Converts a slice of [`Token`]s into protocol [`Value`] items suitable for execution.
///
/// # Rules
///
/// - The first token (command verb) is always [`Value::Text`].
/// - For commands whose key positions are known (`put`, `fetch`, `has`, `bump` at pos 1;
///   `drop` at all positions >= 1), the key arguments are always [`Value::Text`].
/// - Quoted tokens are always [`Value::Text`].
/// - Unquoted tokens are typed according to:
///   1. Strict signed integer (`i64`) -> [`Value::Int`].
///   2. `"true"` / `"false"` -> [`Value::Bool`].
///   3. `"nil"` -> [`Value::Nil`].
///   4. Decimal or exponential float (`f64`) -> [`Value::Float`].
///   5. Starts with `"hex:"` with even hex digits -> [`Value::Bytes`].
///   6. Fallback -> [`Value::Text`].
///
/// # Errors
///
/// Returns [`CliError::Parse`] if the tokens slice is empty or contains an invalid hex literal.
pub fn build_command(tokens: &[Token]) -> Result<Vec<Value>, CliError> {
    if tokens.is_empty() {
        return Err(CliError::Parse("empty command".to_string()));
    }

    let verb = tokens[0].text.to_ascii_lowercase();
    let is_drop = verb == "drop";
    let is_single_key = matches!(verb.as_str(), "put" | "fetch" | "has" | "bump");

    let mut values = Vec::with_capacity(tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        let is_key_arg = (is_drop && i >= 1) || (is_single_key && i == 1);
        if i == 0 || is_key_arg || token.is_quoted {
            values.push(Value::Text(token.text.clone()));
            continue;
        }

        // Unquoted token parsing
        if let Some(n) = parse_cli_i64(&token.text) {
            values.push(Value::Int(n));
        } else if token.text == "true" {
            values.push(Value::Bool(true));
        } else if token.text == "false" {
            values.push(Value::Bool(false));
        } else if token.text == "nil" {
            values.push(Value::Nil);
        } else if (token.text.contains('.') || token.text.contains('e') || token.text.contains('E'))
            && token.text.parse::<f64>().is_ok_and(f64::is_finite)
        {
            let f = token.text.parse::<f64>().unwrap_or(0.0);
            values.push(Value::Float(f));
        } else if let Some(hex_part) = token.text.strip_prefix("hex:") {
            if hex_part.len() % 2 != 0 {
                return Err(CliError::Parse(format!(
                    "invalid hex literal: odd length in '{}'",
                    token.text
                )));
            }
            let mut bytes = Vec::with_capacity(hex_part.len() / 2);
            for chunk in hex_part.as_bytes().chunks_exact(2) {
                let s = std::str::from_utf8(chunk)
                    .map_err(|_| CliError::Parse("invalid hex literal".to_string()))?;
                let b = u8::from_str_radix(s, 16).map_err(|_| {
                    CliError::Parse(format!(
                        "invalid hex literal: non-hex character in '{}'",
                        token.text
                    ))
                })?;
                bytes.push(b);
            }
            values.push(Value::Bytes(bytes));
        } else {
            values.push(Value::Text(token.text.clone()));
        }
    }

    Ok(values)
}

fn escape_str_content(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(&mut out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

const MAX_DISPLAY_BYTES: usize = 1024 * 1024;
const TRUNCATE_PREFIX_BYTES: usize = 1024;

/// Formats a protocol [`Value`] for human-readable output matching Redis-cli style.
#[must_use]
pub fn format_value(v: &Value) -> String {
    match v {
        Value::Nil => "(nil)".to_string(),
        Value::Int(n) => format!("(integer) {n}"),
        Value::Float(f) => format!("(float) {f}"),
        Value::Bool(b) => format!("(bool) {b}"),
        Value::Text(s) => {
            if s.len() > MAX_DISPLAY_BYTES {
                let mut cut = TRUNCATE_PREFIX_BYTES;
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                let snippet = &s[..cut];
                format!(
                    "\"{}\"... ({} bytes total)",
                    escape_str_content(snippet),
                    s.len()
                )
            } else {
                format!("\"{}\"", escape_str_content(s))
            }
        }
        Value::Bytes(b) => {
            if b.len() > MAX_DISPLAY_BYTES {
                let snippet = &b[..TRUNCATE_PREFIX_BYTES];
                if let Ok(s) = std::str::from_utf8(snippet) {
                    format!("\"{}\"... ({} bytes total)", escape_str_content(s), b.len())
                } else {
                    let mut hex = String::from("hex:");
                    for byte in snippet {
                        use std::fmt::Write as _;
                        let _ = write!(&mut hex, "{byte:02x}");
                    }
                    format!("{hex}... ({} bytes total)", b.len())
                }
            } else if let Ok(s) = std::str::from_utf8(b) {
                format!("\"{}\"", escape_str_content(s))
            } else {
                let mut hex = String::from("hex:");
                for byte in b {
                    use std::fmt::Write as _;
                    let _ = write!(&mut hex, "{byte:02x}");
                }
                hex
            }
        }
        Value::Array(items) => format_array_at_depth(items, 0),
        Value::Map(entries) => {
            if entries.is_empty() {
                "(empty map)".to_string()
            } else {
                let mut lines = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    lines.push(format!("{} => {}", format_value(k), format_value(v)));
                }
                lines.join("\n")
            }
        }
        Value::Error { code, message } => format!("(error {code}) {message}"),
    }
}

fn format_array_at_depth(items: &[Value], depth: usize) -> String {
    if items.is_empty() {
        return "(empty array)".to_string();
    }

    let mut lines = Vec::new();
    let indent = "   ".repeat(depth);

    for (idx, item) in items.iter().enumerate() {
        let num = idx + 1;
        match item {
            Value::Array(nested) => {
                if nested.is_empty() {
                    lines.push(format!("{indent}{num}) (empty array)"));
                } else {
                    let nested_formatted = format_array_at_depth(nested, depth + 1);
                    let expected_nested_indent = "   ".repeat(depth + 1);
                    let mut nested_lines = nested_formatted.lines();
                    if let Some(first_line) = nested_lines.next() {
                        let stripped = first_line
                            .strip_prefix(&expected_nested_indent)
                            .unwrap_or(first_line);
                        lines.push(format!("{indent}{num}) {stripped}"));
                    }
                    for following_line in nested_lines {
                        lines.push(following_line.to_string());
                    }
                }
            }
            other => {
                let formatted = format_value(other);
                lines.push(format!("{indent}{num}) {formatted}"));
            }
        }
    }

    lines.join("\n")
}

fn run_one_shot<W: Write, E: Write>(
    mut client: Client,
    command_args: Vec<String>,
    stdout: &mut W,
    stderr: &mut E,
) -> i32 {
    let tokens: Vec<Token> = if command_args.len() == 1 {
        match parse_line(&command_args[0]) {
            Ok(t) => t,
            Err(e) => {
                let _ = writeln!(stderr, "{e}");
                return 2;
            }
        }
    } else {
        command_args
            .into_iter()
            .map(|s| {
                if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                    Token {
                        text: s[1..s.len() - 1].to_string(),
                        is_quoted: true,
                    }
                } else {
                    Token {
                        text: s,
                        is_quoted: false,
                    }
                }
            })
            .collect()
    };

    let values = match build_command(&tokens) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(stderr, "{e}");
            return 2;
        }
    };

    match client.command(values) {
        Ok(reply) => {
            let _ = writeln!(stdout, "{}", format_value(&reply));
            0
        }
        Err(ClientError::Server { code, message }) => {
            let _ = writeln!(stderr, "(error {code}) {message}");
            1
        }
        Err(e) => {
            let _ = writeln!(stderr, "connection error: {e}");
            2
        }
    }
}

fn run_repl<R: BufRead, W: Write, E: Write>(
    mut client: Client,
    addr: &str,
    mut stdin: R,
    stdout: &mut W,
    stderr: &mut E,
    is_terminal: bool,
) -> i32 {
    if is_terminal {
        let info = client.server_info();
        let _ = writeln!(
            stdout,
            "connected to {} {} at {}",
            info.server_name, info.server_version, addr
        );
    }

    loop {
        if is_terminal {
            let _ = write!(stdout, "skiff> ");
            let _ = stdout.flush();
        }

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                let _ = writeln!(stderr, "read error: {e}");
                return 2;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let tokens = match parse_line(&line) {
            Ok(t) => t,
            Err(e) => {
                let _ = writeln!(stderr, "{e}");
                continue;
            }
        };

        if tokens.is_empty() {
            continue;
        }

        if tokens.len() == 1 {
            if tokens[0].text.eq_ignore_ascii_case("help") {
                let _ = writeln!(stdout, "{HELP_TEXT}");
                continue;
            }
            if tokens[0].text.eq_ignore_ascii_case("exit")
                || tokens[0].text.eq_ignore_ascii_case("quit")
            {
                let _ = client.quit();
                break;
            }
        }

        let values = match build_command(&tokens) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(stderr, "{e}");
                continue;
            }
        };

        match client.command(values) {
            Ok(reply) => {
                let _ = writeln!(stdout, "{}", format_value(&reply));
            }
            Err(ClientError::Server { code, message }) => {
                let _ = writeln!(stderr, "(error {code}) {message}");
            }
            Err(_) => {
                let _ = writeln!(stderr, "connection closed by server");
                return 2;
            }
        }
    }

    0
}

/// Runs the CLI with given configuration and I/O streams.
///
/// # Returns
///
/// - `0`: success (one-shot command succeeded, or REPL exited cleanly).
/// - `1`: server returned a command error to a one-shot command.
/// - `2`: usage error, connection error, or unexpected disconnection.
pub fn run<R: BufRead, W: Write, E: Write>(
    config: CliConfig,
    stdin: R,
    stdout: &mut W,
    stderr: &mut E,
    is_terminal: bool,
) -> i32 {
    let addr = format!("{}:{}", config.host, config.port);
    let client = match Client::connect(&addr) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(stderr, "connection error: {e}");
            return 2;
        }
    };

    if config.command.is_empty() {
        run_repl(client, &addr, stdin, stdout, stderr, is_terminal)
    } else {
        run_one_shot(client, config.command, stdout, stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_defaults_and_options() {
        let args = vec![];
        let cfg = parse_args(&args).expect("default args");
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 7878);
        assert!(!cfg.show_help);
        assert!(!cfg.show_version);
        assert_eq!(cfg.command, Vec::<String>::new());

        let args = vec![
            "--host".to_string(),
            "10.0.0.1".to_string(),
            "--port".to_string(),
            "9000".to_string(),
            "put".to_string(),
            "k".to_string(),
            "v".to_string(),
        ];
        let cfg = parse_args(&args).expect("custom args");
        assert_eq!(cfg.host, "10.0.0.1");
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.command, vec!["put", "k", "v"]);

        assert!(parse_args(&["--unknown".to_string()]).is_err());
        assert!(parse_args(&["--port".to_string(), "abc".to_string()]).is_err());
        assert!(parse_args(&["--port".to_string(), "0".to_string()]).is_err());
        assert!(parse_args(&["--port".to_string()]).is_err());
        assert!(parse_args(&["--host".to_string()]).is_err());
    }

    #[test]
    fn test_parse_line_simple_and_escapes() {
        let tokens = parse_line("put foo bar").expect("simple");
        assert_eq!(
            tokens,
            vec![
                Token {
                    text: "put".to_string(),
                    is_quoted: false
                },
                Token {
                    text: "foo".to_string(),
                    is_quoted: false
                },
                Token {
                    text: "bar".to_string(),
                    is_quoted: false
                },
            ]
        );

        let tokens = parse_line("put \"hello world\" 123").expect("quotes");
        assert_eq!(
            tokens,
            vec![
                Token {
                    text: "put".to_string(),
                    is_quoted: false
                },
                Token {
                    text: "hello world".to_string(),
                    is_quoted: true
                },
                Token {
                    text: "123".to_string(),
                    is_quoted: false
                },
            ]
        );

        let tokens = parse_line(r#"put "\"quoted\"" test\\esc\n\t\x41"#).expect("escapes");
        assert_eq!(
            tokens,
            vec![
                Token {
                    text: "put".to_string(),
                    is_quoted: false
                },
                Token {
                    text: "\"quoted\"".to_string(),
                    is_quoted: true
                },
                Token {
                    text: "test\\esc\n\tA".to_string(),
                    is_quoted: false
                },
            ]
        );

        assert_eq!(parse_line("").expect("empty"), vec![]);
        assert_eq!(parse_line("   \t  \r \n  ").expect("whitespace"), vec![]);
        assert!(parse_line(r#""unterminated"#).is_err());
        assert!(parse_line(r"bad\escape").is_err());
        assert!(parse_line(r"\x1").is_err());
        assert!(parse_line(r"\xzz").is_err());
        assert!(parse_line(r"trailing\").is_err());
    }

    #[test]
    fn test_build_command_types() {
        let tokens = vec![
            Token {
                text: "put".to_string(),
                is_quoted: false,
            },
            Token {
                text: "123".to_string(),
                is_quoted: false,
            }, // key pos for put -> Text
            Token {
                text: "456".to_string(),
                is_quoted: false,
            }, // Int
            Token {
                text: "true".to_string(),
                is_quoted: false,
            }, // Bool
            Token {
                text: "nil".to_string(),
                is_quoted: false,
            }, // Nil
            Token {
                text: "12.34".to_string(),
                is_quoted: false,
            }, // Float
            Token {
                text: "hex:01020aff".to_string(),
                is_quoted: false,
            }, // Bytes
            Token {
                text: "789".to_string(),
                is_quoted: true,
            }, // Quoted -> Text
        ];
        let values = build_command(&tokens).expect("build");
        assert_eq!(values[0], Value::Text("put".to_string()));
        assert_eq!(values[1], Value::Text("123".to_string()));
        assert_eq!(values[2], Value::Int(456));
        assert_eq!(values[3], Value::Bool(true));
        assert_eq!(values[4], Value::Nil);
        assert_eq!(values[5], Value::Float(12.34));
        assert_eq!(values[6], Value::Bytes(vec![1, 2, 10, 255]));
        assert_eq!(values[7], Value::Text("789".to_string()));

        // Hex error check
        let bad_hex = vec![
            Token {
                text: "put".to_string(),
                is_quoted: false,
            },
            Token {
                text: "k".to_string(),
                is_quoted: false,
            },
            Token {
                text: "hex:123".to_string(),
                is_quoted: false,
            },
        ];
        assert!(build_command(&bad_hex).is_err());

        // Empty tokens
        assert!(build_command(&[]).is_err());
    }

    #[test]
    fn test_build_command_keys() {
        // drop: all arguments after verb are Text keys (even unquoted numbers)
        let drop_tokens = vec![
            Token {
                text: "drop".to_string(),
                is_quoted: false,
            },
            Token {
                text: "123".to_string(),
                is_quoted: false,
            },
            Token {
                text: "456".to_string(),
                is_quoted: false,
            },
        ];
        let drop_vals = build_command(&drop_tokens).expect("build drop");
        assert_eq!(
            drop_vals,
            vec![
                Value::Text("drop".to_string()),
                Value::Text("123".to_string()),
                Value::Text("456".to_string()),
            ]
        );

        // put: only pos 1 is key (Text); pos 2 is value (Int 456)
        let put_tokens = vec![
            Token {
                text: "put".to_string(),
                is_quoted: false,
            },
            Token {
                text: "123".to_string(),
                is_quoted: false,
            },
            Token {
                text: "456".to_string(),
                is_quoted: false,
            },
        ];
        let put_vals = build_command(&put_tokens).expect("build put");
        assert_eq!(
            put_vals,
            vec![
                Value::Text("put".to_string()),
                Value::Text("123".to_string()),
                Value::Int(456),
            ]
        );
    }

    #[test]
    fn test_format_value() {
        assert_eq!(format_value(&Value::Nil), "(nil)");
        assert_eq!(format_value(&Value::Int(42)), "(integer) 42");
        assert_eq!(format_value(&Value::Float(3.5)), "(float) 3.5");
        assert_eq!(format_value(&Value::Bool(true)), "(bool) true");
        assert_eq!(format_value(&Value::Bool(false)), "(bool) false");
        assert_eq!(
            format_value(&Value::Text("hello \"world\"\n".to_string())),
            "\"hello \\\"world\\\"\\n\""
        );
        assert_eq!(format_value(&Value::Bytes(b"hello".to_vec())), "\"hello\"");
        assert_eq!(
            format_value(&Value::Bytes(vec![0xff, 0x00, 0xca])),
            "hex:ff00ca"
        );
        assert_eq!(
            format_value(&Value::Error {
                code: 2001,
                message: "unknown command".to_string(),
            }),
            "(error 2001) unknown command"
        );

        let arr = Value::Array(vec![
            Value::Text("first".to_string()),
            Value::Int(2),
            Value::Nil,
        ]);
        assert_eq!(format_value(&arr), "1) \"first\"\n2) (integer) 2\n3) (nil)");

        let nested_arr = Value::Array(vec![
            Value::Text("first".to_string()),
            Value::Array(vec![
                Value::Text("sub1".to_string()),
                Value::Text("sub2".to_string()),
            ]),
            Value::Nil,
        ]);
        assert_eq!(
            format_value(&nested_arr),
            "1) \"first\"\n2) 1) \"sub1\"\n   2) \"sub2\"\n3) (nil)"
        );

        let map = Value::Map(vec![
            (Value::Text("k1".to_string()), Value::Int(1)),
            (Value::Text("k2".to_string()), Value::Text("v2".to_string())),
        ]);
        assert_eq!(
            format_value(&map),
            "\"k1\" => (integer) 1\n\"k2\" => \"v2\""
        );

        // Truncation check
        let huge_text = "a".repeat(1024 * 1024 + 10);
        let formatted = format_value(&Value::Text(huge_text));
        assert!(formatted.contains("... (1048586 bytes total)"));
        assert!(formatted.starts_with("\"aaaa"));
    }

    /// Linear congruential / XorShift PRNG strictly following AGENTS.md §11.1
    struct TestPrng {
        state: u64,
    }

    impl TestPrng {
        fn new(seed: u64) -> Self {
            assert!(seed != 0, "Seed must not be zero per AGENTS.md §11.1");
            Self { state: seed }
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state = x;
            x
        }

        fn next_char(&mut self) -> char {
            const CANDIDATES: &[u8] = b" abcdef0123456789\\\"ntx\r\n\t;:/?[]{}+-";
            let diff = CANDIDATES.len() as u64;
            let idx = (self.next_u64() % diff) as usize;
            CANDIDATES[idx] as char
        }
    }

    #[test]
    fn test_parse_line_fuzz_20k() {
        let mut prng = TestPrng::new(0xDEAD_BEEF_CAFE_BABE);
        for _ in 0..20_000 {
            let len = (prng.next_u64() % 40) as usize;
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                s.push(prng.next_char());
            }
            // Must never panic, either Ok or Err
            let _ = parse_line(&s);
        }
    }
}
