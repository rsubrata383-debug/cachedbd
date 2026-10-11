//! Custom wire protocol serialization and deserialization.

pub mod decode;
pub mod encode;
pub mod header;
pub mod hello;
pub mod item;
pub mod limits;
pub mod varint;

/// Result of decoding an element from a buffer that may be partially received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded<T> {
    /// Buffer ends before the item is complete; caller should wait for more data.
    NeedMore,
    /// Item was decoded completely.
    Complete(T),
}

/// A complete Cachedb wire protocol frame consisting of a 14-byte header and a typed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// 14-byte wire protocol header.
    pub header: Header,
    /// Typed payload value.
    pub payload: Value,
}

impl Frame {
    /// Creates a new frame with the specified header and payload.
    ///
    /// The `payload_len` in `header` is dynamically computed and updated during encoding.
    #[must_use]
    pub const fn new(header: Header, payload: Value) -> Self {
        Self { header, payload }
    }
}

pub use decode::{decode_frame, decode_frame_with_limit, decode_frame_with_limits};
pub use encode::{encode_frame, encode_frame_with_limit};
pub use header::{
    DEFAULT_MAX_PAYLOAD_LEN, DecodedHeader, FLAG_ERROR, FLAG_MORE_FRAMES, FLAG_PUSH, FLAG_RESPONSE,
    HEADER_LEN, Header, MAGIC, Opcode, ProtoError, VERSION,
};
pub use hello::{HelloRequest, HelloResponse, MAX_CLIENT_NAME_LEN, negotiate_version};
pub use item::{
    MAX_NESTING_DEPTH, TAG_ARRAY, TAG_BOOL, TAG_BYTES, TAG_ERROR, TAG_FLOAT, TAG_INT, TAG_MAP,
    TAG_NIL, TAG_TEXT, Value, decode_value, decode_value_with_max_depth, encode_value, skip_value,
};
pub use limits::{DEFAULT_MAX_ARGS, Limits};
pub use varint::{
    MAX_VARINT_BYTES, decode_ivarint, decode_uvarint, encode_ivarint, encode_uvarint,
    zigzag_decode, zigzag_encode,
};
