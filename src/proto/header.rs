//! Fixed 14-byte binary wire protocol frame header encoding and decoding.
//!
//! Every Cachedb protocol frame begins with this fixed-length header in
//! Network Byte Order (Big-Endian):
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          Magic (2B)           |  Version (1B) |   Flags (1B)  |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                          Request ID (4B)                      |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          Opcode (2B)          |       Payload Length (4B)     |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+                               +
//! |                           (continued)                         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```

use crate::proto::Decoded;
use std::error::Error;
use std::fmt;

/// Exact length of the fixed wire protocol frame header in bytes.
pub const HEADER_LEN: usize = 14;

/// Protocol magic identifier bytes: `0xCA 0xDB` ("CADB").
pub const MAGIC: [u8; 2] = [0xCA, 0xDB];

/// Initial wire protocol version (`1`).
pub const VERSION: u8 = 1;

/// Flag bit 0: Indicates a response frame sent from server to client.
pub const FLAG_RESPONSE: u8 = 0x01;

/// Flag bit 1: Indicates payload contains a structured error response.
pub const FLAG_ERROR: u8 = 0x02;

/// Flag bit 2: Indicates an unsolicited server push notification (request_id = 0).
pub const FLAG_PUSH: u8 = 0x04;

/// Flag bit 3: Multi-frame chunk continuation flag (reserved in v0.1).
pub const FLAG_MORE_FRAMES: u8 = 0x08;

/// Bitmask covering all reserved flag bits in v0.1 (`0xF8` = bits 3 through 7).
pub const FLAGS_RESERVED_MASK: u8 = 0xF8;

/// Default maximum allowable payload size: 64 MB (67,108,864 bytes).
pub const DEFAULT_MAX_PAYLOAD_LEN: u32 = 64 * 1024 * 1024;

/// Protocol error containing a numeric error code and a specification message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoError {
    code: u16,
    message: &'static str,
}

impl ProtoError {
    /// Constructs a new protocol error with code and message.
    #[must_use]
    pub const fn new(code: u16, message: &'static str) -> Self {
        Self { code, message }
    }

    /// Returns the numeric error code from the catalog in `docs/spec/errors.md`.
    #[must_use]
    pub const fn code(&self) -> u16 {
        self.code
    }

    /// Returns the standard human-readable specification message.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// Error 1001: Unsupported protocol version.
    pub const ERR_UNSUPPORTED_PROTOCOL_VERSION: Self =
        Self::new(1001, "unsupported protocol version");

    /// Error 1002: Unsupported or reserved flag bit set.
    pub const ERR_UNSUPPORTED_FLAG: Self = Self::new(1002, "unsupported flag");

    /// Error 1003: Frame payload exceeds maximum permitted size.
    pub const ERR_FRAME_TOO_LARGE: Self = Self::new(1003, "frame exceeds maximum size");

    /// Error 1004: Invalid protocol magic header (not `0xCA 0xDB`).
    pub const ERR_INVALID_MAGIC: Self = Self::new(1004, "invalid protocol magic header");

    /// Error 1005: Malformed varint encoding (exceeds 10 bytes, overflows 64 bits, or truncated).
    pub const ERR_MALFORMED_VARINT: Self = Self::new(1005, "malformed varint encoding");

    /// Error 1006: Payload decoding failed (truncated item, invalid boolean, or invalid UTF-8).
    pub const ERR_MALFORMED_PAYLOAD: Self = Self::new(1006, "payload decoding failed");

    /// Error 1007: Unknown payload type tag.
    pub const ERR_UNKNOWN_TYPE_TAG: Self = Self::new(1007, "unknown type tag");

    /// Error 2001: Unknown command or unsupported opcode.
    pub const ERR_UNKNOWN_COMMAND: Self = Self::new(2001, "unknown command");
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "protocol error {}: {}", self.code, self.message)
    }
}

impl Error for ProtoError {}

/// Primary wire operation identifier specified in the frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Opcode {
    /// Initial client-server connection handshake (`0x0001`).
    Hello = 0x0001,
    /// Client command invocation or server response (`0x0002`).
    Command = 0x0002,
    /// Unsolicited server push event (`0x0003`).
    Push = 0x0003,
}

impl Opcode {
    /// Converts the opcode into its raw big-endian `u16` numeric representation.
    #[must_use]
    pub const fn to_u16(self) -> u16 {
        self as u16
    }
}

impl TryFrom<u16> for Opcode {
    type Error = ProtoError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0x0001 => Ok(Self::Hello),
            0x0002 => Ok(Self::Command),
            0x0003 => Ok(Self::Push),
            _ => Err(ProtoError::ERR_UNKNOWN_COMMAND),
        }
    }
}

