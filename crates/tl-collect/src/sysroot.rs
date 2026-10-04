//! Root paths for /proc, /sys and /run, real or fixture (ADR-013), the kept file
//! descriptor policy (ADR-016) and bounded one-shot reads (ADR-015).
//!
//! No function here logs a path or file contents (ADR-015).

use std::path::{Path, PathBuf};

use rustix::buffer::spare_capacity;
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags};

use crate::CollectError;

/// Default capacity for one-shot reads of small sysfs files.
pub const SMALL_FILE_LIMIT: usize = 4096;

/// Smallest growth step for a read buffer.
const GROW_STEP: usize = 4096;

const NUMBER_ERROR: CollectError = CollectError::Parse {
    file: "sysfs number",
    what: "not a number",
};

/// Where `/proc`, `/sys` and `/run` live: the host or a fixture directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysRoot {
    pub proc: PathBuf,
    pub sys: PathBuf,
    /// The udev database lives under `/run/udev/data` (R-140 item 2).
    pub run: PathBuf,
}

impl SysRoot {
    /// `/proc`, `/sys` and `/run`.
    pub fn host() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
            sys: PathBuf::from("/sys"),
            run: PathBuf::from("/run"),
        }
    }

    /// `<base>/proc`, `<base>/sys` and `<base>/run` (fixture roots and multi-step fixture steps).
    pub fn at(base: &Path) -> Self {
        Self {
            proc: base.join("proc"),
            sys: base.join("sys"),
            run: base.join("run"),
        }
    }

    /// `rel` has no leading `/`, for example `proc_path("net/dev")`.
    pub fn proc_path(&self, rel: &str) -> PathBuf {
        self.proc.join(rel)
    }

    /// `rel` has no leading `/`, for example `sys_path("block")`.
    pub fn sys_path(&self, rel: &str) -> PathBuf {
        self.sys.join(rel)
    }

    /// `rel` has no leading `/`, for example `run_path("udev/data/b8:0")` (R-140 item 2).
    pub fn run_path(&self, rel: &str) -> PathBuf {
        self.run.join(rel)
    }
}

/// A file kept open and re-read from offset 0 every tick (ADR-016).
pub struct KeptFile {
    path: PathBuf,
    fd: Option<OwnedFd>,
}

impl KeptFile {
    /// Does not open the file yet. The first `read` opens it.
    pub fn new(path: PathBuf) -> Self {
        Self { path, fd: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_open(&self) -> bool {
        self.fd.is_some()
    }

    /// Clears `buf`, opens the file if needed, reads the whole file, returns its length.
    ///
    /// On any error the file is closed, and the next call reopens it.
    pub fn read(&mut self, buf: &mut Vec<u8>) -> Result<usize, CollectError> {
        buf.clear();
        let result = self.read_inner(buf);
        if result.is_err() {
            self.fd = None;
            buf.clear();
        }
        result
    }

    fn read_inner(&mut self, buf: &mut Vec<u8>) -> Result<usize, CollectError> {
        let fd = match self.fd.take() {
            Some(fd) => fd,
            None => open_rdonly(&self.path)?,
        };
        loop {
            if buf.len() == buf.capacity() {
                buf.reserve(GROW_STEP.max(buf.len()));
            }
            let offset = buf.len() as u64;
            let n = rustix::io::pread(&fd, spare_capacity(buf), offset)?;
            if n == 0 {
                break;
            }
        }
        self.fd = Some(fd);
        Ok(buf.len())
    }
}

/// Open, read at most `limit` bytes, close. Returns (bytes kept, truncated).
pub fn read_file(
    path: &Path,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<(usize, bool), CollectError> {
    buf.clear();
    let fd = open_rdonly(path)?;
    read_limited(fd.as_fd(), buf, limit)
}

/// Same, relative to an open directory fd (`openat`), for per-process files (E05).
pub fn read_at(
    dir: BorrowedFd<'_>,
    name: &str,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<(usize, bool), CollectError> {
    buf.clear();
    let fd = rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    read_limited(fd.as_fd(), buf, limit)
}

/// Reads a small file and returns it with ASCII whitespace trimmed at both ends.
pub fn read_trimmed<'b>(path: &Path, buf: &'b mut Vec<u8>) -> Result<&'b [u8], CollectError> {
    read_file(path, buf, SMALL_FILE_LIMIT)?;
    Ok(buf.trim_ascii())
}

/// Reads a small file holding one unsigned decimal number (whitespace trimmed).
pub fn read_u64(path: &Path, buf: &mut Vec<u8>) -> Result<u64, CollectError> {
    parse_number(read_trimmed(path, buf)?)
}

/// Same for a signed number (hwmon temperatures can be negative).
pub fn read_i64(path: &Path, buf: &mut Vec<u8>) -> Result<i64, CollectError> {
    parse_number(read_trimmed(path, buf)?)
}

fn parse_number<T: std::str::FromStr>(bytes: &[u8]) -> Result<T, CollectError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(NUMBER_ERROR)
}

fn open_rdonly(path: &Path) -> Result<OwnedFd, CollectError> {
    Ok(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?)
}

/// Reads from the current position until EOF or until `limit` bytes are kept.
/// `buf` must be empty.
fn read_limited(
    fd: BorrowedFd<'_>,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<(usize, bool), CollectError> {
    while buf.len() < limit {
        if buf.len() == buf.capacity() {
            let want = GROW_STEP.max(buf.len()).min(limit - buf.len());
            buf.reserve(want);
        }
        let n = rustix::io::read(fd, spare_capacity(buf))?;
        if n == 0 {
            return Ok((buf.len(), false));
        }
    }
    if buf.len() > limit {
        // The spare capacity was larger than the limit, so the file has more bytes.
        buf.truncate(limit);
        return Ok((limit, true));
    }
    let mut probe = [0u8; 1];
    let n = rustix::io::read(fd, &mut probe)?;
    Ok((buf.len(), n == 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn sysroot_paths() {
        let root = SysRoot::at(Path::new("/x"));
        assert_eq!(root.proc_path("net/dev"), PathBuf::from("/x/proc/net/dev"));
        assert_eq!(root.sys_path("block"), PathBuf::from("/x/sys/block"));
        assert_eq!(
            root.run_path("udev/data/n2"),
            PathBuf::from("/x/run/udev/data/n2")
        );

        let host = SysRoot::host();
        assert_eq!(host.proc, PathBuf::from("/proc"));
        assert_eq!(host.sys, PathBuf::from("/sys"));
        assert_eq!(host.run, PathBuf::from("/run"));
    }

    #[test]
    fn kept_file_rereads_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, "abc").unwrap();

        let mut kept = KeptFile::new(path.clone());
        assert_eq!(kept.path(), path.as_path());
        assert!(!kept.is_open());
        let mut buf = Vec::new();
        for _ in 0..2 {
            assert_eq!(kept.read(&mut buf).unwrap(), 3);
            assert_eq!(buf, b"abc");
            assert!(kept.is_open());
        }

        // Rewrite in place: same inode, so the kept fd sees the new content.
        let inode_before = fs::metadata(&path).unwrap();
        fs::write(&path, "abcdef").unwrap();
        let inode_after = fs::metadata(&path).unwrap();
        assert_eq!(inode_before.ino(), inode_after.ino());
        assert_eq!(kept.read(&mut buf).unwrap(), 6);
        assert_eq!(buf, b"abcdef");
        assert!(kept.is_open());
    }

    #[test]
    fn kept_file_grows_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        let content: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&path, &content).unwrap();

