//! Socket inode to process map (SYS-030, PROC-032, ADR-016 "Socket to pid map", U-23).
//!
//! One walk of `/proc/<pid>/fd` per rebuild. Each link `socket:[<inode>]` maps the inode to the
//! process that holds it. A pid whose fd directory cannot be read is recorded as restricted with
//! its owner uid, so downstream shows `Restricted`, never 0. Nothing is cached between rebuilds.

use std::ffi::CStr;
use std::mem::MaybeUninit;

use rustc_hash::FxHashMap;
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{AtFlags, Mode, OFlags, RawDir};
use smallvec::SmallVec;
use tl_model::ids::ProcessKey;
use tl_model::time::{BootId, TimeNs, now_boot_ns};

use crate::CollectError;
use crate::proc::pid_stat::read_start_ticks;
use crate::proc::{is_denied, is_gone};
use crate::sysroot::SysRoot;

/// getdents buffer for `/proc` and for each fd directory, kept between rebuilds.
const DIRENT_BUF: usize = 32 * 1024;
/// `socket:[` plus 20 digits plus `]` is 29 bytes, so 64 always holds a socket target.
const LINK_BUF: usize = 64;

/// Which processes hold each socket inode, built from one walk of `/proc/<pid>/fd`.
pub struct SocketMap {
    /// inode -> every process holding an fd to it, sorted by ProcessKey, no duplicates.
    pub owners: FxHashMap<u64, SmallVec<[ProcessKey; 1]>>,
    /// pids whose /proc/<pid>/fd could not be read (EACCES or EPERM), with their uid from fstat of /proc/<pid>.
    pub restricted: FxHashMap<u32, u32>, // pid -> uid
    pub built_ns: TimeNs, // now_boot_ns() at the start of the walk
    pub elapsed_ns: u64,  // walk duration, for the U-23 budget
    dirents: Vec<MaybeUninit<u8>>,
    /// The socket inodes of the pid being read.
    inodes: Vec<u64>,
    stat_buf: Vec<u8>,
}

impl Default for SocketMap {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SocketMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketMap")
            .field("inodes", &self.owners.len())
            .field("restricted", &self.restricted.len())
            .field("built_ns", &self.built_ns)
            .field("elapsed_ns", &self.elapsed_ns)
            .finish_non_exhaustive()
    }
}

impl SocketMap {
    pub fn new() -> Self {
        Self {
            owners: FxHashMap::default(),
            restricted: FxHashMap::default(),
            built_ns: 0,
            elapsed_ns: 0,
            dirents: vec![MaybeUninit::uninit(); DIRENT_BUF],
            inodes: Vec::new(),
            stat_buf: Vec::new(),
        }
    }

    /// Rebuilds in place, reusing the maps' capacity.
    pub fn rebuild(&mut self, root: &SysRoot, boot: BootId) -> Result<(), CollectError> {
        self.owners.clear();
        self.restricted.clear();
        self.built_ns = now_boot_ns();
        let proc_fd = open_dir(&root.proc, None)?;

        // Process directories are opened while `/proc` is listed, so the fd dirents need a
        // second buffer. It is taken out of `self` for the walk and put back afterwards.
        let mut fd_dirents = std::mem::take(&mut self.dirents);
        let mut proc_dirents = vec![MaybeUninit::<u8>::uninit(); DIRENT_BUF];
        let mut dir = RawDir::new(proc_fd.as_fd(), &mut proc_dirents[..]);
        while let Some(entry) = dir.next() {
            let Ok(entry) = entry else { break };
            let name = entry.file_name();
            let Some(pid) = parse_pid(name.to_bytes()) else {
                continue;
            };
            self.read_pid(proc_fd.as_fd(), name, pid, boot, &mut fd_dirents);
        }
        self.dirents = fd_dirents;
        self.elapsed_ns = now_boot_ns().saturating_sub(self.built_ns);
        Ok(())
    }

    /// Every process holding `inode`, in `ProcessKey` order. Empty when unknown.
    pub fn owners_of(&self, inode: u64) -> &[ProcessKey] {
        self.owners.get(&inode).map_or(&[], |keys| &keys[..])
    }

