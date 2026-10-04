//! Open file descriptor count of a process.
//!
//! R-064: the count is `st_size` of `/proc/<pid>/fd`. Kernels that report 0 there get a readdir
//! count instead. Only counts are read here, never fd targets.
use std::os::fd::BorrowedFd;
use std::sync::atomic::{AtomicU32, Ordering};

use rustix::fs::{AtFlags, Dir, Mode, OFlags};
use rustix::io::Errno;

use super::{is_denied, is_gone};

/// Result of one fd count read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdCount {
    /// Number of open file descriptors.
    Count(u32),
    /// The read was not allowed (EACCES or EPERM). Shown as a lock, never as zero.
    Restricted,
    /// The process went away during the read.
    Gone,
}

/// Number of calls that needed the readdir fallback.
static FALLBACK_USED: AtomicU32 = AtomicU32::new(0);

/// Counts the open fds of the process whose `/proc/<pid>` directory is `pid_dir`.
pub fn fd_count(pid_dir: BorrowedFd<'_>) -> FdCount {
    match rustix::fs::statat(pid_dir, "fd", AtFlags::empty()) {
        Ok(st) if st.st_size > 0 => FdCount::Count(u32::try_from(st.st_size).unwrap_or(u32::MAX)),
        Ok(_) => {
            FALLBACK_USED.fetch_add(1, Ordering::Relaxed);
            readdir_count(pid_dir).unwrap_or_else(classify)
        }
        Err(e) => classify(e),
    }
}

/// Test hook: how many calls used the readdir fallback in this process.
#[doc(hidden)]
pub fn fd_count_fallbacks() -> u32 {
    FALLBACK_USED.load(Ordering::Relaxed)
}

fn readdir_count(pid_dir: BorrowedFd<'_>) -> Result<FdCount, Errno> {
    let fd = rustix::fs::openat(
        pid_dir,
        "fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut n: u32 = 0;
    for entry in Dir::new(fd)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            n = n.saturating_add(1);
        }
    }
    Ok(FdCount::Count(n))
}

/// Maps a read error to a state. EACCES and EPERM give Restricted, ENOENT and ESRCH give Gone.
/// Any other error also gives Gone: the count is unknown and must never show as a number.
fn classify(e: Errno) -> FdCount {
    if is_denied(e) {
        FdCount::Restricted
    } else {
        if !is_gone(e) {
            tracing::trace!(errno = ?e, "unexpected fd count error");
        }
        FdCount::Gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::{AsFd, OwnedFd};
    use std::process::{Command, Stdio};

    fn open_dir(path: &str) -> OwnedFd {
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open proc dir")
    }

    fn readdir_self() -> u32 {
        u32::try_from(
            std::fs::read_dir("/proc/self/fd")
                .expect("read fd dir")
                .count(),
        )
        .expect("fd count fits u32")
    }

    #[test]
    fn self_count_matches_readdir() {
        let _extra: Vec<File> = (0..3)
            .map(|_| File::open("/dev/null").expect("open /dev/null"))
            .collect();
        let dir = open_dir("/proc/self");
        // Other test threads open and close fds too, so retry until the count is stable.
        for _ in 0..50 {
            let before = readdir_self();
            let got = fd_count(dir.as_fd());
            let after = readdir_self();
            if before != after {
                continue;
            }
            let FdCount::Count(n) = got else {
                panic!("own process must give a count, got {got:?}");
            };
            assert!(
                n.abs_diff(before) <= 1,
                "fd_count {n} vs readdir {before} differ by more than 1"
            );
            return;
        }
        panic!("fd table never stable for 50 tries");
    }

    #[test]
    fn reaped_child_is_gone() {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let dir = open_dir(&format!("/proc/{}", child.id()));
        child.kill().expect("kill own child");
        child.wait().expect("reap own child");
        assert_eq!(fd_count(dir.as_fd()), FdCount::Gone);
    }

    #[test]
    fn errors_map_to_states() {
        assert_eq!(classify(Errno::ACCESS), FdCount::Restricted);
        assert_eq!(classify(Errno::PERM), FdCount::Restricted);
        assert_eq!(classify(Errno::NOENT), FdCount::Gone);
        assert_eq!(classify(Errno::SRCH), FdCount::Gone);
    }
}