        let mut kept = KeptFile::new(path);
        let mut buf = Vec::new();
        assert_eq!(kept.read(&mut buf).unwrap(), 20_000);
        assert_eq!(buf, content);

        // Steady state: the second read does not grow the buffer.
        let cap = buf.capacity();
        assert_eq!(kept.read(&mut buf).unwrap(), 20_000);
        assert_eq!(buf.capacity(), cap);
        assert_eq!(buf, content);
    }

    #[test]
    fn kept_file_reopens_after_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("later");
        let mut kept = KeptFile::new(path.clone());
        let mut buf = Vec::new();

        assert!(matches!(kept.read(&mut buf), Err(CollectError::NotFound)));
        assert!(!kept.is_open());

        fs::write(&path, "now").unwrap();
        assert_eq!(kept.read(&mut buf).unwrap(), 3);
        assert_eq!(buf, b"now");
        assert!(kept.is_open());
    }

    #[test]
    fn kept_file_permission_denied() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        let mut kept = KeptFile::new(path);
        let mut buf = Vec::new();
        assert!(matches!(
            kept.read(&mut buf),
            Err(CollectError::PermissionDenied)
        ));
        assert!(!kept.is_open());
    }

    #[test]
    fn read_file_limit_and_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ten");
        fs::write(&path, "0123456789").unwrap();
        let mut buf = Vec::new();

        assert_eq!(read_file(&path, &mut buf, 4).unwrap(), (4, true));
        assert_eq!(buf, b"0123");
        assert_eq!(read_file(&path, &mut buf, 10).unwrap(), (10, false));
        assert_eq!(buf, b"0123456789");
        assert_eq!(read_file(&path, &mut buf, 64).unwrap(), (10, false));
        assert_eq!(buf, b"0123456789");

        // A buffer with more spare capacity than the limit still keeps only `limit` bytes.
        let mut big = Vec::with_capacity(8192);
        assert_eq!(read_file(&path, &mut big, 4).unwrap(), (4, true));
        assert_eq!(big, b"0123");
    }

    #[test]
    fn read_at_matches_read_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status");
        fs::write(&path, "Name:\tbash\nState:\tS\n").unwrap();

        let dir_fd = rustix::fs::open(
            dir.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        let mut via_at = Vec::new();
        let mut via_path = Vec::new();
        let at = read_at(dir_fd.as_fd(), "status", &mut via_at, 64).unwrap();
        let full = read_file(&path, &mut via_path, 64).unwrap();
        assert_eq!(at, full);
        assert_eq!(via_at, via_path);

        assert!(matches!(
            read_at(dir_fd.as_fd(), "missing", &mut via_at, 64),
            Err(CollectError::NotFound)
        ));
    }

    #[test]
    fn read_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &str| {
            let path = dir.path().join(name);
            fs::write(&path, content).unwrap();
            path
        };
        let pos = write("pos", " 42\n");
        let neg = write("neg", "-5000\n");
        let empty = write("empty", "");
        let junk = write("junk", "4x");
        let huge = write("huge", "18446744073709551616\n");
        let mut buf = Vec::new();

        assert_eq!(read_trimmed(&pos, &mut buf).unwrap(), b"42");
        assert_eq!(read_u64(&pos, &mut buf).unwrap(), 42);
        assert_eq!(read_i64(&pos, &mut buf).unwrap(), 42);
        assert_eq!(read_i64(&neg, &mut buf).unwrap(), -5000);
        assert!(matches!(
            read_u64(&neg, &mut buf),
            Err(CollectError::Parse { .. })
        ));
        for path in [&empty, &junk] {
            assert!(matches!(
                read_u64(path, &mut buf),
                Err(CollectError::Parse { .. })
            ));
            assert!(matches!(
                read_i64(path, &mut buf),
                Err(CollectError::Parse { .. })
            ));
        }
        assert!(matches!(
            read_u64(&huge, &mut buf),
            Err(CollectError::Parse { .. })
        ));
    }
}
