//! Parser for /proc/<pid>/smaps_rollup and smaps.
//!
//! PSS and USS come from `smaps_rollup`, which costs about 1 ms for a large process, so they are
//! read only for the keys the engine asks for (`Demand.detail_keys`, at most 512). The fds stay
//! in the 128-entry `FdCache` (ADR-016). The engine applies the 1 Hz cap and the "only while a
//! PSS or USS column is shown" rule (ADR-008); this reader does what it is asked.

use std::sync::Arc;

use memchr::memchr;
use rustc_hash::FxHashMap;
use rustix::buffer::spare_capacity;
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use tl_model::ids::ProcessKey;
use tl_model::process::{Coverage, ProcessRecord};
use tl_model::validity::{ProcessField, Validity};

use crate::fdcache::FdCache;
use crate::proc::pid_stat::read_start_ticks;
use crate::proc::{is_denied, is_gone};
use crate::{CollectError, SysRoot};

const FILE: &str = "smaps_rollup";

/// Bytes read from one `smaps_rollup`. The real file is about 1 KiB.
pub const ROLLUP_READ_MAX: usize = 16 * 1024;

const PARSE_ERROR: CollectError = CollectError::ProcParse { pid: 0, file: FILE };

/// PSS, USS and swap of one process, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rollup {
    /// `Pss:`.
    pub pss: u64,
    /// `Private_Clean:` + `Private_Dirty:`.
    pub uss: u64,
    /// `Swap:`.
    pub swap: u64,
}

/// Parses `smaps_rollup`. Values are kB × 1024. A buffer without `Pss:` is an error.
pub fn parse_rollup(buf: &[u8], out: &mut Rollup) -> Result<(), CollectError> {
    *out = Rollup::default();
    let mut have_pss = false;
    let mut rest = buf;
    while !rest.is_empty() {
        let (line, tail) = match memchr(b'\n', rest) {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = tail;
        let Some(colon) = memchr(b':', line) else {
            continue;
        };
        let name = &line[..colon];
        let slot = match name {
            b"Pss" => {
                have_pss = true;
                &mut out.pss
            }
            b"Private_Clean" | b"Private_Dirty" => &mut out.uss,
            b"Swap" => &mut out.swap,
            _ => continue,
        };
        let bytes = kb_value(&line[colon + 1..]).ok_or(PARSE_ERROR)?;
        *slot = slot.saturating_add(bytes);
    }
    if have_pss { Ok(()) } else { Err(PARSE_ERROR) }
}

/// `"   2048 kB"` to 2048 × 1024 bytes.
fn kb_value(field: &[u8]) -> Option<u64> {
    let field = field.trim_ascii();
    let digits = field.strip_suffix(b"kB").unwrap_or(field).trim_ascii_end();
    if digits.is_empty() {
        return None;
    }
    let kb = digits.iter().try_fold(0u64, |n, &d| {
        if d.is_ascii_digit() {
            n.checked_mul(10)?.checked_add(u64::from(d - b'0'))
        } else {
            None
        }
    })?;
    Some(kb.saturating_mul(1024))
}

/// The result of reading one key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RollupRead {
    Ok(Rollup),
    /// EACCES or EPERM: another user's process. The helper path is E29's.
    Restricted,
    /// The process exited, or the pid now belongs to another process.
    Gone,
    /// The file did not parse, or another errno. Validity `Failed` for that key.
    Failed,
}

/// Reads `smaps_rollup` for the demanded keys and keeps their fds open between calls.
#[derive(Default)]
pub struct PssReader {
    cache: FdCache,
    opens: u64,
}

impl PssReader {
    pub fn new() -> Self {
        Self {
            cache: FdCache::new(),
            opens: 0,
        }
    }

    /// Number of cached fds.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// Number of `smaps_rollup` opens so far. For tests: a cached fd is not opened again.
    pub fn opens(&self) -> u64 {
        self.opens
    }

    /// Reads every key in `keys` and appends one result per key to `out` (cleared first).
    ///
    /// Cache entries for keys not in `keys` are closed first, so their slots are free. When the
    /// cache is full, a new key is read through a fresh fd that is closed right after; the fds
    /// already cached stay (no eviction).
    pub fn read(
        &mut self,
        root: &SysRoot,
        keys: &[ProcessKey],
        buf: &mut Vec<u8>,
        out: &mut Vec<(ProcessKey, RollupRead)>,
    ) {
        out.clear();
        self.cache.retain_demanded(keys);
        for &key in keys {
            let result = self.read_one(root, key, buf);
            if !matches!(result, RollupRead::Ok(_)) {
                self.cache.remove(&key);
            }
            out.push((key, result));
        }
    }

    fn read_one(&mut self, root: &SysRoot, key: ProcessKey, buf: &mut Vec<u8>) -> RollupRead {
        if let Some(fd) = self.cache.get(&key) {
            return read_rollup(fd, buf);
        }
        let fd = match open_checked(root, key, buf) {
            Ok(fd) => fd,
            Err(result) => return result,
        };
        self.opens += 1;
        let result = read_rollup(fd.as_fd(), buf);
        if matches!(result, RollupRead::Ok(_)) {
            // A full cache hands the fd back; dropping it closes it.
            let _ = self.cache.insert(key, fd);
        }
        result
    }
}

