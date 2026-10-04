//! Retention and disk cap of the history store (ADR-012, PROC-087, R-025).
//!
//! `enforce` runs after every appended block. `prune_other_boots` runs at start, and
//! `purge_current_boot` at shutdown when `history.persist = false`.
//!
//! Times of different boots compare in CLOCK_REALTIME: a segment's boottime plus the
//! `realtime_offset_ns` in its header.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use tl_model::time::{BootId, TimeNs};

use crate::error::HistoryError;
use crate::format::BLOCK_SECS;
use crate::segment::SegmentReader;
use crate::store::{
    BAD_DELETED_KEYS_FILE, DELETED_KEYS_FILE, Seg, Store, parse_boot_dir, segment_paths,
};

const BLOCK_NS: u64 = BLOCK_SECS * 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub deleted_segments: u32,
    pub deleted_dirs: u32,
    pub freed_bytes: u64,
}

/// Collects the counts and the first error while deletion goes on.
#[derive(Default)]
struct Deleter {
    report: RetentionReport,
    first_err: Option<HistoryError>,
}

impl Deleter {
    /// Removes one file. A file that is already gone counts as deleted.
    fn file(&mut self, path: &Path, size: u64) -> bool {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                self.fail(path, source);
                return false;
            }
        }
        self.report.deleted_segments += 1;
        self.report.freed_bytes += size;
        true
    }

    fn fail(&mut self, path: &Path, source: std::io::Error) {
        self.first_err.get_or_insert(HistoryError::Io {
            path: path.to_path_buf(),
            source,
        });
    }

    fn finish(self) -> Result<RetentionReport, HistoryError> {
        self.first_err.map_or(Ok(self.report), Err)
    }
}

/// End of the last block of `seg` in boottime; the header start when it has no block.
fn seg_end(seg: &Seg) -> u64 {
    seg.reader
        .index
        .last()
        .map_or(seg.reader.header.start_t_ns, |e| {
            e.start_t_ns.saturating_add(BLOCK_NS)
        })
}

/// `t` of the segment's boot in CLOCK_REALTIME.
fn to_realtime(seg: &Seg, t: u64) -> i64 {
    (t as i64).saturating_add(seg.reader.header.realtime_offset_ns)
}

/// Deletes the segments of `segs[..closed]` for which `old` is true. A segment whose delete
/// fails stays in the index.
fn delete_old(segs: &mut Vec<Seg>, closed: usize, del: &mut Deleter, old: impl Fn(&Seg) -> bool) {
    let mut i = 0;
    let mut closed = closed;
    while i < closed {
        if old(&segs[i]) && del.file(&segs[i].path, segs[i].reader.size) {
            segs.remove(i);
            closed -= 1;
        } else {
            i += 1;
        }
    }
}

/// Deletes closed segments whose last block ends before now - retention, then the oldest closed
/// segments until disk_bytes <= max_disk_bytes. Never deletes the open segment.
///
/// Files the store does not read (`expired`) are deleted first. On a failed delete it goes on
/// with the next file and returns the first error at the end.
pub fn enforce(store: &mut Store, now_t_ns: TimeNs) -> Result<RetentionReport, HistoryError> {
    let mut del = Deleter::default();
    for (path, size) in std::mem::take(&mut store.expired) {
        if !del.file(&path, size) {
            store.expired.push((path, size));
        }
    }

    let retention = store.cfg.retention_ns;
    let closed = store.segs.len() - usize::from(store.has_open_segment());
    delete_old(&mut store.segs, closed, &mut del, |s| {
        now_t_ns.saturating_sub(seg_end(s)) >= retention
    });
    let now_rt = (now_t_ns as i64).saturating_add(store.realtime_offset_ns());
    let others = store.others.len();
    delete_old(&mut store.others, others, &mut del, |s| {
        now_rt.saturating_sub(to_realtime(s, seg_end(s))) >= retention as i64
    });

    let max = store.cfg.max_disk_bytes;
    if store.disk_bytes() > max {
        let closed = store.segs.len() - usize::from(store.has_open_segment());
        let mut oldest: Vec<(i64, PathBuf)> = store.segs[..closed]
            .iter()
            .chain(&store.others)
            .map(|s| (to_realtime(s, s.reader.header.start_t_ns), s.path.clone()))
            .collect();
        oldest.sort();
        for (_, path) in oldest {
            if store.disk_bytes() <= max {
                break;
            }
            let (list, i) = match store.segs.iter().position(|s| s.path == path) {
                Some(i) => (&mut store.segs, i),
                None => match store.others.iter().position(|s| s.path == path) {
                    Some(i) => (&mut store.others, i),
                    None => continue,
                },
            };
            if del.file(&path, list[i].reader.size) {
                list.remove(i);
            }
        }
    }
    del.finish()
}

