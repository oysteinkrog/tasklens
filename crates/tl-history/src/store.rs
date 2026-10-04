//! The on-disk history of the current boot (ADR-012, PROC-087, PROC-088, R-103).
//!
//! Layout: `root/<boot id, 16 hex digits>/seg-<start seconds>.tlh`, one segment per 10 minutes,
//! plus `deleted-keys.bin` beside the segments. Directories have mode 0700, files 0600.
//!
//! Every call does blocking file I/O. The engine calls the store only on the blocking pool.
//!
//! `span` and `blocks` cover the segments of the current boot. Segments of other boots inside
//! retention are indexed (they count in `disk_bytes` and decide `Stale`), but their boottime
//! values cannot be expressed as a `TimeNs` of this boot, so queries do not return them.
//!
//! `MetaId`s restart at 0 in every segment. When `blocks` returns blocks of more than one
//! segment, it shifts the meta ids of each later segment past the ids of the earlier ones, so
//! the one returned `MetaTable` resolves every block.

use std::collections::VecDeque;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tl_model::ids::ProcessKey;
use tl_model::time::{BootId, TimeNs};

use crate::block::encode_block;
use crate::error::HistoryError;
use crate::format::{
    BLOCK_SECS, Block, FooterEntry, MetaId, SEGMENT_SECS, SegmentHeader, boot_dir_name,
    segment_file_name,
};
use crate::read::{DeletedKeys, MetaTable};
use crate::segment::{SegmentReader, SegmentWriter};

/// Name of the per-boot deleted-keys file (R-169 OI-3).
pub const DELETED_KEYS_FILE: &str = "deleted-keys.bin";
/// Most decoded blocks kept in RAM (16 MB history budget, ADR-012).
pub const BLOCK_CACHE_LEN: usize = 16;

const NS_PER_SEC: u64 = 1_000_000_000;
const NS_PER_MS: u64 = 1_000_000;
const BLOCK_NS: u64 = BLOCK_SECS * NS_PER_SEC;
const SEGMENT_NS: u64 = SEGMENT_SECS * NS_PER_SEC;
/// The newest block of another boot is stale when older than retention minus this.
const STALE_MARGIN_NS: u64 = 60 * NS_PER_SEC;

/// What went wrong while loading at start (PROC-087). The UI shows the text in each comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadProblem {
    /// "Could not open process history store."
    OpenFailed,
    /// "Process history store is empty or truncated."
    EmptyOrTruncated,
    /// "Process history store version/layout mismatch; starting fresh."
    VersionMismatch,
    /// "No in-range process history in store."
    NoInRange,
    /// "Process history store is stale."
    Stale,
    /// "Process history load timed out; starting fresh."
    TimedOut,
}

/// Result of `Store::open`.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadReport {
    pub problem: Option<LoadProblem>,
    /// Segments loaded for reading, of this boot and of other boots.
    pub segments: usize,
    pub span: Option<(TimeNs, TimeNs)>,
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub root: PathBuf,
    pub boot: BootId,
    pub retention_ns: u64,
    pub max_disk_bytes: u64,
    /// False when `history.persist = false`: load nothing at start (PROC-087).
    pub load_existing: bool,
}

/// One indexed segment file.
pub(crate) struct Seg {
    pub(crate) path: PathBuf,
    pub(crate) reader: SegmentReader,
}

/// Decoded blocks, least recently used first. Key: segment start and block offset.
#[derive(Default)]
struct BlockCache {
    entries: VecDeque<((TimeNs, u64), Arc<Block>)>,
}

impl BlockCache {
    fn get(&mut self, key: (TimeNs, u64)) -> Option<Arc<Block>> {
        let pos = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(pos)?;
        let b = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(b)
    }

    fn put(&mut self, key: (TimeNs, u64), b: Arc<Block>) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(pos);
        }
        self.entries.push_back((key, b));
        while self.entries.len() > BLOCK_CACHE_LEN {
            self.entries.pop_front();
        }
    }
}

