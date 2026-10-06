//! Network namespaces other than tasklens's own (SYS-037, R-004, ADR-016).
//!
//! One walk of `/proc/<pid>/ns/net` per request. Each link `net:[<ino>]` groups the pid under that
//! namespace. The dump inside each namespace runs in tl-helper; this module only finds the
//! namespaces and names them. Runs only when the Connections tab shows "Namespaces: All" (P4).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags};

use crate::CollectError;
use crate::sysroot::SysRoot;

/// `net:[` plus 20 digits plus `]` is 26 bytes, so 64 always holds a namespace target.
const LINK_BUF: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct NetnsInfo {
    pub ino: u64,          // inode of the net namespace
    pub name: Arc<str>,    // display name: run_netns file, else cgroup leaf, else `net:[<ino>]`
    pub member_pid: u32,   // lowest readable pid in it
    pub member_count: u32, // readable pids in it
}

/// Every net namespace seen through /proc/<pid>/ns/net, except tasklens's own (/proc/self/ns/net).
/// Sorted by name, then ino.
///
/// Only failure to read `root.proc` is returned. A pid whose link cannot be read is skipped.
pub fn list_netns(root: &SysRoot, run_netns: &Path) -> Result<Vec<NetnsInfo>, CollectError> {
    let host_ino = read_ns_link(&root.proc.join("self/ns/net"));

    // ino -> (lowest pid, member count)
    let mut members: BTreeMap<u64, (u32, u32)> = BTreeMap::new();
    let proc_fd = open_dir(&root.proc, None)?;
    for entry in std::fs::read_dir(&root.proc)? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = parse_pid(entry.file_name().as_encoded_bytes()) else {
            continue;
        };
        let Some(ino) = read_pid_ns(proc_fd.as_fd(), &entry.file_name()) else {
            continue;
        };
        if Some(ino) == host_ino {
            continue;
        }
        let slot = members.entry(ino).or_insert((pid, 0));
        slot.0 = slot.0.min(pid);
        slot.1 += 1;
    }
    if members.is_empty() {
        return Ok(Vec::new());
    }

    let named = run_netns_names(run_netns);
    let mut out: Vec<NetnsInfo> = members
        .into_iter()
        .map(|(ino, (member_pid, member_count))| {
            let name = named
                .get(&ino)
                .cloned()
                .or_else(|| cgroup_leaf(&root.proc, member_pid))
                .unwrap_or_else(|| format!("net:[{ino}]").into());
            tracing::debug!(ino, %name, member_pid, member_count, "net namespace");
            NetnsInfo {
                ino,
                name,
                member_pid,
                member_count,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.ino.cmp(&b.ino)));
    Ok(out)
}

/// The namespace inode of one pid, or `None` when its link cannot be read (EACCES, EPERM, ENOENT
/// or any other error) or is not `net:[<ino>]`.
fn read_pid_ns(proc_fd: BorrowedFd<'_>, pid_name: &OsStr) -> Option<u64> {
    let pid_dir = open_dir(pid_name, Some(proc_fd)).ok()?;
    let mut link = [0u8; LINK_BUF];
    let n = rustix::fs::readlinkat_raw(pid_dir.as_fd(), c"ns/net", &mut link[..]).ok()?;
    parse_ns_link(&link[..n])
}

fn read_ns_link(path: &Path) -> Option<u64> {
    let target = std::fs::read_link(path).ok()?;
    parse_ns_link(target.as_os_str().as_encoded_bytes())
}

/// File name in `run_netns` for each namespace inode. A missing directory gives an empty map.
fn run_netns_names(run_netns: &Path) -> BTreeMap<u64, Arc<str>> {
    let mut names = BTreeMap::new();
    let Ok(dir) = std::fs::read_dir(run_netns) else {
        return names;
    };
    for entry in dir.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some(ino) = ino_of(&entry.path()) {
            names.entry(ino).or_insert_with(|| Arc::from(name));
        }
    }
    names
}

/// The inode of a `/run/netns` entry, which is a bind mount of the namespace file.
#[cfg(not(test))]
fn ino_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.ino())
}

/// Fixture files cannot carry namespace inodes, so tests read them from `<dir>/.inodes`, one
/// `<file name> <ino>` per line.
#[cfg(test)]
fn ino_of(path: &Path) -> Option<u64> {
    let file = path.file_name()?.to_str()?;
    let table = std::fs::read_to_string(path.parent()?.join(".inodes")).ok()?;
    table.lines().find_map(|line| {
        let (name, ino) = line.split_once(' ')?;
        (name == file).then(|| ino.trim().parse().ok())?
    })
}

/// The last component of the cgroup v2 path (the `0::` line) of `pid`, such as
/// `docker-3f2a.scope`. `None` when unreadable, absent, or the root cgroup.
fn cgroup_leaf(proc: &Path, pid: u32) -> Option<Arc<str>> {
    let text = std::fs::read_to_string(proc.join(pid.to_string()).join("cgroup")).ok()?;
    let path = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    let leaf = path.trim_end().rsplit('/').next()?;
    (!leaf.is_empty()).then(|| Arc::from(leaf))
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

/// The inode in `net:[<digits>]`. Anything else, or a number that does not fit, is `None`.
fn parse_ns_link(target: &[u8]) -> Option<u64> {
    let digits = target.strip_prefix(b"net:[")?.strip_suffix(b"]")?;
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
    use std::path::PathBuf;

    use super::*;

    fn case(name: &str) -> (SysRoot, PathBuf) {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/netns-basic")
            .join(name);
        (SysRoot::at(&base), base.join("run/netns"))
    }

    fn list(name: &str) -> Vec<NetnsInfo> {
        let (root, run_netns) = case(name);
        list_netns(&root, &run_netns).unwrap()
    }

    #[test]
    fn named_by_run_netns() {
        assert_eq!(
            list("run-netns"),
            vec![NetnsInfo {
                ino: 4026532001,
                name: "blue".into(),
                member_pid: 20,
                member_count: 1,
            }]
        );
    }

    #[test]
    fn named_by_cgroup() {
        let got = list("cgroup");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ino, 4026532002);
        assert_eq!(&*got[0].name, "docker-3f2a.scope");
        assert_eq!(got[0].member_pid, 30);
    }

    #[test]
    fn named_by_inode() {
        let got = list("inode");
        assert_eq!(got.len(), 1);
        assert_eq!(&*got[0].name, "net:[4026532003]");
        assert_eq!(got[0].member_pid, 40);
    }

    #[test]
    fn host_never_listed() {
        for name in ["run-netns", "cgroup", "inode"] {
            assert!(list(name).iter().all(|ns| ns.ino != 4026531840), "{name}");
        }
    }

    #[test]
    fn missing_proc_is_an_error() {
        let root = SysRoot::at(Path::new("/nonexistent-tasklens-netns"));
        assert!(list_netns(&root, Path::new("/nonexistent")).is_err());
    }

    #[test]
    fn ns_link_parse() {
        assert_eq!(parse_ns_link(b"net:[4026531840]"), Some(4026531840));
        assert_eq!(parse_ns_link(b"net:[]"), None);
        assert_eq!(parse_ns_link(b"mnt:[1]"), None);
        assert_eq!(parse_ns_link(b"net:[99999999999999999999999]"), None);
    }
}