/// Type alias for decoded header result using the generic [`Decoded`] enum.
pub type DecodedHeader = Decoded<Header>;

/// Decoded 14-byte wire protocol frame header.
///
/// Magic (`0xCADB`) and protocol version (`1`) are fixed constants validated
/// on decode and written on encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Control flags bitfield (`FLAG_RESPONSE`, `FLAG_ERROR`, `FLAG_PUSH`).
    pub flags: u8,
    /// Multiplexed request identifier matching responses to requests.
    pub request_id: u32,
    /// Operation opcode (`Hello`, `Command`, `Push`).
    pub opcode: Opcode,
    /// Length of payload following this header in bytes.
    pub payload_len: u32,
}

impl Header {
    /// Creates a new `Header` instance.
    #[must_use]
    pub const fn new(flags: u8, request_id: u32, opcode: Opcode, payload_len: u32) -> Self {
        Self {
            flags,
            request_id,
            opcode,
            payload_len,
        }
    }

    /// Returns `true` if this frame is a server response (`FLAG_RESPONSE`).
    #[must_use]
    pub const fn is_response(&self) -> bool {
        (self.flags & FLAG_RESPONSE) != 0
    }

    /// Returns `true` if this frame indicates an error response (`FLAG_ERROR`).
    #[must_use]
    pub const fn is_error(&self) -> bool {
        (self.flags & FLAG_ERROR) != 0
    }

    /// Returns `true` if this frame is an unsolicited server push (`FLAG_PUSH`).
    #[must_use]
    pub const fn is_push(&self) -> bool {
        (self.flags & FLAG_PUSH) != 0
    }

    /// Serializes this header into a 14-byte buffer in big-endian byte order.
    pub fn encode(&self, out: &mut [u8; HEADER_LEN]) {
        out[0..2].copy_from_slice(&MAGIC);
        out[2] = VERSION;
        out[3] = self.flags;
        out[4..8].copy_from_slice(&self.request_id.to_be_bytes());
        out[8..10].copy_from_slice(&self.opcode.to_u16().to_be_bytes());
        out[10..14].copy_from_slice(&self.payload_len.to_be_bytes());
    }

