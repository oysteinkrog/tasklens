//! Segment files (ADR-012, PROC-087): a 64-byte header, 60 s block frames and, when the segment
//! closed cleanly, a footer index.
//!
//! Footer (R-169 OI-3): postcard `Vec<FooterEntry>`, then `u32 LE footer length | u32 LE crc32 of
//! the footer bytes | b"TLHF"`. A file without a valid footer is a crashed segment: the reader
//! scans frames from byte 64 and keeps every block before the first short read or bad crc.
//!
//! Every length read from disk is checked against the file size before anything is allocated.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::block::{EncodedBlock, FRAME_PREFIX_LEN, decode_block};
use crate::error::HistoryError;
use crate::format::{
    Block, FOOTER_MAGIC, FooterEntry, HEADER_LEN, SegmentHeader, segment_file_name,
};

/// Bytes after the footer body: length, crc32 and magic.
pub const FOOTER_TRAILER_LEN: usize = 12;

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> HistoryError + '_ {
    move |source| HistoryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Writes one segment. Each appended block is on disk before `append` returns.
pub struct SegmentWriter {
    file: File,
    path: PathBuf,
    offset: u64,
    index: Vec<FooterEntry>,
}

impl SegmentWriter {
    /// Creates `dir/seg-XXXXXXXXXX.tlh` with mode 0600 (O_CREAT | O_EXCL) and writes the header.
    pub fn create(dir: &Path, header: SegmentHeader) -> Result<SegmentWriter, HistoryError> {
        let path = dir.join(segment_file_name(header.start_t_ns));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(io_err(&path))?;
        file.write_all(&header.encode()).map_err(io_err(&path))?;
        Ok(SegmentWriter {
            file,
            path,
            offset: HEADER_LEN as u64,
            index: Vec::new(),
        })
    }

    /// Writes the frame, then `fdatasync`.
    pub fn append(&mut self, b: &EncodedBlock) -> Result<(), HistoryError> {
        self.file.write_all(&b.bytes).map_err(io_err(&self.path))?;
        self.file.sync_data().map_err(io_err(&self.path))?;
        self.index.push(FooterEntry {
            start_t_ns: b.start_t_ns,
            offset: self.offset,
        });
        self.offset += b.bytes.len() as u64;
        Ok(())
    }

    /// Writes the footer, then `fsync`.
    pub fn close(mut self) -> Result<(), HistoryError> {
        let body = postcard::to_stdvec(&self.index)?;
        let len = u32::try_from(body.len())
            .map_err(|_| io_err(&self.path)(std::io::Error::other("footer exceeds 4 GiB")))?;
        let mut footer = body;
        let crc = crc32fast::hash(&footer);
        footer.extend_from_slice(&len.to_le_bytes());
        footer.extend_from_slice(&crc.to_le_bytes());
        footer.extend_from_slice(&FOOTER_MAGIC);
        self.file.write_all(&footer).map_err(io_err(&self.path))?;
        self.file.sync_all().map_err(io_err(&self.path))
    }

    /// Header and block bytes written so far, without the footer.
    pub fn bytes_written(&self) -> u64 {
        self.offset
    }
}

/// An opened segment. Only the index is kept in RAM; blocks are read on demand.
pub struct SegmentReader {
    pub header: SegmentHeader,
    pub index: Vec<FooterEntry>,
    /// True when the footer was missing or bad and the index came from a scan.
    pub recovered: bool,
    pub size: u64,
}

impl SegmentReader {
    /// Reads the header and the footer; without a valid footer scans blocks from byte 64 and sets
    /// recovered = true.
    pub fn open(path: &Path) -> Result<SegmentReader, HistoryError> {
        let mut file = File::open(path).map_err(io_err(path))?;
        let size = file.metadata().map_err(io_err(path))?.len();
        if size < HEADER_LEN as u64 {
            return Err(HistoryError::BadHeader);
        }
        let mut head = [0u8; HEADER_LEN];
        file.read_exact(&mut head).map_err(io_err(path))?;
        let header = SegmentHeader::decode(&head)?;

        if let Some(index) = read_footer(&mut file, path, size)? {
            return Ok(SegmentReader {
                header,
                index,
                recovered: false,
                size,
            });
        }
        let index = scan(&mut file, path, size)?;
        Ok(SegmentReader {
            header,
            index,
            recovered: true,
            size,
        })
    }

