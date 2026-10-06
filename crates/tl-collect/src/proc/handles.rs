//! Open handles of every process, like `lsof`: the Search File Handles walk (PROC-096) and the
//! pipe peers of the File descriptors tab (PROC-080).
//!
//! Every file is opened one path part at a time with `O_NOFOLLOW` and read up to a fixed limit.
//! Link targets (`fd/*`, `cwd`, `exe`, `root`) are read as text and never opened. A file that
//! cannot be read gives no hits; no error surfaces. Paths are never logged (ADR-015).

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags, RawDir};
use smallvec::SmallVec;
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

use crate::proc::detail::{
    FD_DIRENT_BUF, MAPS_READ_MAX, READ_FLAGS, bracket_inode, is_file_backed, open_beneath,
    parse_maps_line, parse_tid, read_bounded,
};
use crate::proc::meta::ReadBufs;
use crate::proc::pid_stat::{PidStat, parse_pid_stat};
use crate::proc::{is_denied, is_gone};
use crate::sanitize::{EXE_LINK_MAX, sanitize_comm, sanitize_link};
use crate::sysroot::{SMALL_FILE_LIMIT, SysRoot};

/// Most fds read per process. A process can hold about a million; the fds past this are not
/// searched, so one process cannot stall the walk between cancel checks.
const FD_WALK_MAX: usize = 65_536;
/// Read size for `maps`, which is read in chunks so a large file needs no large buffer.
const MAPS_CHUNK: usize = 64 * 1024;

/// A case-insensitive search pattern. With `*` or `?` it is a glob over the whole path (`*` any
/// run, `?` one character), else a substring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// The lowercased pattern text.
    text: Box<str>,
    /// The lowercased glob, when the text holds `*` or `?`.
    glob: Option<Box<[char]>>,
}

impl Pattern {
    pub fn new(text: &str) -> Pattern {
        let mut lower = String::new();
        lowercase(text, &mut lower);
        let glob = lower.contains(['*', '?']).then(|| lower.chars().collect());
        Pattern {
            text: lower.into_boxed_str(),
            glob,
        }
    }

    /// True when `path` matches. `lower` is a reused buffer for the lowercased path. An empty
    /// substring pattern matches every path.
    pub fn matches(&self, path: &str, lower: &mut String) -> bool {
        lowercase(path, lower);
        match &self.glob {
            Some(glob) => glob_match(glob, lower),
            None => lower.contains(&*self.text),
        }
    }
}

/// Writes the lowercase form of `text` into `out` (cleared first).
fn lowercase(text: &str, out: &mut String) {
    out.clear();
    for c in text.chars() {
        out.extend(c.to_lowercase());
    }
}

/// Glob match of the whole `text`. On a mismatch after a `*`, the `*` takes one more character
/// and the match goes on from there, so the work is at most pattern length times text length.
fn glob_match(glob: &[char], text: &str) -> bool {
    let mut p = 0;
    let mut t = 0;
    // The pattern index after the last `*`, and the text index that `*` stopped at.
    let mut star: Option<(usize, usize)> = None;
    while let Some(c) = text[t..].chars().next() {
        match glob.get(p) {
            Some(&'*') => {
                star = Some((p + 1, t));
                p += 1;
            }
            Some(&g) if g == '?' || g == c => {
                p += 1;
                t += c.len_utf8();
            }
            _ => {
                let Some((after, from)) = star else {
                    return false;
                };
                let Some(skipped) = text[from..].chars().next() else {
                    return false;
                };
                let next = from + skipped.len_utf8();
                star = Some((after, next));
                p = after;
                t = next;
            }
        }
    }
    glob[p..].iter().all(|&g| g == '*')
}

/// Where a handle hit came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    Fd,
    Mmap,
    Cwd,
    Exe,
    Root,
}

impl HitKind {
    /// The Type column text.
    pub fn label(self) -> &'static str {
        match self {
            HitKind::Fd => "fd",
            HitKind::Mmap => "mmap",
            HitKind::Cwd => "cwd",
            HitKind::Exe => "exe",
            HitKind::Root => "root",
        }
    }
}

/// One matching handle. `path` is an open file path and is never logged (ADR-015).
#[derive(Clone, Debug, PartialEq)]
pub struct HandleHitInfo {
    pub key: ProcessKey,
    pub pid: u32,
    /// Sanitized `comm` from `stat`.
    pub comm: Arc<str>,
    /// Owner of `/proc/<pid>`: the effective uid, or 0 for a process that is not dumpable.
    pub uid: u32,
    pub kind: HitKind,
    /// The fd number, for `Fd` hits.
    pub fd: Option<u32>,
    /// Sanitized path.
    pub path: Arc<str>,
}

