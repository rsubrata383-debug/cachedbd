#![no_main]

use cachedb::proto::{decode_frame, encode_frame, Decoded};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(Decoded::Complete((frame, used))) = decode_frame(data) {
        assert!(used <= data.len());

        let mut encoded = Vec::new();
        encode_frame(&frame, &mut encoded).expect("encode_frame must succeed on decoded frame");

        let re_decoded = decode_frame(&encoded).expect("decode_frame must succeed on encoded frame");
        assert_eq!(re_decoded, Decoded::Complete((frame, encoded.len())));
    }
});