    /// Encodes this header into a stack-allocated 14-byte array.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        self.encode(&mut buf);
        buf
    }

    /// Decodes a header from input bytes with the default 64 MB maximum payload limit.
    ///
    /// # Errors
    ///
    /// Returns a [`ProtoError`] if the header is corrupt, version is unsupported,
    /// reserved flags are set, opcode is unknown, or payload exceeds 64 MB.
    pub fn decode(input: &[u8]) -> Result<DecodedHeader, ProtoError> {
        Self::decode_with_max_len(input, DEFAULT_MAX_PAYLOAD_LEN)
    }

    /// Decodes a header from input bytes with an explicit maximum payload size limit.
    ///
    /// # Errors
    ///
    /// Returns a [`ProtoError`] if validation fails against protocol specifications.
    pub fn decode_with_max_len(
        input: &[u8],
        max_payload_len: u32,
    ) -> Result<DecodedHeader, ProtoError> {
        if input.len() < HEADER_LEN {
            return Ok(DecodedHeader::NeedMore);
        }

        // Validate Magic (bytes 0..2)
        if input[0..2] != MAGIC {
            return Err(ProtoError::ERR_INVALID_MAGIC);
        }

        // Validate Version (byte 2)
        if input[2] != VERSION {
            return Err(ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION);
        }

        // Validate Flags (byte 3): reject reserved flags
        let flags = input[3];
        if (flags & FLAGS_RESERVED_MASK) != 0 {
            return Err(ProtoError::ERR_UNSUPPORTED_FLAG);
        }

        // Parse Request ID (bytes 4..8, Big-Endian)
        let request_id = u32::from_be_bytes([input[4], input[5], input[6], input[7]]);

        // Parse Opcode (bytes 8..10, Big-Endian)
        let raw_opcode = u16::from_be_bytes([input[8], input[9]]);
        let opcode = Opcode::try_from(raw_opcode)?;

        // Parse Payload Length (bytes 10..14, Big-Endian)
        let payload_len = u32::from_be_bytes([input[10], input[11], input[12], input[13]]);
        if payload_len > max_payload_len {
            return Err(ProtoError::ERR_FRAME_TOO_LARGE);
        }

        Ok(DecodedHeader::Complete(Self {
            flags,
            request_id,
            opcode,
            payload_len,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        hex.split_whitespace()
            .map(|byte_str| u8::from_str_radix(byte_str, 16).expect("valid hex byte"))
            .collect()
    }

    #[test]
    fn test_header_round_trip() {
        let original = Header::new(FLAG_RESPONSE | FLAG_ERROR, 42, Opcode::Command, 1024);
        let encoded = original.to_bytes();
        let decoded = Header::decode(&encoded).expect("decode succeeds");

        assert_eq!(decoded, DecodedHeader::Complete(original));
        assert!(original.is_response());
        assert!(original.is_error());
        assert!(!original.is_push());
    }

    #[test]
    fn test_fewer_than_14_bytes_returns_need_more() {
        assert_eq!(Header::decode(&[]).unwrap(), DecodedHeader::NeedMore);
        assert_eq!(Header::decode(&[0xCA]).unwrap(), DecodedHeader::NeedMore);
        assert_eq!(
            Header::decode(&[
                0xCA, 0xDB, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00
            ])
            .unwrap(),
            DecodedHeader::NeedMore
        );
    }

    #[test]
    fn test_protocol_spec_example_1_ping_request() {
        // Example 1: PING request with request_id = 1, opcode = COMMAND (0x0002), payload_len = 8
        let bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 08");
        let decoded = Header::decode(&bytes).unwrap();

        assert_eq!(
            decoded,
            DecodedHeader::Complete(Header {
                flags: 0x00,
                request_id: 1,
                opcode: Opcode::Command,
                payload_len: 8,
            })
        );
    }

    #[test]
    fn test_protocol_spec_example_2_pong_reply() {
        // Example 2: PONG reply with request_id = 1, flags = FLAG_RESPONSE (0x01), payload_len = 6
        let bytes = hex_to_bytes("CA DB 01 01 00 00 00 01 00 02 00 00 00 06");
        let decoded = Header::decode(&bytes).unwrap();

        assert_eq!(
            decoded,
            DecodedHeader::Complete(Header {
                flags: FLAG_RESPONSE,
                request_id: 1,
                opcode: Opcode::Command,
                payload_len: 6,
            })
        );
    }

    #[test]
    fn test_protocol_spec_example_3_error_reply() {
        // Example 3: Error reply with request_id = 2, flags = 0x03, payload_len = 19 (0x13)
        let bytes = hex_to_bytes("CA DB 01 03 00 00 00 02 00 02 00 00 00 13");
        let decoded = Header::decode(&bytes).unwrap();

        assert_eq!(
            decoded,
            DecodedHeader::Complete(Header {
                flags: FLAG_RESPONSE | FLAG_ERROR,
                request_id: 2,
                opcode: Opcode::Command,
                payload_len: 19,
            })
        );
    }

    #[test]
    fn test_protocol_spec_example_4_push_message() {
        // Example 4: Push message with request_id = 0, flags = 0x05, opcode = PUSH (0x0003), payload_len = 15
        let bytes = hex_to_bytes("CA DB 01 05 00 00 00 00 00 03 00 00 00 0F");
        let decoded = Header::decode(&bytes).unwrap();

        assert_eq!(
            decoded,
            DecodedHeader::Complete(Header {
                flags: FLAG_RESPONSE | FLAG_PUSH,
                request_id: 0,
                opcode: Opcode::Push,
                payload_len: 15,
            })
        );
        let header = match decoded {
            DecodedHeader::Complete(h) => h,
            DecodedHeader::NeedMore => unreachable!(),
        };
        assert!(header.is_push());
    }

    #[test]
    fn test_protocol_spec_example_5_pipelined_headers() {
        // Example 5 Frame 1: PUT (ID 10, payload_len 15)
        let f1 = hex_to_bytes("CA DB 01 00 00 00 00 0A 00 02 00 00 00 0F");
        assert_eq!(
            Header::decode(&f1).unwrap(),
            DecodedHeader::Complete(Header {
                flags: 0x00,
                request_id: 10,
                opcode: Opcode::Command,
                payload_len: 15,
            })
        );

        // Example 5 Frame 2: FETCH (ID 11, payload_len 15)
        let f2 = hex_to_bytes("CA DB 01 00 00 00 00 0B 00 02 00 00 00 0F");
        assert_eq!(
            Header::decode(&f2).unwrap(),
            DecodedHeader::Complete(Header {
                flags: 0x00,
                request_id: 11,
                opcode: Opcode::Command,
                payload_len: 15,
            })
        );
    }

    #[test]
    fn test_protocol_spec_example_6_split_cut_in_payload() {
        // Example 6 Read 1 first 14 bytes: identical to Example 1
        let bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 08");
        assert_eq!(
            Header::decode(&bytes).unwrap(),
            DecodedHeader::Complete(Header {
                flags: 0x00,
                request_id: 1,
                opcode: Opcode::Command,
                payload_len: 8,
            })
        );
    }

    #[test]
    fn test_protocol_spec_example_7_split_cut_in_header() {
        // Example 7 Read 1: only 5 bytes received -> NeedMore
        let partial = hex_to_bytes("CA DB 01 01 00");
        assert_eq!(Header::decode(&partial).unwrap(), DecodedHeader::NeedMore);

        // Joined full 14 header bytes: identical to Example 2
        let joined = hex_to_bytes("CA DB 01 01 00 00 00 01 00 02 00 00 00 06");
        assert_eq!(
            Header::decode(&joined).unwrap(),
            DecodedHeader::Complete(Header {
                flags: FLAG_RESPONSE,
                request_id: 1,
                opcode: Opcode::Command,
                payload_len: 6,
            })
        );
    }

    #[test]
    fn test_protocol_spec_example_8_reserved_flag_rejected() {
        // Example 8 Client Request with reserved bit 3 set: flags = 0x08 -> ERR_UNSUPPORTED_FLAG (1002)
        let client_frame = hex_to_bytes("CA DB 01 08 00 00 00 05 00 02 00 00 00 08");
        let err = Header::decode(&client_frame).unwrap_err();
        assert_eq!(err, ProtoError::ERR_UNSUPPORTED_FLAG);
        assert_eq!(err.code(), 1002);

        // Example 8 Server Error Reply header: flags = 0x03, ID = 5, payload_len = 20
        let server_reply = hex_to_bytes("CA DB 01 03 00 00 00 05 00 02 00 00 00 14");
        assert_eq!(
            Header::decode(&server_reply).unwrap(),
            DecodedHeader::Complete(Header {
                flags: FLAG_RESPONSE | FLAG_ERROR,
                request_id: 5,
                opcode: Opcode::Command,
                payload_len: 20,
            })
        );
    }

    #[test]
    fn test_invalid_magic_returns_error_1004() {
        let mut bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 08");
        bytes[0] = 0xDE;
        bytes[1] = 0xAD;

        let err = Header::decode(&bytes).unwrap_err();
        assert_eq!(err, ProtoError::ERR_INVALID_MAGIC);
        assert_eq!(err.code(), 1004);
    }

    #[test]
    fn test_unsupported_version_returns_error_1001() {
        let mut bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 08");
        bytes[2] = 2; // version 2

        let err = Header::decode(&bytes).unwrap_err();
        assert_eq!(err, ProtoError::ERR_UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(err.code(), 1001);
    }

    #[test]
    fn test_reserved_flags_return_error_1002() {
        // High bits 4-7
        for bit in [0x10, 0x20, 0x40, 0x80] {
            let mut bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 08");
            bytes[3] = bit;
            let err = Header::decode(&bytes).unwrap_err();
            assert_eq!(err, ProtoError::ERR_UNSUPPORTED_FLAG);
            assert_eq!(err.code(), 1002);
        }
    }

    #[test]
    fn test_unknown_opcode_returns_error_2001() {
        let mut bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 08");
        bytes[8] = 0x00;
        bytes[9] = 0x99; // opcode 0x0099 unknown

        let err = Header::decode(&bytes).unwrap_err();
        assert_eq!(err, ProtoError::ERR_UNKNOWN_COMMAND);
        assert_eq!(err.code(), 2001);
    }

    #[test]
    fn test_payload_length_boundary_exact_max_and_max_plus_one() {
        let max_len = DEFAULT_MAX_PAYLOAD_LEN; // 64 MB
        let mut max_bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 00");
        max_bytes[10..14].copy_from_slice(&max_len.to_be_bytes());

        let decoded = Header::decode(&max_bytes).expect("exact max payload len accepted");
        assert_eq!(
            decoded,
            DecodedHeader::Complete(Header {
                flags: 0x00,
                request_id: 1,
                opcode: Opcode::Command,
                payload_len: max_len,
            })
        );

        let over_len = max_len + 1;
        let mut over_bytes = hex_to_bytes("CA DB 01 00 00 00 00 01 00 02 00 00 00 00");
        over_bytes[10..14].copy_from_slice(&over_len.to_be_bytes());

        let err = Header::decode(&over_bytes).unwrap_err();
        assert_eq!(err, ProtoError::ERR_FRAME_TOO_LARGE);
        assert_eq!(err.code(), 1003);
    }
}