/// Counts for the progress text and the footer note.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WalkProgress {
    /// Processes whose fds were read.
    pub searched: u32,
    /// Processes still alive or already searched. A process that exits during the walk is
    /// taken off.
    pub total: u32,
    /// Processes whose `fd/` could not be read, for example another user's without the helper.
    pub skipped: u32,
    /// Searched processes with more than `FD_WALK_MAX` fds, whose later fds were not searched.
    /// When it is not 0 the results are partial.
    pub truncated_pids: u32,
    /// fd links read.
    pub handles: u64,
}

/// Whether a walk has processes left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    More,
    Done,
}

/// A handle search over every process, run in steps so the caller can cancel between pids.
pub struct HandleWalk {
    pids: Vec<u32>,
    next: usize,
    pattern: Pattern,
    include_maps: bool,
    progress: WalkProgress,
    /// Reused lowercase buffer for matching.
    lower: String,
    /// Most fds read per process: `FD_WALK_MAX`, lower in tests.
    fd_cap: usize,
}

/// What happened to one pid.
enum PidOutcome {
    Searched,
    Skipped,
    Gone,
}

impl HandleWalk {
    /// Lists the numeric entries of the proc root. A root that cannot be listed gives an empty
    /// walk.
    pub fn start(root: &SysRoot, pattern: &str, include_maps: bool) -> Self {
        let pids = open_proc(root)
            .map(|p| list_pids(p.as_fd()))
            .unwrap_or_default();
        let total = u32::try_from(pids.len()).unwrap_or(u32::MAX);
        HandleWalk {
            pids,
            next: 0,
            pattern: Pattern::new(pattern),
            include_maps,
            progress: WalkProgress {
                total,
                ..WalkProgress::default()
            },
            lower: String::new(),
            fd_cap: FD_WALK_MAX,
        }
    }

