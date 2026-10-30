//! Per-process metadata that changes rarely: `comm`, `exe`, `cmdline` and `cgroup`, cached per
//! `ProcessKey` and re-read every [`META_REFRESH_SWEEPS`] sweeps or at once when `comm` changes
//! (R-064, ADR-016). Also the interner and the user-name lookup.
//!
//! Every string passes through `sanitize` before it is kept. Command lines, executable paths and
//! user names are never logged (ADR-015).

use std::cell::Cell;
use std::collections::hash_map::Entry;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::{AsFd, BorrowedFd};
use smallvec::SmallVec;
use tl_model::ids::ProcessKey;
use tl_model::process::WineProcess;

use crate::CollectError;
use crate::apps::naming::{exe_basename, is_wine, wine_exe_path};
use crate::proc::cgroup::{CgroupPath, parse_cgroup_file};
use crate::proc::detail::ENVIRON_MAX;
use crate::proc::is_denied;
use crate::sanitize::{
    CMDLINE_MAX, COMM_MAX, EXE_LINK_MAX, sanitize_cmdline, sanitize_comm, sanitize_link,
    sanitize_text,
};
use crate::sysroot::{SMALL_FILE_LIMIT, read_at, read_file};

/// A cached entry is re-read after this many sweeps.
pub const META_REFRESH_SWEEPS: u32 = 10;

/// Number of argv tokens kept in [`RawMeta::argv`].
const ARGV_KEPT: usize = 4;
/// Read limit for one argv token, in bytes.
const ARGV_TOKEN_MAX: usize = 4096;
/// Entries an [`Interner`] holds before it is cleared.
const INTERNER_CAP: usize = 16_384;
/// Read limit for `/etc/passwd`.
const PASSWD_MAX: usize = 1 << 20;
/// Read limit for one user name in `/etc/passwd`.
const USER_NAME_MAX: usize = 256;
/// `/etc/passwd` is checked for a new mtime at most this often.
const PASSWD_RECHECK: Duration = Duration::from_secs(10);
const DELETED_SUFFIX: &str = " (deleted)";

thread_local! {
    static META_FILE_READS: Cell<u32> = const { Cell::new(0) };
}

/// Test hook: how many per-process files `MetaCache` read on this thread.
#[doc(hidden)]
pub fn meta_file_reads() -> u32 {
    META_FILE_READS.with(Cell::get)
}

fn count_read() {
    META_FILE_READS.with(|c| c.set(c.get() + 1));
}

/// The target of `/proc/<pid>/exe`.
#[derive(Clone, PartialEq, Eq)]
pub enum ExeLink {
    /// The sanitized link text. A ` (deleted)` suffix stays in `path` and sets `deleted`.
    Path { path: Arc<str>, deleted: bool },
    /// EACCES or EPERM: another user's process.
    Restricted,
    /// No link: kernel threads, zombies, or any other error.
    None,
}

impl fmt::Debug for ExeLink {
    // Prints only the length of the path (ADR-015).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path { path, deleted } => f
                .debug_struct("Path")
                .field("path_len", &path.len())
                .field("deleted", deleted)
                .finish(),
            Self::Restricted => f.write_str("Restricted"),
            Self::None => f.write_str("None"),
        }
    }
}

/// The rarely changing metadata of one process, sanitized.
#[derive(Clone)]
pub struct RawMeta {
    /// The `comm` file, or the stat `comm` when that read fails.
    pub comm: Arc<str>,
    pub exe: ExeLink,
    /// The whole command line, NULs as spaces, cut at 32 KiB with `…`. `None` when empty or
    /// unreadable.
    pub cmdline: Option<Arc<str>>,
    /// The first 4 NUL-separated tokens of the command line.
    pub argv: SmallVec<[Arc<str>; 4]>,
    pub cgroup: Option<CgroupPath>,
    /// The `cgroup` file gave EACCES or EPERM.
    pub cgroup_denied: bool,
    /// The Windows image and Wine prefix of a Wine process; `None` for every other process.
    pub wine: Option<Arc<WineProcess>>,
}

