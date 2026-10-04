//! Segment writer and reader: footer index, crash recovery and corrupt files (ADR-012, ADR-013).

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use proptest::prelude::*;
use tl_history::HistoryError;
use tl_history::block::{FRAME_PREFIX_LEN, encode_block};
use tl_history::format::{Block, HEADER_LEN, HistMeta, HistProcess, SegmentHeader};
use tl_history::segment::{FOOTER_TRAILER_LEN, SegmentReader, SegmentWriter};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

const START_NS: u64 = 12_345_000_000_000;
const BLOCK_NS: u64 = 60_000_000_000;

fn header() -> SegmentHeader {
    SegmentHeader {
        boot_id: BootId(0xfeed),
        realtime_offset_ns: 1_700_000_000_000_000_000,
        start_t_ns: START_NS,
    }
}

fn block(i: u64) -> Block {
    Block {
        start_t_ns: START_NS + i * BLOCK_NS,
        gap_before: i == 3,
        mem_total_kib: 65_746_000,
        catalog: None,
        new_meta: vec![(
            i as u32,
            HistMeta {
                comm: Arc::from(format!("comm{i}")),
                name: Arc::from(format!("Name {i}")),
                exe_basename: None,
                app_id: None,
                exe_path: None,
                user: Arc::from("alice"),
                uid: 1000,
                system: false,
            },
        )],
        keyframe: (0..20u32)
            .map(|p| HistProcess {
                key: ProcessKey {
                    start_ticks: u64::from(p) + i,
                    boot_id: BootId(0xfeed),
                    pid: 100 + p,
                },
                meta: i as u32,
                ppid: 1,
                cpu_permille: (p * 13 % 1001) as u16,
                mem_anon_kib: p * 4096,
                io_r_kibps: p,
                io_w_kibps: 0,
                net_kibps: 0,
                gpu_pct: 0,
            })
            .collect(),
        deltas: vec![],
        system: vec![],
    }
}

/// Writes `n` blocks. Returns the segment path and the offset of each block. With `close` false
/// the writer is dropped as if the process crashed.
fn write_segment(dir: &Path, n: u64, close: bool) -> (PathBuf, Vec<u64>) {
    let mut w = SegmentWriter::create(dir, header()).unwrap();
    let mut offsets = Vec::new();
    for i in 0..n {
        offsets.push(w.bytes_written());
        w.append(&encode_block(&block(i)).unwrap()).unwrap();
    }
    if close {
        w.close().unwrap();
    }
    (dir.join("seg-0000012345.tlh"), offsets)
}

fn set_len(path: &Path, len: u64) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(len)
        .unwrap();
}

fn flip_byte(path: &Path, at: u64) {
    let mut bytes = std::fs::read(path).unwrap();
    bytes[at as usize] ^= 0x01;
    std::fs::write(path, bytes).unwrap();
}

fn assert_blocks(r: &SegmentReader, path: &Path, offsets: &[u64]) {
    let got: Vec<u64> = r.index.iter().map(|e| e.offset).collect();
    assert_eq!(got, offsets);
    for (i, e) in r.index.iter().enumerate() {
        assert_eq!(e.start_t_ns, block(i as u64).start_t_ns);
        assert_eq!(r.read_block(path, e).unwrap(), block(i as u64));
    }
}

#[test]
fn footer_index_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 10, true);
    let r = SegmentReader::open(&path).unwrap();
    assert_eq!(r.header, header());
    assert_eq!(r.index.len(), 10);
    assert!(!r.recovered);
    assert_eq!(r.size, std::fs::metadata(&path).unwrap().len());
    assert_eq!(offsets[0], HEADER_LEN as u64);
    assert_blocks(&r, &path, &offsets);
}

#[test]
fn missing_footer_truncated_last_block() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 10, true);
    // The footer body length is in the trailer.
    let bytes = std::fs::read(&path).unwrap();
    let t = bytes.len() - FOOTER_TRAILER_LEN;
    let footer_len = u32::from_le_bytes(bytes[t..t + 4].try_into().unwrap()) as u64;
    let blocks_end = t as u64 - footer_len;

    set_len(&path, blocks_end - 5);
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert_eq!(r.index.len(), 9);
    assert_blocks(&r, &path, &offsets[..9]);
}

#[test]
fn crashed_writer_without_close_recovers_all_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 10, false);
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert_blocks(&r, &path, &offsets);
}

