//! Executable hashing (SHA-256 and MD5), streamed and cached by file identity (PROC-074,
//! PROC-078, R-096). Executable paths are never logged (ADR-015).

use std::cell::Cell;
use std::collections::VecDeque;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use md5::Md5;
use rustc_hash::FxHashMap;
use rustix::fs::{FileType, Mode, OFlags};
use rustix::io::Errno;
use sha2::{Digest, Sha256};
use tl_model::ids::ProcessKey;

use crate::proc::detail::READ_FLAGS;
use crate::proc::is_denied;
use crate::proc::pid_stat::read_start_ticks;
use crate::sysroot::SysRoot;

/// Read size per `read` call.
pub const HASH_BUF: usize = 64 * 1024;
/// Most files the cache holds. The oldest inserted goes first.
const HASH_CACHE_MAX: usize = 256;

thread_local! {
    static HASH_READS: Cell<u32> = const { Cell::new(0) };
}

/// Test hook: how many files `hash_fd` read on this thread.
#[doc(hidden)]
pub fn hash_reads() -> u32 {
    HASH_READS.with(Cell::get)
}

/// Both digests of one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hashes {
    pub sha256: [u8; 32],
    pub md5: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HashError {
    /// The process image could not be opened: the process is gone, its pid was reused, or
    /// access was denied and `argv0` gave no readable file.
    ExeUnavailable,
    Read {
        errno: i32,
    },
}

impl HashError {
    /// Text for the General tab.
    pub fn message(&self) -> &'static str {
        match self {
            Self::ExeUnavailable => "Executable path unavailable",
            Self::Read { .. } => "Failed to compute one or more hashes",
        }
    }
}

/// Reads `fd` from offset 0 to the end in `HASH_BUF` chunks and feeds both digests in one pass.
pub fn hash_fd(fd: BorrowedFd<'_>, buf: &mut Vec<u8>) -> Result<Hashes, HashError> {
    HASH_READS.with(|c| c.set(c.get().wrapping_add(1)));
    buf.resize(HASH_BUF, 0);
    let mut sha = Sha256::new();
    let mut md = Md5::new();
    let mut off = 0u64;
    loop {
        match rustix::io::pread(fd, &mut buf[..], off) {
            Ok(0) => break,
            Ok(n) => {
                sha.update(&buf[..n]);
                md.update(&buf[..n]);
                off += n as u64;
            }
            Err(Errno::INTR) => {}
            Err(e) => {
                return Err(HashError::Read {
                    errno: e.raw_os_error(),
                });
            }
        }
    }
    Ok(Hashes {
        sha256: sha.finalize().into(),
        md5: md.finalize().into(),
    })
}

/// (dev, inode, mtime seconds, mtime nanoseconds).
type FileKey = (u64, u64, i64, u32);

/// Hashes by file identity, so a second lookup of the same binary reads nothing.
#[derive(Default)]
pub struct HashCache {
    map: FxHashMap<FileKey, Hashes>,
    order: VecDeque<FileKey>,
}

impl HashCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// True when `file` has a cached entry.
    pub fn contains(&self, file: BorrowedFd<'_>) -> bool {
        file_key(file).is_ok_and(|key| self.map.contains_key(&key))
    }

    fn insert(&mut self, key: FileKey, hashes: Hashes) {
        if self.map.insert(key, hashes).is_some() {
            return;
        }
        if self.order.len() >= HASH_CACHE_MAX
            && let Some(old) = self.order.pop_front()
        {
            self.map.remove(&old);
        }
        self.order.push_back(key);
    }
}

#[allow(
    clippy::unnecessary_cast,
    reason = "stat field widths differ by target"
)]
fn file_key(file: BorrowedFd<'_>) -> Result<FileKey, HashError> {
    let st = rustix::fs::fstat(file).map_err(|e| HashError::Read {
        errno: e.raw_os_error(),
    })?;
    Ok((
        st.st_dev as u64,
        st.st_ino as u64,
        st.st_mtime as i64,
        st.st_mtime_nsec as u32,
    ))
}

/// Hashes an open file, or returns the cached result for the same (dev, inode, mtime). Only a
/// successful hash is cached.
pub fn hash_cached(
    file: BorrowedFd<'_>,
    cache: &mut HashCache,
    buf: &mut Vec<u8>,
) -> Result<Hashes, HashError> {
    let key = file_key(file)?;
    if let Some(hit) = cache.map.get(&key) {
        return Ok(*hit);
    }
    let hashes = hash_fd(file, buf)?;
    cache.insert(key, hashes);
    Ok(hashes)
}

/// Opens `argv0` for reading when it is an absolute path to a regular file. The target process
/// sets its own argv0, so a FIFO or a device must never be read.
fn open_argv0(argv0: Option<&str>) -> Result<OwnedFd, HashError> {
    let path = argv0
        .filter(|p| p.starts_with('/'))
        .ok_or(HashError::ExeUnavailable)?;
    let fd = rustix::fs::open(path, READ_FLAGS | OFlags::CLOEXEC, Mode::empty())
        .map_err(|_| HashError::ExeUnavailable)?;
    let st = rustix::fs::fstat(&fd).map_err(|_| HashError::ExeUnavailable)?;
    if FileType::from_raw_mode(st.st_mode) == FileType::RegularFile {
        Ok(fd)
    } else {
        Err(HashError::ExeUnavailable)
    }
}

/// Hashes the executable of `key`. The start time is checked first, so a reused pid never
/// hashes another process's image. When the `exe` link is denied, an absolute `argv0` that
/// opens for reading stands in.
pub fn hash_process_exe(
    root: &SysRoot,
    key: ProcessKey,
    argv0: Option<&str>,
    cache: &mut HashCache,
    buf: &mut Vec<u8>,
) -> Result<Hashes, HashError> {
    let dir = rustix::fs::open(
        root.proc.join(key.pid.to_string()),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| HashError::ExeUnavailable)?;
    match read_start_ticks(dir.as_fd(), buf) {
        Ok(ticks) if ticks == key.start_ticks => {}
        _ => return Err(HashError::ExeUnavailable),
    }
    // `exe` is a symlink the kernel resolves, so it opens with a plain `openat`. The dir fd
    // pins the process: a reused pid gives ESRCH.
    let fd = match rustix::fs::openat(
        dir.as_fd(),
        "exe",
        READ_FLAGS | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(e) if is_denied(e) => open_argv0(argv0)?,
        Err(_) => return Err(HashError::ExeUnavailable),
    };
    hash_cached(fd.as_fd(), cache, buf)
}