pub struct Store {
    pub(crate) cfg: StoreConfig,
    boot_dir: PathBuf,
    /// Segments of this boot inside retention, oldest first. While `writer` is set, the last
    /// one is the open segment.
    pub(crate) segs: Vec<Seg>,
    /// Files of this boot that are not read: past retention at open, or without a block.
    /// Retention deletes them.
    pub(crate) expired: Vec<(PathBuf, u64)>,
    /// Segments of other boots inside retention.
    pub(crate) others: Vec<Seg>,
    writer: Option<SegmentWriter>,
    /// Time of the last sample in the newest block of this boot.
    last_end: Option<TimeNs>,
    deleted: DeletedKeys,
    /// The keys in this boot's deleted-keys file.
    own_deleted: Vec<ProcessKey>,
    realtime_offset_ns: i64,
    cache: Mutex<BlockCache>,
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> HistoryError + '_ {
    move |source| HistoryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// CLOCK_REALTIME now in nanoseconds since the epoch.
fn realtime_now_ns() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
}

/// Time of the last sample in `b`: the last sweep or the last system point.
fn block_end(b: &Block) -> TimeNs {
    let sweep = b.deltas.last().map_or(0, |d| d.dt_ms);
    let system = b
        .system
        .iter()
        .filter_map(|(_, points)| points.last().map(|p| p.0))
        .max()
        .unwrap_or(0);
    b.start_t_ns + u64::from(sweep.max(system)) * NS_PER_MS
}

/// A copy of `b` with every meta id moved up by `base`.
fn shift_meta(b: &Block, base: MetaId) -> Block {
    let mut b = b.clone();
    for (id, _) in &mut b.new_meta {
        *id = id.saturating_add(base);
    }
    for p in b
        .keyframe
        .iter_mut()
        .chain(b.deltas.iter_mut().flat_map(|d| d.started.iter_mut()))
    {
        p.meta = p.meta.saturating_add(base);
    }
    b
}

/// The boot id of a directory named with 16 lowercase hex digits.
fn parse_boot_dir(name: &str) -> Option<BootId> {
    let lower_hex = name.len() == 16
        && name
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    lower_hex
        .then(|| u64::from_str_radix(name, 16).ok().map(BootId))
        .flatten()
}

