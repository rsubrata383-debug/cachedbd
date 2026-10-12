//! Synchronous client library for SkiffDB.
//!
//! Provides a zero-dependency, blocking client backed by [`std::net::TcpStream`] that
//! works identically across Linux and Windows.
//!
//! # Examples
//!
//! ```no_run
//! use skiffdb::client::Client;
//! use skiffdb::proto::Value;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = Client::connect("127.0.0.1:4222")?;
//!
//! // Check connection
//! assert_eq!(client.ping()?, "pong");
//!
//! // Store and retrieve values
//! client.put(b"greeting", Value::Text("hello world".to_string()))?;
//! let value = client.fetch(b"greeting")?;
//! assert_eq!(value, Some(Value::Text("hello world".to_string())));
//!
//! // Clean up
//! client.quit()?;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::proto::decode::decode_frame_with_limit;
use crate::proto::encode::encode_frame;
use crate::proto::header::{
    DEFAULT_MAX_PAYLOAD_LEN, FLAG_ERROR, FLAG_RESPONSE, HEADER_LEN, Header, Opcode, ProtoError,
};
use crate::proto::hello::{HelloRequest, HelloResponse};
use crate::proto::{Decoded, Frame, Value};

/// Configuration parameters for [`Client`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// Maximum time allowed to establish a TCP connection.
    pub connect_timeout: Option<Duration>,
    /// Socket read timeout.
    pub read_timeout: Option<Duration>,
    /// Socket write timeout.
    pub write_timeout: Option<Duration>,
    /// Identifying name sent during the `HELLO` handshake.
    pub client_name: String,
    /// Maximum allowed reply payload size in bytes (default 64 MB).
    pub max_reply_size: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: None,
            read_timeout: None,
            write_timeout: None,
            client_name: format!("skiff-client/{}", env!("CARGO_PKG_VERSION")),
            max_reply_size: DEFAULT_MAX_PAYLOAD_LEN as usize,
        }
    }
}

impl ClientConfig {
    /// Creates a new `ClientConfig` with default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the connection timeout.
    #[must_use]
    pub const fn with_connect_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the socket read timeout.
    #[must_use]
    pub const fn with_read_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Sets the socket write timeout.
    #[must_use]
    pub const fn with_write_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Sets the client identifying name.
    #[must_use]
    pub fn with_client_name(mut self, name: impl Into<String>) -> Self {
        self.client_name = name.into();
        self
    }

    /// Sets the maximum allowed reply payload size in bytes.
    #[must_use]
    pub const fn with_max_reply_size(mut self, max_size: usize) -> Self {
        self.max_reply_size = max_size;
        self
    }
}

/// Metadata and limits negotiated with the server during handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// Server implementation name (e.g. `"skiffd"`).
    pub server_name: String,
    /// Server software version (e.g. `"0.1.0"`).
    pub server_version: String,
    /// Unique connection identifier assigned to this client.
    pub client_id: i64,
    /// Maximum allowed key size in bytes, if advertised.
    pub max_key_size: Option<i64>,
    /// Maximum allowed value size in bytes, if advertised.
    pub max_value_size: Option<i64>,
    /// Maximum allowed frame size in bytes, if advertised.
    pub max_frame_size: Option<i64>,
}

impl ServerInfo {
    /// Returns the server implementation name.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Returns the server software version.
    #[must_use]
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// Returns the connection client ID.
    #[must_use]
    pub const fn client_id(&self) -> i64 {
        self.client_id
    }

    /// Returns the maximum allowed key size in bytes.
    #[must_use]
    pub const fn max_key_size(&self) -> Option<i64> {
        self.max_key_size
    }

    /// Returns the maximum allowed value size in bytes.
    #[must_use]
    pub const fn max_value_size(&self) -> Option<i64> {
        self.max_value_size
    }

    /// Returns the maximum allowed frame size in bytes.
    #[must_use]
    pub const fn max_frame_size(&self) -> Option<i64> {
        self.max_frame_size
    }
}