impl RawMeta {
    /// The value after `prefix` (for example `--app-id=`) in the first argv token that starts
    /// with it, else in the first space-separated token of `cmdline` that does.
    pub fn argv_has_prefix(&self, prefix: &str) -> Option<Arc<str>> {
        if let Some(v) = self.argv.iter().find_map(|t| t.strip_prefix(prefix)) {
            return Some(Arc::from(v));
        }
        self.cmdline
            .as_deref()?
            .split(' ')
            .find_map(|t| t.strip_prefix(prefix))
            .map(Arc::from)
    }
}

impl fmt::Debug for RawMeta {
    // Prints `comm` and lengths only: command lines and paths are never logged (ADR-015).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMeta")
            .field("comm", &self.comm)
            .field("exe", &self.exe)
            .field("cmdline_len", &self.cmdline.as_ref().map(|c| c.len()))
            .field("argv_len", &self.argv.len())
            .field("cgroup", &self.cgroup.is_some())
            .field("cgroup_denied", &self.cgroup_denied)
            .field("wine", &self.wine.is_some())
            .finish()
    }
}

/// What `MetaCache::get_or_refresh` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaChange {
    /// First sight of the key: everything was read.
    New,
    /// The counter reached `META_REFRESH_SWEEPS` or `comm` changed: everything was re-read.
    Refreshed,
    /// Nothing was read.
    Unchanged,
}

/// Buffers reused across processes.
#[derive(Debug, Default)]
pub struct ReadBufs {
    pub small: Vec<u8>,
    pub cmdline: Vec<u8>,
    pub text: String,
}

struct CacheEntry {
    meta: RawMeta,
    /// The stat `comm_raw` at the last read.
    stat_comm: SmallVec<[u8; 16]>,
    sweeps: u32,
}

/// `RawMeta` per live process.
#[derive(Default)]
pub struct MetaCache {
    entries: FxHashMap<ProcessKey, CacheEntry>,
    interner: Interner,
}

