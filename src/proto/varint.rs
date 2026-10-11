//! Variable-length integer encoding and decoding (LEB128 and Zigzag).
//!
//! Implements Section 4.1 of `docs/spec/protocol.md`:
//! - **Unsigned Varint (LEB128)**: 7 data bits per byte with MSB (bit 7) as continuation flag.
//!   Values span up to 64 bits (at most 10 bytes on the wire).
//! - **Signed Varint (Zigzag)**: Maps signed 64-bit values to positive unsigned integers:
//!   `zigzag(n) = (n << 1) ^ (n >> 63)`.

use crate::proto::Decoded;
use crate::proto::header::ProtoError;

/// Maximum number of bytes an unsigned 64-bit LEB128 varint can occupy on the wire.
pub const MAX_VARINT_BYTES: usize = 10;

/// Maps a signed 64-bit integer to an unsigned 64-bit integer using Zigzag encoding.
///
/// Ensures small negative integers encode into small positive values with few varint bytes:
/// `0 -> 0, -1 -> 1, 1 -> 2, -2 -> 3, 42 -> 84`.
#[inline]
#[must_use]
pub const fn zigzag_encode(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

/// Decodes an unsigned 64-bit integer back to a signed 64-bit integer using Zigzag decoding.
#[inline]
#[must_use]
pub const fn zigzag_decode(n: u64) -> i64 {
    ((n >> 1) as i64) ^ (-((n & 1) as i64))
}

/// Encodes an unsigned 64-bit integer into a buffer using LEB128 variable-length encoding.
pub fn encode_uvarint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Decodes an unsigned 64-bit LEB128 integer from an input byte slice.
///
/// # Returns
///
/// - `Ok(Decoded::Complete((value, bytes_consumed)))`: The varint was parsed successfully.
/// - `Ok(Decoded::NeedMore)`: The input ends before the varint's terminating byte is reached.
///
/// # Errors
///
/// Returns [`ProtoError::ERR_MALFORMED_VARINT`] (`1005`) if the varint exceeds 10 bytes
/// or if bits in the 10th byte would overflow a 64-bit integer.
pub fn decode_uvarint(input: &[u8]) -> Result<Decoded<(u64, usize)>, ProtoError> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;

    for (i, &byte) in input.iter().enumerate() {
        if i == 9 {
            // 10th byte: only 1 bit of value fits in a 64-bit integer (shift 63).
            // Bit 7 (continuation) must be 0, and bits 1..6 must be 0 to avoid overflow.
            if (byte & 0x80) != 0 || (byte & 0x7E) != 0 {
                return Err(ProtoError::ERR_MALFORMED_VARINT);
            }
            let val = u64::from(byte & 0x01);
            result |= val << 63;
            return Ok(Decoded::Complete((result, i + 1)));
        }

        let val = u64::from(byte & 0x7F);
        result |= val << shift;
        shift += 7;

        if (byte & 0x80) == 0 {
            return Ok(Decoded::Complete((result, i + 1)));
        }
    }

    Ok(Decoded::NeedMore)
}

/// Encodes a signed 64-bit integer into a buffer using Zigzag LEB128 encoding.
pub fn encode_ivarint(value: i64, out: &mut Vec<u8>) {
    encode_uvarint(zigzag_encode(value), out);
}

