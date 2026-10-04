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
///
/// After a failed write the file may end in a partial frame. The writer is then broken: every
/// later `append` and `close` fails at once and writes nothing, so the file has no footer and a
/// reader keeps every block before the partial frame.
pub struct SegmentWriter {
    file: File,
    path: PathBuf,
    offset: u64,
    index: Vec<FooterEntry>,
    /// Kind of the first failed write, or `None` while the writer is healthy.
    broken: Option<std::io::ErrorKind>,
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
            broken: None,
        })
    }

    /// `Io` with the kind of the first failed write when the writer is broken.
    fn check_broken(&self) -> Result<(), HistoryError> {
        match self.broken {
            Some(kind) => Err(io_err(&self.path)(std::io::Error::new(
                kind,
                "segment writer is broken by an earlier failed write",
            ))),
            None => Ok(()),
        }
    }

    /// Writes the frame, then `fdatasync`. Any write error breaks the writer.
    pub fn append(&mut self, b: &EncodedBlock) -> Result<(), HistoryError> {
        self.check_broken()?;
        let written = self
            .file
            .write_all(&b.bytes)
            .and_then(|()| self.file.sync_data());
        if let Err(e) = written {
            self.broken = Some(e.kind());
            return Err(io_err(&self.path)(e));
        }
        self.index.push(FooterEntry {
            start_t_ns: b.start_t_ns,
            offset: self.offset,
        });
        self.offset += b.bytes.len() as u64;
        Ok(())
    }

    /// Writes the footer, then `fsync`. A broken writer writes no footer and returns `Io`.
    pub fn close(mut self) -> Result<(), HistoryError> {
        self.check_broken()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::encode_block;
    use crate::format::Block;
    use tl_model::time::BootId;

    fn block(i: u64) -> Block {
        Block {
            start_t_ns: i * 60_000_000_000,
            gap_before: false,
            mem_total_kib: 1024,
            catalog: None,
            new_meta: vec![],
            keyframe: vec![],
            deltas: vec![],
            system: vec![],
        }
    }

    #[test]
    fn failed_append_breaks_writer() {
        let dir = tempfile::tempdir().unwrap();
        let header = SegmentHeader {
            boot_id: BootId(1),
            realtime_offset_ns: 0,
            start_t_ns: 0,
        };
        let mut w = SegmentWriter::create(dir.path(), header).unwrap();
        w.append(&encode_block(&block(0)).unwrap()).unwrap();
        let good_len = w.bytes_written();

        // Swap in a read-only handle so the next write fails with EBADF.
        w.file = File::open(&w.path).unwrap();
        let second = encode_block(&block(1)).unwrap();
        let first_err = w.append(&second).unwrap_err();
        let HistoryError::Io { source, .. } = first_err else {
            panic!("expected Io, got {first_err:?}");
        };
        let kind = source.kind();

        // A partial frame, as a short write before the failure would leave it.
        let mut raw = OpenOptions::new().append(true).open(&w.path).unwrap();
        raw.write_all(&second.bytes[..second.bytes.len() / 2])
            .unwrap();
        let size = std::fs::metadata(&w.path).unwrap().len();

        // Later appends fail at once with the same kind and write nothing.
        w.file = OpenOptions::new().append(true).open(&w.path).unwrap();
        let again = w.append(&encode_block(&block(2)).unwrap()).unwrap_err();
        assert!(matches!(again, HistoryError::Io { ref source, .. } if source.kind() == kind));
        assert_eq!(w.bytes_written(), good_len);

        // Close writes no footer.
        let path = w.path.clone();
        assert!(matches!(w.close(), Err(HistoryError::Io { .. })));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size);

        let r = SegmentReader::open(&path).unwrap();
        assert!(r.recovered);
        assert_eq!(r.index.len(), 1);
        assert_eq!(r.read_block(&path, &r.index[0]).unwrap(), block(0));
    }
}
