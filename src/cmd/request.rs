//! Pure request processing connecting wire protocol frames to the command engine.

use crate::cmd::dispatch::{Engine, Session};
use crate::cmd::error::{
    ERR_MALFORMED_PAYLOAD, ERR_UNKNOWN_COMMAND, ERR_UNSUPPORTED_PROTOCOL_VERSION,
};
use crate::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
use crate::proto::hello::{HelloRequest, HelloResponse, negotiate_version};
use crate::proto::{Frame, Value};

/// Default maximum allowable payload size: 64 MB (67,108,864 bytes).
const DEFAULT_MAX_FRAME_SIZE: i64 = 64 * 1024 * 1024;

/// Result of processing an incoming wire protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameOutcome {
    /// Outgoing reply frame to send to the client.
    pub reply_frame: Frame,
    /// Whether the server must close the connection after transmitting the reply.
    pub close: bool,
}

/// Pure request handler that processes a decoded [`Frame`] against the [`Engine`].
///
/// This function performs zero socket I/O and never panics on any input.
///
/// # Behavior
///
/// - **`Opcode::Command` (`0x0002`)**:
///   - Calls [`Engine::execute`] with the frame's payload [`Value`].
///   - Constructs a reply frame with the same `request_id`, opcode `0x0002`,
///     and [`FLAG_RESPONSE`] (`0x01`). If the reply is [`Value::Error`],
///     [`FLAG_ERROR`] (`0x02`) is also set (`flags = 0x03`).
///   - If execution indicated connection close (e.g. `quit`), `close` is set to `true`.
/// - **`Opcode::Hello` (`0x0001`)**:
///   - Decodes the payload using [`HelloRequest::from_value`].
///   - If payload is malformed, returns an error frame with code `1006` and sets `close = true`.
///   - Validates the protocol version via [`negotiate_version`]. If not version `1`,
///     returns error code `1001` and sets `close = true`.
///   - Authenticates the session and returns a server configuration [`HelloResponse`] map
///     with opcode `0x0001` and flags `0x01`.
/// - **Unsupported Opcode (e.g. `Opcode::Push` from client)**:
///   - Returns error `2001` ([`ERR_UNKNOWN_COMMAND`]) with flags `0x03` and sets `close = true`.
#[must_use]
pub fn handle_request(engine: &mut Engine, session: &mut Session, frame: &Frame) -> FrameOutcome {
    match frame.header.opcode {
        Opcode::Command => {
            let outcome = engine.execute(session, &frame.payload);
            let is_error = matches!(outcome.reply, Value::Error { .. });
            let flags = if is_error {
                FLAG_RESPONSE | FLAG_ERROR
            } else {
                FLAG_RESPONSE
            };

            let header = Header::new(flags, frame.header.request_id, Opcode::Command, 0);
            FrameOutcome {
                reply_frame: Frame::new(header, outcome.reply),
                close: outcome.close,
            }
        }
        Opcode::Hello => handle_hello_frame(engine, session, frame),
        Opcode::Push => {
            let reply = ERR_UNKNOWN_COMMAND.to_value();
            let header = Header::new(
                FLAG_RESPONSE | FLAG_ERROR,
                frame.header.request_id,
                frame.header.opcode,
                0,
            );
            FrameOutcome {
                reply_frame: Frame::new(header, reply),
                close: true,
            }
        }
    }
}

fn handle_hello_frame(engine: &Engine, session: &mut Session, frame: &Frame) -> FrameOutcome {
    let Ok(req) = HelloRequest::from_value(&frame.payload) else {
        let reply = ERR_MALFORMED_PAYLOAD.to_value();
        let header = Header::new(
            FLAG_RESPONSE | FLAG_ERROR,
            frame.header.request_id,
            Opcode::Hello,
            0,
        );
        return FrameOutcome {
            reply_frame: Frame::new(header, reply),
            close: true,
        };
    };

    if negotiate_version(req.protocol_version).is_err() {
        let reply = ERR_UNSUPPORTED_PROTOCOL_VERSION.to_value();
        let header = Header::new(
            FLAG_RESPONSE | FLAG_ERROR,
            frame.header.request_id,
            Opcode::Hello,
            0,
        );
        return FrameOutcome {
            reply_frame: Frame::new(header, reply),
            close: true,
        };
    }

    session.authenticated = true;

    let client_id = i64::try_from(session.client_id).unwrap_or(0);
    let max_key_size = i64::try_from(engine.config().max_key_size).unwrap_or(1024);
    let max_value_size = i64::try_from(engine.config().max_value_size).unwrap_or(32 * 1024 * 1024);

    let resp = HelloResponse {
        server_name: "cachedbd".to_string(),
        server_version: "0.1.0".to_string(),
        protocol_version: 1,
        client_id,
        max_frame_size: Some(DEFAULT_MAX_FRAME_SIZE),
        max_key_size: Some(max_key_size),
        max_value_size: Some(max_value_size),
    };

    let header = Header::new(FLAG_RESPONSE, frame.header.request_id, Opcode::Hello, 0);
    FrameOutcome {
        reply_frame: Frame::new(header, resp.to_value()),
        close: false,
    }
}
