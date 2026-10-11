//! Streaming frame parser and request decoder.

use crate::proto::Decoded;
use crate::proto::Frame;
use crate::proto::header::{HEADER_LEN, Header, Opcode, ProtoError};
use crate::proto::item::{TAG_ARRAY, decode_value_with_max_depth};
use crate::proto::limits::Limits;
use crate::proto::varint::decode_uvarint;

/// Decodes a complete [`Frame`] from a stream buffer using the default protocol limits.
///
/// # Outcomes
///
/// 1. `Ok(Decoded::NeedMore)`: The buffer does not yet contain enough bytes to form a complete frame.
///    This occurs if `buf.len() < 14` (incomplete header) or `buf.len() < 14 + payload_len` (incomplete
///    payload). The caller should wait for more data from the network without discarding buffer contents.
/// 2. `Ok(Decoded::Complete((frame, used_bytes)))`: A complete and valid frame was parsed. `used_bytes`
///    is the exact number of bytes consumed (`14 + header.payload_len as usize`). The caller can consume
///    these bytes and immediately call `decode_frame` on the remainder of the buffer (pipelining).
/// 3. `Err(ProtoError)`: A framing, header, limits, or payload corruption error occurred (such as invalid magic,
///    unsupported version, reserved flag, opcode mismatch, payload exceeding 64 MB, argument count exceeding
///    65,536, or payload decoding failure). Per protocol specification, fatal framing errors indicate connection closure.
///
/// # Streaming Efficiency
///
/// No memory is allocated for the payload until the header has been validated and all payload bytes
/// have arrived in `buf`. Header errors are returned as soon as 14 bytes are available, without waiting
/// for the payload.
///
/// # Errors
///
/// - [`ProtoError::ERR_INVALID_MAGIC`] (`1004`): First 2 bytes are not `0xCA 0xDB`.
/// - [`ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION`] (`1001`): Version is not `1`.
/// - [`ProtoError::ERR_UNSUPPORTED_FLAG`] (`1002`): Reserved flag bits 3..7 are set.
/// - [`ProtoError::ERR_UNKNOWN_COMMAND`] (`2001`): Opcode is unrecognized.
/// - [`ProtoError::ERR_FRAME_TOO_LARGE`] (`1003`): `payload_len` exceeds 64 MB.
/// - [`ProtoError::ERR_TOO_MANY_ARGS`] (`2007`): COMMAND request Array count exceeds argument limit.
/// - [`ProtoError::ERR_MALFORMED_PAYLOAD`] (`1006`): Payload bytes are malformed, truncated, or contain
///   unconsumed trailing bytes after the single top-level item.
pub fn decode_frame(buf: &[u8]) -> Result<Decoded<(Frame, usize)>, ProtoError> {
    decode_frame_with_limits(buf, &Limits::default())
}

/// Decodes a complete [`Frame`] from a stream buffer with an explicit maximum payload size limit.
///
/// # Errors
///
/// Returns a [`ProtoError`] if validation fails against protocol specifications or payload size limit.
pub fn decode_frame_with_limit(
    buf: &[u8],
    max_payload_len: u32,
) -> Result<Decoded<(Frame, usize)>, ProtoError> {
    let limits = Limits {
        max_frame_size: max_payload_len as usize,
        ..Limits::default()
    };
    decode_frame_with_limits(buf, &limits)
}