/// Decodes a signed 64-bit Zigzag LEB128 integer from an input byte slice.
///
/// # Returns
///
/// - `Ok(Decoded::Complete((value, bytes_consumed)))`: The signed varint was parsed successfully.
/// - `Ok(Decoded::NeedMore)`: The input buffer ends before the terminating byte.
///
/// # Errors
///
/// Returns [`ProtoError::ERR_MALFORMED_VARINT`] (`1005`) if the varint exceeds 10 bytes
/// or overflows 64 bits.
pub fn decode_ivarint(input: &[u8]) -> Result<Decoded<(i64, usize)>, ProtoError> {
    match decode_uvarint(input)? {
        Decoded::NeedMore => Ok(Decoded::NeedMore),
        Decoded::Complete((uval, len)) => Ok(Decoded::Complete((zigzag_decode(uval), len))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct XorShift64(u64);

    impl XorShift64 {
        fn new(seed: u64) -> Self {
            Self(if seed == 0 {
                0x1234_5678_9ABC_DEF0
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
    fn test_uvarint_round_trip_critical_values() {
        let test_cases = [
            0u64,
            1,
            127,
            128,
            255,
            16_383,
            16_384,
            u64::from(u32::MAX),
            u64::MAX,
        ];

        for &val in &test_cases {
            let mut buf = Vec::new();
            encode_uvarint(val, &mut buf);
            let decoded = decode_uvarint(&buf).expect("decode succeeds");
            assert_eq!(decoded, Decoded::Complete((val, buf.len())));
        }
    }

    #[test]
    fn test_ivarint_round_trip_critical_values() {
        let test_cases = [
            0i64,
            -1,
            1,
            -42,
            42,
            -127,
            127,
            -128,
            128,
            i64::from(i32::MIN),
            i64::from(i32::MAX),
            i64::MIN,
            i64::MAX,
        ];

        for &val in &test_cases {
            let mut buf = Vec::new();
            encode_ivarint(val, &mut buf);
            let decoded = decode_ivarint(&buf).expect("decode succeeds");
            assert_eq!(decoded, Decoded::Complete((val, buf.len())));
        }
    }

    #[test]
    fn test_uvarint_exact_bytes() {
        // 0 -> [0x00]
        let mut b0 = Vec::new();
        encode_uvarint(0, &mut b0);
        assert_eq!(b0, [0x00]);

        // 127 -> [0x7F]
        let mut b127 = Vec::new();
        encode_uvarint(127, &mut b127);
        assert_eq!(b127, [0x7F]);

        // 128 -> [0x80, 0x01]
        let mut b128 = Vec::new();
        encode_uvarint(128, &mut b128);
        assert_eq!(b128, [0x80, 0x01]);

        // 300 -> [0xAC, 0x02] (300 = 44 + 2 * 128)
        let mut b300 = Vec::new();
        encode_uvarint(300, &mut b300);
        assert_eq!(b300, [0xAC, 0x02]);

        // u64::MAX -> 10 bytes: 9 x 0xFF followed by 0x01
        let mut b_max = Vec::new();
        encode_uvarint(u64::MAX, &mut b_max);
        assert_eq!(
            b_max,
            [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]
        );
    }

    #[test]
    fn test_empty_and_truncated_returns_need_more() {
        assert_eq!(decode_uvarint(&[]).unwrap(), Decoded::NeedMore);
        assert_eq!(decode_ivarint(&[]).unwrap(), Decoded::NeedMore);

        // Continuation bit set without following byte
        assert_eq!(decode_uvarint(&[0x80]).unwrap(), Decoded::NeedMore);
        assert_eq!(decode_ivarint(&[0x80]).unwrap(), Decoded::NeedMore);

        // Multi-byte truncation
        assert_eq!(
            decode_uvarint(&[0xFF, 0xFF, 0xFF]).unwrap(),
            Decoded::NeedMore
        );
    }

    #[test]
    fn test_varint_overflow_and_too_long_returns_error_1005() {
        // 10th byte with continuation bit set -> exceeds 10 bytes
        let too_long = [0xFF; 10];
        let err = decode_uvarint(&too_long).unwrap_err();
        assert_eq!(err, ProtoError::ERR_MALFORMED_VARINT);
        assert_eq!(err.code(), 1005);

        // 10th byte with value bit 1 set (0x02), which would shift into bit 64 (overflow)
        let mut overflow_byte10 = [0x80; 10];
        overflow_byte10[9] = 0x02;
        let err2 = decode_uvarint(&overflow_byte10).unwrap_err();
        assert_eq!(err2, ProtoError::ERR_MALFORMED_VARINT);

        // 11 bytes input
        let eleven_bytes = [0xFF; 11];
        let err3 = decode_uvarint(&eleven_bytes).unwrap_err();
        assert_eq!(err3, ProtoError::ERR_MALFORMED_VARINT);
    }

    #[test]
    fn test_non_canonical_encoding_accepted_per_standard_leb128() {
        // [0x80, 0x00] is a non-shortest encoding of 0 taking 2 bytes
        let input = [0x80, 0x00];
        let res = decode_uvarint(&input).unwrap();
        assert_eq!(res, Decoded::Complete((0, 2)));

        // [0x80, 0x80, 0x00] non-shortest encoding of 0 taking 3 bytes
        let input3 = [0x80, 0x80, 0x00];
        let res3 = decode_uvarint(&input3).unwrap();
        assert_eq!(res3, Decoded::Complete((0, 3)));
    }

    #[test]
    fn test_trailing_extra_bytes_are_preserved() {
        // Varint 300 followed by arbitrary trailing data
        let input = [0xAC, 0x02, 0xDE, 0xAD, 0xBE, 0xEF];
        let res = decode_uvarint(&input).unwrap();
        assert_eq!(res, Decoded::Complete((300, 2)));

        let res_signed = decode_ivarint(&input).unwrap();
        assert_eq!(res_signed, Decoded::Complete((zigzag_decode(300), 2)));
    }

    #[test]
    fn test_protocol_spec_hex_examples_varints() {
        // Example 3: Error Code 2001 -> D1 0F
        let ex3 = [0xD1, 0x0F];
        let res3 = decode_uvarint(&ex3).unwrap();
        assert_eq!(res3, Decoded::Complete((2001, 2)));

        // Example 5: Int 42 (zigzag 84 = 0x54) -> 54
        let ex5 = [0x54];
        let res5 = decode_ivarint(&ex5).unwrap();
        assert_eq!(res5, Decoded::Complete((42, 1)));

        // Example 8: Error Code 1002 -> EA 07
        let ex8 = [0xEA, 0x07];
        let res8 = decode_uvarint(&ex8).unwrap();
        assert_eq!(res8, Decoded::Complete((1002, 2)));
    }

    #[test]
    fn test_pseudorandom_round_trip_loop_1000() {
        let mut rng = XorShift64::new(42);

        for _ in 0..1000 {
            let uval = rng.next_u64();
            let mut ubuf = Vec::new();
            encode_uvarint(uval, &mut ubuf);
            assert_eq!(
                decode_uvarint(&ubuf).unwrap(),
                Decoded::Complete((uval, ubuf.len()))
            );

            let ival = uval as i64;
            let mut ibuf = Vec::new();
            encode_ivarint(ival, &mut ibuf);
            assert_eq!(
                decode_ivarint(&ibuf).unwrap(),
                Decoded::Complete((ival, ibuf.len()))
            );
        }
    }
}