/// `seg-*.tlh` files in `dir`, sorted by name (that is, by start time).
fn segment_paths(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("seg-") && name.ends_with(".tlh") && entry.file_type()?.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// The keys in `dir/deleted-keys.bin`; empty when the file is missing or does not decode.
fn read_deleted_keys(dir: &Path) -> Vec<ProcessKey> {
    fs::read(dir.join(DELETED_KEYS_FILE))
        .ok()
        .and_then(|bytes| postcard::from_bytes(&bytes).ok())
        .unwrap_or_default()
}

/// Writes `dir/deleted-keys.bin` (mode 0600) through a temp file, fsync and rename.
fn write_deleted_keys(dir: &Path, keys: &[ProcessKey]) -> Result<(), HistoryError> {
    let bytes = postcard::to_stdvec(keys)?;
    let tmp = dir.join(format!("{DELETED_KEYS_FILE}.tmp"));
    let path = dir.join(DELETED_KEYS_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(io_err(&tmp))?;
    file.write_all(&bytes).map_err(io_err(&tmp))?;
    file.sync_all().map_err(io_err(&tmp))?;
    fs::rename(&tmp, &path).map_err(io_err(&path))?;
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(io_err(dir))
}

impl Store {
    fn empty(cfg: StoreConfig, realtime_offset_ns: i64) -> Store {
        Store {
            boot_dir: cfg.root.join(boot_dir_name(cfg.boot)),
            cfg,
            segs: Vec::new(),
            expired: Vec::new(),
            others: Vec::new(),
            writer: None,
            last_end: None,
            deleted: DeletedKeys::default(),
            own_deleted: Vec::new(),
            realtime_offset_ns,
            cache: Mutex::new(BlockCache::default()),
        }
    }

    /// Creates root/<boot> (0700) if needed, opens every segment of this boot and of other boots
    /// inside retention, stops when `deadline` passes.
    pub fn open(cfg: StoreConfig, now_t_ns: TimeNs, deadline: Instant) -> (Store, LoadReport) {
        let now_offset = realtime_now_ns().saturating_sub(now_t_ns as i64);
        let mut store = Store::empty(cfg, now_offset);
        let problem = store.load(now_t_ns, deadline);
        if problem == Some(LoadProblem::TimedOut) {
            store.segs.clear();
            store.expired.clear();
            store.others.clear();
            store.last_end = None;
        }
        let report = LoadReport {
            problem,
            segments: store.segs.len() + store.others.len(),
            span: store.span(),
        };
        (store, report)
    }

    fn load(&mut self, now_t_ns: TimeNs, deadline: Instant) -> Option<LoadProblem> {
        let created = DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.boot_dir);
        if created.is_err() {
            return Some(LoadProblem::OpenFailed);
        }
        if !self.cfg.load_existing {
            return None;
        }
        // This boot's deleted keys apply to new data too, so they load before any deadline.
        self.own_deleted = read_deleted_keys(&self.boot_dir);
        self.deleted.insert_all(&self.own_deleted);

        let Ok(entries) = fs::read_dir(&self.cfg.root) else {
            return Some(LoadProblem::OpenFailed);
        };
        let mut boots: Vec<(BootId, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let boot = parse_boot_dir(e.file_name().to_str()?)?;
                Some((boot, e.path()))
            })
            .collect();
        boots.sort();

        let retention = self.cfg.retention_ns;
        let now_rt = (now_t_ns as i64).saturating_add(self.realtime_offset_ns);
        let mut readable_files = 0usize;
        let mut any_block = false;
        let mut version = false;
        let mut newest_other_rt: Option<i64> = None;
        for (boot, dir) in boots {
            let current = boot == self.cfg.boot;
            let paths = match segment_paths(&dir) {
                Ok(p) => p,
                Err(_) if current => return Some(LoadProblem::OpenFailed),
                Err(_) => continue,
            };
            let mut boot_has_segments = false;
            for path in paths {
                if Instant::now() >= deadline {
                    return Some(LoadProblem::TimedOut);
                }
                let reader = match SegmentReader::open(&path) {
                    Ok(r) => r,
                    Err(HistoryError::Version(_)) => {
                        version = true;
                        continue;
                    }
                    Err(_) => {
                        readable_files += 1;
                        if current {
                            let size = fs::metadata(&path).map_or(0, |m| m.len());
                            self.expired.push((path, size));
                        }
                        continue;
                    }
                };
                readable_files += 1;
                let Some(last) = reader.index.last() else {
                    if current {
                        self.expired.push((path, reader.size));
                    }
                    continue;
                };
                any_block = true;
                let last_start = last.start_t_ns;
                let end = last_start.saturating_add(BLOCK_NS);
                if current {
                    if now_t_ns.saturating_sub(end) < retention {
                        self.segs.push(Seg { path, reader });
                    } else {
                        self.expired.push((path, reader.size));
                    }
                    continue;
                }
                let offset = reader.header.realtime_offset_ns;
                let end_rt = (end as i64).saturating_add(offset);
                if now_rt.saturating_sub(end_rt) < retention as i64 {
                    let start_rt = (last_start as i64).saturating_add(offset);
                    newest_other_rt = newest_other_rt.max(Some(start_rt));
                    self.others.push(Seg { path, reader });
                    boot_has_segments = true;
                }
            }
            if boot_has_segments {
                self.deleted.insert_all(&read_deleted_keys(&dir));
            }
        }
        self.segs
            .sort_by_key(|s| s.reader.header.start_t_ns);
        self.last_end = self.newest_block_end();

        if version {
            Some(LoadProblem::VersionMismatch)
        } else if readable_files > 0 && !any_block {
            Some(LoadProblem::EmptyOrTruncated)
        } else if any_block && self.segs.is_empty() && self.others.is_empty() {
            Some(LoadProblem::NoInRange)
        } else if self.segs.is_empty()
            && newest_other_rt.is_some_and(|t| {
                now_rt.saturating_sub(t) > retention.saturating_sub(STALE_MARGIN_NS) as i64
            })
        {
            Some(LoadProblem::Stale)
        } else {
            None
        }
    }

    /// End of the newest indexed block of this boot. Decodes that one block.
    fn newest_block_end(&self) -> Option<TimeNs> {
        let seg = self.segs.iter().rev().find(|s| !s.reader.index.is_empty())?;
        let entry = seg.reader.index.last()?;
        Some(
            self.cached_block(seg, entry)
                .map_or(entry.start_t_ns, |b| block_end(&b)),
        )
    }

    /// True when the next block starts a new segment: no segment is open, or `next_start` is at
    /// or past the open segment's start plus `SEGMENT_SECS`. The recorder then calls
    /// `reset_meta_scope`.
    pub fn will_rotate(&self, next_start: TimeNs) -> bool {
        match (&self.writer, self.segs.last()) {
            (Some(_), Some(seg)) => {
                next_start >= seg.reader.header.start_t_ns.saturating_add(SEGMENT_NS)
            }
            _ => true,
        }
    }

    /// Encodes and writes `b`, rotating to a new segment at `SEGMENT_SECS`. After a failed write
    /// the segment is left without a footer (readers recover its good blocks) and the next
    /// append starts a new segment.
    pub fn append(&mut self, b: Block, realtime_offset_ns: i64) -> Result<(), HistoryError> {
        self.realtime_offset_ns = realtime_offset_ns;
        if self.writer.is_some() && self.will_rotate(b.start_t_ns) {
            self.close_writer()?;
        }
        if self.writer.is_none() {
            let header = SegmentHeader {
                boot_id: self.cfg.boot,
                realtime_offset_ns,
                start_t_ns: b.start_t_ns,
            };
            let w = SegmentWriter::create(&self.boot_dir, header.clone())?;
            self.segs.push(Seg {
                path: self.boot_dir.join(segment_file_name(header.start_t_ns)),
                reader: SegmentReader {
                    header,
                    index: Vec::new(),
                    recovered: false,
                    size: w.bytes_written(),
                },
            });
            self.writer = Some(w);
        }
        let encoded = encode_block(&b)?;
        let (Some(w), Some(seg)) = (self.writer.as_mut(), self.segs.last_mut()) else {
            unreachable!("a writer and its segment were set above");
        };
        let offset = w.bytes_written();
        if let Err(e) = w.append(&encoded) {
            self.writer = None;
            seg.reader.recovered = true;
            return Err(e);
        }
        seg.reader.index.push(FooterEntry {
            start_t_ns: b.start_t_ns,
            offset,
        });
        seg.reader.size = w.bytes_written();
        let key = (seg.reader.header.start_t_ns, offset);
        self.last_end = Some(block_end(&b));
        self.lock_cache().put(key, Arc::new(b));
        Ok(())
    }

    /// Writes the open segment's footer. The next append starts a new segment.
    fn close_writer(&mut self) -> Result<(), HistoryError> {
        let Some(w) = self.writer.take() else {
            return Ok(());
        };
        let closed = w.close();
        if let Some(seg) = self.segs.last_mut() {
            match fs::metadata(&seg.path) {
                Ok(m) => seg.reader.size = m.len(),
                Err(e) => return closed.and(Err(io_err(&seg.path)(e))),
            }
            seg.reader.recovered = closed.is_err();
        }
        closed
    }

    /// Closes the open segment with its footer.
    pub fn close(&mut self) -> Result<(), HistoryError> {
        self.close_writer()
    }

    /// First block start and last sample time of this boot's history.
    pub fn span(&self) -> Option<(TimeNs, TimeNs)> {
        let first = self
            .segs
            .iter()
            .find_map(|s| s.reader.index.first())?
            .start_t_ns;
        Some((first, self.last_end?.max(first)))
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, BlockCache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cached_block(&self, seg: &Seg, entry: &FooterEntry) -> Result<Arc<Block>, HistoryError> {
        let key = (seg.reader.header.start_t_ns, entry.offset);
        if let Some(b) = self.lock_cache().get(key) {
            return Ok(b);
        }
        let b = Arc::new(seg.reader.read_block(&seg.path, entry)?);
        self.lock_cache().put(key, Arc::clone(&b));
        Ok(b)
    }

    /// Decoded blocks overlapping [from, to] in time order, plus the metas needed to resolve
    /// them. A block covers `[start, start + BLOCK_SECS)`.
    pub fn blocks(
        &self,
        from: TimeNs,
        to: TimeNs,
    ) -> Result<(Vec<Arc<Block>>, MetaTable), HistoryError> {
        let mut out = Vec::new();
        let mut metas = MetaTable::default();
        let mut base: MetaId = 0;
        for seg in &self.segs {
            let index = &seg.reader.index;
            let hit = |e: &FooterEntry| e.start_t_ns <= to && e.start_t_ns + BLOCK_NS > from;
            let Some(first) = index.iter().position(hit) else {
                continue;
            };
            let last = index.iter().rposition(hit).unwrap_or(first);
            // Block k needs the metas of blocks 0..=k of its segment.
            let mut next_id: MetaId = 0;
            for (k, entry) in index[..=last].iter().enumerate() {
                let b = self.cached_block(seg, entry)?;
                if let Some(max) = b.new_meta.iter().map(|(id, _)| *id).max() {
                    next_id = next_id.max(max.saturating_add(1));
                }
                let b = if base == 0 {
                    b
                } else {
                    Arc::new(shift_meta(&b, base))
                };
                metas.absorb(&b);
                if k >= first {
                    out.push(b);
                }
            }
            base = base.saturating_add(next_id);
        }
        Ok((out, metas))
    }

    /// Adds keys to the deleted list (R-103). Keys of this boot go into this boot's file; keys of
    /// another boot go into that boot's file when its directory exists. Segments are not
    /// rewritten. Every key is filtered from queries even when a file write fails.
    pub fn delete_keys(&mut self, keys: &[ProcessKey]) -> Result<(), HistoryError> {
        self.deleted.insert_all(keys);
        let mut boots: Vec<BootId> = keys.iter().map(|k| k.boot_id).collect();
        boots.sort();
        boots.dedup();
        let mut result = Ok(());
        for boot in boots {
            let of_boot = keys.iter().filter(|k| k.boot_id == boot);
            let written = if boot == self.cfg.boot {
                for k in of_boot {
                    if !self.own_deleted.contains(k) {
                        self.own_deleted.push(*k);
                    }
                }
                write_deleted_keys(&self.boot_dir, &self.own_deleted)
            } else {
                let dir = self.cfg.root.join(boot_dir_name(boot));
                if !dir.is_dir() {
                    continue;
                }
                let mut list = read_deleted_keys(&dir);
                for k in of_boot {
                    if !list.contains(k) {
                        list.push(*k);
                    }
                }
                write_deleted_keys(&dir, &list)
            };
            result = result.and(written);
        }
        result
    }

    pub fn deleted(&self) -> &DeletedKeys {
        &self.deleted
    }

    /// Bytes of every indexed segment file: this boot (including expired files) and other boots.
    pub fn disk_bytes(&self) -> u64 {
        let segs = self
            .segs
            .iter()
            .chain(&self.others)
            .map(|s| s.reader.size)
            .sum::<u64>();
        segs + self.expired.iter().map(|(_, size)| size).sum::<u64>()
    }

    /// CLOCK_REALTIME minus CLOCK_BOOTTIME: from the last append, or measured at open.
    pub fn realtime_offset_ns(&self) -> i64 {
        self.realtime_offset_ns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{HistMeta, HistProcess};
    use std::os::unix::fs::PermissionsExt;

    const MIN: u64 = 60 * NS_PER_SEC;
    const BOOT: BootId = BootId(0xabc);

    fn cfg(root: &Path) -> StoreConfig {
        StoreConfig {
            root: root.to_path_buf(),
            boot: BOOT,
            retention_ns: 6 * 3600 * NS_PER_SEC,
            max_disk_bytes: 256 << 20,
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

    fn meta(name: &str) -> HistMeta {
        HistMeta {
            comm: Arc::from(name),
            name: Arc::from(name),
            exe_basename: None,
            app_id: None,
            exe_path: None,
            user: Arc::from("u"),
            uid: 1000,
            system: false,
        }
    }

    /// Block at `minute`. The first block of each segment brings meta 0 named `seg<n>`.
    fn block(minute: u64) -> Block {
        let first_of_segment = minute.is_multiple_of(10);
        Block {
            start_t_ns: minute * MIN,
            gap_before: false,
            mem_total_kib: 1024,
            catalog: None,
            new_meta: if first_of_segment {
                vec![(0, meta(&format!("seg{}", minute / 10)))]
            } else {
                vec![]
            },
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

    fn fill(store: &mut Store, minutes: u64) {
        for m in 0..minutes {
            if store.will_rotate(m * MIN) {
                assert!(m.is_multiple_of(10), "rotation at minute {m}");
            }
            store.append(block(m), 1_000).unwrap();
        }
    }

    fn segment_files(dir: &Path) -> Vec<PathBuf> {
        segment_paths(dir).unwrap()
    }

    fn has_footer(path: &Path) -> bool {
        let bytes = fs::read(path).unwrap();
        bytes.ends_with(b"TLHF")
    }

    #[test]
    fn open_creates_boot_dir_0700() {
        let root = tempfile::tempdir().unwrap();
        let (_store, report) = Store::open(cfg(root.path()), 10 * MIN, later());
        let dir = root.path().join(boot_dir_name(BOOT));
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(report.problem, None);
        assert_eq!(report.span, None);
        assert_eq!(report.segments, 0);
    }

    #[test]
    fn rotation_every_10_minutes() {
        let root = tempfile::tempdir().unwrap();
        let (mut store, _) = Store::open(cfg(root.path()), 0, later());
        fill(&mut store, 25);
        let files = segment_files(&root.path().join(boot_dir_name(BOOT)));
        assert_eq!(files.len(), 3);
        assert!(has_footer(&files[0]));
        assert!(has_footer(&files[1]));
        assert!(!has_footer(&files[2]));
        assert!(files[1].ends_with("seg-0000000600.tlh"));
        assert_eq!(store.span(), Some((0, 24 * MIN)));
        store.close().unwrap();
        assert!(has_footer(&files[2]));
    }

    #[test]
    fn blocks_range_lookup() {
        let root = tempfile::tempdir().unwrap();
        {
            let (mut store, _) = Store::open(cfg(root.path()), 0, later());
            fill(&mut store, 25);
            store.close().unwrap();
        }
        let (store, report) = Store::open(cfg(root.path()), 25 * MIN, later());
        assert_eq!(report.problem, None);
        assert_eq!(report.segments, 3);
        assert_eq!(report.span, Some((0, 24 * MIN)));
        let (blocks, metas) = store.blocks(12 * MIN, 14 * MIN).unwrap();
        let starts: Vec<u64> = blocks.iter().map(|b| b.start_t_ns / MIN).collect();
        assert_eq!(starts, vec![12, 13, 14]);
        // Block 12 resolves meta 0 from the segment's first block, outside the range.
        assert_eq!(&*metas.get(0).unwrap().name, "seg1");
    }

    #[test]
    fn blocks_across_segments_remap_meta() {
        let root = tempfile::tempdir().unwrap();
        let (mut store, _) = Store::open(cfg(root.path()), 0, later());
        fill(&mut store, 25);
        let (blocks, metas) = store.blocks(9 * MIN, 10 * MIN).unwrap();
        assert_eq!(blocks.len(), 2);
        let name = |b: &Block| metas.get(b.keyframe[0].meta).unwrap().name.clone();
        assert_eq!(&*name(&blocks[0]), "seg0");
        assert_eq!(&*name(&blocks[1]), "seg1");
    }

    #[test]
    fn version_mismatch_skipped() {
        let root = tempfile::tempdir().unwrap();
        {
            let (mut store, _) = Store::open(cfg(root.path()), 0, later());
            fill(&mut store, 15);
            store.close().unwrap();
        }
        let files = segment_files(&root.path().join(boot_dir_name(BOOT)));
        let mut bytes = fs::read(&files[1]).unwrap();
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        fs::write(&files[1], &bytes).unwrap();

        let (store, report) = Store::open(cfg(root.path()), 15 * MIN, later());
        assert_eq!(report.problem, Some(LoadProblem::VersionMismatch));
        assert_eq!(report.segments, 1);
        let (blocks, _) = store.blocks(0, 20 * MIN).unwrap();
        assert_eq!(blocks.len(), 10);
    }

    #[test]
    fn open_failed_on_file_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("history");
        fs::write(&root, b"not a directory").unwrap();
        let (_store, report) = Store::open(cfg(&root), 0, later());
        assert_eq!(report.problem, Some(LoadProblem::OpenFailed));
        assert_eq!(report.span, None);
    }

    #[test]
    fn load_existing_false_loads_nothing() {
        let root = tempfile::tempdir().unwrap();
        {
            let (mut store, _) = Store::open(cfg(root.path()), 0, later());
            fill(&mut store, 15);
            store.close().unwrap();
        }
        let mut c = cfg(root.path());
        c.load_existing = false;
        let (store, report) = Store::open(c, 15 * MIN, later());
        assert_eq!(report.problem, None);
        assert_eq!(report.span, None);
        assert_eq!(store.span(), None);
        let files = segment_files(&root.path().join(boot_dir_name(BOOT)));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn deadline_gives_timed_out() {
        let root = tempfile::tempdir().unwrap();
        {
            let (mut store, _) = Store::open(cfg(root.path()), 0, later());
            fill(&mut store, 15);
            store.close().unwrap();
        }
        let (mut store, report) = Store::open(cfg(root.path()), 15 * MIN, Instant::now());
        assert_eq!(report.problem, Some(LoadProblem::TimedOut));
        assert_eq!(report.span, None);
        assert_eq!(report.segments, 0);
        // It still writes.
        store.append(block(20), 1_000).unwrap();
        assert_eq!(store.span(), Some((20 * MIN, 20 * MIN)));
    }

    #[test]
    fn empty_segment_is_empty_or_truncated() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(boot_dir_name(BOOT));
        fs::create_dir_all(&dir).unwrap();
        let header = SegmentHeader {
            boot_id: BOOT,
            realtime_offset_ns: 0,
            start_t_ns: 0,
        };
        SegmentWriter::create(&dir, header).unwrap();
        let (_store, report) = Store::open(cfg(root.path()), MIN, later());
        assert_eq!(report.problem, Some(LoadProblem::EmptyOrTruncated));
    }

    #[test]
    fn old_segments_are_no_in_range() {
        let root = tempfile::tempdir().unwrap();
        let mut c = cfg(root.path());
        c.retention_ns = 3600 * NS_PER_SEC;
        {
            let (mut store, _) = Store::open(c.clone(), 0, later());
            fill(&mut store, 15);
            store.close().unwrap();
        }
        let (store, report) = Store::open(c, 3 * 3600 * NS_PER_SEC, later());
        assert_eq!(report.problem, Some(LoadProblem::NoInRange));
        assert_eq!(report.span, None);
        assert_eq!(store.expired.len(), 2);
        assert!(store.disk_bytes() > 0);
    }

    #[test]
    fn deleted_keys_persist_without_rewrite() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(boot_dir_name(BOOT));
        {
            let (mut store, _) = Store::open(cfg(root.path()), 0, later());
            fill(&mut store, 15);
            store.close().unwrap();
        }
        let before: Vec<Vec<u8>> = segment_files(&dir)
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect();
        let k = key(1);
        let other = ProcessKey {
            boot_id: BootId(0x99),
            ..key(2)
        };
        let other_dir = root.path().join(boot_dir_name(BootId(0x99)));
        fs::create_dir_all(&other_dir).unwrap();
        {
            let (mut store, _) = Store::open(cfg(root.path()), 15 * MIN, later());
            store.delete_keys(&[k, other]).unwrap();
            assert!(store.deleted().contains(&k));
            store.close().unwrap();
        }
        let mode = fs::metadata(dir.join(DELETED_KEYS_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(read_deleted_keys(&dir), vec![k]);
        assert_eq!(read_deleted_keys(&other_dir), vec![other]);

        let (store, _) = Store::open(cfg(root.path()), 15 * MIN, later());
        assert!(store.deleted().contains(&k));
        let after: Vec<Vec<u8>> = segment_files(&dir)
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn block_cache_bounded_16() {
        let root = tempfile::tempdir().unwrap();
        {
            let (mut store, _) = Store::open(cfg(root.path()), 0, later());
            fill(&mut store, 25);
            store.close().unwrap();
        }
        let (store, _) = Store::open(cfg(root.path()), 25 * MIN, later());
        let (blocks, _) = store.blocks(0, 25 * MIN).unwrap();
        assert_eq!(blocks.len(), 25);
        assert_eq!(store.lock_cache().entries.len(), BLOCK_CACHE_LEN);
    }
}
