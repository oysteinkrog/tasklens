//! Parser for /proc/<pid>/cgroup, cgroup v2 placement, and readers for the cgroup stat files
//! of apps and units (R-103, R-137).

use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;
use smallvec::SmallVec;

use crate::CollectError;
use crate::sysroot::{SMALL_FILE_LIMIT, read_at};

/// The cgroup v2 path of a process, split into the parts the grouping needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CgroupPath {
    /// The v2 path without the leading `/`, relative to `/sys/fs/cgroup`. Empty for the root.
    pub rel: Arc<str>,
    /// The innermost component that ends in `.service` or `.scope`, still escaped.
    pub unit: Option<Arc<str>>,
    /// The components that end in `.slice`, outermost first.
    pub slices: SmallVec<[Arc<str>; 4]>,
}

/// Parses `/proc/<pid>/cgroup`. Uses the `0::` line only. Returns `None` when there is no such
/// line (cgroup v1 only, or an empty file), or when its path is not absolute or not UTF-8.
pub fn parse_cgroup_file(buf: &[u8]) -> Option<CgroupPath> {
    let line = buf
        .split(|&b| b == b'\n')
        .find_map(|line| line.strip_prefix(b"0::"))?;
    let path = std::str::from_utf8(line).ok()?;
    let rel = path.strip_prefix('/')?.trim_end_matches('/');
    let mut unit = None;
    let mut slices = SmallVec::new();
    for comp in rel.split('/').filter(|c| !c.is_empty()) {
        if comp.ends_with(".slice") {
            slices.push(Arc::from(comp));
        } else if comp.ends_with(".service") || comp.ends_with(".scope") {
            unit = Some(comp);
        }
    }
    Some(CgroupPath {
        rel: Arc::from(rel),
        unit: unit.map(Arc::from),
        slices,
    })
}

/// Replaces every systemd `\xNN` escape (two hex digits) in `name` with that byte and writes the
/// result to `out`, which is cleared first. A malformed escape is copied as is. Bytes that do not
/// form valid UTF-8 become U+FFFD.
pub fn unescape_unit(name: &str, out: &mut String) {
    out.clear();
    if !name.contains("\\x") {
        out.push_str(name);
        return;
    }
    let src = name.as_bytes();
    let mut bytes = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'\\'
            && src.get(i + 1) == Some(&b'x')
            && let (Some(hi), Some(lo)) = (
                src.get(i + 2).and_then(|&b| hex(b)),
                src.get(i + 3).and_then(|&b| hex(b)),
            )
        {
            bytes.push((hi << 4) | lo);
            i += 4;
        } else {
            bytes.push(src[i]);
            i += 1;
        }
    }
    out.push_str(&String::from_utf8_lossy(&bytes));
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Where a process sits relative to the systemd user slices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserPlacement {
    /// Under `user.slice/user-<uid>.slice/user@<uid>.service/`.
    InUserManager { uid: u32 },
    /// In `user.slice/user-<uid>.slice/session-<n>.scope`.
    InSessionScope { uid: u32 },
    /// Anywhere else: `system.slice`, `init.scope`, `machine.slice`, the root.
    OutsideUserSlice,
}

/// Classifies a cgroup path by its user slice.
pub fn placement(p: &CgroupPath) -> UserPlacement {
    let mut comps = p.rel.split('/');
    if comps.next() != Some("user.slice") {
        return UserPlacement::OutsideUserSlice;
    }
    let Some(uid) = comps.next().and_then(user_slice_uid) else {
        return UserPlacement::OutsideUserSlice;
    };
    match comps.next() {
        Some(c) if user_manager_uid(c) == Some(uid) => UserPlacement::InUserManager { uid },
        Some(c) if is_session_scope(c) => UserPlacement::InSessionScope { uid },
        _ => UserPlacement::OutsideUserSlice,
    }
}

/// Returns the app unit when the path is under
/// `user.slice/user-{uid}.slice/user@{uid}.service/app.slice/`: the first component after
/// `app.slice/`, which may itself be a `.slice`. Returns `None` otherwise.
pub fn app_slice_unit(p: &CgroupPath, uid: u32) -> Option<&str> {
    let mut comps = p.rel.split('/');
    if comps.next() != Some("user.slice")
        || comps.next().and_then(user_slice_uid) != Some(uid)
        || comps.next().and_then(user_manager_uid) != Some(uid)
        || comps.next() != Some("app.slice")
    {
        return None;
    }
    comps.next().filter(|c| !c.is_empty())
}