#[test]
fn bad_crc_mid_segment_stops_scan() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 10, false);
    // A byte inside the compressed data of block 5.
    flip_byte(&path, offsets[5] + FRAME_PREFIX_LEN as u64 + 3);
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert_blocks(&r, &path, &offsets[..5]);
    assert!(matches!(
        r.read_block(&path, &tl_history::format::FooterEntry {
            start_t_ns: 0,
            offset: offsets[5],
        }),
        Err(HistoryError::Crc(o)) if o == offsets[5]
    ));
}

#[test]
fn bad_footer_crc_falls_back_to_scan() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 10, true);
    let size = std::fs::metadata(&path).unwrap().len();
    // The last byte of the footer body.
    flip_byte(&path, size - FOOTER_TRAILER_LEN as u64 - 1);
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert_blocks(&r, &path, &offsets);
}

#[test]
fn footer_length_past_file_start_falls_back_to_scan() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 3, true);
    let mut bytes = std::fs::read(&path).unwrap();
    let t = bytes.len() - FOOTER_TRAILER_LEN;
    bytes[t..t + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert_blocks(&r, &path, &offsets);
}

#[test]
fn huge_block_length_does_not_allocate() {
    let dir = tempfile::tempdir().unwrap();
    let (path, offsets) = write_segment(dir.path(), 4, false);
    let mut bytes = std::fs::read(&path).unwrap();
    let at = offsets[2] as usize;
    bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert_blocks(&r, &path, &offsets[..2]);
}

#[test]
fn header_only_segment_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_segment(dir.path(), 0, false);
    let r = SegmentReader::open(&path).unwrap();
    assert!(r.recovered);
    assert!(r.index.is_empty());
    assert_eq!(r.size, HEADER_LEN as u64);
}

#[test]
fn short_or_bad_header_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_segment(dir.path(), 2, true);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] = b'X';
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        SegmentReader::open(&path),
        Err(HistoryError::BadHeader)
    ));

    std::fs::write(&path, &bytes[..HEADER_LEN - 1]).unwrap();
    assert!(matches!(
        SegmentReader::open(&path),
        Err(HistoryError::BadHeader)
    ));

    let mut v2 = header().encode();
    v2[4] = 2;
    std::fs::write(&path, v2).unwrap();
    assert!(matches!(
        SegmentReader::open(&path),
        Err(HistoryError::Version(2))
    ));
}

#[test]
fn read_block_outside_file_is_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_segment(dir.path(), 1, true);
    let r = SegmentReader::open(&path).unwrap();
    for offset in [0, r.size - 4, u64::MAX - 2] {
        let e = tl_history::format::FooterEntry {
            start_t_ns: 0,
            offset,
        };
        assert!(matches!(
            r.read_block(&path, &e),
            Err(HistoryError::Truncated(o)) if o == offset
        ));
    }
}

#[test]
fn segment_file_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_segment(dir.path(), 1, true);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn create_refuses_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seg-0000012345.tlh");
    std::fs::write(&path, b"keep me").unwrap();
    assert!(matches!(
        SegmentWriter::create(dir.path(), header()),
        Err(HistoryError::Io { path: p, .. }) if p == path
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any bytes after a valid header open without a panic, and every listed block reads back.
    #[test]
    fn arbitrary_tail_never_panics(tail in proptest::collection::vec(any::<u8>(), 0..512)) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg-0000012345.tlh");
        let mut bytes = header().encode().to_vec();
        bytes.extend_from_slice(&tail);
        std::fs::write(&path, &bytes).unwrap();
        let r = SegmentReader::open(&path).unwrap();
        for e in &r.index {
            let _ = r.read_block(&path, e);
        }
    }

    /// Cutting a closed segment at any length never panics and never lists a block that does
    /// not read back.
    #[test]
    fn any_truncation_recovers_a_prefix(cut in 0u64..4096) {
        let dir = tempfile::tempdir().unwrap();
        let (path, offsets) = write_segment(dir.path(), 3, true);
        let size = std::fs::metadata(&path).unwrap().len();
        set_len(&path, cut.min(size));
        match SegmentReader::open(&path) {
            Ok(r) => {
                prop_assert!(r.index.len() <= 3);
                let got: Vec<u64> = r.index.iter().map(|e| e.offset).collect();
                prop_assert_eq!(&got[..], &offsets[..r.index.len()]);
                for (i, e) in r.index.iter().enumerate() {
                    prop_assert_eq!(r.read_block(&path, e).unwrap(), block(i as u64));
                }
            }
            Err(e) => prop_assert!(matches!(e, HistoryError::BadHeader)),
        }
    }
}