/// Modification time of `path` in nanoseconds since the epoch; 0 when unknown.
fn mtime_ns(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// Realtime end of the newest data in a boot directory: the last block end of each segment, the
/// file mtime for a segment that does not open, and the directory mtime when there is no segment.
fn newest_end_rt(dir: &Path, paths: &[PathBuf]) -> i64 {
    paths
        .iter()
        .map(|path| match SegmentReader::open(path) {
            Ok(reader) => {
                let seg = Seg {
                    path: path.clone(),
                    reader,
                };
                to_realtime(&seg, seg_end(&seg))
            }
            Err(_) => mtime_ns(path),
        })
        .max()
        .unwrap_or_else(|| mtime_ns(dir))
}

/// At start: deletes boot directories other than the current one whose newest segment ends before
/// now - retention, using the segment header realtime_offset_ns to compare across boots in
/// CLOCK_REALTIME.
///
/// Only directories named with 16 lowercase hex digits are boot directories. Every other entry
/// in `root` is left alone. A missing `root` is not an error.
pub fn prune_other_boots(
    root: &Path,
    current: BootId,
    now_realtime_ns: i64,
    retention_ns: u64,
) -> Result<RetentionReport, HistoryError> {
    let mut del = Deleter::default();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(del.report),
        Err(source) => {
            del.fail(root, source);
            return del.finish();
        }
    };
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(boot) = entry.file_name().to_str().and_then(parse_boot_dir) else {
            continue;
        };
        if boot == current {
            continue;
        }
        let dir = entry.path();
        let paths = match segment_paths(&dir) {
            Ok(p) => p,
            Err(source) => {
                del.fail(&dir, source);
                continue;
            }
        };
        let age = now_realtime_ns.saturating_sub(newest_end_rt(&dir, &paths));
        if age < retention_ns as i64 {
            continue;
        }
        let bytes: u64 = paths
            .iter()
            .filter_map(|p| fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        match fs::remove_dir_all(&dir) {
            Ok(()) => {
                del.report.deleted_dirs += 1;
                del.report.deleted_segments += u32::try_from(paths.len()).unwrap_or(u32::MAX);
                del.report.freed_bytes += bytes;
            }
            Err(source) => del.fail(&dir, source),
        }
    }
    del.finish()
}

