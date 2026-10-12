//! Pure request processing connecting wire protocol frames to the command engine.

use crate::cmd::dispatch::{Engine, Session};
use crate::cmd::error::{
    ERR_AUTH_REQUIRED, ERR_MALFORMED_PAYLOAD, ERR_UNKNOWN_COMMAND, ERR_UNSUPPORTED_PROTOCOL_VERSION,
};
use crate::cmd::handlers::build_hello_response;
use crate::proto::header::{FLAG_ERROR, FLAG_RESPONSE, Header, Opcode};
use crate::proto::hello::{HelloRequest, negotiate_version};
use crate::proto::{Frame, Value};

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
///   - If `require_auth` is enabled, returns `5001` ([`ERR_AUTH_REQUIRED`]) with flags `0x03`
///     and leaves connection open.
///   - Authenticates the session and returns a server configuration [`crate::proto::hello::HelloResponse`] map
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

    if engine.config().require_auth {
        // Auth backend comes in a later phase:
        // Do not authenticate the session and return ERR_AUTH_REQUIRED (5001).
        let reply = ERR_AUTH_REQUIRED.to_value();
        let header = Header::new(
            FLAG_RESPONSE | FLAG_ERROR,
            frame.header.request_id,
            Opcode::Hello,
            0,
        );
        return FrameOutcome {
            reply_frame: Frame::new(header, reply),
            close: false,
        };
    }

    session.authenticated = true;

    let resp = build_hello_response(engine.config(), session);
    let header = Header::new(FLAG_RESPONSE, frame.header.request_id, Opcode::Hello, 0);
    FrameOutcome {
        reply_frame: Frame::new(header, resp.to_value()),
        close: false,
    }
}
