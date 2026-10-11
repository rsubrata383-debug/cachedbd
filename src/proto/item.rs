//! Typed binary payload item serialization and deserialization.
//!
//! Implements Section 4.2 of `docs/spec/protocol.md` for scalar payload items:
//! - `Nil` (`0x00`): Tag only (1 byte).
//! - `Int` (`0x01`): Tag + signed Zigzag varint (64-bit integer).
//! - `Float` (`0x02`): Tag + 8-byte big-endian IEEE-754 double precision float.
//! - `Bool` (`0x03`): Tag + 1 byte (`0x00` = false, `0x01` = true).
//! - `Text` (`0x04`): Tag + unsigned LEB128 varint byte length + UTF-8 bytes.
//! - `Bytes` (`0x05`): Tag + unsigned LEB128 varint byte length + raw bytes.
//!
//! Note: Compound items (`Array` `0x06`, `Map` `0x07`, and `Error` `0x08`) will be
//! implemented in Step 3b.

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

/// An owned typed value representing a scalar payload item in the Cachedb protocol.
///
/// Compound items (`Array`, `Map`, `Error`) are deferred to Phase 1 Week 5 Step 3b.
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
            _ => false,
        }
    }
}

impl Eq for Value {}

/// Serializes a scalar [`Value`] into an output buffer per Section 4.2 of `docs/spec/protocol.md`.
pub fn encode_value(value: &Value, out: &mut Vec<u8>) {
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
    }
}

