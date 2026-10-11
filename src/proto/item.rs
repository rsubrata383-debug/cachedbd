//! Typed binary payload item serialization and deserialization.
//!
//! Implements Section 4 of `docs/spec/protocol.md` and Section 4 of `docs/spec/values.md`:
//! - `Nil` (`0x00`): Tag only (1 byte).
//! - `Int` (`0x01`): Tag + signed Zigzag varint (64-bit integer).
//! - `Float` (`0x02`): Tag + 8-byte big-endian IEEE-754 double precision float.
//! - `Bool` (`0x03`): Tag + 1 byte (`0x00` = false, `0x01` = true).
//! - `Text` (`0x04`): Tag + unsigned LEB128 varint byte length + UTF-8 bytes.
//! - `Bytes` (`0x05`): Tag + unsigned LEB128 varint byte length + raw bytes.
//! - `Array` (`0x06`): Tag + unsigned LEB128 varint count + sequence of typed items.
//! - `Map` (`0x07`): Tag + unsigned LEB128 varint pair count + sequence of key-value pairs.
//! - `Error` (`0x08`): Tag + unsigned LEB128 varint code + unsigned LEB128 varint byte length + UTF-8 string.
//!
//! # Forward Compatibility and Unknown Tags
//!
//! Per Section 4.3 of `docs/spec/protocol.md`, future type tags (`>= 0x09`) encode an unsigned
//! varint byte length directly following the tag. Older decoders read this length and safely skip
//! past the unknown item. Unknown items are skipped without failing collection parsing, and
//! are not stored in the [`Value`] enum.
//!
//! # Collection Nesting Depth
//!
//! Nesting depth is 1-indexed collection nesting: a top-level collection (`Array` or `Map`) is at
//! depth 1, and each nested collection increments depth by 1. A collection exceeding
//! [`MAX_NESTING_DEPTH`] (8 levels) is rejected with [`ProtoError::ERR_MAX_DEPTH_EXCEEDED`].
//! Scalar items and [`Value::Error`] do not increase collection nesting depth.

use crate::proto::Decoded;
use crate::proto::header::ProtoError;
use crate::proto::varint::{decode_ivarint, decode_uvarint, encode_ivarint, encode_uvarint};

/// Item type tag byte for a `Nil` item (`0x00`).
pub const TAG_NIL: u8 = 0x00;

/// Item type tag byte for an `Int` item (`0x01`).
pub const TAG_INT: u8 = 0x01;

/// Item type tag byte for a `Float` item (`0x02`).
pub const TAG_FLOAT: u8 = 0x02;

/// Item type tag byte for a `Bool` item (`0x03`).
pub const TAG_BOOL: u8 = 0x03;

/// Item type tag byte for a `Text` item (`0x04`).
pub const TAG_TEXT: u8 = 0x04;

/// Item type tag byte for a `Bytes` item (`0x05`).
pub const TAG_BYTES: u8 = 0x05;

/// Item type tag byte for an `Array` item (`0x06`).
pub const TAG_ARRAY: u8 = 0x06;

/// Item type tag byte for a `Map` item (`0x07`).
pub const TAG_MAP: u8 = 0x07;

/// Item type tag byte for an `Error` item (`0x08`).
pub const TAG_ERROR: u8 = 0x08;

/// Maximum collection nesting depth per Section 4 of `docs/spec/values.md` (8 levels).
pub const MAX_NESTING_DEPTH: usize = 8;

/// Maximum initial allocation capacity for incoming collections to prevent OOM amplification.
const MAX_INITIAL_COLLECTION_CAPACITY: usize = 1024;

/// An owned typed value representing a payload item in the Cachedb protocol.
#[derive(Debug, Clone)]
pub enum Value {
    /// Missing or null value (`0x00`).
    Nil,
    /// Signed 64-bit integer (`0x01`, encoded using Zigzag varint).
    Int(i64),
    /// IEEE-754 64-bit floating point number (`0x02`, big-endian).
    Float(f64),
    /// Boolean value (`0x03`, `0x00` = false, `0x01` = true).
    Bool(bool),
    /// UTF-8 encoded text string (`0x04`, unsigned varint byte length + UTF-8 bytes).
    Text(String),
    /// Arbitrary raw binary data (`0x05`, unsigned varint byte length + raw bytes).
    Bytes(Vec<u8>),
    /// Ordered list of typed items (`0x06`, unsigned varint count + sequence of items).
    Array(Vec<Self>),
    /// Ordered key-value associative mapping of typed items (`0x07`, unsigned varint pair count + key-value pairs).
    Map(Vec<(Self, Self)>),
    /// Structured error object (`0x08`, unsigned varint code + unsigned varint length + UTF-8 message).
    Error {
        /// Numeric error code (must fit in a 16-bit unsigned integer).
        code: u16,
        /// Human-readable error message.
        message: String,
    },
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Int(a), Self::Int(b)) => a == b,
            // Compare floating point numbers by exact bit representation so that NaN == NaN
            // and -0.0 != +0.0 round-trip with bit-level preservation per protocol spec.
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Map(a), Self::Map(b)) => a == b,
            (
                Self::Error {
                    code: c1,
                    message: m1,
                },
                Self::Error {
                    code: c2,
                    message: m2,
                },
            ) => c1 == c2 && m1 == m2,
            _ => false,
        }
    }
}

