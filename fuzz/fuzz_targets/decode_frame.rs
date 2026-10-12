#![no_main]

use libfuzzer_sys::fuzz_target;
use skiffdb::proto::{Decoded, HEADER_LEN, decode_frame, encode_frame};

fuzz_target!(|data: &[u8]| {
    if let Ok(Decoded::Complete((frame, used))) = decode_frame(data) {
        assert!(used <= data.len());

        let mut encoded = Vec::new();
        encode_frame(&frame, &mut encoded).expect("encode_frame must succeed on decoded frame");

        let re_decoded =
            decode_frame(&encoded).expect("decode_frame must succeed on encoded frame");
        if let Decoded::Complete((re_frame, re_used)) = re_decoded {
            assert_eq!(re_used, encoded.len());
            assert_eq!(
                re_frame.header.payload_len as usize,
                encoded.len() - HEADER_LEN
            );
            assert_eq!(re_frame.header.flags, frame.header.flags);
            assert_eq!(re_frame.header.request_id, frame.header.request_id);
            assert_eq!(re_frame.header.opcode, frame.header.opcode);
            assert_eq!(re_frame.payload, frame.payload);
        } else {
            panic!("expected Decoded::Complete on re-encoded frame");
        }
    }
});
