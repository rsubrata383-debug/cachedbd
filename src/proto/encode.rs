//! Protocol framing serialization and frame encoding.

use crate::proto::Frame;
use crate::proto::header::{DEFAULT_MAX_PAYLOAD_LEN, ProtoError};
use crate::proto::item::encode_value;

/// Encodes a complete [`Frame`] (14-byte header + typed payload) into an output buffer.
///
/// The header's `payload_len` is automatically computed to match the exact byte size
/// of the serialized payload.
///
/// # Atomicity
///
/// If serialization fails (for example, if payload exceeds the maximum frame size or
/// collection nesting exceeds 8 levels), the output buffer `out` is preserved unchanged
/// (truncated back to its original length).
///
/// # Errors
///
/// - [`ProtoError::ERR_FRAME_TOO_LARGE`] (`1003`): Serialized payload exceeds 64 MB.
/// - [`ProtoError::ERR_MAX_DEPTH_EXCEEDED`] (`3005`): Collection nesting exceeds 8 levels.
pub fn encode_frame(frame: &Frame, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    encode_frame_with_limit(frame, out, DEFAULT_MAX_PAYLOAD_LEN)
}

/// Encodes a complete [`Frame`] into an output buffer with an explicit maximum payload size limit.
///
/// # Errors
///
/// - [`ProtoError::ERR_FRAME_TOO_LARGE`] (`1003`): Serialized payload exceeds `max_payload_len`.
/// - [`ProtoError::ERR_MAX_DEPTH_EXCEEDED`] (`3005`): Collection nesting exceeds 8 levels.
pub fn encode_frame_with_limit(
    frame: &Frame,
    out: &mut Vec<u8>,
    max_payload_len: u32,
) -> Result<(), ProtoError> {
    let original_len = out.len();

    // Encode payload into a temporary buffer first to measure size and protect `out` on error.
    let mut payload_buf = Vec::new();
    if let Err(err) = encode_value(&frame.payload, &mut payload_buf) {
        out.truncate(original_len);
        return Err(err);
    }

    let payload_len_u64 = payload_buf.len() as u64;
    if payload_len_u64 > u64::from(max_payload_len) {
        out.truncate(original_len);
        return Err(ProtoError::ERR_FRAME_TOO_LARGE);
    }

    let Ok(payload_len) = u32::try_from(payload_buf.len()) else {
        out.truncate(original_len);
        return Err(ProtoError::ERR_FRAME_TOO_LARGE);
    };

    let mut header = frame.header;
    header.payload_len = payload_len;

    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(&payload_buf);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::header::{Header, Opcode, ProtoError};
    use crate::proto::item::Value;

    #[test]
    fn test_encode_frame_with_payload_larger_than_max_leaves_out_unchanged() {
        let header = Header::new(0, 1, Opcode::Command, 0);
        let payload = Value::Bytes(vec![0xAA; 100]);
        let frame = Frame::new(header, payload);

        let mut out = vec![0x11, 0x22, 0x33];
        // Enforce max payload limit of 10 bytes (smaller than the 100-byte payload)
        let res = encode_frame_with_limit(&frame, &mut out, 10);
        assert_eq!(res, Err(ProtoError::ERR_FRAME_TOO_LARGE));
        // Verify out was left completely untouched
        assert_eq!(out, vec![0x11, 0x22, 0x33]);
    }
}