impl Eq for Value {}

/// Serializes a [`Value`] into an output buffer per Section 4 of `docs/spec/protocol.md`.
///
/// # Errors
///
/// Returns [`ProtoError::ERR_MAX_DEPTH_EXCEEDED`] (`3005`) if the collection nesting depth
/// exceeds [`MAX_NESTING_DEPTH`] (8 levels).
pub fn encode_value(value: &Value, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    encode_value_depth(value, out, 0)
}

fn encode_value_depth(value: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), ProtoError> {
    match value {
        Value::Nil => {
            out.push(TAG_NIL);
        }
        Value::Int(n) => {
            out.push(TAG_INT);
            encode_ivarint(*n, out);
        }
        Value::Float(f) => {
            out.push(TAG_FLOAT);
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Value::Bool(b) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*b));
        }
        Value::Text(s) => {
            out.push(TAG_TEXT);
            encode_uvarint(s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Bytes(b) => {
            out.push(TAG_BYTES);
            encode_uvarint(b.len() as u64, out);
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            if depth >= MAX_NESTING_DEPTH {
                return Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED);
            }
            out.push(TAG_ARRAY);
            encode_uvarint(items.len() as u64, out);
            for item in items {
                encode_value_depth(item, out, depth + 1)?;
            }
        }
        Value::Map(pairs) => {
            if depth >= MAX_NESTING_DEPTH {
                return Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED);
            }
            out.push(TAG_MAP);
            encode_uvarint(pairs.len() as u64, out);
            for (k, v) in pairs {
                encode_value_depth(k, out, depth + 1)?;
                encode_value_depth(v, out, depth + 1)?;
            }
        }
        Value::Error { code, message } => {
            out.push(TAG_ERROR);
            encode_uvarint(u64::from(*code), out);
            encode_uvarint(message.len() as u64, out);
            out.extend_from_slice(message.as_bytes());
        }
    }
    Ok(())
}

/// Deserializes a [`Value`] from a slice representing a complete frame payload.
///
/// # Complete Payload Semantic
///
/// The `input` slice represents a buffer for which the frame header length has already
/// declared completion. Therefore, running out of bytes in the middle of an item is a
/// framing violation error ([`ProtoError::ERR_MALFORMED_PAYLOAD`], code `1006`), **not**
/// a `NeedMore` condition. Any `NeedMore` returned by an underlying varint decoder is
/// translated into `ERR_MALFORMED_PAYLOAD`.
///
/// # Errors
///
/// - [`ProtoError::ERR_MALFORMED_PAYLOAD`] (`1006`): Input stream ends prematurely, length exceeds
///   available buffer bytes, boolean byte is not `0x00`/`0x01`, error code overflows `u16`, or text
///   contains invalid UTF-8.
/// - [`ProtoError::ERR_UNKNOWN_TYPE_TAG`] (`1007`): Standalone unrecognized type tag byte encountered.
/// - [`ProtoError::ERR_MALFORMED_VARINT`] (`1005`): Varint encoding is corrupted or overflows 64 bits.
/// - [`ProtoError::ERR_MAX_DEPTH_EXCEEDED`] (`3005`): Collection nesting exceeds [`MAX_NESTING_DEPTH`] (8 levels).
pub fn decode_value(input: &[u8]) -> Result<(Value, usize), ProtoError> {
    decode_value_depth(input, 0)
}

fn decode_value_depth(input: &[u8], depth: usize) -> Result<(Value, usize), ProtoError> {
    let (&tag, rest) = input
        .split_first()
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;

    match tag {
        TAG_NIL => Ok((Value::Nil, 1)),
        TAG_INT => decode_int(rest),
        TAG_FLOAT => decode_float(rest),
        TAG_BOOL => decode_bool(rest),
        TAG_TEXT => decode_text(rest),
        TAG_BYTES => decode_bytes(rest),
        TAG_ARRAY => decode_array(rest, depth),
        TAG_MAP => decode_map(rest, depth),
        TAG_ERROR => decode_error(rest),
        _ => Err(ProtoError::ERR_UNKNOWN_TYPE_TAG),
    }
}

fn decode_int(rest: &[u8]) -> Result<(Value, usize), ProtoError> {
    let (n, varint_len) = match decode_ivarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    Ok((Value::Int(n), 1 + varint_len))
}

fn decode_float(rest: &[u8]) -> Result<(Value, usize), ProtoError> {
    let float_bytes = rest.get(0..8).ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let bits = u64::from_be_bytes(
        float_bytes
            .try_into()
            .map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?,
    );
    Ok((Value::Float(f64::from_bits(bits)), 1 + 8))
}

fn decode_bool(rest: &[u8]) -> Result<(Value, usize), ProtoError> {
    let (&b, _) = rest
        .split_first()
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    match b {
        0x00 => Ok((Value::Bool(false), 2)),
        0x01 => Ok((Value::Bool(true), 2)),
        _ => Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    }
}

