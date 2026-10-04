//! Framing tests for `tl_model::wire` (E03-068). The peer is untrusted, so
//! `oversize_prefix_rejected_without_alloc` is the security regression test.

use std::io::Cursor;

use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use tl_model::wire::{
    MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, MAX_PROBE_FRAME, WireError, read_frame, read_reply,
    write_frame,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TestMsg {
    last: bool,
    data: Vec<u8>,
}

/// A message whose postcard payload is exactly `payload_len` bytes.
fn msg_with_payload(payload_len: usize, last: bool) -> TestMsg {
    // 1 byte for the bool, then a varint length, then the bytes.
    for varint in 1..=5 {
        let data = vec![0xA5; payload_len - 1 - varint];
        let msg = TestMsg { last, data };
        if postcard::serialize_with_flavor::<_, _, usize>(
            &msg,
            postcard::ser_flavors::Size::default(),
        )
        .expect("a Vec<u8> always serializes")
            == payload_len
        {
            return msg;
        }
    }
    panic!("no TestMsg has a payload of {payload_len} bytes");
}

fn frame(msg: &TestMsg, max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    write_frame(&mut out, msg, max).expect("frame fits the limit");
    out
}

#[test]
fn max_size_roundtrip() {
    let msg = msg_with_payload(MAX_HELPER_FRAME, true);
    let bytes = frame(&msg, MAX_HELPER_FRAME);
    assert_eq!(bytes.len(), 4 + MAX_HELPER_FRAME);
    assert_eq!(&bytes[..4], &(MAX_HELPER_FRAME as u32).to_le_bytes());
    let mut r = Cursor::new(bytes);
    let back: TestMsg = read_frame(&mut r, MAX_HELPER_FRAME).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn oversize_prefix_rejected_without_alloc() {
    let len = MAX_HELPER_FRAME + 1;
    let mut bytes = (len as u32).to_le_bytes().to_vec();
    bytes.extend(std::iter::repeat_n(0u8, len));
    let mut r = Cursor::new(bytes);
    let err = read_frame::<_, TestMsg>(&mut r, MAX_HELPER_FRAME).unwrap_err();
    assert!(
        matches!(err, WireError::TooLarge { len: l, max } if l == len && max == MAX_HELPER_FRAME),
        "{err:?}"
    );
    assert_eq!(r.position(), 4, "no payload byte may be read");

    // A hostile u32::MAX prefix must fail the same way without trying a 4 GiB buffer.
    let mut r = Cursor::new(u32::MAX.to_le_bytes().to_vec());
    let err = read_frame::<_, TestMsg>(&mut r, MAX_HELPER_FRAME).unwrap_err();
    assert!(matches!(err, WireError::TooLarge { .. }), "{err:?}");
    assert_eq!(r.position(), 4);
}

#[test]
fn truncated_payload() {
    let mut bytes = 10u32.to_le_bytes().to_vec();
    bytes.extend([1u8; 6]);
    let err = read_frame::<_, TestMsg>(&mut Cursor::new(bytes), MAX_HELPER_FRAME).unwrap_err();
    assert!(matches!(err, WireError::Truncated), "{err:?}");
}

#[test]
fn truncated_prefix_and_clean_eof() {
    let err =
        read_frame::<_, TestMsg>(&mut Cursor::new(vec![7u8, 0]), MAX_HELPER_FRAME).unwrap_err();
    assert!(matches!(err, WireError::Truncated), "{err:?}");

    let err = read_frame::<_, TestMsg>(&mut Cursor::new(Vec::new()), MAX_HELPER_FRAME).unwrap_err();
    assert!(matches!(err, WireError::Eof), "{err:?}");
}

#[test]
fn bad_postcard_is_decode_error() {
    // The bool byte must be 0 or 1.
    let mut bytes = 2u32.to_le_bytes().to_vec();
    bytes.extend([7u8, 0]);
    let err = read_frame::<_, TestMsg>(&mut Cursor::new(bytes), MAX_HELPER_FRAME).unwrap_err();
    assert!(matches!(err, WireError::Decode(_)), "{err:?}");

    // A valid value followed by extra bytes inside the frame is also rejected.
    let mut bytes = 3u32.to_le_bytes().to_vec();
    bytes.extend([1u8, 0, 9]);
    let err = read_frame::<_, TestMsg>(&mut Cursor::new(bytes), MAX_HELPER_FRAME).unwrap_err();
    assert!(matches!(err, WireError::Decode(_)), "{err:?}");
}

#[test]
fn write_refuses_oversize() {
    let msg = msg_with_payload(MAX_HELPER_FRAME + 1, true);
    let mut out = Vec::new();
    let err = write_frame(&mut out, &msg, MAX_HELPER_FRAME).unwrap_err();
    assert!(
        matches!(err, WireError::TooLarge { len, max } if len == MAX_HELPER_FRAME + 1 && max == MAX_HELPER_FRAME),
        "{err:?}"
    );
    assert!(out.is_empty());
}

#[test]
fn reply_total_cap() {
    let mut bytes = Vec::new();
    for i in 0..5 {
        let msg = msg_with_payload(MAX_PROBE_FRAME, i == 4);
        bytes.extend(frame(&msg, MAX_PROBE_FRAME));
    }
    let mut r = Cursor::new(bytes);
    let err = read_reply::<_, TestMsg>(&mut r, MAX_PROBE_FRAME, MAX_HELPER_REPLY_TOTAL, |m| m.last)
        .unwrap_err();
    assert!(
        matches!(err, WireError::ReplyTooLarge { total, max }
            if total == 5 * MAX_PROBE_FRAME && max == MAX_HELPER_REPLY_TOTAL),
        "{err:?}"
    );
    // Four full frames were read, then only the fifth prefix.
    assert_eq!(r.position() as usize, 4 * (4 + MAX_PROBE_FRAME) + 4);
}

#[test]
fn reply_reads_until_last() {
    let msgs: Vec<TestMsg> = (0..3u8)
        .map(|i| TestMsg {
            last: i == 2,
            data: vec![i; 3],
        })
        .collect();
    let mut bytes = Vec::new();
    for m in &msgs {
        bytes.extend(frame(m, MAX_HELPER_FRAME));
    }
    // A frame after the last one must stay unread.
    bytes.extend(frame(
        &TestMsg {
            last: true,
            data: vec![9],
        },
        MAX_HELPER_FRAME,
    ));
    let mut r = Cursor::new(bytes);
    let got =
        read_reply::<_, TestMsg>(&mut r, MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, |m| m.last)
            .unwrap();
    assert_eq!(got, msgs);
    let rest: TestMsg = read_frame(&mut r, MAX_HELPER_FRAME).unwrap();
    assert_eq!(rest.data, vec![9]);
}

proptest! {
    #[test]
    fn prop_roundtrip_bytes(data in proptest::collection::vec(any::<u8>(), 0..=4096), last in any::<bool>()) {
        let msg = TestMsg { last, data };
        let mut out = Vec::new();
        write_frame(&mut out, &msg, 8192).unwrap();
        let back: TestMsg = read_frame(&mut Cursor::new(out), 8192).unwrap();
        prop_assert_eq!(back, msg);
    }

    #[test]
    fn prop_random_stream_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..=8192)) {
        let mut r = Cursor::new(bytes);
        // Ok or any error is fine; a panic fails the test.
        let _ = read_frame::<_, TestMsg>(&mut r, 4096);
    }
}