/// `user-<uid>.slice` to its uid.
fn user_slice_uid(c: &str) -> Option<u32> {
    parse_uid(c.strip_prefix("user-")?.strip_suffix(".slice")?)
}

/// `user@<uid>.service` to its uid.
fn user_manager_uid(c: &str) -> Option<u32> {
    parse_uid(c.strip_prefix("user@")?.strip_suffix(".service")?)
}

/// `session-<n>.scope`, where `<n>` is not empty.
fn is_session_scope(c: &str) -> bool {
    c.strip_prefix("session-")
        .and_then(|s| s.strip_suffix(".scope"))
        .is_some_and(|n| !n.is_empty())
}

fn parse_uid(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Largest `memory.stat` kept. The real file is about 1.5 KB.
const MEMORY_STAT_LIMIT: usize = 4 * SMALL_FILE_LIMIT;
/// Largest `cgroup.procs` kept: about 500 000 pids.
const PROCS_LIMIT: usize = 4 << 20;

thread_local! {
    static MEMORY_CURRENT_OPENS: Cell<u32> = const { Cell::new(0) };
}

/// Test hook: how many times `read_stats` tried to open `memory.current` on this thread.
#[doc(hidden)]
pub fn memory_current_opens() -> u32 {
    MEMORY_CURRENT_OPENS.with(Cell::get)
}

/// Values from the stat files of one cgroup. `None` means the file was absent, not wanted, or
/// denied; the sweep maps it to Unsupported or Restricted, never to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CgroupStats {
    /// `memory.stat` `anon`, in bytes.
    pub anon: Option<u64>,
    /// `memory.swap.current`, in bytes.
    pub swap_current: Option<u64>,
    /// `memory.current`, in bytes. Read only in `EngineMode::LowMemory` (R-137).
    pub memory_current: Option<u64>,
    /// `cpu.stat` `usage_usec`.
    pub usage_usec: Option<u64>,
    /// `io.stat` sums over all devices. Valid only when `io_valid` is true.
    pub rbytes: u64,
    pub wbytes: u64,
    pub rios: u64,
    pub wios: u64,
    /// True when `io.stat` was read. An empty file is valid with zeros.
    pub io_valid: bool,
    /// `pids.current`.
    pub pids_current: Option<u64>,
    /// `cgroup.events` `frozen`.
    pub frozen: Option<bool>,
    /// True when any wanted file gave EACCES or EPERM.
    pub denied: bool,
}

/// Which stat files `read_stats` opens.
#[derive(Clone, Copy, Debug)]
pub struct CgroupWant {
    pub memory_stat: bool,
    pub swap: bool,
    pub memory_current: bool,
    pub cpu_stat: bool,
    pub io_stat: bool,
    pub pids: bool,
    pub events: bool,
}

/// An open directory fd on `/sys/fs/cgroup`. Every cgroup file is opened relative to it and
/// closed in the same call; no other fd is kept (R-137).
pub struct CgroupRoot {
    dirfd: OwnedFd,
}