/// Errors returned by [`Client`] operations.
#[derive(Debug)]
pub enum ClientError {
    /// Network I/O failure on underlying TCP stream.
    Io(io::Error),
    /// Protocol framing violation, bad header, wrong request ID, or unexpected flags.
    Protocol(String),
    /// Server returned an error reply frame (`FLAG_ERROR`).
    Server {
        /// Numerical error code returned by the server.
        code: u16,
        /// Human-readable error message.
        message: String,
    },
    /// Server returned an unexpected reply type for a typed helper.
    UnexpectedReply(String),
    /// Connection was closed by the peer or broken by a prior fatal error.
    ConnectionClosed,
    /// Server frame exceeded configured `max_reply_size`.
    ReplyTooLarge,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "i/o error: {err}"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Server { code, message } => write!(f, "server error {code}: {message}"),
            Self::UnexpectedReply(msg) => write!(f, "unexpected reply: {msg}"),
            Self::ConnectionClosed => write!(f, "connection closed"),
            Self::ReplyTooLarge => write!(f, "reply too large"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl PartialEq for ClientError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind(),
            (Self::Protocol(a), Self::Protocol(b))
            | (Self::UnexpectedReply(a), Self::UnexpectedReply(b)) => a == b,
            (
                Self::Server {
                    code: c1,
                    message: m1,
                },
                Self::Server {
                    code: c2,
                    message: m2,
                },
            ) => c1 == c2 && m1 == m2,
            (Self::ConnectionClosed, Self::ConnectionClosed)
            | (Self::ReplyTooLarge, Self::ReplyTooLarge) => true,
            _ => false,
        }
    }
}

fn map_io_error(err: io::Error) -> ClientError {
    if matches!(
        err.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    ) {
        ClientError::ConnectionClosed
    } else {
        ClientError::Io(err)
    }
}

fn connect_stream(
    addr: impl ToSocketAddrs,
    config: &ClientConfig,
) -> Result<TcpStream, ClientError> {
    let addrs = addr.to_socket_addrs().map_err(ClientError::Io)?;
    let mut stream = None;
    let mut last_err = None;

    for sa in addrs {
        let res = if let Some(timeout) = config.connect_timeout {
            TcpStream::connect_timeout(&sa, timeout)
        } else {
            TcpStream::connect(sa)
        };

        match res {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    let Some(stream) = stream else {
        return Err(ClientError::Io(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no socket addresses resolved",
            )
        })));
    };

    stream
        .set_read_timeout(config.read_timeout)
        .map_err(ClientError::Io)?;
    stream
        .set_write_timeout(config.write_timeout)
        .map_err(ClientError::Io)?;
    let _ = stream.set_nodelay(true);

    Ok(stream)
}

/// Maximum number of commands sent in a single pipelined transmission chunk (256 commands).
pub const PIPELINE_CHUNK_COMMANDS: usize = 256;

/// Maximum cumulative encoded frame bytes sent in a single pipelined transmission chunk (256 KB).
pub const PIPELINE_CHUNK_BYTES: usize = 256 * 1024;

/// Synchronous, blocking client connection to a Cachedb server.
#[derive(Debug)]
pub struct Client {
    stream: TcpStream,
    config: ClientConfig,
    server_info: ServerInfo,
    next_request_id: u32,
    buf: Vec<u8>,
    read_cursor: usize,
    is_broken: bool,
    read_scratch: Box<[u8]>,
}

impl Client {
    /// Connects to a Cachedb server at `addr` using default configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Io`] on socket failure, or [`ClientError::Protocol`] / [`ClientError::Server`]
    /// if version negotiation or handshake fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Client::connect("127.0.0.1:4222")?;
    /// println!("Connected to server: {}", client.server_info().server_name());
    /// # Ok(())
    /// # }
    /// ```
    pub fn connect(addr: impl ToSocketAddrs) -> Result<Self, ClientError> {
        Self::connect_with(addr, ClientConfig::default())
    }

    /// Connects to a Cachedb server at `addr` using custom [`ClientConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if connecting or handshaking fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::{Client, ClientConfig};
    /// use std::time::Duration;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = ClientConfig::new()
    ///     .with_connect_timeout(Some(Duration::from_secs(3)))
    ///     .with_read_timeout(Some(Duration::from_secs(5)));
    ///
    /// let client = Client::connect_with("127.0.0.1:4222", config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn connect_with(
        addr: impl ToSocketAddrs,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        let stream = connect_stream(addr, &config)?;