    /// Reads and decodes the block that `entry` points at.
    pub fn read_block(&self, path: &Path, entry: &FooterEntry) -> Result<Block, HistoryError> {
        let mut file = File::open(path).map_err(io_err(path))?;
        let frame = read_frame(&mut file, path, entry.offset, self.size)?;
        decode_block(&frame, entry.offset).map(|(b, _)| b)
    }
}

/// Reads the frame at `offset` without decoding it. A frame that runs past `size` is `Truncated`.
fn read_frame(
    file: &mut File,
    path: &Path,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, HistoryError> {
    let prefix_end = offset.checked_add(FRAME_PREFIX_LEN as u64);
    if offset < HEADER_LEN as u64 || prefix_end.is_none_or(|end| end > size) {
        return Err(HistoryError::Truncated(offset));
    }
    file.seek(SeekFrom::Start(offset)).map_err(io_err(path))?;
    let mut prefix = [0u8; FRAME_PREFIX_LEN];
    file.read_exact(&mut prefix).map_err(io_err(path))?;
    let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
    let total = FRAME_PREFIX_LEN as u64 + u64::from(len);
    if offset + total > size {
        return Err(HistoryError::Truncated(offset));
    }
    let mut frame = vec![0u8; total as usize];
    frame[..FRAME_PREFIX_LEN].copy_from_slice(&prefix);
    file.read_exact(&mut frame[FRAME_PREFIX_LEN..])
        .map_err(io_err(path))?;
    Ok(frame)
}

/// The footer index, or `None` when the file has no valid footer. A footer whose crc matches but
/// whose entries point outside the block area is treated as missing.
fn read_footer(
    file: &mut File,
    path: &Path,
    size: u64,
) -> Result<Option<Vec<FooterEntry>>, HistoryError> {
    let min = (HEADER_LEN + FOOTER_TRAILER_LEN) as u64;
    if size < min {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(size - FOOTER_TRAILER_LEN as u64))
        .map_err(io_err(path))?;
    let mut trailer = [0u8; FOOTER_TRAILER_LEN];
    file.read_exact(&mut trailer).map_err(io_err(path))?;
    if trailer[8..12] != FOOTER_MAGIC {
        return Ok(None);
    }
    let len = u64::from(u32::from_le_bytes([
        trailer[0], trailer[1], trailer[2], trailer[3],
    ]));
    let crc = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    if len > size - min {
        return Ok(None);
    }
    let footer_start = size - FOOTER_TRAILER_LEN as u64 - len;
    file.seek(SeekFrom::Start(footer_start))
        .map_err(io_err(path))?;
    let mut body = vec![0u8; len as usize];
    file.read_exact(&mut body).map_err(io_err(path))?;
    if crc32fast::hash(&body) != crc {
        return Ok(None);
    }
    let Ok(index) = postcard::from_bytes::<Vec<FooterEntry>>(&body) else {
        return Ok(None);
    };
    let in_range = index.iter().all(|e| {
        e.offset >= HEADER_LEN as u64
            && e.offset
                .checked_add(FRAME_PREFIX_LEN as u64)
                .is_some_and(|end| end <= footer_start)
    });
    Ok(in_range.then_some(index))
}

/// Crash recovery: lists every frame from byte 64 up to the first short read, bad crc or block
/// that does not decode.
fn scan(file: &mut File, path: &Path, size: u64) -> Result<Vec<FooterEntry>, HistoryError> {
    let mut index = Vec::new();
    let mut offset = HEADER_LEN as u64;
    while offset < size {
        let frame = match read_frame(file, path, offset, size) {
            Ok(f) => f,
            Err(HistoryError::Truncated(_)) => break,
            Err(e) => return Err(e),
        };
        let Ok((block, used)) = decode_block(&frame, offset) else {
            break;
        };
        index.push(FooterEntry {
            start_t_ns: block.start_t_ns,
            offset,
        });
        offset += used as u64;
    }
    Ok(index)
}
