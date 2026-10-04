//! Block frame codec (ADR-012): `u32 LE length | u32 LE crc32 | zstd frame` of a postcard `Block`.
//!
//! Decoding never panics on bad input: a crashed segment is read up to the first bad frame.

use crate::error::HistoryError;
use crate::format::{Block, ZSTD_LEVEL};

/// Bytes before the zstd frame: compressed length and crc32.
pub const FRAME_PREFIX_LEN: usize = 8;
/// Largest decompressed block accepted, 64 MiB. A frame claiming more is `Zstd`.
pub const MAX_BLOCK_RAW: usize = 64 << 20;

/// One encoded block. `bytes` is the full frame including the 8-byte prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedBlock {
    pub start_t_ns: u64,
    pub bytes: Vec<u8>,
}

/// Encodes `b` as postcard, compresses it with zstd level 3 and prefixes length and crc32.
pub fn encode_block(b: &Block) -> Result<EncodedBlock, HistoryError> {
    let raw = postcard::to_stdvec(b)?;
    let compressed = zstd::bulk::compress(&raw, ZSTD_LEVEL).map_err(HistoryError::Zstd)?;
    let len = u32::try_from(compressed.len()).map_err(|_| {
        HistoryError::Zstd(std::io::Error::other("compressed block exceeds 4 GiB"))
    })?;
    let mut bytes = Vec::with_capacity(FRAME_PREFIX_LEN + compressed.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&compressed).to_le_bytes());
    bytes.extend_from_slice(&compressed);
    Ok(EncodedBlock {
        start_t_ns: b.start_t_ns,
        bytes,
    })
}

/// Decodes the frame at the start of `frame`. `offset` is the frame's position in its file and
/// only goes into errors. Returns the block and the number of bytes consumed; bytes after the
/// frame are left alone.
pub fn decode_block(frame: &[u8], offset: u64) -> Result<(Block, usize), HistoryError> {
    let Some((prefix, rest)) = frame.split_first_chunk::<FRAME_PREFIX_LEN>() else {
        return Err(HistoryError::Truncated(offset));
    };
    let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    let crc = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
    let Some(compressed) = rest.get(..len) else {
        return Err(HistoryError::Truncated(offset));
    };
    if crc32fast::hash(compressed) != crc {
        return Err(HistoryError::Crc(offset));
    }
    // Size the output from the frame header when it has one, so a small block does not
    // reserve 64 MiB.
    let capacity = match zstd::zstd_safe::get_frame_content_size(compressed) {
        Ok(Some(n)) if n > MAX_BLOCK_RAW as u64 => {
            return Err(HistoryError::Zstd(std::io::Error::other(
                "block exceeds 64 MiB",
            )));
        }
        Ok(Some(n)) => n as usize,
        _ => MAX_BLOCK_RAW,
    };
    let raw = zstd::bulk::decompress(compressed, capacity).map_err(HistoryError::Zstd)?;
    let block = postcard::from_bytes(&raw)?;
    Ok((block, FRAME_PREFIX_LEN + len))
}
