//! Custom wire protocol serialization and deserialization.

pub mod decode;
pub mod encode;
pub mod header;
pub mod varint;

/// Result of decoding an element from a buffer that may be partially received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded<T> {
    /// Buffer ends before the item is complete; caller should wait for more data.
    NeedMore,
    /// Item was decoded completely.
    Complete(T),
}

pub use header::{
    DEFAULT_MAX_PAYLOAD_LEN, DecodedHeader, FLAG_ERROR, FLAG_MORE_FRAMES, FLAG_PUSH, FLAG_RESPONSE,
    HEADER_LEN, Header, MAGIC, Opcode, ProtoError, VERSION,
};
pub use varint::{
    decode_ivarint, decode_uvarint, encode_ivarint, encode_uvarint, zigzag_decode, zigzag_encode,
};