/// Deserializes a scalar [`Value`] from a slice representing a complete frame payload.
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
///   available buffer bytes, boolean byte is not `0x00`/`0x01`, or text contains invalid UTF-8.
/// - [`ProtoError::ERR_UNKNOWN_TYPE_TAG`] (`1007`): Unrecognized tag byte encountered.
/// - [`ProtoError::ERR_MALFORMED_VARINT`] (`1005`): Varint encoding is corrupted or overflows 64 bits.
pub fn decode_value(input: &[u8]) -> Result<(Value, usize), ProtoError> {
    let (&tag, rest) = input
        .split_first()
        .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;

    match tag {
        TAG_NIL => Ok((Value::Nil, 1)),
        TAG_INT => {
            let (n, varint_len) = match decode_ivarint(rest)? {
                Decoded::Complete((val, len)) => (val, len),
                Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
            };
            Ok((Value::Int(n), 1 + varint_len))
        }
        TAG_FLOAT => {
            let float_bytes = rest.get(0..8).ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
            let bits = u64::from_be_bytes(
                float_bytes
                    .try_into()
                    .map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?,
            );
            Ok((Value::Float(f64::from_bits(bits)), 1 + 8))
        }
        TAG_BOOL => {
            let (&b, _) = rest
                .split_first()
                .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
            match b {
                0x00 => Ok((Value::Bool(false), 2)),
                0x01 => Ok((Value::Bool(true), 2)),
                _ => Err(ProtoError::ERR_MALFORMED_PAYLOAD),
            }
        }
        TAG_TEXT => {
            let (raw_len, varint_len) = match decode_uvarint(rest)? {
                Decoded::Complete((val, len)) => (val, len),
                Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
            };
            let len = usize::try_from(raw_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

            // Protection against bad input: ensure enough payload bytes exist BEFORE allocating
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
        TAG_BYTES => {
            let (raw_len, varint_len) = match decode_uvarint(rest)? {
                Decoded::Complete((val, len)) => (val, len),
                Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
            };
            let len = usize::try_from(raw_len).map_err(|_| ProtoError::ERR_MALFORMED_PAYLOAD)?;

            // Protection against bad input: verify available bytes before allocating Vec
            let available = rest.len().saturating_sub(varint_len);
            if available < len {
                return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
            }

            let payload_bytes = rest
                .get(varint_len..varint_len + len)
                .ok_or(ProtoError::ERR_MALFORMED_PAYLOAD)?;
            let bytes = payload_bytes.to_vec();

            Ok((Value::Bytes(bytes), 1 + varint_len + len))
        }
        _ => Err(ProtoError::ERR_UNKNOWN_TYPE_TAG),
    }
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
            encode_value(&val, &mut buf);
            let (decoded, consumed) = decode_value(&buf).expect("decode succeeds");
            assert_eq!(consumed, buf.len());
            assert_eq!(decoded, val);
        }
    }

    #[test]
    fn test_exact_bytes_for_each_type() {
        // Nil -> 0x00
        let mut b_nil = Vec::new();
        encode_value(&Value::Nil, &mut b_nil);
        assert_eq!(b_nil, [0x00]);

        // Int 42 -> 0x01 followed by zigzag 84 (0x54)
        let mut b_int = Vec::new();
        encode_value(&Value::Int(42), &mut b_int);
        assert_eq!(b_int, [0x01, 0x54]);

        // Float 1.0 -> 0x02 followed by 0x3F 0xF0 0x00 0x00 0x00 0x00 0x00 0x00
        let mut b_flt = Vec::new();
        encode_value(&Value::Float(1.0), &mut b_flt);
        assert_eq!(
            b_flt,
            [0x02, 0x3F, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );

        // Bool false -> 0x03 0x00
        let mut b_bool_false = Vec::new();
        encode_value(&Value::Bool(false), &mut b_bool_false);
        assert_eq!(b_bool_false, [0x03, 0x00]);

        // Bool true -> 0x03 0x01
        let mut b_bool_true = Vec::new();
        encode_value(&Value::Bool(true), &mut b_bool_true);
        assert_eq!(b_bool_true, [0x03, 0x01]);

        // Text "hi" -> 0x04 0x02 'h' 'i'
        let mut b_txt = Vec::new();
        encode_value(&Value::Text("hi".to_string()), &mut b_txt);
        assert_eq!(b_txt, [0x04, 0x02, b'h', b'i']);

        // Bytes [0xAA, 0xBB] -> 0x05 0x02 0xAA 0xBB
        let mut b_raw = Vec::new();
        encode_value(&Value::Bytes(vec![0xAA, 0xBB]), &mut b_raw);
        assert_eq!(b_raw, [0x05, 0x02, 0xAA, 0xBB]);
    }

    #[test]
    fn test_protocol_spec_hex_examples_items() {
        // Example 1: Text "ping" -> 04 04 70 69 6E 67
        let ex1 = [0x04, 0x04, 0x70, 0x69, 0x6E, 0x67];
        let (val1, len1) = decode_value(&ex1).unwrap();
        assert_eq!(val1, Value::Text("ping".to_string()));
        assert_eq!(len1, 6);

        // Example 2: Text "pong" -> 04 04 70 6F 6E 67
        let ex2 = [0x04, 0x04, 0x70, 0x6F, 0x6E, 0x67];
        let (val2, len2) = decode_value(&ex2).unwrap();
        assert_eq!(val2, Value::Text("pong".to_string()));
        assert_eq!(len2, 6);

        // Example 5: Int 42 -> 01 54
        let ex5_int = [0x01, 0x54];
        let (val5, len5) = decode_value(&ex5_int).unwrap();
        assert_eq!(val5, Value::Int(42));
        assert_eq!(len5, 2);

        // Example 5: Text "put" -> 04 03 70 75 74
        let ex5_put = [0x04, 0x03, 0x70, 0x75, 0x74];
        let (val_put, len_put) = decode_value(&ex5_put).unwrap();
        assert_eq!(val_put, Value::Text("put".to_string()));
        assert_eq!(len_put, 5);

        // Example 5: Text "user" -> 04 04 75 73 65 72
        let ex5_user = [0x04, 0x04, 0x75, 0x73, 0x65, 0x72];
        let (val_user, len_user) = decode_value(&ex5_user).unwrap();
        assert_eq!(val_user, Value::Text("user".to_string()));
        assert_eq!(len_user, 6);
    }

    #[test]
    fn test_error_conditions_on_bad_inputs() {
        // Empty input -> error 1006
        let err_empty = decode_value(&[]).unwrap_err();
        assert_eq!(err_empty, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Unknown tag 0x7F -> error 1007
        let err_tag = decode_value(&[0x7F]).unwrap_err();
        assert_eq!(err_tag, ProtoError::ERR_UNKNOWN_TYPE_TAG);

        // Int with truncated varint -> error 1006
        let err_int = decode_value(&[TAG_INT, 0x80]).unwrap_err();
        assert_eq!(err_int, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Float with only 7 bytes instead of 8 -> error 1006
        let err_flt = decode_value(&[TAG_FLOAT, 1, 2, 3, 4, 5, 6, 7]).unwrap_err();
        assert_eq!(err_flt, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Bool byte 0x02 -> error 1006
        let err_bool = decode_value(&[TAG_BOOL, 0x02]).unwrap_err();
        assert_eq!(err_bool, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Text with invalid UTF-8 (e.g. 0xFF) -> error 1006
        let err_utf8 = decode_value(&[TAG_TEXT, 0x01, 0xFF]).unwrap_err();
        assert_eq!(err_utf8, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Text declaring length 10 but only 3 bytes present -> error 1006 (no giant allocation)
        let err_short_text = decode_value(&[TAG_TEXT, 10, b'a', b'b', b'c']).unwrap_err();
        assert_eq!(err_short_text, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Bytes declaring length 50 but only 2 present -> error 1006
        let err_short_bytes = decode_value(&[TAG_BYTES, 50, 0x01, 0x02]).unwrap_err();
        assert_eq!(err_short_bytes, ProtoError::ERR_MALFORMED_PAYLOAD);

        // Length = u64::MAX -> error (overflow/bounds check prevents allocation)
        let mut overflow_len_bytes = vec![TAG_TEXT];
        overflow_len_bytes
            .extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
        overflow_len_bytes.extend_from_slice(b"abc");
        let err_max_len = decode_value(&overflow_len_bytes).unwrap_err();
        assert_eq!(err_max_len, ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    #[test]
    fn test_used_bytes_leaves_trailing_bytes_intact() {
        let input = [TAG_BOOL, 0x01, 0xDE, 0xAD, 0xBE, 0xEF];
        let (val, consumed) = decode_value(&input).unwrap();
        assert_eq!(val, Value::Bool(true));
        assert_eq!(consumed, 2);
        assert_eq!(&input[consumed..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_deterministic_round_trip_1000_int_and_bytes() {
        let mut rng = XorShift64::new(12345);

        for _ in 0..1000 {
            // Int test
            let n = rng.next_u64() as i64;
            let val_int = Value::Int(n);
            let mut buf_int = Vec::new();
            encode_value(&val_int, &mut buf_int);
            let (dec_int, len_int) = decode_value(&buf_int).unwrap();
            assert_eq!(dec_int, val_int);
            assert_eq!(len_int, buf_int.len());

            // Bytes test
            let byte_count = (rng.next_u64() % 64) as usize;
            let mut raw = Vec::with_capacity(byte_count);
            for _ in 0..byte_count {
                raw.push((rng.next_u64() & 0xFF) as u8);
            }
            let val_bytes = Value::Bytes(raw);
            let mut buf_bytes = Vec::new();
            encode_value(&val_bytes, &mut buf_bytes);
            let (dec_bytes, len_bytes) = decode_value(&buf_bytes).unwrap();
            assert_eq!(dec_bytes, val_bytes);
            assert_eq!(len_bytes, buf_bytes.len());
        }
    }

    #[test]
    fn test_no_panic_on_2000_pseudorandom_byte_inputs() {
        let mut rng = XorShift64::new(99999);

        for _ in 0..2000 {
            let len = (rng.next_u64() % 65) as usize; // length 0..=64
            let mut garbage = Vec::with_capacity(len);
            for _ in 0..len {
                garbage.push((rng.next_u64() & 0xFF) as u8);
            }
            // Must return Ok or Err, never panic
            let _ = decode_value(&garbage);
        }
    }
}
