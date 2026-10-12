#![no_main]

use libfuzzer_sys::fuzz_target;
use skiffdb::proto::{HelloRequest, HelloResponse, decode_value, encode_value};

fuzz_target!(|data: &[u8]| {
    if let Ok((val, _used)) = decode_value(data) {
        let _ = HelloRequest::from_value(&val);
        let _ = HelloResponse::from_value(&val);

        let mut encoded = Vec::new();
        encode_value(&val, &mut encoded).expect("encode_value must succeed for decoded value");

        let (re_decoded, re_used) =
            decode_value(&encoded).expect("decode_value must succeed on re-encoded value");
        assert_eq!(re_used, encoded.len());
        assert_eq!(re_decoded, val);
    }
});