/// persist = false at shutdown: closes and deletes every segment and the deleted-keys file of
/// the current boot.
///
/// It lists the boot directory on disk, so files the store did not index go too. The
/// `deleted-keys.bin.bad` and `.tmp` files go as well; they count in `freed_bytes` only.
pub fn purge_current_boot(mut store: Store) -> Result<RetentionReport, HistoryError> {
    let mut del = Deleter::default();
    if let Err(e) = store.close() {
        del.first_err = Some(e);
    }
    let dir = store.boot_dir.clone();
    drop(store);
    match segment_paths(&dir) {
        Ok(paths) => {
            for path in paths {
                let size = fs::metadata(&path).map_or(0, |m| m.len());
                del.file(&path, size);
            }
        }
        Err(source) => del.fail(&dir, source),
    }
    let tmp = format!("{DELETED_KEYS_FILE}.tmp");
    for name in [DELETED_KEYS_FILE, BAD_DELETED_KEYS_FILE, tmp.as_str()] {
        let path = dir.join(name);
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        match fs::remove_file(&path) {
            Ok(()) => del.report.freed_bytes += meta.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => del.fail(&path, source),
        }
    }
    del.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    use crate::block::encode_block;
    use crate::format::{Block, HistMeta, HistProcess, SegmentHeader, boot_dir_name};
    use crate::segment::SegmentWriter;
    use crate::store::StoreConfig;
    use tl_model::ids::ProcessKey;

    const SEC: u64 = 1_000_000_000;
    const MIN: u64 = 60 * SEC;
    const HOUR: u64 = 3600 * SEC;
    const MIB: u64 = 1024 * 1024;
    const BOOT: BootId = BootId(0xabc);
    /// A fixed CLOCK_REALTIME "now" for the cross-boot tests.
    const NOW_RT: i64 = 1_800_000_000 * SEC as i64;

    fn cfg(root: &Path) -> StoreConfig {
        StoreConfig {
            root: root.to_path_buf(),
            boot: BOOT,
            retention_ns: 6 * HOUR,
            max_disk_bytes: 256 * MIB,
            load_existing: true,
        }
    }

    fn later() -> Instant {
        Instant::now() + std::time::Duration::from_secs(30)
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 500,
            boot_id: BOOT,
            pid,
        }
    }

    fn block(start_t_ns: u64) -> Block {
        Block {
            start_t_ns,
            gap_before: false,
            mem_total_kib: 1024,
            catalog: None,
            new_meta: vec![(
                0,
                HistMeta {
                    comm: Arc::from("a"),
                    name: Arc::from("a"),
                    exe_basename: None,
                    app_id: None,
                    exe_path: None,
                    user: Arc::from("u"),
                    uid: 1000,
                    system: false,
                },
            )],
            keyframe: vec![HistProcess {
                key: key(1),
                meta: 0,
                ppid: 1,
                cpu_permille: 10,
                mem_anon_kib: 100,
                io_r_kibps: 0,
                io_w_kibps: 0,
                net_kibps: 0,
                gpu_pct: 0,
            }],
            deltas: vec![],
            system: vec![],
        }
    }

    /// A store with one block at the start of each 10-minute segment in [0, minutes).
    fn store_with_segments(root: &Path, minutes: u64) -> Store {
        let (mut store, _) = Store::open(cfg(root), 0, later());
        for m in (0..minutes).step_by(10) {
            store.append(block(m * MIN), 1_000).unwrap();
        }
        store
    }

    /// Writes a closed segment of `boot` with one block at 0 that ended `age` ago in realtime.
    fn other_boot_segment(root: &Path, boot: BootId, age: u64) -> PathBuf {
        let dir = root.join(boot_dir_name(boot));
        fs::create_dir_all(&dir).unwrap();
        let header = SegmentHeader {
            boot_id: boot,
            realtime_offset_ns: NOW_RT - age as i64 - BLOCK_NS as i64,
            start_t_ns: 0,
        };
        let mut w = SegmentWriter::create(&dir, header).unwrap();
        w.append(&encode_block(&block(0)).unwrap()).unwrap();
        w.close().unwrap();
        dir
    }

    fn boot_dir(root: &Path) -> PathBuf {
        root.join(boot_dir_name(BOOT))
    }

    #[test]
    fn age_based_deletion() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store_with_segments(root.path(), 8 * 60);
        assert_eq!(segment_paths(&boot_dir(root.path())).unwrap().len(), 48);

        let report = enforce(&mut store, 8 * HOUR).unwrap();
        // Segments 0..=110 min ended at least 6 h before 8 h; the one at 120 min ended 5 h 59 min ago.
        assert_eq!(report.deleted_segments, 12);
        assert_eq!(report.deleted_dirs, 0);
        assert!(report.freed_bytes > 0);
        assert_eq!(segment_paths(&boot_dir(root.path())).unwrap().len(), 36);
        assert_eq!(store.span().map(|s| s.0), Some(120 * MIN));

        // A shorter retention applies at the next enforce.
        store.set_limits(HOUR, 256 * MIB);
        let report = enforce(&mut store, 8 * HOUR).unwrap();
        assert_eq!(report.deleted_segments, 30);
        assert_eq!(store.span().map(|s| s.0), Some(7 * HOUR));
    }

    #[test]
    fn disk_cap_oldest_first_keeps_open() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store_with_segments(root.path(), 100);
        // 10 segments counted as 30 MiB each: a 300 MiB store, without writing 300 MiB.
        for seg in &mut store.segs {
            seg.reader.size = 30 * MIB;
        }
        assert_eq!(store.disk_bytes(), 300 * MIB);

        let report = enforce(&mut store, 100 * MIN).unwrap();
        assert_eq!(report.deleted_segments, 2);
        assert_eq!(report.freed_bytes, 60 * MIB);
        assert_eq!(store.disk_bytes(), 240 * MIB);
        assert_eq!(store.span().map(|s| s.0), Some(20 * MIN));

        store.set_limits(6 * HOUR, 0);
        let report = enforce(&mut store, 100 * MIN).unwrap();
        assert_eq!(report.deleted_segments, 7);
        let files = segment_paths(&boot_dir(root.path())).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("seg-0000005400.tlh"));
        assert_eq!(store.span(), Some((90 * MIN, 90 * MIN)));
    }

    #[test]
    fn other_boots_pruned_by_realtime() {
        let root = tempfile::tempdir().unwrap();
        let old = other_boot_segment(root.path(), BootId(0x1), 7 * HOUR);
        let recent = other_boot_segment(root.path(), BootId(0x2), 2 * HOUR);
        // The current boot is never pruned here, however old its data.
        let current = other_boot_segment(root.path(), BOOT, 10 * HOUR);

        let report = prune_other_boots(root.path(), BOOT, NOW_RT, 6 * HOUR).unwrap();
        assert_eq!(report.deleted_dirs, 1);
        assert_eq!(report.deleted_segments, 1);
        assert!(report.freed_bytes > 0);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(current.exists());
    }

    #[test]
    fn foreign_files_untouched() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join(boot_dir_name(BootId(0x5)));
        fs::write(&file, b"not a boot directory").unwrap();
        let notes = root.path().join("notes.txt");
        fs::write(&notes, b"keep").unwrap();
        let foreign_dir = root.path().join("lost+found");
        fs::create_dir(&foreign_dir).unwrap();
        fs::write(foreign_dir.join("seg-0000000000.tlh"), b"x").unwrap();
        let old = other_boot_segment(root.path(), BootId(0x1), 7 * HOUR);

        let report = prune_other_boots(root.path(), BOOT, NOW_RT, 6 * HOUR).unwrap();
        assert_eq!(report.deleted_dirs, 1);
        assert!(!old.exists());
        assert_eq!(fs::read(&file).unwrap(), b"not a boot directory");
        assert!(notes.exists());
        assert!(foreign_dir.join("seg-0000000000.tlh").exists());
    }

    #[test]
    fn purge_current_boot_removes_all() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store_with_segments(root.path(), 20);
        store.delete_keys(&[key(1)]).unwrap();
        let dir = boot_dir(root.path());
        assert!(dir.join(DELETED_KEYS_FILE).exists());

        let report = purge_current_boot(store).unwrap();
        assert_eq!(report.deleted_segments, 2);
        assert!(segment_paths(&dir).unwrap().is_empty());
        assert!(!dir.join(DELETED_KEYS_FILE).exists());
    }

    /// R-271: segments hidden because their deleted keys were lost are deleted by `enforce`.
    #[test]
    fn expired_files_deleted() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store_with_segments(root.path(), 20);
        store.close().unwrap();
        drop(store);
        let dir = boot_dir(root.path());
        fs::write(dir.join(DELETED_KEYS_FILE), b"\xff\xff").unwrap();

        let (mut store, report) = Store::open(cfg(root.path()), 20 * MIN, later());
        assert_eq!(
            report.bad_deleted_keys,
            vec![dir.join(BAD_DELETED_KEYS_FILE)]
        );
        let report = enforce(&mut store, 20 * MIN).unwrap();
        assert_eq!(report.deleted_segments, 2);
        assert!(segment_paths(&dir).unwrap().is_empty());
        assert_eq!(store.disk_bytes(), 0);
    }
}