/// Decodes a complete [`Frame`] from a stream buffer enforcing configurable operational [`Limits`].
///
/// # Limits Enforced
///
/// 1. `max_frame_size`: Frame payload size is validated immediately after 14 header bytes.
/// 2. `max_args`: If the frame is a `COMMAND` request whose top-level item is an `Array`, its item count
///    is validated as soon as the varint count is read, **before** decoding any items and before reserving memory.
///    Breaching this limit returns [`ProtoError::ERR_TOO_MANY_ARGS`] (`2007`).
/// 3. `max_depth`: Collection nesting depth is enforced against `limits.max_depth`.
///
/// # Errors
///
/// Returns a [`ProtoError`] if framing, header, limits, or payload validation fails.
pub fn decode_frame_with_limits(
    buf: &[u8],
    limits: &Limits,
) -> Result<Decoded<(Frame, usize)>, ProtoError> {
    if buf.len() < HEADER_LEN {
        return Ok(Decoded::NeedMore);
    }

    let max_payload_len = u32::try_from(limits.max_frame_size).unwrap_or(u32::MAX);

    // Decode and validate 14-byte header immediately.
    // If the header is invalid (e.g. payload_len > max), this returns Err right away,
    // before waiting for any payload bytes.
    let header = match Header::decode_with_max_len(&buf[..HEADER_LEN], max_payload_len)? {
        Decoded::Complete(hdr) => hdr,
        Decoded::NeedMore => return Ok(Decoded::NeedMore),
    };

    let payload_len = header.payload_len as usize;
    let total_frame_len = HEADER_LEN + payload_len;

    // Check if entire payload has arrived. If not, yield NeedMore without allocating.
    if buf.len() < total_frame_len {
        return Ok(Decoded::NeedMore);
    }

    let payload_slice = &buf[HEADER_LEN..total_frame_len];

    // For a COMMAND request, check max_args immediately before decoding array elements or reserving memory.
    if header.opcode == Opcode::Command
        && header.flags == 0x00
        && payload_slice.first() == Some(&TAG_ARRAY)
    {
        let (raw_count, _) = match decode_uvarint(&payload_slice[1..])? {
            Decoded::Complete((val, len)) => (val, len),
            Decoded::NeedMore => return Err(ProtoError::ERR_MALFORMED_PAYLOAD),
        };
        if raw_count > limits.max_args as u64 {
            return Err(ProtoError::ERR_TOO_MANY_ARGS);
        }
    }

    let (payload, consumed) = decode_value_with_max_depth(payload_slice, limits.max_depth)?;

    // Protocol requires the payload to match the declared payload_len exactly (no trailing junk).
    if consumed != payload_len {
        return Err(ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    let frame = Frame { header, payload };
    Ok(Decoded::Complete((frame, total_frame_len)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::encode::encode_frame;
    use crate::proto::header::{FLAG_ERROR, FLAG_PUSH, FLAG_RESPONSE, MAGIC, Opcode, VERSION};
    use crate::proto::item::Value;

    struct XorShift64(u64);

    impl XorShift64 {
        fn new(seed: u64) -> Self {
            Self(if seed == 0 {
                0xCAFE_BABE_DEAD_BEEF
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

    fn assert_frame_round_trip(wire_bytes: &[u8], frame: &Frame) {
        let mut encoded = Vec::new();
        encode_frame(frame, &mut encoded).expect("encode succeeds");
        assert_eq!(encoded, wire_bytes);
        assert_eq!(
            decode_frame(wire_bytes).expect("decode succeeds"),
            Decoded::Complete((frame.clone(), wire_bytes.len()))
        );
    }

    #[test]
    fn test_round_trip_all_eight_spec_examples() {
        // Spec Example 1: Request (PING)
        let ex1_bytes = [
            0xCA, 0xDB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x08,
            0x06, 0x01, 0x04, 0x04, 0x70, 0x69, 0x6E, 0x67,
        ];
        let ex1_frame = Frame::new(
            Header::new(0, 1, Opcode::Command, 8),
            Value::Array(vec![Value::Text("ping".to_string())]),
        );
        assert_frame_round_trip(&ex1_bytes, &ex1_frame);

        // Spec Example 2: Reply (PONG)
        let ex2_bytes = [
            0xCA, 0xDB, 0x01, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x06,
            0x04, 0x04, 0x70, 0x6F, 0x6E, 0x67,
        ];
        let ex2_frame = Frame::new(
            Header::new(FLAG_RESPONSE, 1, Opcode::Command, 6),
            Value::Text("pong".to_string()),
        );
        assert_frame_round_trip(&ex2_bytes, &ex2_frame);

        // Spec Example 3: Error Reply (Unknown Command)
        let ex3_bytes = [
            0xCA, 0xDB, 0x01, 0x03, 0x00, 0x00, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0x13,
            0x08, 0xD1, 0x0F, 0x0F, 0x75, 0x6E, 0x6B, 0x6E, 0x6F, 0x77, 0x6E, 0x20, 0x63, 0x6F,
            0x6D, 0x6D, 0x61, 0x6E, 0x64,
        ];
        let ex3_frame = Frame::new(
            Header::new(FLAG_RESPONSE | FLAG_ERROR, 2, Opcode::Command, 19),
            Value::Error {
                code: 2001,
                message: "unknown command".to_string(),
            },
        );
        assert_frame_round_trip(&ex3_bytes, &ex3_frame);

        // Spec Example 4: Push Message (Pub/Sub Event)
        let ex4_bytes = [
            0xCA, 0xDB, 0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x0F,
            0x06, 0x02, 0x04, 0x04, 0x6E, 0x65, 0x77, 0x73, 0x04, 0x05, 0x68, 0x65, 0x6C, 0x6C,
            0x6F,
        ];
        let ex4_frame = Frame::new(
            Header::new(FLAG_RESPONSE | FLAG_PUSH, 0, Opcode::Push, 15),
            Value::Array(vec![
                Value::Text("news".to_string()),
                Value::Text("hello".to_string()),
            ]),
        );
        assert_frame_round_trip(&ex4_bytes, &ex4_frame);

        // Spec Example 5 Frame 1: PUT user 42
        let ex5_f1_bytes = [
            0xCA, 0xDB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x02, 0x00, 0x00, 0x00, 0x0F,
            0x06, 0x03, 0x04, 0x03, 0x70, 0x75, 0x74, 0x04, 0x04, 0x75, 0x73, 0x65, 0x72, 0x01,
            0x54,
        ];
        let ex5_f1_frame = Frame::new(
            Header::new(0, 10, Opcode::Command, 15),
            Value::Array(vec![
                Value::Text("put".to_string()),
                Value::Text("user".to_string()),
                Value::Int(42),
            ]),
        );
        assert_frame_round_trip(&ex5_f1_bytes, &ex5_f1_frame);

        // Spec Example 5 Frame 2: FETCH user
        let ex5_f2_bytes = [
            0xCA, 0xDB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x0B, 0x00, 0x02, 0x00, 0x00, 0x00, 0x0F,
            0x06, 0x02, 0x04, 0x05, 0x66, 0x65, 0x74, 0x63, 0x68, 0x04, 0x04, 0x75, 0x73, 0x65,
            0x72,
        ];
        let ex5_f2_frame = Frame::new(
            Header::new(0, 11, Opcode::Command, 15),
            Value::Array(vec![
                Value::Text("fetch".to_string()),
                Value::Text("user".to_string()),
            ]),
        );
        assert_frame_round_trip(&ex5_f2_bytes, &ex5_f2_frame);

        // Spec Examples 6 & 7: identical payload/frames to Examples 1 & 2
        assert_eq!(
            decode_frame(&ex1_bytes).unwrap(),
            Decoded::Complete((ex1_frame, ex1_bytes.len()))
        );
        assert_eq!(
            decode_frame(&ex2_bytes).unwrap(),
            Decoded::Complete((ex2_frame, ex2_bytes.len()))
        );

        // Spec Example 8: Server Error Reply (Unsupported flag error 1002)
        let ex8_bytes = [
            0xCA, 0xDB, 0x01, 0x03, 0x00, 0x00, 0x00, 0x05, 0x00, 0x02, 0x00, 0x00, 0x00, 0x14,
            0x08, 0xEA, 0x07, 0x10, 0x75, 0x6E, 0x73, 0x75, 0x70, 0x70, 0x6F, 0x72, 0x74, 0x65,
            0x64, 0x20, 0x66, 0x6C, 0x61, 0x67,
        ];
        let ex8_frame = Frame::new(
            Header::new(FLAG_RESPONSE | FLAG_ERROR, 5, Opcode::Command, 20),
            Value::Error {
                code: 1002,
                message: "unsupported flag".to_string(),
            },
        );
        assert_frame_round_trip(&ex8_bytes, &ex8_frame);
    }

    #[test]
    fn test_pipelining_two_frames_back_to_back() {
        // Spec Example 5: PUT user 42 followed by FETCH user
        let mut pipe_buf = Vec::new();
        let f1 = Frame::new(
            Header::new(0, 10, Opcode::Command, 0),
            Value::Array(vec![
                Value::Text("put".to_string()),
                Value::Text("user".to_string()),
                Value::Int(42),
            ]),
        );
        let f2 = Frame::new(
            Header::new(0, 11, Opcode::Command, 0),
            Value::Array(vec![
                Value::Text("fetch".to_string()),
                Value::Text("user".to_string()),
            ]),
        );

        encode_frame(&f1, &mut pipe_buf).unwrap();
        encode_frame(&f2, &mut pipe_buf).unwrap();

        // Decode first frame
        let res1 = decode_frame(&pipe_buf).expect("first frame decodes");
        let (dec1, used1) = match res1 {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("unexpected NeedMore on first frame"),
        };
        assert_eq!(dec1.header.request_id, 10);
        assert_eq!(dec1.payload, f1.payload);

        // Advance buffer past first frame
        let remaining = &pipe_buf[used1..];

        // Decode second frame
        let res2 = decode_frame(remaining).expect("second frame decodes");
        let (dec2, used2) = match res2 {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("unexpected NeedMore on second frame"),
        };
        assert_eq!(dec2.header.request_id, 11);
        assert_eq!(dec2.payload, f2.payload);

        // Buffer exhausted -> NeedMore
        let trailing = &remaining[used2..];
        assert_eq!(decode_frame(trailing).unwrap(), Decoded::NeedMore);
    }

    #[test]
    fn test_split_reads_every_possible_split_point() {
        // Test split across all byte positions for Example 1 (22 bytes) and Example 3 (33 bytes)
        let sample_frames = vec![
            Frame::new(
                Header::new(0, 1, Opcode::Command, 0),
                Value::Array(vec![Value::Text("ping".to_string())]),
            ),
            Frame::new(
                Header::new(FLAG_RESPONSE | FLAG_ERROR, 2, Opcode::Command, 0),
                Value::Error {
                    code: 2001,
                    message: "unknown command".to_string(),
                },
            ),
        ];

        for mut frame in sample_frames {
            let mut wire = Vec::new();
            encode_frame(&frame, &mut wire).unwrap();
            frame.header.payload_len = (wire.len() - HEADER_LEN) as u32;

            for split in 0..wire.len() {
                let prefix = &wire[..split];
                // Prefix must return NeedMore (not error, not complete)
                let prefix_res = decode_frame(prefix).unwrap();
                assert_eq!(prefix_res, Decoded::NeedMore);

                // Feeding full buffer must return Complete matching frame
                let full_res = decode_frame(&wire).unwrap();
                assert_eq!(full_res, Decoded::Complete((frame.clone(), wire.len())));
            }
        }
    }

    #[test]
    fn test_one_byte_at_a_time_streaming() {
        let mut frame = Frame::new(
            Header::new(0, 1, Opcode::Command, 0),
            Value::Array(vec![Value::Text("ping".to_string())]),
        );
        let mut wire = Vec::new();
        encode_frame(&frame, &mut wire).unwrap();
        frame.header.payload_len = (wire.len() - HEADER_LEN) as u32;

        let mut stream = Vec::new();
        for (i, &byte) in wire.iter().enumerate() {
            stream.push(byte);
            let res = decode_frame(&stream).unwrap();
            if i + 1 < wire.len() {
                assert_eq!(res, Decoded::NeedMore);
            } else {
                assert_eq!(res, Decoded::Complete((frame.clone(), wire.len())));
            }
        }
    }

    #[test]
    fn test_oversized_payload_len_in_header_fails_after_14_bytes() {
        // Construct a 14-byte header that claims payload_len = 100,000,000 bytes (larger than 64 MB)
        let mut hdr = [0u8; 14];
        hdr[0..2].copy_from_slice(&MAGIC);
        hdr[2] = VERSION;
        hdr[3] = 0x00; // flags
        hdr[4..8].copy_from_slice(&1u32.to_be_bytes()); // request_id
        hdr[8..10].copy_from_slice(&Opcode::Command.to_u16().to_be_bytes()); // opcode
        hdr[10..14].copy_from_slice(&100_000_000u32.to_be_bytes()); // 100 MB

        // Buffer contains ONLY the 14-byte header, with NO payload bytes present
        let err = decode_frame(&hdr).unwrap_err();
        assert_eq!(err, ProtoError::ERR_FRAME_TOO_LARGE);
    }

    #[test]
    fn test_bad_magic_version_reserved_flag_error_after_14_bytes() {
        let mut valid_hdr = [0u8; 14];
        valid_hdr[0..2].copy_from_slice(&MAGIC);
        valid_hdr[2] = VERSION;
        valid_hdr[3] = 0x00;
        valid_hdr[4..8].copy_from_slice(&1u32.to_be_bytes());
        valid_hdr[8..10].copy_from_slice(&Opcode::Command.to_u16().to_be_bytes());
        valid_hdr[10..14].copy_from_slice(&0u32.to_be_bytes());

        // Bad magic
        let mut bad_magic = valid_hdr;
        bad_magic[0] = 0xFF;
        assert_eq!(
            decode_frame(&bad_magic).unwrap_err(),
            ProtoError::ERR_INVALID_MAGIC
        );

        // Bad version
        let mut bad_version = valid_hdr;
        bad_version[2] = 2;
        assert_eq!(
            decode_frame(&bad_version).unwrap_err(),
            ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION
        );

        // Reserved flag 0x08
        let mut bad_flag = valid_hdr;
        bad_flag[3] = 0x08;
        assert_eq!(
            decode_frame(&bad_flag).unwrap_err(),
            ProtoError::ERR_UNSUPPORTED_FLAG
        );
    }

    #[test]
    fn test_payload_with_trailing_bytes_after_item() {
        // Frame claims payload_len = 10, but item is Int(42) which takes only 2 bytes (0x01, 0x54)
        let header = Header::new(0, 1, Opcode::Command, 10);
        let mut buf = Vec::new();
        buf.extend_from_slice(&header.to_bytes());
        buf.extend_from_slice(&[0x01, 0x54]); // Int(42)
        buf.extend_from_slice(&[0x00; 8]); // 8 unconsumed trailing bytes

        let err = decode_frame(&buf).unwrap_err();
        assert_eq!(err, ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    #[test]
    fn test_truncated_item_inside_complete_payload_returns_error_1006_not_need_more() {
        // Frame claims payload_len = 5. All 14 + 5 = 19 bytes are in the buffer.
        // Payload has Text item with declared string length 20, but payload only has 3 bytes of string.
        let header = Header::new(0, 1, Opcode::Command, 5);
        let mut buf = Vec::new();
        buf.extend_from_slice(&header.to_bytes());
        buf.extend_from_slice(&[0x04, 20, b'a', b'b', b'c']); // Tag Text, len 20, but only 3 chars

        let err = decode_frame(&buf).unwrap_err();
        assert_eq!(err, ProtoError::ERR_MALFORMED_PAYLOAD);
    }

    #[test]
    fn test_deterministic_loops_random_frames_round_trip() {
        let mut rng = XorShift64::new(0x1234_5678_9ABC_DEF0);

        let legal_flags = [0x00, 0x01, 0x03, 0x05];
        let opcodes = [Opcode::Hello, Opcode::Command, Opcode::Push];

        for _ in 0..300 {
            let flags = legal_flags[(rng.next_u64() % 4) as usize];
            let opcode = opcodes[(rng.next_u64() % 3) as usize];
            let request_id = if flags == 0x05 {
                0 // Push must have request_id = 0
            } else {
                rng.next_u64() as u32
            };

            let payload = random_value(&mut rng, 0);
            let mut frame = Frame::new(Header::new(flags, request_id, opcode, 0), payload);

            let mut out = Vec::new();
            encode_frame(&frame, &mut out).expect("encode frame succeeds");
            frame.header.payload_len = (out.len() - HEADER_LEN) as u32;

            // Complete decode
            let decoded = decode_frame(&out).expect("decode frame succeeds");
            assert_eq!(decoded, Decoded::Complete((frame.clone(), out.len())));

            // Chunked read test
            if out.len() > 1 {
                let split = (rng.next_u64() as usize % (out.len() - 1)) + 1;
                let chunk1 = &out[..split];
                let res1 = decode_frame(chunk1).expect("chunk 1 decodes");
                assert_eq!(res1, Decoded::NeedMore);
            }
        }
    }

    #[test]
    fn test_fuzz_never_panic_3000_random_bytes_and_3000_header_plus_random_payload() {
        let mut rng = XorShift64::new(0xFEDC_BA98_7654_3210);

        // 1. 3000 arbitrary byte slices
        for _ in 0..3000 {
            let len = (rng.next_u64() % 129) as usize;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                bytes.push(rng.next_u64() as u8);
            }
            let _ = decode_frame(&bytes);
        }

        // 2. 3000 slices with valid 14-byte header + arbitrary random payload
        for _ in 0..3000 {
            let payload_len = (rng.next_u64() % 64) as u32;
            let header = Header::new(0, 1, Opcode::Command, payload_len);
            let mut bytes = Vec::with_capacity(14 + payload_len as usize);
            bytes.extend_from_slice(&header.to_bytes());
            for _ in 0..payload_len {
                bytes.push(rng.next_u64() as u8);
            }
            let _ = decode_frame(&bytes);
        }
    }

    #[test]
    fn test_regression_fuzz_unknown_tag_in_array_payload_len_shrinks() {
        // This case was found by fuzzing in CI on 2026-10-11.
        // A wire frame with an unknown tag (0x0d) skipped inside an Array has wire payload_len = 15.
        // After re-encoding the resulting in-memory Value, the new wire payload_len is 10.
        let raw = [
            0xca, 0xdb, 0x01, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x02, 0x00, 0x00, 0x00, 0x0f,
            0x06, 0x03, 0x0d, 0x03, 0x70, 0x75, 0x74, 0x04, 0x04, 0x75, 0x73, 0x65, 0x72, 0x01,
            0x54,
        ];

        let decoded = decode_frame(&raw).expect("decode_frame succeeds on wire frame");
        let (frame, used) = match decoded {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("expected Decoded::Complete"),
        };

        assert_eq!(used, 29);
        assert_eq!(frame.header.payload_len, 15);
        assert_eq!(
            frame.payload,
            Value::Array(vec![Value::Text("user".to_string()), Value::Int(42)])
        );

        let mut re_encoded = Vec::new();
        encode_frame(&frame, &mut re_encoded).expect("encode_frame succeeds");

        let re_decoded = decode_frame(&re_encoded).expect("decode_frame succeeds on re-encoded");
        let (re_frame, re_used) = match re_decoded {
            Decoded::Complete((f, u)) => (f, u),
            Decoded::NeedMore => panic!("expected Decoded::Complete"),
        };

        assert_eq!(re_used, re_encoded.len());
        assert_eq!(re_frame.header.payload_len, 10);
        assert_eq!(re_frame.header.flags, frame.header.flags);
        assert_eq!(re_frame.header.request_id, frame.header.request_id);
        assert_eq!(re_frame.header.opcode, frame.header.opcode);
        assert_eq!(re_frame.payload, frame.payload);
    }

    #[test]
    fn test_limits_max_args_accepted_and_rejected() {
        let custom_limits = Limits {
            max_args: 100,
            ..Limits::default()
        };

        // 1. Exactly max_args (100) items -> accepted
        let mut payload_100 = Vec::new();
        payload_100.push(TAG_ARRAY);
        crate::proto::varint::encode_uvarint(100, &mut payload_100);
        payload_100.resize(payload_100.len() + 100, crate::proto::item::TAG_NIL);
        let hdr_100 = Header::new(0, 1, Opcode::Command, payload_100.len() as u32);
        let mut frame_100 = Vec::new();
        frame_100.extend_from_slice(&hdr_100.to_bytes());
        frame_100.extend_from_slice(&payload_100);

        let res_100 =
            decode_frame_with_limits(&frame_100, &custom_limits).expect("decodes 100 args");
        assert!(matches!(res_100, Decoded::Complete(_)));

        // 2. max_args + 1 (101) items -> rejected with ERR_TOO_MANY_ARGS (2007)
        let mut payload_101 = Vec::new();
        payload_101.push(TAG_ARRAY);
        crate::proto::varint::encode_uvarint(101, &mut payload_101);
        payload_101.resize(payload_101.len() + 101, crate::proto::item::TAG_NIL);
        let hdr_101 = Header::new(0, 1, Opcode::Command, payload_101.len() as u32);
        let mut frame_101 = Vec::new();
        frame_101.extend_from_slice(&hdr_101.to_bytes());
        frame_101.extend_from_slice(&payload_101);

        let err = decode_frame_with_limits(&frame_101, &custom_limits).unwrap_err();
        assert_eq!(err, ProtoError::ERR_TOO_MANY_ARGS);
        assert_eq!(err.code(), 2007);

        // 3. Rejection happens quickly before reading elements (truncated buffer with count 101)
        let mut truncated = Vec::new();
        truncated.extend_from_slice(&Header::new(0, 1, Opcode::Command, 500).to_bytes());
        truncated.push(TAG_ARRAY);
        crate::proto::varint::encode_uvarint(101, &mut truncated);
        // Pad to 514 bytes so total_frame_len arrives
        truncated.resize(14 + 500, 0);
        let err_fast = decode_frame_with_limits(&truncated, &custom_limits).unwrap_err();
        assert_eq!(err_fast, ProtoError::ERR_TOO_MANY_ARGS);
    }

    #[test]
    fn test_limits_max_frame_size_rejected_at_header_time() {
        let custom_limits = Limits {
            max_frame_size: 1000,
            ..Limits::default()
        };
        // Frame claims payload_len = 1001
        let hdr = Header::new(0, 1, Opcode::Command, 1001);
        let err = decode_frame_with_limits(&hdr.to_bytes(), &custom_limits).unwrap_err();
        assert_eq!(err, ProtoError::ERR_FRAME_TOO_LARGE);
    }

    #[test]
    fn test_limits_custom_depth_limit() {
        let custom_limits = Limits {
            max_depth: 3,
            ..Limits::default()
        };
        // Build 4 levels of nested Arrays: Array(Array(Array(Array(Nil))))
        let val_depth_4 = Value::Array(vec![Value::Array(vec![Value::Array(vec![Value::Array(
            vec![Value::Nil],
        )])])]);
        let mut encoded = Vec::new();
        crate::proto::item::encode_value(&val_depth_4, &mut encoded).unwrap();
        let hdr = Header::new(0, 1, Opcode::Command, encoded.len() as u32);
        let mut frame_buf = Vec::new();
        frame_buf.extend_from_slice(&hdr.to_bytes());
        frame_buf.extend_from_slice(&encoded);

        let err = decode_frame_with_limits(&frame_buf, &custom_limits).unwrap_err();
        assert_eq!(err, ProtoError::ERR_MAX_DEPTH_EXCEEDED);
    }
}
