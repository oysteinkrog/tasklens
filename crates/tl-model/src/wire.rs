//! Length-prefixed postcard framing for the GPU probe and the helper (ADR-007).
//!
//! frame = u32 little-endian payload length | payload (postcard)
//!
//! The peer is a separate process (the probe) or a root process (the helper), so every length is
//! checked against the limit before any payload buffer is allocated. Blocking `std::io` only;
//! callers run these functions on a blocking thread. Errors carry sizes only, never payload bytes.

use std::io::{ErrorKind, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Largest payload of one GPU probe frame: 1 MiB.
pub const MAX_PROBE_FRAME: usize = 1_048_576;
/// Largest payload of one helper frame: 64 KiB.
pub const MAX_HELPER_FRAME: usize = 65_536;
/// Largest helper reply per request, summed over its frames: 4 MiB.
pub const MAX_HELPER_REPLY_TOTAL: usize = 4_194_304;

const PREFIX_LEN: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("frame of {len} bytes exceeds the limit of {max}")]
    TooLarge { len: usize, max: usize },
    #[error("reply of {total} bytes exceeds the limit of {max}")]
    ReplyTooLarge { total: usize, max: usize },
    /// EOF inside the length prefix or the payload.
    #[error("truncated frame")]
    Truncated,
    /// Clean EOF before the first byte of a frame.
    #[error("end of stream")]
    Eof,
    #[error("decode error: {0}")]
    Decode(postcard::Error),
    #[error("encode error: {0}")]
    Encode(postcard::Error),
    #[error("i/o error: {0}")]
    Io(std::io::Error),
}

/// Appends the length prefix and the postcard payload of `msg` to `out`.
///
/// Fails with `TooLarge` when the payload is over `max` or does not fit in a u32. On any error
/// `out` is left as it was.
pub fn encode_frame<T: Serialize>(msg: &T, max: usize, out: &mut Vec<u8>) -> Result<(), WireError> {
    let len = postcard::serialize_with_flavor(msg, postcard::ser_flavors::Size::default())
        .map_err(WireError::Encode)?;
    let max = max.min(u32::MAX as usize);
    if len > max {
        return Err(WireError::TooLarge { len, max });
    }
    let start = out.len();
    out.reserve(PREFIX_LEN + len);
    // len <= max <= u32::MAX, checked above.
    out.extend_from_slice(&(len as u32).to_le_bytes());
    if let Err(e) = postcard::to_extend(msg, VecExtend(out)) {
        out.truncate(start);
        return Err(WireError::Encode(e));
    }
    Ok(())
}

/// Writes one frame. A payload over `max` fails with `TooLarge` before anything is written.
pub fn write_frame<W: Write, T: Serialize>(
    w: &mut W,
    msg: &T,
    max: usize,
) -> Result<(), WireError> {
    let mut buf = Vec::new();
    encode_frame(msg, max, &mut buf)?;
    w.write_all(&buf).map_err(WireError::Io)
}

/// Reads one frame and decodes it as `T`.
///
/// The length is checked against `max` before the payload buffer is allocated, so an oversize
/// prefix leaves the reader just past the 4 prefix bytes. Trailing bytes after the postcard
/// value are a decode error, because they mean the two sides disagree on the message type.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R, max: usize) -> Result<T, WireError> {
    let len = read_prefix(r)?;
    if len > max {
        return Err(WireError::TooLarge { len, max });
    }
    read_payload(r, len)
}

/// Reads frames until `is_last(&msg)` is true. Fails with ReplyTooLarge when the summed payload
/// bytes pass `max_total`, checked before reading each payload.
pub fn read_reply<R: Read, T: DeserializeOwned>(
    r: &mut R,
    max_frame: usize,
    max_total: usize,
    is_last: impl Fn(&T) -> bool,
) -> Result<Vec<T>, WireError> {
    let mut msgs = Vec::new();
    let mut total: usize = 0;
    loop {
        let len = read_prefix(r)?;
        if len > max_frame {
            return Err(WireError::TooLarge {
                len,
                max: max_frame,
            });
        }
        total = total.saturating_add(len);
        if total > max_total {
            return Err(WireError::ReplyTooLarge {
                total,
                max: max_total,
            });
        }
        let msg: T = read_payload(r, len)?;
        let last = is_last(&msg);
        msgs.push(msg);
        if last {
            return Ok(msgs);
        }
    }
}

/// Reads the 4-byte prefix. `Eof` only when the stream ends before its first byte.
fn read_prefix<R: Read>(r: &mut R) -> Result<usize, WireError> {
    let mut prefix = [0u8; PREFIX_LEN];
    let mut filled = 0;
    while filled < PREFIX_LEN {
        match r.read(&mut prefix[filled..]) {
            Ok(0) if filled == 0 => return Err(WireError::Eof),
            Ok(0) => return Err(WireError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(WireError::Io(e)),
        }
    }
    Ok(u32::from_le_bytes(prefix) as usize)
}

/// Reads exactly `len` payload bytes into one buffer of that size and decodes it.
fn read_payload<R: Read, T: DeserializeOwned>(r: &mut R, len: usize) -> Result<T, WireError> {
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).map_err(|e| match e.kind() {
        ErrorKind::UnexpectedEof => WireError::Truncated,
        _ => WireError::Io(e),
    })?;
    let (msg, rest) = postcard::take_from_bytes(&payload).map_err(WireError::Decode)?;
    if !rest.is_empty() {
        return Err(WireError::Decode(postcard::Error::DeserializeBadEncoding));
    }
    Ok(msg)
}

/// Lets postcard append to a borrowed `Vec` so `encode_frame` can restore it on error.
struct VecExtend<'a>(&'a mut Vec<u8>);

impl Extend<u8> for VecExtend<'_> {
    fn extend<I: IntoIterator<Item = u8>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}