impl CgroupRoot {
    /// Opens `<sys>/fs/cgroup`. `sys` is `SysRoot.sys`, so fixtures work.
    pub fn open(sys: &Path) -> Result<Self, CollectError> {
        let dirfd = rustix::fs::open(
            sys.join("fs/cgroup"),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(dir_error)?;
        Ok(Self { dirfd })
    }

    /// Reads the wanted stat files of cgroup `rel` (relative to `/sys/fs/cgroup`, empty for the
    /// root) into `out`, which is reset first. A missing or denied file leaves its field `None`.
    /// A missing directory gives `ProcRead { file: "cgroup", errno: ENOENT }`.
    pub fn read_stats(
        &self,
        rel: &str,
        want: CgroupWant,
        buf: &mut Vec<u8>,
        out: &mut CgroupStats,
    ) -> Result<(), CollectError> {
        *out = CgroupStats::default();
        let dir = self.open_dir(rel)?;
        let dir = dir.as_fd();
        if want.memory_stat && read_file(dir, "memory.stat", MEMORY_STAT_LIMIT, buf, out) {
            out.anon = parse_memory_stat_anon(buf);
        }
        if want.swap && read_file(dir, "memory.swap.current", SMALL_FILE_LIMIT, buf, out) {
            out.swap_current = parse_u64(buf.trim_ascii());
        }
        if want.memory_current {
            MEMORY_CURRENT_OPENS.with(|n| n.set(n.get().wrapping_add(1)));
            if read_file(dir, "memory.current", SMALL_FILE_LIMIT, buf, out) {
                out.memory_current = parse_u64(buf.trim_ascii());
            }
        }
        if want.cpu_stat && read_file(dir, "cpu.stat", SMALL_FILE_LIMIT, buf, out) {
            out.usage_usec = key_value(buf, b"usage_usec");
        }
        if want.io_stat && read_file(dir, "io.stat", SMALL_FILE_LIMIT, buf, out) {
            parse_io_stat(buf, out);
        }
        if want.pids && read_file(dir, "pids.current", SMALL_FILE_LIMIT, buf, out) {
            out.pids_current = parse_u64(buf.trim_ascii());
        }
        if want.events && read_file(dir, "cgroup.events", SMALL_FILE_LIMIT, buf, out) {
            out.frozen = match key_value(buf, b"frozen") {
                Some(0) => Some(false),
                Some(1) => Some(true),
                _ => None,
            };
        }
        Ok(())
    }

    /// Appends the pids in `cgroup.procs` of cgroup `rel` to `out`, and when `recursive`, those
    /// of every descendant, depth-first. A child that goes away during the walk is skipped.
    pub fn read_procs(
        &self,
        rel: &str,
        recursive: bool,
        buf: &mut Vec<u8>,
        out: &mut Vec<u32>,
    ) -> Result<(), CollectError> {
        let dir = self.open_dir(rel)?;
        read_procs_file(dir.as_fd(), buf, out).map_err(procs_error)?;
        if recursive {
            let dir = Dir::new(dir).map_err(procs_error)?;
            walk_children(dir, buf, out).map_err(procs_error)?;
        }
        Ok(())
    }

    /// The inode of the cgroup directory: the `cgroup_id` that `CgroupRef` holds (ADR-006).
    pub fn dir_ino(&self, rel: &str) -> Result<u64, CollectError> {
        let dir = self.open_dir(rel)?;
        let st = rustix::fs::fstat(&dir).map_err(dir_error)?;
        Ok(st.st_ino)
    }

    /// Appends the names of the child directories of cgroup `rel` to `out`. Names that are not
    /// UTF-8 are skipped. Symlinks are never followed, so a symlink to a directory is skipped.
    pub fn child_dirs(&self, rel: &str, out: &mut Vec<Arc<str>>) -> Result<(), CollectError> {
        let mut dir = Dir::new(self.open_dir(rel)?).map_err(dir_error)?;
        while let Some(entry) = dir.read() {
            let entry = entry.map_err(dir_error)?;
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            let is_dir = match entry.file_type() {
                FileType::Directory => true,
                // No d_type: ask lstat.
                FileType::Unknown => dir.fd().is_ok_and(|fd| {
                    rustix::fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW)
                        .is_ok_and(|st| FileType::from_raw_mode(st.st_mode) == FileType::Directory)
                }),
                _ => false,
            };
            if is_dir && let Ok(name) = std::str::from_utf8(name.to_bytes()) {
                out.push(Arc::from(name));
            }
        }
        Ok(())
    }