    /// Reads one pid. A gone process is skipped; a denied fd directory is recorded.
    fn read_pid(
        &mut self,
        proc_fd: BorrowedFd<'_>,
        name: &CStr,
        pid: u32,
        boot: BootId,
        dirents: &mut [MaybeUninit<u8>],
    ) {
        let pid_dir = match open_dir(name, Some(proc_fd)) {
            Ok(dir) => dir,
            Err(e) if is_denied(e) => {
                // No directory fd, so the uid comes from a stat through `/proc`.
                if let Ok(st) = rustix::fs::statat(proc_fd, name, AtFlags::empty()) {
                    self.restricted.insert(pid, st.st_uid);
                }
                return;
            }
            Err(_) => return,
        };
        let fd_dir = match open_dir(c"fd", Some(pid_dir.as_fd())) {
            Ok(dir) => dir,
            Err(e) if is_denied(e) => {
                if let Ok(st) = rustix::fs::fstat(&pid_dir) {
                    self.restricted.insert(pid, st.st_uid);
                }
                return;
            }
            Err(e) => {
                if !is_gone(e) {
                    tracing::trace!(pid, errno = ?e, "fd directory not opened");
                }
                return;
            }
        };

        self.inodes.clear();
        let mut link = [0u8; LINK_BUF];
        let mut dir = RawDir::new(fd_dir.as_fd(), dirents);
        while let Some(entry) = dir.next() {
            // An error here means the process exited during the listing.
            let Ok(entry) = entry else { return };
            let fd_name = entry.file_name();
            if !fd_name.to_bytes().first().is_some_and(u8::is_ascii_digit) {
                continue;
            }
            let Ok(n) = rustix::fs::readlinkat_raw(fd_dir.as_fd(), fd_name, &mut link[..]) else {
                continue;
            };
            if let Some(inode) = parse_socket_link(&link[..n]) {
                self.inodes.push(inode);
            }
        }
        if self.inodes.is_empty() {
            return;
        }

        let Ok(start_ticks) = read_start_ticks(pid_dir.as_fd(), &mut self.stat_buf) else {
            return;
        };
        let key = ProcessKey {
            start_ticks,
            boot_id: boot,
            pid,
        };
        for &inode in &self.inodes {
            tracing::trace!(pid, inode, "socket owner");
            let keys = self.owners.entry(inode).or_default();
            if let Err(at) = keys.binary_search(&key) {
                keys.insert(at, key);
            }
        }
    }
}

fn open_dir<P: rustix::path::Arg>(
    path: P,
    dir: Option<BorrowedFd<'_>>,
) -> Result<OwnedFd, rustix::io::Errno> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    match dir {
        Some(dir) => rustix::fs::openat(dir, path, flags, Mode::empty()),
        None => rustix::fs::open(path, flags, Mode::empty()),
    }
}

/// The inode in `socket:[<digits>]`. Anything else, or a number that does not fit, is `None`.
fn parse_socket_link(target: &[u8]) -> Option<u64> {
    let digits = target.strip_prefix(b"socket:[")?.strip_suffix(b"]")?;
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

/// A directory name made of ASCII digits only that fits a `u32`.
fn parse_pid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    name.iter().try_fold(0u32, |n, &d| {
        n.checked_mul(10)?.checked_add(u32::from(d - b'0'))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const BOOT: BootId = BootId(7);

    fn fixture(name: &str) -> SysRoot {
        SysRoot::at(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
    }

    fn key(pid: u32, start_ticks: u64) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BOOT,
            pid,
        }
    }

    #[test]
    fn fixture_shared_inode() {
        let mut map = SocketMap::new();
        map.rebuild(&fixture("sockets-basic"), BOOT).unwrap();
        assert_eq!(map.owners_of(5001), &[key(100, 5000), key(200, 6000)]);
        assert!(map.owners_of(4).is_empty());
        assert_eq!(map.owners_of(5002), &[key(100, 5000), key(200, 6000)]);
        assert_eq!(map.owners.len(), 2);
        assert!(map.restricted.is_empty());
        assert!(map.built_ns > 0);

        // A second rebuild gives the same map, with no duplicate owners.
        map.rebuild(&fixture("sockets-basic"), BOOT).unwrap();
        assert_eq!(map.owners_of(5001), &[key(100, 5000), key(200, 6000)]);
    }

    #[test]
    fn parse_socket_link() {
        let parse = super::parse_socket_link;
        assert_eq!(parse(b"socket:[5001]"), Some(5001));
        assert_eq!(parse(b"socket:[12x]"), None);
        assert_eq!(parse(b"socket:[]"), None);
        assert_eq!(parse(b"socket:[99999999999999999999]"), None);
        assert_eq!(parse(b"pipe:[1]"), None);
        assert_eq!(parse(b"/dev/null"), None);
    }

    #[test]
    fn vanished_pid_is_skipped() {
        // Pid 300 has a stat file but no `fd` directory, as if it exited after readdir.
        let mut map = SocketMap::new();
        map.rebuild(&fixture("sockets-basic"), BOOT).unwrap();
        assert!(!map.restricted.contains_key(&300));
        assert!(
            map.owners
                .values()
                .all(|keys| keys.iter().all(|k| k.pid != 300))
        );
    }

    #[test]
    fn missing_proc_is_an_error() {
        let mut map = SocketMap::new();
        let root = SysRoot::at(Path::new("/nonexistent-tasklens-root"));
        assert!(map.rebuild(&root, BOOT).is_err());
    }
}