    /// Walks up to `budget` pids (at least 1) and pushes the matching handles to `hits`.
    pub fn step(
        &mut self,
        root: &SysRoot,
        ctx_boot: BootId,
        budget: usize,
        bufs: &mut ReadBufs,
        hits: &mut Vec<HandleHitInfo>,
    ) -> StepResult {
        let end = self.next.saturating_add(budget.max(1)).min(self.pids.len());
        if self.next < end {
            match open_proc(root) {
                Ok(proc) => {
                    let mut stat = PidStat::default();
                    for i in self.next..end {
                        let pid = self.pids[i];
                        let outcome =
                            self.walk_pid(proc.as_fd(), pid, ctx_boot, &mut stat, bufs, hits);
                        match outcome {
                            PidOutcome::Searched => self.progress.searched += 1,
                            PidOutcome::Skipped => self.progress.skipped += 1,
                            PidOutcome::Gone => {
                                self.progress.total = self.progress.total.saturating_sub(1);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::trace!(errno = ?e, "proc root not opened, pids skipped");
                    let n = u32::try_from(end - self.next).unwrap_or(u32::MAX);
                    self.progress.skipped = self.progress.skipped.saturating_add(n);
                }
            }
            self.next = end;
        }
        if self.next < self.pids.len() {
            StepResult::More
        } else {
            StepResult::Done
        }
    }

    pub fn progress(&self) -> WalkProgress {
        self.progress
    }

    fn walk_pid(
        &mut self,
        proc: BorrowedFd<'_>,
        pid: u32,
        ctx_boot: BootId,
        stat: &mut PidStat,
        bufs: &mut ReadBufs,
        hits: &mut Vec<HandleHitInfo>,
    ) -> PidOutcome {
        let pid_dir = match open_pid_dir(proc, pid) {
            Ok(dir) => dir,
            Err(e) => return failure(e),
        };
        if let Err(e) = read_stat(pid_dir.as_fd(), stat, &mut bufs.small) {
            return failure(e);
        }
        let fd_dir = match open_fd_dir(pid_dir.as_fd()) {
            Ok(dir) => dir,
            Err(e) => {
                tracing::trace!(pid, errno = ?e, "fd dir not read");
                return failure(e);
            }
        };
        let uid = rustix::fs::fstat(&pid_dir).map_or(u32::MAX, |st| st.st_uid);
        let mut hit = HitBase {
            key: ProcessKey {
                start_ticks: stat.start_ticks,
                boot_id: ctx_boot,
                pid,
            },
            uid,
            comm_raw: &stat.comm_raw[..],
            comm: None,
        };

        let mut dirents = [MaybeUninit::<u8>::uninit(); FD_DIRENT_BUF];
        let mut dir = RawDir::new(fd_dir.as_fd(), &mut dirents[..]);
        let mut read = 0;
        while let Some(Ok(entry)) = dir.next() {
            let Some(fd) = parse_tid(entry.file_name().to_bytes()) else {
                continue;
            };
            if read == self.fd_cap {
                self.progress.truncated_pids += 1;
                break;
            }
            read += 1;
            if !read_link(fd_dir.as_fd(), entry.file_name(), bufs) {
                continue;
            }
            self.progress.handles += 1;
            if self.pattern.matches(&bufs.text, &mut self.lower) {
                hits.push(hit.hit(HitKind::Fd, Some(fd), &bufs.text));
            }
        }

        if self.include_maps {
            self.walk_maps(pid_dir.as_fd(), &mut hit, bufs, hits);
            for (name, kind) in [
                (c"cwd", HitKind::Cwd),
                (c"exe", HitKind::Exe),
                (c"root", HitKind::Root),
            ] {
                if read_link(pid_dir.as_fd(), name, bufs)
                    && self.pattern.matches(&bufs.text, &mut self.lower)
                {
                    hits.push(hit.hit(kind, None, &bufs.text));
                }
            }
        }
        PidOutcome::Searched
    }

    /// Pushes one `Mmap` hit per matching file path of `maps`.
    fn walk_maps(
        &mut self,
        pid_dir: BorrowedFd<'_>,
        hit: &mut HitBase<'_>,
        bufs: &mut ReadBufs,
        hits: &mut Vec<HandleHitInfo>,
    ) {
        let Ok(maps) = open_beneath(pid_dir, &["maps"], READ_FLAGS) else {
            return;
        };
        // Allocated only for a match.
        let mut seen: FxHashSet<Arc<str>> = FxHashSet::default();
        let text = &mut bufs.text;
        for_each_line(maps.as_fd(), &mut bufs.small, |line| {
            let Some((_, at)) = parse_maps_line(line) else {
                return;
            };
            let raw = &line[at..];
            if raw.is_empty() {
                return;
            }
            sanitize_link(raw, text);
            if !is_file_backed(text)
                || seen.contains(text.as_str())
                || !self.pattern.matches(text, &mut self.lower)
            {
                return;
            }
            let found = hit.hit(HitKind::Mmap, None, text);
            seen.insert(found.path.clone());
            hits.push(found);
        });
    }
}

/// The parts of a hit shared by every handle of one process. `comm` is sanitized on the first
/// hit.
struct HitBase<'a> {
    key: ProcessKey,
    uid: u32,
    comm_raw: &'a [u8],
    comm: Option<Arc<str>>,
}

impl HitBase<'_> {
    fn hit(&mut self, kind: HitKind, fd: Option<u32>, path: &str) -> HandleHitInfo {
        let comm = self.comm.get_or_insert_with(|| comm_text(self.comm_raw));
        HandleHitInfo {
            key: self.key,
            pid: self.key.pid,
            comm: comm.clone(),
            uid: self.uid,
            kind,
            fd,
            path: Arc::from(path),
        }
    }
}

fn comm_text(raw: &[u8]) -> Arc<str> {
    let mut text = String::new();
    sanitize_comm(raw, &mut text);
    Arc::from(text)
}

/// The processes on the other end of each pipe inode: pid and sanitized `comm`.
pub type PipePeers = FxHashMap<u64, SmallVec<[(u32, Arc<str>); 2]>>;

/// Records, for every `pipe:[<ino>]` fd link whose inode is in `inodes`, the pid and sanitized
/// `comm` of each process that holds it, except `skip_pid` (the File descriptors tab names a
/// pipe by its peer). `out` is cleared first. Processes that cannot be read are left out.
pub fn find_pipe_peers(
    root: &SysRoot,
    inodes: &FxHashSet<u64>,
    skip_pid: u32,
    bufs: &mut ReadBufs,
    out: &mut PipePeers,
) {
    out.clear();
    if inodes.is_empty() {
        return;
    }
    let Ok(proc) = open_proc(root) else {
        return;
    };
    let mut stat = PidStat::default();
    for pid in list_pids(proc.as_fd()) {
        if pid == skip_pid {
            continue;
        }
        let Ok(pid_dir) = open_pid_dir(proc.as_fd(), pid) else {
            continue;
        };
        let Ok(fd_dir) = open_fd_dir(pid_dir.as_fd()) else {
            continue;
        };
        // Read once, on the first matching pipe.
        let mut comm: Option<Option<Arc<str>>> = None;
        let mut link = [0u8; 32];
        let mut dirents = [MaybeUninit::<u8>::uninit(); FD_DIRENT_BUF];
        let mut dir = RawDir::new(fd_dir.as_fd(), &mut dirents[..]);
        let mut read = 0;
        while read < FD_WALK_MAX
            && let Some(Ok(entry)) = dir.next()
        {
            if parse_tid(entry.file_name().to_bytes()).is_none() {
                continue;
            }
            read += 1;
            // `pipe:[<u64>]` is at most 27 bytes; a full buffer is another kind of link.
            let n = match rustix::fs::readlinkat_raw(
                fd_dir.as_fd(),
                entry.file_name(),
                &mut link[..],
            ) {
                Ok(n) if n < link.len() => n,
                _ => continue,
            };
            let Some(ino) = link[..n].strip_prefix(b"pipe:").and_then(bracket_inode) else {
                continue;
            };
            if !inodes.contains(&ino) {
                continue;
            }
            let found = comm.get_or_insert_with(|| {
                read_stat(pid_dir.as_fd(), &mut stat, &mut bufs.small)
                    .ok()
                    .map(|()| comm_text(&stat.comm_raw))
            });
            let Some(found) = found else {
                continue;
            };
            let peers = out.entry(ino).or_default();
            if !peers.iter().any(|(p, _)| *p == pid) {
                peers.push((pid, found.clone()));
            }
        }
    }
}

/// The proc root as an `O_PATH` directory.
fn open_proc(root: &SysRoot) -> Result<OwnedFd, rustix::io::Errno> {
    rustix::fs::open(
        &root.proc,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
}

/// The numeric entries of the proc root, sorted. `self` and other names are left out, so no
/// symlink is followed.
fn list_pids(proc: BorrowedFd<'_>) -> Vec<u32> {
    let mut pids = Vec::new();
    // `O_PATH` cannot be listed, so open `.` again for reading.
    let Ok(list) = rustix::fs::openat(
        proc,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return pids;
    };
    let mut dirents = [MaybeUninit::<u8>::uninit(); FD_DIRENT_BUF];
    let mut dir = RawDir::new(list.as_fd(), &mut dirents[..]);
    while let Some(Ok(entry)) = dir.next() {
        pids.extend(parse_tid(entry.file_name().to_bytes()));
    }
    pids.sort_unstable();
    pids
}

fn open_pid_dir(proc: BorrowedFd<'_>, pid: u32) -> Result<OwnedFd, rustix::io::Errno> {
    open_beneath(proc, &[&pid.to_string()], OFlags::PATH | OFlags::DIRECTORY)
}

fn open_fd_dir(pid_dir: BorrowedFd<'_>) -> Result<OwnedFd, rustix::io::Errno> {
    open_beneath(pid_dir, &["fd"], OFlags::RDONLY | OFlags::DIRECTORY)
}

/// Reads and parses `stat`. A file that does not parse is `EINVAL`, so the pid is skipped.
fn read_stat(
    pid_dir: BorrowedFd<'_>,
    stat: &mut PidStat,
    buf: &mut Vec<u8>,
) -> Result<(), rustix::io::Errno> {
    let fd = open_beneath(pid_dir, &["stat"], READ_FLAGS)?;
    read_bounded(fd.as_fd(), buf, SMALL_FILE_LIMIT)?;
    parse_pid_stat(buf, stat).map_err(|_| rustix::io::Errno::INVAL)
}

/// `Gone` for ENOENT or ESRCH, `Skipped` for anything else, denied or not.
fn failure(e: rustix::io::Errno) -> PidOutcome {
    if is_gone(e) {
        PidOutcome::Gone
    } else {
        if !is_denied(e) {
            tracing::trace!(errno = ?e, "pid not read");
        }
        PidOutcome::Skipped
    }
}

/// Reads the link `<dir>/<name>` into `bufs.text`, sanitized. False when it cannot be read.
fn read_link(dir: BorrowedFd<'_>, name: &CStr, bufs: &mut ReadBufs) -> bool {
    // One byte past the limit, so `sanitize_link` sees the cut and adds `…`.
    bufs.small.clear();
    bufs.small.resize(EXE_LINK_MAX + 1, 0);
    match rustix::fs::readlinkat_raw(dir, name, &mut bufs.small[..]) {
        Ok(n) => {
            bufs.small.truncate(n);
            sanitize_link(&bufs.small, &mut bufs.text);
            true
        }
        Err(_) => false,
    }
}

/// Calls `f` with each line of `fd` (without `\n`), reading at most `MAPS_READ_MAX` bytes in
/// `MAPS_CHUNK` pieces. A line cut by the limit, or longer than one chunk, is dropped. A read
/// error ends the lines.
fn for_each_line(fd: BorrowedFd<'_>, buf: &mut Vec<u8>, mut f: impl FnMut(&[u8])) {
    buf.clear();
    buf.resize(MAPS_CHUNK, 0);
    let mut filled = 0;
    let mut total = 0;
    // True while dropping the rest of a line longer than one chunk.
    let mut dropping = false;
    while total < MAPS_READ_MAX {
        let room = (buf.len() - filled).min(MAPS_READ_MAX - total);
        let n = match rustix::io::read(fd, &mut buf[filled..filled + room]) {
            Ok(n) => n,
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return,
        };
        if n == 0 {
            if !dropping && filled > 0 {
                f(&buf[..filled]);
            }
            return;
        }
        filled += n;
        total += n;
        let mut start = 0;
        while let Some(i) = memchr::memchr(b'\n', &buf[start..filled]) {
            if !dropping {
                f(&buf[start..start + i]);
            }
            dropping = false;
            start += i + 1;
        }
        if start == 0 && filled == buf.len() {
            dropping = true;
            filled = 0;
        } else {
            buf.copy_within(start..filled, 0);
            filled -= start;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(bytes: &[u8]) -> Vec<Vec<u8>> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("maps");
        std::fs::write(&path, bytes).expect("write");
        let fd =
            rustix::fs::open(&path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()).expect("open");
        let mut buf = Vec::new();
        let mut out = Vec::new();
        for_each_line(fd.as_fd(), &mut buf, |l| out.push(l.to_vec()));
        out
    }

    #[test]
    fn lines_split_and_last_line_kept() {
        assert_eq!(lines_of(b"a\nbc\n"), [b"a".to_vec(), b"bc".to_vec()]);
        assert_eq!(lines_of(b"a\nbc"), [b"a".to_vec(), b"bc".to_vec()]);
        assert!(lines_of(b"").is_empty());
    }

    #[test]
    fn line_longer_than_a_chunk_dropped() {
        let mut bytes = b"first\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', MAPS_CHUNK * 2 + 10));
        bytes.extend_from_slice(b"\nlast\n");
        assert_eq!(lines_of(&bytes), [b"first".to_vec(), b"last".to_vec()]);
    }

    #[test]
    fn fd_cap_counts_truncated_pid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let proc = tmp.path().join("proc");
        let stat = "(x) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 500 0 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0";
        // 7 has 3 fds, 8 has exactly 2.
        for (pid, fds) in [(7, 3), (8, 2)] {
            let dir = proc.join(format!("{pid}"));
            std::fs::create_dir_all(dir.join("fd")).expect("mkdir");
            std::fs::write(dir.join("stat"), format!("{pid} {stat}\n")).expect("stat");
            for fd in 0..fds {
                std::os::unix::fs::symlink("/f", dir.join(format!("fd/{fd}"))).expect("link");
            }
        }
        let root = SysRoot::at(tmp.path());
        let run = |cap: usize| {
            let mut walk = HandleWalk::start(&root, "/f", false);
            walk.fd_cap = cap;
            let mut bufs = ReadBufs::default();
            let mut hits = Vec::new();
            while walk.step(&root, BootId(1), 8, &mut bufs, &mut hits) == StepResult::More {}
            (walk.progress(), hits.len())
        };
        let (p, hits) = run(2);
        assert_eq!(
            (p.searched, p.truncated_pids, p.handles, hits),
            (2, 1, 4, 4)
        );
        let (p, hits) = run(FD_WALK_MAX);
        assert_eq!(
            (p.searched, p.truncated_pids, p.handles, hits),
            (2, 0, 5, 5)
        );
    }

    #[test]
    fn globs() {
        let glob = |p: &str, t: &str| glob_match(&p.chars().collect::<Vec<_>>(), t);
        assert!(glob("*.txt", "/a/b.txt"));
        assert!(!glob("*.txt", "/a/b.txt2"));
        assert!(glob("/a/?.txt", "/a/é.txt"));
        assert!(glob("**", ""));
        assert!(glob("*a*b*", "xxaxxbxx"));
        assert!(!glob("*a*b", "xxaxxbxx"));
        assert!(!glob("?", ""));
    }
}
