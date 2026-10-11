#![no_main]

use cachedb::proto::{decode_frame, Decoded, Frame, ProtoError};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let split_raw = u16::from_be_bytes([data[0], data[1]]) as usize;
    let stream = &data[2..];
    let split = if stream.is_empty() {
        0
    } else {
        split_raw % (stream.len() + 1)
    };

    // Decode all-at-once
    let mut full_frames: Vec<Result<Frame, ProtoError>> = Vec::new();
    let mut cursor = 0;
    while cursor < stream.len() {
        match decode_frame(&stream[cursor..]) {
            Ok(Decoded::Complete((frame, used))) => {
                if used == 0 {
                    break;
                }
                full_frames.push(Ok(frame));
                cursor += used;
            }
            Ok(Decoded::NeedMore) => break,
            Err(err) => {
                full_frames.push(Err(err));
                break;
            }
        }
    }

    // Decode chunked across split point
    let chunk1 = &stream[..split];
    let chunk2 = &stream[split..];

    let mut chunked_frames: Vec<Result<Frame, ProtoError>> = Vec::new();
    let mut buf = Vec::with_capacity(stream.len());
    let mut buf_cursor = 0;

    for chunk in [chunk1, chunk2] {
        buf.extend_from_slice(chunk);
        loop {
            if buf_cursor >= buf.len() {
                break;
            }
            match decode_frame(&buf[buf_cursor..]) {
                Ok(Decoded::Complete((frame, used))) => {
                    if used == 0 {
                        break;
                    }
                    chunked_frames.push(Ok(frame));
                    buf_cursor += used;
                }
                Ok(Decoded::NeedMore) => break,
                Err(err) => {
                    chunked_frames.push(Err(err));
                    break;
                }
            }
        }
        if let Some(Err(_)) = chunked_frames.last() {
            break;
        }
    }

    assert_eq!(full_frames, chunked_frames);
});