/// Opens `/proc/<pid>`, checks that its start time is `key.start_ticks`, then opens
/// `smaps_rollup` relative to that directory fd. The directory fd pins the process, so a pid
/// reused after the check gives ESRCH or ENOENT, never another process's file.
fn open_checked(root: &SysRoot, key: ProcessKey, buf: &mut Vec<u8>) -> Result<OwnedFd, RollupRead> {
    let dir = rustix::fs::open(
        root.proc.join(key.pid.to_string()),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_result)?;
    match read_start_ticks(dir.as_fd(), buf) {
        Ok(ticks) if ticks == key.start_ticks => {}
        Ok(_) => return Err(RollupRead::Gone),
        Err(CollectError::PermissionDenied) => return Err(RollupRead::Restricted),
        Err(CollectError::NotFound) => return Err(RollupRead::Gone),
        Err(_) => return Err(RollupRead::Failed),
    }
    rustix::fs::openat(
        dir.as_fd(),
        FILE,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_result)
}

/// `pread` from offset 0 until EOF or `ROLLUP_READ_MAX`, then parse.
fn read_rollup(fd: BorrowedFd<'_>, buf: &mut Vec<u8>) -> RollupRead {
    buf.clear();
    while buf.len() < ROLLUP_READ_MAX {
        if buf.len() == buf.capacity() {
            buf.reserve(4096.max(buf.len()).min(ROLLUP_READ_MAX - buf.len()));
        }
        let offset = buf.len() as u64;
        match rustix::io::pread(fd, spare_capacity(buf), offset) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return errno_result(e),
        }
    }
    if buf.is_empty() {
        return RollupRead::Gone;
    }
    let mut rollup = Rollup::default();
    match parse_rollup(buf, &mut rollup) {
        Ok(()) => RollupRead::Ok(rollup),
        Err(_) => RollupRead::Failed,
    }
}

fn errno_result(e: Errno) -> RollupRead {
    if is_denied(e) {
        RollupRead::Restricted
    } else if is_gone(e) {
        RollupRead::Gone
    } else {
        RollupRead::Failed
    }
}

/// Collects the keys that got PSS and USS this sweep and builds `Coverage` from them.
#[derive(Debug, Default)]
pub struct CoverageBuilder {
    keys: Vec<ProcessKey>,
}

impl CoverageBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, key: ProcessKey) {
        self.keys.push(key);
    }

    /// The covered keys sorted ascending and without duplicates. Empties the builder.
    pub fn build(&mut self) -> Coverage {
        self.keys.sort_unstable();
        self.keys.dedup();
        let pss_uss: Arc<[ProcessKey]> = Arc::from(self.keys.as_slice());
        self.keys.clear();
        Coverage { pss_uss }
    }
}

/// Copies the results into the matching records. `Ok` sets `pss_bytes`, `uss_bytes` and validity
/// `Valid` and adds the key to `coverage`. `Restricted` and `Failed` set that validity. `Gone`
/// and records without a result keep their current values (`WarmingUp` when never covered).
pub fn apply_rollups(
    records: &mut [ProcessRecord],
    results: &[(ProcessKey, RollupRead)],
    coverage: &mut CoverageBuilder,
) {
    if results.is_empty() {
        return;
    }
    let by_key: FxHashMap<ProcessKey, RollupRead> = results.iter().copied().collect();
    for record in records.iter_mut() {
        let Some(result) = by_key.get(&record.key) else {
            continue;
        };
        let validity = match *result {
            RollupRead::Ok(rollup) => {
                record.pss_bytes = rollup.pss;
                record.uss_bytes = rollup.uss;
                coverage.push(record.key);
                Validity::Valid
            }
            RollupRead::Restricted => Validity::Restricted,
            RollupRead::Failed => Validity::Failed,
            RollupRead::Gone => continue,
        };
        record.validity.set(ProcessField::Pss, validity);
        record.validity.set(ProcessField::Uss, validity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kb_values() {
        assert_eq!(kb_value(b"   2048 kB"), Some(2_097_152));
        assert_eq!(kb_value(b" 0 kB"), Some(0));
        assert_eq!(kb_value(b" 7"), Some(7 * 1024));
        assert_eq!(kb_value(b" kB"), None);
        assert_eq!(kb_value(b" 1x kB"), None);
        assert_eq!(kb_value(b" 99999999999999999999 kB"), None);
    }

    #[test]
    fn similar_names_ignored() {
        let buf = b"Pss_Anon: 5 kB\nPss: 1 kB\nSwapPss: 9 kB\nPss_Dirty: 3 kB\n";
        let mut out = Rollup::default();
        parse_rollup(buf, &mut out).unwrap();
        assert_eq!(out.pss, 1024);
        assert_eq!(out.swap, 0);
    }

    #[test]
    fn missing_pss_is_error() {
        let mut out = Rollup::default();
        let err = parse_rollup(b"Rss: 4 kB\n", &mut out).unwrap_err();
        assert!(matches!(
            err,
            CollectError::ProcParse {
                pid: 0,
                file: "smaps_rollup"
            }
        ));
    }
}