fn decode_text(rest: &[u8]) -> Result<(Value, usize), ProtoError> {
    let (raw_len, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let len = usize::try_from(raw_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let available = rest.len().saturating_sub(varint_len);
    if available < len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    let text_bytes = rest
        .get(varint_len..varint_len + len)
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let text = std::str::from_utf8(text_bytes)
        .map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?
        .to_owned();
    Ok((Value::Text(text), 1 + varint_len + len))
}

fn decode_bytes(rest: &[u8]) -> Result<(Value, usize), ProtoError> {
    let (raw_len, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let len = usize::try_from(raw_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let available = rest.len().saturating_sub(varint_len);
    if available < len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    let payload_bytes = rest
        .get(varint_len..varint_len + len)
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    Ok((Value::Bytes(payload_bytes.to_vec()), 1 + varint_len + len))
}

fn decode_array(rest: &[u8], depth: usize) -> Result<(Value, usize), ProtoError> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED);
    }
    let (raw_count, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let count = usize::try_from(raw_count).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

    let bytes_left = rest.len().saturating_sub(varint_len);
    if count > bytes_left {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    let initial_cap = count.min(bytes_left).min(MAX_INITIAL_COLLECTION_CAPACITY);
    let mut items = Vec::with_capacity(initial_cap);

    let mut cursor = varint_len;
    for _ in 0..count {
        if cursor >= rest.len() {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }
        let item_tag = rest[cursor];
        if item_tag >= 0x09 {
            let skipped = skip_unknown_tag(&rest[cursor..])?;
            cursor += skipped;
        } else {
            let (item, consumed) = decode_value_depth(&rest[cursor..], depth + 1)?;
            items.push(item);
            cursor += consumed;
        }
    }

    Ok((Value::Array(items), 1 + cursor))
}

fn decode_map(rest: &[u8], depth: usize) -> Result<(Value, usize), ProtoError> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED);
    }
    let (raw_count, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let count = usize::try_from(raw_count).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

    let bytes_left = rest.len().saturating_sub(varint_len);
    if count > bytes_left / 2 {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    let initial_cap = count
        .min(bytes_left / 2)
        .min(MAX_INITIAL_COLLECTION_CAPACITY);
    let mut pairs = Vec::with_capacity(initial_cap);

    let mut cursor = varint_len;
    for _ in 0..count {
        if cursor >= rest.len() {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }
        let key_tag = rest[cursor];
        if key_tag >= 0x09 {
            let skipped_key = skip_unknown_tag(&rest[cursor..])?;
            cursor += skipped_key;
            if cursor >= rest.len() {
                return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
            }
            let skipped_val = skip_value_depth(&rest[cursor..], depth + 1)?;
            cursor += skipped_val;
            continue;
        }

        let (key, key_consumed) = decode_value_depth(&rest[cursor..], depth + 1)?;
        cursor += key_consumed;

        if cursor >= rest.len() {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }
        let val_tag = rest[cursor];
        if val_tag >= 0x09 {
            let skipped_val = skip_unknown_tag(&rest[cursor..])?;
            cursor += skipped_val;
            continue;
        }

        let (val, val_consumed) = decode_value_depth(&rest[cursor..], depth + 1)?;
        cursor += val_consumed;

        pairs.push((key, val));
    }

    Ok((Value::Map(pairs), 1 + cursor))
}

fn decode_error(rest: &[u8]) -> Result<(Value, usize), ProtoError> {
    let (raw_code, code_varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let code = u16::try_from(raw_code).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

    let msg_rest = rest
        .get(code_varint_len..)
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let (raw_msg_len, msg_len_varint_len) = match decode_uvarint(msg_rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let msg_len = usize::try_from(raw_msg_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

    let header_consumed = code_varint_len + msg_len_varint_len;
    let available = rest.len().saturating_sub(header_consumed);
    if available < msg_len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    let msg_bytes = rest
        .get(header_consumed..header_consumed + msg_len)
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let message = std::str::from_utf8(msg_bytes)
        .map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?
        .to_owned();

    Ok((
        Value::Error { code, message },
        1 + header_consumed + msg_len,
    ))
}

/// Skips an unknown forward-compatible tag (`>= 0x09`) per Section 4.3 of `docs/spec/protocol.md`.
///
/// In Section 4.3, every future type tag places an unsigned varint byte length directly
/// after the tag. Older decoders read the length and safely skip past the unknown item.
///
/// Returns the total bytes consumed (`1` tag byte + varint length bytes + data length).
fn skip_unknown_tag(input: &[u8]) -> Result<usize, ProtoError> {
    let (&_tag, rest) = input
        .split_first()
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let (raw_len, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let len = usize::try_from(raw_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let available = rest.len().saturating_sub(varint_len);
    if available < len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    Ok(1 + varint_len + len)
}

/// Skips a single complete encoded item (scalar, compound, or forward-compatible tag >= 0x09).
///
/// Returns the number of bytes consumed by the item.
///
/// # Errors
///
/// - [`ProtoError::ERR_MALFORMED_PAYLOAD`] (`1006`): Truncated payload or corrupted field bounds.
/// - [`ProtoError::ERR_MALFORMED_VARINT`] (`1005`): Varint encoding error.
/// - [`ProtoError::ERR_MAX_DEPTH_EXCEEDED`] (`3005`): Collection nesting exceeds 8 levels.
pub fn skip_value(input: &[u8]) -> Result<usize, ProtoError> {
    skip_value_depth(input, 0)
}

fn skip_value_depth(input: &[u8], depth: usize) -> Result<usize, ProtoError> {
    let (&tag, rest) = input
        .split_first()
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;

    if tag >= 0x09 {
        return skip_unknown_tag(input);
    }

    match tag {
        TAG_NIL => Ok(1),
        TAG_INT => skip_int(rest),
        TAG_FLOAT => skip_float(rest),
        TAG_BOOL => skip_bool(rest),
        TAG_TEXT | TAG_BYTES => skip_text_or_bytes(rest),
        TAG_ARRAY => skip_array(rest, depth),
        TAG_MAP => skip_map(rest, depth),
        TAG_ERROR => skip_error(rest),
        _ => Err(ProtoError::ERR_UNKNOWN_TYPE_TAG),
    }
}

fn skip_int(rest: &[u8]) -> Result<usize, ProtoError> {
    let (_, varint_len) = match decode_ivarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    Ok(1 + varint_len)
}

fn skip_float(rest: &[u8]) -> Result<usize, ProtoError> {
    if rest.len() < 8 {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    Ok(1 + 8)
}

fn skip_bool(rest: &[u8]) -> Result<usize, ProtoError> {
    let (&b, _) = rest
        .split_first()
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    if b != 0x00 && b != 0x01 {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    Ok(2)
}

fn skip_text_or_bytes(rest: &[u8]) -> Result<usize, ProtoError> {
    let (raw_len, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let len = usize::try_from(raw_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let available = rest.len().saturating_sub(varint_len);
    if available < len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    Ok(1 + varint_len + len)
}

fn skip_array(rest: &[u8], depth: usize) -> Result<usize, ProtoError> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED);
    }
    let (raw_count, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let count = usize::try_from(raw_count).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let bytes_left = rest.len().saturating_sub(varint_len);
    if count > bytes_left {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    let mut cursor = varint_len;
    for _ in 0..count {
        if cursor >= rest.len() {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }
        let skipped = skip_value_depth(&rest[cursor..], depth + 1)?;
        cursor += skipped;
    }
    Ok(1 + cursor)
}

fn skip_map(rest: &[u8], depth: usize) -> Result<usize, ProtoError> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(ProtoError::ERR_MAX_DEPTH_EXCEEDED);
    }
    let (raw_count, varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let count = usize::try_from(raw_count).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let bytes_left = rest.len().saturating_sub(varint_len);
    if count > bytes_left / 2 {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    let mut cursor = varint_len;
    for _ in 0..count {
        if cursor >= rest.len() {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }
        let skipped_key = skip_value_depth(&rest[cursor..], depth + 1)?;
        cursor += skipped_key;

        if cursor >= rest.len() {
            return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
        }
        let skipped_val = skip_value_depth(&rest[cursor..], depth + 1)?;
        cursor += skipped_val;
    }
    Ok(1 + cursor)
}

fn skip_error(rest: &[u8]) -> Result<usize, ProtoError> {
    let (raw_code, code_varint_len) = match decode_uvarint(rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let _code = u16::try_from(raw_code).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

    let msg_rest = rest
        .get(code_varint_len..)
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
    let (raw_msg_len, msg_len_varint_len) = match decode_uvarint(msg_rest)? {
        Decoded::Complete((val, len)) => (val, len),
        Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
    };
    let msg_len = usize::try_from(raw_msg_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

    let header_consumed = code_varint_len + msg_len_varint_len;
    let available = rest.len().saturating_sub(header_consumed);
    if available < msg_len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }
    Ok(1 + header_consumed + msg_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct XorShift64(u64);

    impl XorShift64 {
        fn new(seed: u64) -> Self {
            Self(if seed == 0 {
                0xDEAD_BEEF_CAFE_BABE
            } else {
                seed
            })
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    fn random_value(rng: &mut XorShift64, depth: usize) -> Value {
        let kind = if depth >= 4 {
            rng.next_u64() % 6
        } else {
            rng.next_u64() % 9
        };
        match kind {
            0 => Value::Nil,
            1 => Value::Int(rng.next_u64() as i64),
            2 => Value::Float(f64::from_bits(rng.next_u64())),
            3 => Value::Bool(rng.next_u64() % 2 == 1),
            4 => {
                let len = (rng.next_u64() % 16) as usize;
                let mut s = String::with_capacity(len);
                for _ in 0..len {
                    let ch = (b'a' + (rng.next_u64() % 26) as u8) as char;
                    s.push(ch);
                }
                Value::Text(s)
            }
            5 => {
                let len = (rng.next_u64() % 16) as usize;
                let mut b = Vec::with_capacity(len);
                for _ in 0..len {
                    b.push(rng.next_u64() as u8);
                }
                Value::Bytes(b)
            }
            6 => {
                let count = (rng.next_u64() % 4) as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(random_value(rng, depth + 1));
                }
                Value::Array(items)
            }
            7 => {
                let count = (rng.next_u64() % 3) as usize;
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let k = random_value(rng, depth + 1);
                    let v = random_value(rng, depth + 1);
                    pairs.push((k, v));
                }
                Value::Map(pairs)
            }
            8 => {
                let code = (rng.next_u64() % 5000) as u16;
                let len = (rng.next_u64() % 16) as usize;
                let mut s = String::with_capacity(len);
                for _ in 0..len {
                    let ch = (b'a' + (rng.next_u64() % 26) as u8) as char;
                    s.push(ch);
                }
                Value::Error { code, message: s }
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_round_trip_all_variants_with_edges() {
        let edge_values = vec![
            Value::Nil,
            Value::Int(0),
            Value::Int(1),
            Value::Int(-1),
            Value::Int(i64::MIN),
            Value::Int(i64::MAX),
            Value::Float(0.0),
            Value::Float(-0.0),
            Value::Float(1.5),
            Value::Float(f64::MAX),
            Value::Float(f64::MIN_POSITIVE),
            Value::Float(f64::INFINITY),
            Value::Float(f64::NEG_INFINITY),
            Value::Float(f64::NAN),
            Value::Bool(true),
            Value::Bool(false),
            Value::Text(String::new()),
            Value::Text("a".to_string()),
            Value::Text("héllo ✓".to_string()),
            Value::Bytes(Vec::new()),
            Value::Bytes((0u8..=255).collect()),
        ];

        for val in edge_values {
            let mut buf = Vec::new();
            encode_value(&val, &mut buf).expect("encode succeeds");
            let (decoded, consumed) = decode_value(&buf).expect("decode succeeds");
            assert_eq!(consumed, buf.len());
            assert_eq!(decoded, val);
        }
    }

    #[test]
    fn test_round_trip_all_scalar_and_compound_variants() {
        let test_cases = vec![
            // Empty Array
            Value::Array(Vec::new()),
            // Nested Array
            Value::Array(vec![
                Value::Array(vec![Value::Int(1), Value::Int(2)]),
                Value::Array(vec![Value::Text("inner".to_string())]),
            ]),
            // Array of mixed scalars
            Value::Array(vec![
                Value::Nil,
                Value::Int(-42),
                Value::Float(1.25),
                Value::Bool(true),
                Value::Text("mixed".to_string()),
                Value::Bytes(vec![0xAA, 0xBB]),
            ]),
            // Empty Map
            Value::Map(Vec::new()),
            // Map with Text keys
            Value::Map(vec![
                (
                    Value::Text("key1".to_string()),
                    Value::Text("val1".to_string()),
                ),
                (Value::Text("key2".to_string()), Value::Int(100)),
            ]),
            // Map with mixed keys (spec allows ordered pairs of typed items)
            Value::Map(vec![
                (Value::Int(1), Value::Text("one".to_string())),
                (Value::Bytes(vec![0x01]), Value::Bool(false)),
                (Value::Nil, Value::Float(0.0)),
            ]),
            // Error item
            Value::Error {
                code: 2001,
                message: "unknown command".to_string(),
            },
            // Array containing a Map containing an Error
            Value::Array(vec![Value::Map(vec![(
                Value::Text("err_entry".to_string()),
                Value::Error {
                    code: 1002,
                    message: "unsupported flag".to_string(),
                },
            )])]),
        ];

        for val in test_cases {
            let mut buf = Vec::new();
            encode_value(&val, &mut buf).expect("encode succeeds");
            let (decoded, consumed) = decode_value(&buf).expect("decode succeeds");
            assert_eq!(consumed, buf.len());
            assert_eq!(decoded, val);

            let skipped = skip_value(&buf).expect("skip_value succeeds");
            assert_eq!(skipped, buf.len());
        }
    }

    #[test]
    fn test_exact_bytes_for_each_type() {
        // Scalar types
        let mut b_nil = Vec::new();
        encode_value(&Value::Nil, &mut b_nil).unwrap();
        assert_eq!(b_nil, vec![0x00]);

        let mut b_int = Vec::new();
        encode_value(&Value::Int(42), &mut b_int).unwrap();
        assert_eq!(b_int, vec![0x01, 0x54]);

        let mut b_flt = Vec::new();
        encode_value(&Value::Float(1.0), &mut b_flt).unwrap();
        assert_eq!(
            b_flt,
            vec![0x02, 0x3F, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );

        let mut b_bool_false = Vec::new();
        encode_value(&Value::Bool(false), &mut b_bool_false).unwrap();
        assert_eq!(b_bool_false, vec![0x03, 0x00]);

        let mut b_bool_true = Vec::new();
        encode_value(&Value::Bool(true), &mut b_bool_true).unwrap();
        assert_eq!(b_bool_true, vec![0x03, 0x01]);

        let mut b_txt = Vec::new();
        encode_value(&Value::Text("hi".to_string()), &mut b_txt).unwrap();
        assert_eq!(b_txt, vec![0x04, 0x02, b'h', b'i']);

        let mut b_raw = Vec::new();
        encode_value(&Value::Bytes(vec![0xAA, 0xBB]), &mut b_raw).unwrap();
        assert_eq!(b_raw, vec![0x05, 0x02, 0xAA, 0xBB]);

        // Compound types: Array, Map, Error
        // Array([Int(42)]): Tag 0x06, count 1 (0x01), Int 42 (0x01, 0x54)
        let mut arr_out = Vec::new();
        encode_value(&Value::Array(vec![Value::Int(42)]), &mut arr_out).unwrap();
        assert_eq!(arr_out, vec![0x06, 0x01, 0x01, 0x54]);

        // Map([(Text("k"), Int(1))]):
        // Tag 0x07, count 1 (0x01), Text "k" (0x04, 0x01, 0x6B), Int 1 (0x01, 0x02)
        let mut map_buf = Vec::new();
        encode_value(
            &Value::Map(vec![(Value::Text("k".to_string()), Value::Int(1))]),
            &mut map_buf,
        )
        .unwrap();
        assert_eq!(map_buf, vec![0x07, 0x01, 0x04, 0x01, 0x6B, 0x01, 0x02]);

        // Error { code: 2001, message: "err" }:
        // Tag 0x08, code 2001 (0xD1, 0x0F), len 3 (0x03), UTF-8 "err" (0x65, 0x72, 0x72)
        let mut err_wire = Vec::new();
        encode_value(
            &Value::Error {
                code: 2001,
                message: "err".to_string(),
            },
            &mut err_wire,
        )
        .unwrap();
        assert_eq!(err_wire, vec![0x08, 0xD1, 0x0F, 0x03, 0x65, 0x72, 0x72]);
    }

    #[test]
    fn test_nesting_depth_limit_boundary_and_rejections() {
        // Construct collection nesting exactly at limit (8 levels)
        // Level 8: Array([])
        let mut deep_8 = Value::Array(Vec::new());
        for _ in 1..8 {
            deep_8 = Value::Array(vec![deep_8]);
        }

        // Depth 8 encodes and decodes cleanly
        let mut buf_8 = Vec::new();
        encode_value(&deep_8, &mut buf_8).expect("depth 8 encodes cleanly");
        let (decoded_8, len_8) = decode_value(&buf_8).expect("depth 8 decodes cleanly");
        assert_eq!(len_8, buf_8.len());
        assert_eq!(decoded_8, deep_8);

        // Construct collection nesting at limit + 1 (9 levels)
        let deep_9 = Value::Array(vec![deep_8]);

        // Depth 9 fails encode with ERR_MAX_DEPTH_EXCEEDED (3005)
        let mut buf_9 = Vec::new();
        let enc_err = encode_value(&deep_9, &mut buf_9).unwrap_err();
        assert_eq!(enc_err, ProtoError::ERR_MAX_DEPTH_EXCEEDED);

        // Synthesize wire payload with 9 nesting levels: Array(1) x 9 + Nil
        let mut wire_9 = Vec::new();
        for _ in 0..9 {
            wire_9.push(TAG_ARRAY);
            wire_9.push(0x01); // count 1
        }
        wire_9.push(TAG_NIL);

        let dec_err = decode_value(&wire_9).unwrap_err();
        assert_eq!(dec_err, ProtoError::ERR_MAX_DEPTH_EXCEEDED);

        let skip_err = skip_value(&wire_9).unwrap_err();
        assert_eq!(skip_err, ProtoError::ERR_MAX_DEPTH_EXCEEDED);
    }

    #[test]
    fn test_allocation_protection_excessive_count() {
        // Array count = u64::MAX with few bytes -> fails immediately without huge allocation
        let mut arr_huge = vec![TAG_ARRAY];
        encode_uvarint(u64::MAX, &mut arr_huge);
        arr_huge.extend_from_slice(&[0x01, 0x02, 0x03]);
        let err1 = decode_value(&arr_huge).unwrap_err();
        assert_eq!(err1, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Array count = 1000 with 3 bytes of data -> fails immediately
        let mut arr_1000 = vec![TAG_ARRAY];
        encode_uvarint(1000, &mut arr_1000);
        arr_1000.extend_from_slice(&[0x00, 0x00, 0x00]);
        let err2 = decode_value(&arr_1000).unwrap_err();
        assert_eq!(err2, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Map count = 1000 with 3 bytes of data -> fails immediately
        let mut map_1000 = vec![TAG_MAP];
        encode_uvarint(1000, &mut map_1000);
        map_1000.extend_from_slice(&[0x00, 0x00, 0x00]);
        let err3 = decode_value(&map_1000).unwrap_err();
        assert_eq!(err3, ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    #[test]
    fn test_truncated_compound_items_return_error_1006() {
        // Truncated Array: tag only
        assert_eq!(
            decode_value(&[TAG_ARRAY]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Array: incomplete varint count
        assert_eq!(
            decode_value(&[TAG_ARRAY, 0x80]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Array: declared 2 items, only 1 provided
        assert_eq!(
            decode_value(&[TAG_ARRAY, 0x02, TAG_NIL]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Truncated Map: tag only
        assert_eq!(
            decode_value(&[TAG_MAP]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Map: incomplete varint count
        assert_eq!(
            decode_value(&[TAG_MAP, 0x80]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Map: declared 1 pair, only key provided
        assert_eq!(
            decode_value(&[TAG_MAP, 0x01, TAG_NIL]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Truncated Error: tag only
        assert_eq!(
            decode_value(&[TAG_ERROR]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Error: code varint cut
        assert_eq!(
            decode_value(&[TAG_ERROR, 0x80]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Error: code present, missing message length
        assert_eq!(
            decode_value(&[TAG_ERROR, 0x01]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Error: message length cut
        assert_eq!(
            decode_value(&[TAG_ERROR, 0x01, 0x80]),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
        // Truncated Error: declared message length 5, only 2 bytes present
        assert_eq!(
            decode_value(&[TAG_ERROR, 0x01, 0x05, b'o', b'k']),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
    }

    #[test]
    fn test_unknown_tag_0x09_and_0xff_skipped_per_spec() {
        // Unknown tag 0x09: tag + varint len 3 + 3 data bytes
        let item_09 = [0x09, 0x03, 0xAA, 0xBB, 0xCC];
        let skipped_09 = skip_value(&item_09).expect("skips tag 0x09");
        assert_eq!(skipped_09, 5);

        // Unknown tag 0xFF: tag + varint len 2 + 2 data bytes
        let item_ff = [0xFF, 0x02, 0x11, 0x22];
        let skipped_ff = skip_value(&item_ff).expect("skips tag 0xFF");
        assert_eq!(skipped_ff, 4);

        // Standalone decode_value returns ERR_UNKNOWN_TYPE_TAG (1007)
        assert_eq!(
            decode_value(&item_09),
            Err(ProtoError::ERR_UNKNOWN_TYPE_TAG)
        );
        assert_eq!(
            decode_value(&item_ff),
            Err(ProtoError::ERR_UNKNOWN_TYPE_TAG)
        );

        // Inside Array: elements with tag 0x09 and 0xFF are safely skipped per Section 4.3
        let mut arr_payload = Vec::new();
        arr_payload.push(TAG_ARRAY);
        encode_uvarint(3, &mut arr_payload);
        arr_payload.extend_from_slice(&item_09);
        encode_value(&Value::Int(42), &mut arr_payload).unwrap();
        arr_payload.extend_from_slice(&item_ff);

        let (val, consumed) = decode_value(&arr_payload).expect("decodes array skipping unknown");
        assert_eq!(consumed, arr_payload.len());
        assert_eq!(val, Value::Array(vec![Value::Int(42)]));

        // Truncated unknown tag data -> ERR_MALFORMED_PAYLOAD (1006)
        let truncated_09 = [0x09, 0x05, 0xAA];
        assert_eq!(
            skip_value(&truncated_09),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
    }

    #[test]
    fn test_error_item_invalid_code_and_utf8() {
        // Error code exceeding u16 (e.g. 70000 = 0x82 0xA2 0x04)
        let mut bad_code = vec![TAG_ERROR];
        encode_uvarint(70000, &mut bad_code);
        bad_code.extend_from_slice(&[0x02, b'o', b'k']);
        assert_eq!(
            decode_value(&bad_code),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );

        // Invalid UTF-8 message string
        let mut bad_utf8 = vec![TAG_ERROR];
        encode_uvarint(2001, &mut bad_utf8);
        encode_uvarint(2, &mut bad_utf8);
        bad_utf8.extend_from_slice(&[0xFF, 0xFE]); // Invalid UTF-8 bytes
        assert_eq!(
            decode_value(&bad_utf8),
            Err(ProtoError::ERR_MALFORMED_PAYLOAD)
        );
    }

    #[test]
    fn test_protocol_spec_hex_examples_all_eight_payloads() {
        // Example 1: Request (PING)
        // Payload: 06 01 04 04 70 69 6E 67
        let ex1_payload = [0x06, 0x01, 0x04, 0x04, 0x70, 0x69, 0x6E, 0x67];
        let ex1_expected = Value::Array(vec![Value::Text("ping".to_string())]);
        let (val1, len1) = decode_value(&ex1_payload).unwrap();
        assert_eq!(len1, 8);
        assert_eq!(val1, ex1_expected);
        let mut enc1 = Vec::new();
        encode_value(&val1, &mut enc1).unwrap();
        assert_eq!(enc1, ex1_payload);

        // Example 2: Reply (PONG)
        // Payload: 04 04 70 6F 6E 67
        let ex2_payload = [0x04, 0x04, 0x70, 0x6F, 0x6E, 0x67];
        let ex2_expected = Value::Text("pong".to_string());
        let (val2, len2) = decode_value(&ex2_payload).unwrap();
        assert_eq!(len2, 6);
        assert_eq!(val2, ex2_expected);
        let mut enc2 = Vec::new();
        encode_value(&val2, &mut enc2).unwrap();
        assert_eq!(enc2, ex2_payload);

        // Example 3: Error Reply (Unknown Command)
        // Payload: 08 D1 0F 0F 75 6E 6B 6E 6F 77 6E 20 63 6F 6D 6D 61 6E 64
        let ex3_payload = [
            0x08, 0xD1, 0x0F, 0x0F, 0x75, 0x6E, 0x6B, 0x6E, 0x6F, 0x77, 0x6E, 0x20, 0x63, 0x6F,
            0x6D, 0x6D, 0x61, 0x6E, 0x64,
        ];
        let ex3_expected = Value::Error {
            code: 2001,
            message: "unknown command".to_string(),
        };
        let (val3, len3) = decode_value(&ex3_payload).unwrap();
        assert_eq!(len3, 19);
        assert_eq!(val3, ex3_expected);
        let mut enc3 = Vec::new();
        encode_value(&val3, &mut enc3).unwrap();
        assert_eq!(enc3, ex3_payload);

        // Example 4: Push Message (Pub/Sub Event)
        // Payload: 06 02 04 04 6E 65 77 73 04 05 68 65 6C 6C 6F
        let ex4_payload = [
            0x06, 0x02, 0x04, 0x04, 0x6E, 0x65, 0x77, 0x73, 0x04, 0x05, 0x68, 0x65, 0x6C, 0x6C,
            0x6F,
        ];
        let ex4_expected = Value::Array(vec![
            Value::Text("news".to_string()),
            Value::Text("hello".to_string()),
        ]);
        let (val4, len4) = decode_value(&ex4_payload).unwrap();
        assert_eq!(len4, 15);
        assert_eq!(val4, ex4_expected);
        let mut enc4 = Vec::new();
        encode_value(&val4, &mut enc4).unwrap();
        assert_eq!(enc4, ex4_payload);

        // Example 5 Frame 1: PUT user 42
        // Payload: 06 03 04 03 70 75 74 04 04 75 73 65 72 01 54
        let ex5_f1_payload = [
            0x06, 0x03, 0x04, 0x03, 0x70, 0x75, 0x74, 0x04, 0x04, 0x75, 0x73, 0x65, 0x72, 0x01,
            0x54,
        ];
        let ex5_f1_expected = Value::Array(vec![
            Value::Text("put".to_string()),
            Value::Text("user".to_string()),
            Value::Int(42),
        ]);
        let (val5_1, len5_1) = decode_value(&ex5_f1_payload).unwrap();
        assert_eq!(len5_1, 15);
        assert_eq!(val5_1, ex5_f1_expected);
        let mut enc5_1 = Vec::new();
        encode_value(&val5_1, &mut enc5_1).unwrap();
        assert_eq!(enc5_1, ex5_f1_payload);

        // Example 5 Frame 2: FETCH user
        // Payload: 06 02 04 05 66 65 74 63 68 04 04 75 73 65 72
        let ex5_f2_payload = [
            0x06, 0x02, 0x04, 0x05, 0x66, 0x65, 0x74, 0x63, 0x68, 0x04, 0x04, 0x75, 0x73, 0x65,
            0x72,
        ];
        let ex5_f2_expected = Value::Array(vec![
            Value::Text("fetch".to_string()),
            Value::Text("user".to_string()),
        ]);
        let (val5_2, len5_2) = decode_value(&ex5_f2_payload).unwrap();
        assert_eq!(len5_2, 15);
        assert_eq!(val5_2, ex5_f2_expected);
        let mut enc5_2 = Vec::new();
        encode_value(&val5_2, &mut enc5_2).unwrap();
        assert_eq!(enc5_2, ex5_f2_payload);

        // Example 6: Split Frame in Two TCP Reads (Cut in Payload) - full payload is same as Example 1
        let (val6, len6) = decode_value(&ex1_payload).unwrap();
        assert_eq!(len6, 8);
        assert_eq!(val6, ex1_expected);

        // Example 7: Split Frame in Two TCP Reads (Cut in Header) - full payload is same as Example 2
        let (val7, len7) = decode_value(&ex2_payload).unwrap();
        assert_eq!(len7, 6);
        assert_eq!(val7, ex2_expected);

        // Example 8: Unsupported Reserved Flag - Error Response payload:
        // 08 EA 07 10 75 6E 73 75 70 70 6F 72 74 65 64 20 66 6C 61 67
        let ex8_payload = [
            0x08, 0xEA, 0x07, 0x10, 0x75, 0x6E, 0x73, 0x75, 0x70, 0x70, 0x6F, 0x72, 0x74, 0x65,
            0x64, 0x20, 0x66, 0x6C, 0x61, 0x67,
        ];
        let ex8_expected = Value::Error {
            code: 1002,
            message: "unsupported flag".to_string(),
        };
        let (val8, len8) = decode_value(&ex8_payload).unwrap();
        assert_eq!(len8, 20);
        assert_eq!(val8, ex8_expected);
        let mut enc8 = Vec::new();
        encode_value(&val8, &mut enc8).unwrap();
        assert_eq!(enc8, ex8_payload);
    }

    #[test]
    fn test_deterministic_round_trip_500_random_values_depth_4() {
        let mut rng = XorShift64::new(0xABCD_EF01_2345_6789);
        for _ in 0..500 {
            let val = random_value(&mut rng, 0);
            let mut buf = Vec::new();
            encode_value(&val, &mut buf).expect("encode random value succeeds");
            let (decoded, consumed) = decode_value(&buf).expect("decode random value succeeds");
            assert_eq!(consumed, buf.len());
            assert_eq!(decoded, val);

            let skipped = skip_value(&buf).expect("skip random value succeeds");
            assert_eq!(skipped, buf.len());
        }
    }

    #[test]
    fn test_no_panic_on_3000_pseudorandom_byte_inputs() {
        let mut rng = XorShift64::new(0x9876_5432_10FE_DCBA);
        for _ in 0..3000 {
            let len = (rng.next_u64() % 97) as usize;
            let mut garbage = Vec::with_capacity(len);
            for _ in 0..len {
                garbage.push(rng.next_u64() as u8);
            }

            let _ = decode_value(&garbage);
            let _ = skip_value(&garbage);
        }
    }

    #[test]
    fn test_used_bytes_leaves_trailing_bytes_intact() {
        let mut input = Vec::new();
        encode_value(&Value::Text("hello".to_string()), &mut input).unwrap();
        input.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let (val, consumed) = decode_value(&input).unwrap();
        assert_eq!(val, Value::Text("hello".to_string()));
        assert_eq!(&input[consumed..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }
}