    fn open_dir(&self, rel: &str) -> Result<OwnedFd, CollectError> {
        let rel = rel.trim_start_matches('/');
        let rel = if rel.is_empty() { "." } else { rel };
        rustix::fs::openat(
            &self.dirfd,
            rel,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(dir_error)
    }
}

fn dir_error(e: Errno) -> CollectError {
    CollectError::ProcRead {
        file: "cgroup",
        errno: e.raw_os_error(),
    }
}

fn procs_error(e: Errno) -> CollectError {
    CollectError::ProcRead {
        file: "cgroup.procs",
        errno: e.raw_os_error(),
    }
}

/// Reads `name` into `buf`. Returns false when the file is absent or unreadable, and sets
/// `out.denied` on EACCES or EPERM.
fn read_file(
    dir: BorrowedFd<'_>,
    name: &str,
    limit: usize,
    buf: &mut Vec<u8>,
    out: &mut CgroupStats,
) -> bool {
    match read_at(dir, name, buf, limit) {
        Ok(_) => true,
        Err(CollectError::PermissionDenied) => {
            out.denied = true;
            false
        }
        Err(_) => false,
    }
}

fn read_procs_file(
    dir: BorrowedFd<'_>,
    buf: &mut Vec<u8>,
    out: &mut Vec<u32>,
) -> Result<(), Errno> {
    buf.clear();
    let fd = rustix::fs::openat(
        dir,
        "cgroup.procs",
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    while buf.len() < PROCS_LIMIT {
        if buf.len() == buf.capacity() {
            buf.reserve(SMALL_FILE_LIMIT.max(buf.len()).min(PROCS_LIMIT - buf.len()));
        }
        if rustix::io::read(fd.as_fd(), rustix::buffer::spare_capacity(buf))? == 0 {
            break;
        }
    }
    out.extend(
        buf.split(|&b| b == b'\n')
            .filter_map(|line| parse_u64(line.trim_ascii()))
            .filter_map(|pid| u32::try_from(pid).ok()),
    );
    Ok(())
}

/// Depth-first over the child directories of `dir`. ENOENT on a child means it was removed
/// during the walk and is skipped.
fn walk_children(mut dir: Dir, buf: &mut Vec<u8>, out: &mut Vec<u32>) -> Result<(), Errno> {
    let mut children: SmallVec<[Vec<u8>; 8]> = SmallVec::new();
    while let Some(entry) = dir.read() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if matches!(entry.file_type(), FileType::Directory | FileType::Unknown) {
            children.push(name.to_vec());
        }
    }
    for name in children {
        let child = match rustix::fs::openat(
            dir.fd()?,
            name.as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT | Errno::NOTDIR) => continue,
            Err(e) => return Err(e),
        };
        match read_procs_file(child.as_fd(), buf, out) {
            Ok(()) => {}
            Err(Errno::NOENT | Errno::NODEV) => continue,
            Err(e) => return Err(e),
        }
        walk_children(Dir::new(child)?, buf, out)?;
    }
    Ok(())
}

/// The `anon <bytes>` line of `memory.stat`.
pub fn parse_memory_stat_anon(buf: &[u8]) -> Option<u64> {
    key_value(buf, b"anon")
}

/// Sums `rbytes`, `wbytes`, `rios` and `wios` over the device lines of `io.stat` into `out` and
/// sets `io_valid`. Other keys and malformed pairs are skipped. Sums saturate.
pub fn parse_io_stat(buf: &[u8], out: &mut CgroupStats) {
    out.rbytes = 0;
    out.wbytes = 0;
    out.rios = 0;
    out.wios = 0;
    for line in buf.split(|&b| b == b'\n') {
        // The first field is `<maj>:<min>`.
        for pair in line.split(|&b| b == b' ').skip(1) {
            let Some(eq) = memchr::memchr(b'=', pair) else {
                continue;
            };
            let Some(n) = parse_u64(&pair[eq + 1..]) else {
                continue;
            };
            let field = match &pair[..eq] {
                b"rbytes" => &mut out.rbytes,
                b"wbytes" => &mut out.wbytes,
                b"rios" => &mut out.rios,
                b"wios" => &mut out.wios,
                _ => continue,
            };
            *field = field.saturating_add(n);
        }
    }
    out.io_valid = true;
}

/// The value of the first `<key> <n>` line in a flat-keyed cgroup file.
fn key_value(buf: &[u8], key: &[u8]) -> Option<u64> {
    buf.split(|&b| b == b'\n').find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(b" ")?;
        parse_u64(rest.trim_ascii())
    })
}

/// Decimal digits only, at least one, no overflow.
fn parse_u64(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |n, &d| {
        if d.is_ascii_digit() {
            n.checked_mul(10)?.checked_add(u64::from(d - b'0'))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::Arc;

    use super::CgroupRoot;

    #[test]
    fn child_dirs_skips_files_symlinks_and_non_utf8() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("fs/cgroup/app.slice");
        std::fs::create_dir_all(base.join("a.service/sub")).expect("mkdir");
        std::fs::create_dir(base.join("b.scope")).expect("mkdir");
        std::fs::create_dir(base.join(OsStr::from_bytes(b"bad\xff.scope"))).expect("mkdir");
        std::fs::write(base.join("cgroup.procs"), "1\n").expect("write");
        std::os::unix::fs::symlink(base.join("b.scope"), base.join("link.scope")).expect("ln");
        let cg = CgroupRoot::open(tmp.path()).expect("open");
        let mut out: Vec<Arc<str>> = Vec::new();
        cg.child_dirs("app.slice", &mut out).expect("list");
        out.sort();
        assert_eq!(out, [Arc::from("a.service"), Arc::from("b.scope")]);
        assert!(cg.child_dirs("missing.slice", &mut out).is_err());
    }
}