        let mut client = Self {
            stream,
            config,
            server_info: ServerInfo {
                server_name: String::new(),
                server_version: String::new(),
                client_id: 0,
                max_key_size: None,
                max_value_size: None,
                max_frame_size: None,
            },
            next_request_id: 1,
            buf: Vec::new(),
            read_cursor: 0,
            is_broken: false,
            read_scratch: vec![0u8; 64 * 1024].into_boxed_slice(),
        };

        client.perform_handshake()?;
        Ok(client)
    }

    fn perform_handshake(&mut self) -> Result<(), ClientError> {
        let hello_req = HelloRequest::new(1, self.config.client_name.clone(), None);
        let hello_hdr = Header::new(0, 1, Opcode::Hello, 0);
        let hello_frame = Frame::new(hello_hdr, hello_req.to_value());

        let mut out = Vec::new();
        encode_frame(&hello_frame, &mut out).map_err(|e| {
            self.is_broken = true;
            ClientError::Protocol(e.message().to_string())
        })?;

        self.stream.write_all(&out).map_err(|e| {
            self.is_broken = true;
            map_io_error(e)
        })?;

        let reply = self.read_frame()?;

        if reply.header.flags & FLAG_RESPONSE == 0 {
            self.is_broken = true;
            return Err(ClientError::Protocol(
                "missing RESPONSE flag in hello reply".to_string(),
            ));
        }

        if reply.header.request_id != 1 {
            self.is_broken = true;
            return Err(ClientError::Protocol(format!(
                "mismatched request_id in hello reply: expected 1, got {}",
                reply.header.request_id
            )));
        }

        if reply.header.flags & FLAG_ERROR != 0 || matches!(reply.payload, Value::Error { .. }) {
            self.is_broken = true;
            if let Value::Error { code, message } = reply.payload {
                return Err(ClientError::Server { code, message });
            }
            return Err(ClientError::Protocol(
                "error flag set but payload is not an Error value".to_string(),
            ));
        }

        if reply.header.opcode != Opcode::Hello {
            self.is_broken = true;
            return Err(ClientError::Protocol(format!(
                "unexpected opcode in hello reply: {:?}",
                reply.header.opcode
            )));
        }

        let hello_resp = HelloResponse::from_value(&reply.payload).map_err(|e| {
            self.is_broken = true;
            ClientError::Protocol(format!("malformed hello response: {}", e.message()))
        })?;

        self.server_info = ServerInfo {
            server_name: hello_resp.server_name,
            server_version: hello_resp.server_version,
            client_id: hello_resp.client_id,
            max_key_size: hello_resp.max_key_size,
            max_value_size: hello_resp.max_value_size,
            max_frame_size: hello_resp.max_frame_size,
        };
        self.next_request_id = 2;

        Ok(())
    }

    /// Returns metadata and operational limits received from the server during handshake.
    #[must_use]
    pub const fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    /// Executes a single command array and returns the response payload.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Server`] on server-side command error, or fatal [`ClientError`]
    /// on transport or framing violations.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    /// use skiffdb::proto::Value;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let reply = client.command(vec![Value::Text("ping".to_string())])?;
    /// assert_eq!(reply, Value::Text("pong".to_string()));
    /// # Ok(())
    /// # }
    /// ```
    pub fn command(&mut self, args: Vec<Value>) -> Result<Value, ClientError> {
        if self.is_broken {
            return Err(ClientError::ConnectionClosed);
        }

        let req_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);

        let header = Header::new(0, req_id, Opcode::Command, 0);
        let frame = Frame::new(header, Value::Array(args));

        let mut out = Vec::new();
        encode_frame(&frame, &mut out).map_err(|e| {
            self.is_broken = true;
            ClientError::Protocol(e.message().to_string())
        })?;

        self.stream.write_all(&out).map_err(|e| {
            self.is_broken = true;
            map_io_error(e)
        })?;

        let reply = self.read_frame()?;
        self.validate_reply(reply, req_id)?
    }

    fn validate_reply(
        &mut self,
        reply: Frame,
        expected_id: u32,
    ) -> Result<Result<Value, ClientError>, ClientError> {
        if reply.header.flags & FLAG_RESPONSE == 0 {
            self.is_broken = true;
            return Err(ClientError::Protocol(
                "missing RESPONSE flag in reply".to_string(),
            ));
        }

        if reply.header.request_id != expected_id {
            self.is_broken = true;
            return Err(ClientError::Protocol(format!(
                "mismatched request_id: expected {expected_id}, got {}",
                reply.header.request_id
            )));
        }

        if reply.header.opcode != Opcode::Command {
            self.is_broken = true;
            return Err(ClientError::Protocol(format!(
                "unexpected opcode in command reply: {:?}",
                reply.header.opcode
            )));
        }

        if reply.header.flags & FLAG_ERROR != 0 || matches!(reply.payload, Value::Error { .. }) {
            if let Value::Error { code, message } = reply.payload {
                return Ok(Err(ClientError::Server { code, message }));
            }
            self.is_broken = true;
            return Err(ClientError::Protocol(
                "error flag set but payload is not an Error value".to_string(),
            ));
        }

        Ok(Ok(reply.payload))
    }

    /// Pipelines multiple command arrays in a single network transmission, returning
    /// results in exact execution order.
    ///
    /// Commands are transmitted in chunks of up to [`PIPELINE_CHUNK_COMMANDS`] commands
    /// or [`PIPELINE_CHUNK_BYTES`] bytes to prevent server-side write buffer deadlock.
    ///
    /// Transport or protocol failures terminate the pipeline with an outer [`Err(ClientError)`](ClientError).
    /// Per-command logical server errors are returned in individual elements.
    ///
    /// # Errors
    ///
    /// Returns an outer [`ClientError`] if network transmission or frame decoding fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    /// use skiffdb::proto::Value;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let commands = vec![
    ///     vec![Value::Text("ping".to_string())],
    ///     vec![Value::Text("size".to_string())],
    /// ];
    /// let replies = client.pipeline(commands)?;
    /// assert_eq!(replies.len(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn pipeline(
        &mut self,
        commands: Vec<Vec<Value>>,
    ) -> Result<Vec<Result<Value, ClientError>>, ClientError> {
        if commands.is_empty() {
            return Ok(Vec::new());
        }

        if self.is_broken {
            return Err(ClientError::ConnectionClosed);
        }

        let mut results = Vec::with_capacity(commands.len());
        let mut iter = commands.into_iter();
        let mut pending_encoded: Option<(u32, Vec<u8>)> = None;

        loop {
            let mut chunk_expected_ids = Vec::new();
            let mut chunk_bytes = Vec::new();

            if let Some((req_id, encoded)) = pending_encoded.take() {
                chunk_expected_ids.push(req_id);
                chunk_bytes.extend_from_slice(&encoded);
            }

            while chunk_expected_ids.len() < PIPELINE_CHUNK_COMMANDS {
                let Some(cmd) = iter.next() else {
                    break;
                };

                let req_id = self.next_request_id;
                self.next_request_id = self.next_request_id.wrapping_add(1);
                let header = Header::new(0, req_id, Opcode::Command, 0);
                let frame = Frame::new(header, Value::Array(cmd));
                let mut encoded = Vec::new();
                encode_frame(&frame, &mut encoded).map_err(|e| {
                    self.is_broken = true;
                    ClientError::Protocol(e.message().to_string())
                })?;

                if !chunk_expected_ids.is_empty()
                    && chunk_bytes.len() + encoded.len() > PIPELINE_CHUNK_BYTES
                {
                    pending_encoded = Some((req_id, encoded));
                    break;
                }

                chunk_expected_ids.push(req_id);
                chunk_bytes.extend_from_slice(&encoded);

                if chunk_bytes.len() >= PIPELINE_CHUNK_BYTES {
                    break;
                }
            }

            if chunk_expected_ids.is_empty() {
                break;
            }

            self.stream.write_all(&chunk_bytes).map_err(|e| {
                self.is_broken = true;
                map_io_error(e)
            })?;

            for expected_id in chunk_expected_ids {
                let reply = self.read_frame()?;
                let res = self.validate_reply(reply, expected_id)?;
                results.push(res);
            }
        }

        Ok(results)
    }

    /// Tests connection liveness with the server.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// assert_eq!(client.ping()?, "pong");
    /// # Ok(())
    /// # }
    /// ```
    pub fn ping(&mut self) -> Result<String, ClientError> {
        match self.command(vec![Value::Text("ping".to_string())])? {
            Value::Text(s) => Ok(s),
            Value::Bytes(b) => String::from_utf8(b).map_err(|e| {
                ClientError::UnexpectedReply(format!("invalid utf-8 in ping reply: {e}"))
            }),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Text for ping, got {other:?}"
            ))),
        }
    }

    /// Stores a key-value pair, unconditionally overwriting any existing value.
    ///
    /// Returns `true` if stored successfully.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if key validation fails or communication fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    /// use skiffdb::proto::Value;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// client.put(b"mykey", Value::Int(100))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn put(&mut self, key: &[u8], value: Value) -> Result<bool, ClientError> {
        match self.command(vec![
            Value::Text("put".to_string()),
            Value::Bytes(key.to_vec()),
            value,
        ])? {
            Value::Bool(b) => Ok(b),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Bool for put, got {other:?}"
            ))),
        }
    }

    /// Stores a key-value pair only if the key does not already exist (`NX`).
    ///
    /// Returns `true` if stored, or `false` if the key was already present.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication or key validation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    /// use skiffdb::proto::Value;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let inserted = client.put_nx(b"unique_key", Value::Int(1))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn put_nx(&mut self, key: &[u8], value: Value) -> Result<bool, ClientError> {
        match self.command(vec![
            Value::Text("put".to_string()),
            Value::Bytes(key.to_vec()),
            value,
            Value::Text("nx".to_string()),
        ])? {
            Value::Bool(b) => Ok(b),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Bool for put_nx, got {other:?}"
            ))),
        }
    }

    /// Stores a key-value pair only if the key already exists (`XX`).
    ///
    /// Returns `true` if updated, or `false` if the key was absent.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication or key validation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    /// use skiffdb::proto::Value;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let updated = client.put_xx(b"existing_key", Value::Int(2))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn put_xx(&mut self, key: &[u8], value: Value) -> Result<bool, ClientError> {
        match self.command(vec![
            Value::Text("put".to_string()),
            Value::Bytes(key.to_vec()),
            value,
            Value::Text("xx".to_string()),
        ])? {
            Value::Bool(b) => Ok(b),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Bool for put_xx, got {other:?}"
            ))),
        }
    }

    /// Fetches the value stored at `key`.
    ///
    /// Returns `Ok(None)` if the key does not exist.
    ///
    /// # Note
    ///
    /// A stored [`Value::Nil`] also reads as `Ok(None)`. To check key existence
    /// without relying on value distinction, use [`has`](Self::has).
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication or key validation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let val = client.fetch(b"missing_key")?;
    /// assert_eq!(val, None);
    /// # Ok(())
    /// # }
    /// ```
    pub fn fetch(&mut self, key: &[u8]) -> Result<Option<Value>, ClientError> {
        match self.command(vec![
            Value::Text("fetch".to_string()),
            Value::Bytes(key.to_vec()),
        ])? {
            Value::Nil => Ok(None),
            val => Ok(Some(val)),
        }
    }

    /// Deletes one or more keys from the database.
    ///
    /// Returns the number of keys that were removed.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let count = client.drop_keys(&[b"k1" as &[u8], b"k2"])?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn drop_keys<K: AsRef<[u8]>>(&mut self, keys: &[K]) -> Result<i64, ClientError> {
        if keys.is_empty() {
            return Ok(0);
        }

        let mut args = Vec::with_capacity(1 + keys.len());
        args.push(Value::Text("drop".to_string()));
        for k in keys {
            args.push(Value::Bytes(k.as_ref().to_vec()));
        }

        match self.command(args)? {
            Value::Int(n) => Ok(n),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Int for drop_keys, got {other:?}"
            ))),
        }
    }

    /// Checks if `key` exists in the database.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication or key validation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let exists = client.has(b"mykey")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn has(&mut self, key: &[u8]) -> Result<bool, ClientError> {
        match self.command(vec![
            Value::Text("has".to_string()),
            Value::Bytes(key.to_vec()),
        ])? {
            Value::Bool(b) => Ok(b),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Bool for has, got {other:?}"
            ))),
        }
    }

    /// Returns the total number of keys stored in the database.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if communication fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let count = client.size()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn size(&mut self) -> Result<i64, ClientError> {
        match self.command(vec![Value::Text("size".to_string())])? {
            Value::Int(n) => Ok(n),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Int for size, got {other:?}"
            ))),
        }
    }

    /// Increments or decrements the integer stored at `key` by `step` (default `1`).
    ///
    /// Returns the new integer value.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] on integer overflow, type incompatibility, or communication error.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut client = Client::connect("127.0.0.1:4222")?;
    /// let counter = client.bump(b"hits", Some(1))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn bump(&mut self, key: &[u8], step: Option<i64>) -> Result<i64, ClientError> {
        let mut args = Vec::with_capacity(3);
        args.push(Value::Text("bump".to_string()));
        args.push(Value::Bytes(key.to_vec()));
        if let Some(s) = step {
            args.push(Value::Int(s));
        }

        match self.command(args)? {
            Value::Int(n) => Ok(n),
            other => Err(ClientError::UnexpectedReply(format!(
                "expected Int for bump, got {other:?}"
            ))),
        }
    }

    /// Gracefully closes the connection by issuing the `quit` command.
    ///
    /// Consumes the client instance so no further operations can be performed.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] if transmitting the quit command fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skiffdb::client::Client;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Client::connect("127.0.0.1:4222")?;
    /// client.quit()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn quit(mut self) -> Result<(), ClientError> {
        let _ = self.command(vec![Value::Text("quit".to_string())])?;
        self.is_broken = true;
        Ok(())
    }

    /// Reads the next complete frame from the internal buffer and socket.
    fn read_frame(&mut self) -> Result<Frame, ClientError> {
        if self.is_broken {
            return Err(ClientError::ConnectionClosed);
        }

        let max_payload_len = u32::try_from(self.config.max_reply_size).unwrap_or(u32::MAX);

        loop {
            let available = &self.buf[self.read_cursor..];
            if available.len() >= HEADER_LEN {
                match Header::decode_with_max_len(&available[..HEADER_LEN], max_payload_len) {
                    Ok(Decoded::Complete(hdr)) => {
                        let total_len = HEADER_LEN + hdr.payload_len as usize;
                        if available.len() >= total_len {
                            match decode_frame_with_limit(available, max_payload_len) {
                                Ok(Decoded::Complete((frame, used))) => {
                                    self.read_cursor += used;
                                    if self.read_cursor == self.buf.len() {
                                        self.buf.clear();
                                        self.read_cursor = 0;
                                    } else if self.read_cursor > self.buf.len() / 2 {
                                        self.buf.copy_within(self.read_cursor.., 0);
                                        let remaining = self.buf.len() - self.read_cursor;
                                        self.buf.truncate(remaining);
                                        self.read_cursor = 0;
                                    }
                                    return Ok(frame);
                                }
                                Ok(Decoded::NeedMore) => {}
                                Err(ProtoError::ERR_FRAME_TOO_LARGE) => {
                                    self.is_broken = true;
                                    return Err(ClientError::ReplyTooLarge);
                                }
                                Err(e) => {
                                    self.is_broken = true;
                                    return Err(ClientError::Protocol(e.message().to_string()));
                                }
                            }
                        }
                    }
                    Ok(Decoded::NeedMore) => {}
                    Err(ProtoError::ERR_FRAME_TOO_LARGE) => {
                        self.is_broken = true;
                        return Err(ClientError::ReplyTooLarge);
                    }
                    Err(e) => {
                        self.is_broken = true;
                        return Err(ClientError::Protocol(e.message().to_string()));
                    }
                }
            }

            // Compact before reading if read cursor is past half the buffer
            if self.read_cursor > 0 && self.read_cursor > self.buf.len() / 2 {
                self.buf.copy_within(self.read_cursor.., 0);
                let remaining = self.buf.len() - self.read_cursor;
                self.buf.truncate(remaining);
                self.read_cursor = 0;
            }

            match self.stream.read(&mut self.read_scratch[..]) {
                Ok(0) => {
                    self.is_broken = true;
                    return Err(ClientError::ConnectionClosed);
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&self.read_scratch[..n]);
                }
                Err(e) => {
                    self.is_broken = true;
                    return Err(map_io_error(e));
                }
            }
        }
    }
}