impl MetaCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached metadata of `key`, read from `dir` (its `/proc/<pid>` directory) on
    /// first sight, then after `META_REFRESH_SWEEPS - pid % META_REFRESH_SWEEPS` calls and every
    /// `META_REFRESH_SWEEPS` calls after that, and when `stat_comm_raw` differs from the stat
    /// `comm` of the last read. No entry is older than `META_REFRESH_SWEEPS` calls.
    pub fn get_or_refresh(
        &mut self,
        key: ProcessKey,
        dir: BorrowedFd<'_>,
        stat_comm_raw: &[u8],
        bufs: &mut ReadBufs,
    ) -> (&RawMeta, MetaChange) {
        match self.get_or_refresh_with(key, stat_comm_raw, bufs, || Some(dir)) {
            Some(found) => found,
            None => unreachable!("open always gives the directory"),
        }
    }

    /// `get_or_refresh` that opens the `/proc/<pid>` directory with `open` only when the entry
    /// must be read (R-286). `None` when `open` gives no directory: the process is gone. An
    /// entry due for a refresh is then left as it was.
    pub fn get_or_refresh_with<D: AsFd>(
        &mut self,
        key: ProcessKey,
        stat_comm_raw: &[u8],
        bufs: &mut ReadBufs,
        open: impl FnOnce() -> Option<D>,
    ) -> Option<(&RawMeta, MetaChange)> {
        self.get_or_refresh_every(key, stat_comm_raw, bufs, META_REFRESH_SWEEPS, open)
    }

    /// `get_or_refresh_with` with an entry re-read at least every `every` calls in place of
    /// `META_REFRESH_SWEEPS`. The memory-only sweep uses its own period (tasklens-8fx.45).
    pub fn get_or_refresh_every<D: AsFd>(
        &mut self,
        key: ProcessKey,
        stat_comm_raw: &[u8],
        bufs: &mut ReadBufs,
        every: u32,
        open: impl FnOnce() -> Option<D>,
    ) -> Option<(&RawMeta, MetaChange)> {
        let every = every.max(1);
        match self.entries.entry(key) {
            Entry::Vacant(v) => {
                let dir = open()?;
                let meta = read_meta(dir.as_fd(), stat_comm_raw, bufs, &mut self.interner);
                // Staggered by pid, so a population first seen in one sweep does not refresh
                // in one sweep: the first refresh comes after every - (pid % every) calls.
                let e = v.insert(CacheEntry {
                    meta,
                    stat_comm: SmallVec::from_slice(stat_comm_raw),
                    sweeps: key.pid % every,
                });
                Some((&e.meta, MetaChange::New))
            }
            Entry::Occupied(o) => {
                let e = o.into_mut();
                if e.sweeps + 1 < every && e.stat_comm.as_slice() == stat_comm_raw {
                    e.sweeps += 1;
                    return Some((&e.meta, MetaChange::Unchanged));
                }
                let dir = open()?;
                e.meta = read_meta(dir.as_fd(), stat_comm_raw, bufs, &mut self.interner);
                e.stat_comm.clear();
                e.stat_comm.extend_from_slice(stat_comm_raw);
                e.sweeps = 0;
                Some((&e.meta, MetaChange::Refreshed))
            }
        }
    }

    /// Drops the entries of keys not in `live`.
    pub fn retain_live(&mut self, live: &FxHashSet<ProcessKey>) {
        self.entries.retain(|k, _| live.contains(k));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn read_meta(
    dir: BorrowedFd<'_>,
    stat_comm_raw: &[u8],
    bufs: &mut ReadBufs,
    interner: &mut Interner,
) -> RawMeta {
    count_read();
    match read_at(dir, "comm", &mut bufs.small, COMM_MAX + 1) {
        Ok(_) => sanitize_comm(&bufs.small, &mut bufs.text),
        Err(_) => sanitize_comm(stat_comm_raw, &mut bufs.text),
    }
    let comm = Arc::from(bufs.text.as_str());

    count_read();
    let exe = read_exe(dir, bufs);

    count_read();
    let (cmdline, argv) = match read_at(dir, "cmdline", &mut bufs.cmdline, CMDLINE_MAX + 1) {
        Ok(_) => {
            let mut argv = SmallVec::new();
            let raw = bufs.cmdline.strip_suffix(b"\0").unwrap_or(&bufs.cmdline);
            if !raw.is_empty() {
                for tok in raw.split(|&b| b == 0).take(ARGV_KEPT) {
                    sanitize_text(tok, ARGV_TOKEN_MAX, &mut bufs.text);
                    argv.push(Arc::from(bufs.text.as_str()));
                }
            }
            sanitize_cmdline(&bufs.cmdline, &mut bufs.text);
            let cmdline = (!bufs.text.is_empty()).then(|| Arc::from(bufs.text.as_str()));
            (cmdline, argv)
        }
        Err(_) => (None, SmallVec::new()),
    };

    count_read();
    let (cgroup, cgroup_denied) = match read_at(dir, "cgroup", &mut bufs.small, SMALL_FILE_LIMIT) {
        Ok((_, false)) => (
            parse_cgroup_file(&bufs.small).map(|p| intern_cgroup(p, interner)),
            false,
        ),
        Ok((_, true)) => (None, false),
        Err(e) => (None, matches!(e, CollectError::PermissionDenied)),
    };

    let exe_base = match &exe {
        ExeLink::Path { path, .. } => Some(exe_basename(path)),
        ExeLink::Restricted | ExeLink::None => None,
    };
    let wine = is_wine(exe_base, &comm).then(|| {
        Arc::new(WineProcess {
            prefix: read_wine_prefix(dir, bufs),
            image: wine_exe_path(&argv).map(Arc::from),
        })
    });

    RawMeta {
        comm,
        exe,
        cmdline,
        argv,
        cgroup,
        cgroup_denied,
        wine,
    }
}

/// The Wine prefix of a Wine process from its `environ`: `WINEPREFIX`, else `$HOME/.wine`, as
/// Wine picks it. Trailing `/` are removed. Only the two values are kept (ADR-015). `None` when
/// the file cannot be read (another user's process) or has neither variable.
fn read_wine_prefix(dir: BorrowedFd<'_>, bufs: &mut ReadBufs) -> Option<Arc<str>> {
    count_read();
    read_at(dir, "environ", &mut bufs.cmdline, ENVIRON_MAX).ok()?;
    let mut home = None;
    let mut prefix = None;
    for var in bufs.cmdline.split(|&b| b == 0) {
        if let Some(v) = var.strip_prefix(b"WINEPREFIX=") {
            prefix = Some(v);
        } else if let Some(v) = var.strip_prefix(b"HOME=") {
            home = Some(v);
        }
    }
    let (value, suffix) = match (prefix.filter(|v| !v.is_empty()), home) {
        (Some(prefix), _) => (prefix, ""),
        (None, Some(home)) if !home.is_empty() => (home, "/.wine"),
        _ => return None,
    };
    sanitize_text(value, EXE_LINK_MAX, &mut bufs.text);
    let trimmed = bufs.text.trim_end_matches('/').len();
    bufs.text.truncate(trimmed);
    bufs.text.push_str(suffix);
    (!bufs.text.is_empty()).then(|| Arc::from(bufs.text.as_str()))
}

fn read_exe(dir: BorrowedFd<'_>, bufs: &mut ReadBufs) -> ExeLink {
    // One byte past the limit, so `sanitize_link` sees the cut and adds `…`.
    bufs.small.clear();
    bufs.small.resize(EXE_LINK_MAX + 1, 0);
    match rustix::fs::readlinkat_raw(dir, "exe", &mut bufs.small[..]) {
        Ok(n) => {
            bufs.small.truncate(n);
            sanitize_link(&bufs.small, &mut bufs.text);
            ExeLink::Path {
                deleted: bufs.text.ends_with(DELETED_SUFFIX),
                path: Arc::from(bufs.text.as_str()),
            }
        }
        Err(e) if is_denied(e) => ExeLink::Restricted,
        Err(_) => ExeLink::None,
    }
}

fn intern_cgroup(p: CgroupPath, interner: &mut Interner) -> CgroupPath {
    CgroupPath {
        rel: interner.intern(&p.rel),
        unit: p.unit.map(|u| interner.intern(&u)),
        slices: p.slices.iter().map(|s| interner.intern(s)).collect(),
    }
}

/// Gives equal strings one shared `Arc`. Holds at most 16,384 entries and is cleared when full.
#[derive(Debug, Default)]
pub struct Interner {
    map: FxHashMap<Box<str>, Arc<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.map.get(s) {
            return a.clone();
        }
        if self.map.len() >= INTERNER_CAP {
            self.map.clear();
        }
        let a: Arc<str> = Arc::from(s);
        self.map.insert(Box::from(s), a.clone());
        a
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Maps a uid to the user name shown in the table.
pub trait UserNames {
    fn name(&mut self, uid: u32) -> Arc<str>;
}

/// The uid as decimal text, for example `"1000"`, cached per uid.
#[derive(Debug, Default)]
pub struct NumericUserNames {
    cache: FxHashMap<u32, Arc<str>>,
}

impl NumericUserNames {
    pub fn new() -> Self {
        Self::default()
    }
}

impl UserNames for NumericUserNames {
    fn name(&mut self, uid: u32) -> Arc<str> {
        self.cache
            .entry(uid)
            .or_insert_with(|| Arc::from(uid.to_string()))
            .clone()
    }
}

/// User names from `/etc/passwd`. A uid that is not in the file gets its numeric text, so users
/// from NSS sources (LDAP, systemd-homed) show their uid. No `libc`, no `getpwuid_r`.
#[derive(Debug, Default)]
pub struct PasswdUserNames {
    path: Option<PathBuf>,
    names: FxHashMap<u32, Arc<str>>,
    numeric: NumericUserNames,
    /// (seconds, nanoseconds) of the file at the last load.
    mtime: Option<(i64, i64)>,
    last_check: Option<Instant>,
}

impl PasswdUserNames {
    /// Reads at most 1 MiB of `path`. A missing or unreadable file gives the numeric text for
    /// every uid until a later check finds it.
    pub fn load(path: &Path) -> Self {
        let mut out = Self {
            path: Some(path.to_path_buf()),
            ..Self::default()
        };
        out.reload();
        out
    }

    /// Parses `bytes` as a passwd file, for tests. Never re-reads anything.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut out = Self::default();
        out.parse(bytes);
        out
    }

    fn reload(&mut self) {
        let Some(path) = &self.path else { return };
        self.last_check = Some(Instant::now());
        self.mtime = file_mtime(path);
        let mut buf = Vec::new();
        match read_file(path, &mut buf, PASSWD_MAX) {
            Ok(_) => self.parse(&buf),
            Err(_) => self.names.clear(),
        }
    }

    fn parse(&mut self, bytes: &[u8]) {
        self.names.clear();
        let text = String::from_utf8_lossy(bytes);
        let mut interner = Interner::new();
        let mut clean = String::new();
        for line in text.lines() {
            let mut fields = line.split(':');
            let (Some(name), Some(_), Some(uid)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Ok(uid) = uid.parse::<u32>() else {
                continue;
            };
            sanitize_text(name.as_bytes(), USER_NAME_MAX, &mut clean);
            if clean.is_empty() {
                continue;
            }
            // The first line for a uid wins, as with getpwuid.
            self.names
                .entry(uid)
                .or_insert_with(|| interner.intern(&clean));
        }
    }

    /// Re-reads the file when its mtime changed, at most once per 10 s.
    fn recheck(&mut self) {
        let Some(path) = &self.path else { return };
        if self
            .last_check
            .is_some_and(|t| t.elapsed() < PASSWD_RECHECK)
        {
            return;
        }
        if file_mtime(path) == self.mtime {
            self.last_check = Some(Instant::now());
            return;
        }
        self.reload();
    }
}

impl UserNames for PasswdUserNames {
    fn name(&mut self, uid: u32) -> Arc<str> {
        if let Some(n) = self.names.get(&uid) {
            return n.clone();
        }
        self.recheck();
        match self.names.get(&uid) {
            Some(n) => n.clone(),
            None => self.numeric.name(uid),
        }
    }
}

fn file_mtime(path: &Path) -> Option<(i64, i64)> {
    let st = rustix::fs::stat(path).ok()?;
    Some((st.st_mtime as i64, st.st_mtime_nsec as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the metadata of a fake `/proc/<pid>` directory.
    fn meta_of(comm: &str, exe: &str, cmdline: &[u8], environ: &[u8]) -> RawMeta {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("comm"), format!("{comm}\n")).unwrap();
        std::os::unix::fs::symlink(exe, dir.path().join("exe")).unwrap();
        std::fs::write(dir.path().join("cmdline"), cmdline).unwrap();
        std::fs::write(dir.path().join("environ"), environ).unwrap();
        let fd = std::fs::File::open(dir.path()).unwrap();
        let key = ProcessKey {
            start_ticks: 1,
            boot_id: tl_model::time::BootId(1),
            pid: 7,
        };
        let mut cache = MetaCache::new();
        let (meta, _) =
            cache.get_or_refresh(key, fd.as_fd(), comm.as_bytes(), &mut ReadBufs::default());
        meta.clone()
    }

    #[test]
    fn wine_prefix_and_image_read() {
        let preloader = "/usr/lib/wine/x86_64-unix/wine64-preloader";
        let meta = meta_of(
            "bztransmit.exe",
            preloader,
            b"C:\\Program Files\\Backblaze\\bztransmit.exe\0-x\0",
            b"HOME=/home/u\0WINEPREFIX=/srv/backblaze/prefix/\0PATH=/bin\0",
        );
        let wine = meta.wine.expect("a Wine process");
        assert_eq!(wine.prefix.as_deref(), Some("/srv/backblaze/prefix"));
        assert_eq!(
            wine.image.as_deref(),
            Some("C:\\Program Files\\Backblaze\\bztransmit.exe")
        );

        // No WINEPREFIX: Wine uses ~/.wine.
        let meta = meta_of(
            "services.exe",
            preloader,
            b"C:\\windows\\system32\\services.exe\0",
            b"WINEPREFIX=\0HOME=/home/u/\0",
        );
        let wine = meta.wine.expect("a Wine process");
        assert_eq!(wine.prefix.as_deref(), Some("/home/u/.wine"));

        // Neither variable.
        let meta = meta_of("game.exe", preloader, b"game.exe\0", b"PATH=/bin\0");
        let wine = meta.wine.expect("a Wine process");
        assert_eq!(wine.prefix, None);
        assert_eq!(wine.image.as_deref(), Some("game.exe"));

        // Not Wine: the environment is not read.
        let meta = meta_of("bash", "/usr/bin/bash", b"bash\0", b"WINEPREFIX=/p\0");
        assert_eq!(meta.wine, None);
    }

    #[test]
    fn interner_shares_arcs() {
        let mut i = Interner::new();
        let a = i.intern("app.slice");
        let b = i.intern(&String::from("app.slice"));
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &i.intern("user.slice")));
        assert_eq!(i.len(), 2);
    }

    #[test]
    fn interner_clears_when_full() {
        let mut i = Interner::new();
        for n in 0..INTERNER_CAP {
            i.intern(&n.to_string());
        }
        assert_eq!(i.len(), INTERNER_CAP);
        i.intern("one more");
        assert_eq!(i.len(), 1);
    }

    #[test]
    fn passwd_user_names_lookup_and_fallback() {
        let mut names = PasswdUserNames::from_bytes(
            b"root:x:0:0::/root:/bin/bash\nalice:x:1000:1000::/home/alice:/bin/bash\nbroken line\n",
        );
        assert_eq!(&*names.name(1000), "alice");
        assert_eq!(&*names.name(0), "root");
        assert_eq!(&*names.name(4242), "4242");
        assert_eq!(names.names.len(), 2, "the broken line is skipped");
    }

    #[test]
    fn passwd_missing_file_is_numeric() {
        let mut names = PasswdUserNames::load(Path::new("/nonexistent/tasklens/passwd"));
        assert_eq!(&*names.name(0), "0");
        assert_eq!(&*names.name(1000), "1000");
    }

    #[test]
    fn passwd_name_is_sanitized() {
        let mut names = PasswdUserNames::from_bytes(b"ev\x1bil\xff:x:7:7::/:/bin/sh\n");
        assert_eq!(&*names.name(7), "ev\u{FFFD}il\u{FFFD}");
    }

    #[test]
    fn numeric_user_names_cached() {
        let mut n = NumericUserNames::new();
        let a = n.name(1000);
        assert_eq!(&*a, "1000");
        assert!(Arc::ptr_eq(&a, &n.name(1000)));
    }

    #[test]
    fn debug_hides_paths_and_cmdline() {
        let meta = RawMeta {
            comm: Arc::from("firefox"),
            exe: ExeLink::Path {
                path: Arc::from("/home/secret/bin/firefox"),
                deleted: false,
            },
            cmdline: Some(Arc::from("firefox --token=hunter2")),
            argv: SmallVec::from_iter([Arc::from("firefox"), Arc::from("--token=hunter2")]),
            cgroup: None,
            cgroup_denied: false,
            wine: None,
        };
        let text = format!("{meta:?}");
        assert!(text.contains("firefox"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn argv_prefix_found_in_argv_then_cmdline() {
        let meta = RawMeta {
            comm: Arc::from("chrome"),
            exe: ExeLink::None,
            cmdline: Some(Arc::from("chrome --a --b --c --profile-directory=Work")),
            argv: SmallVec::from_iter([
                Arc::from("chrome"),
                Arc::from("--app-id=abc"),
                Arc::from("--b"),
                Arc::from("--c"),
            ]),
            cgroup: None,
            cgroup_denied: false,
            wine: None,
        };
        assert_eq!(meta.argv_has_prefix("--app-id=").as_deref(), Some("abc"));
        assert_eq!(
            meta.argv_has_prefix("--profile-directory=").as_deref(),
            Some("Work")
        );
        assert_eq!(meta.argv_has_prefix("--class="), None);
    }
}
