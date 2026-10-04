//! `ResolveSocketInodes`: maps socket inodes to the processes that hold them (SYS-030, R-004).
//!
//! The engine sends the inodes it could not attribute because it cannot read the
//! `/proc/<pid>/fd` of other users. The helper walks `/proc/*/fd` as root. Only `socket:[n]`
//! targets with `n` in the request leave this module; every other fd target is dropped at once.

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{CWD, Dir, Mode, OFlags};
use tl_model::helper_proto::{HelperError, HelperReplyBody};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

/// Answers one `ResolveSocketInodes` request. Fails only when `/proc` itself cannot be opened.
#[cfg_attr(not(test), expect(dead_code, reason = "dispatch arrives with E29-A1"))]
pub fn resolve_socket_inodes(inodes: &[u64], boot: BootId) -> HelperReplyBody {
    if inodes.is_empty() {
        return HelperReplyBody::SocketOwners(Vec::new());
    }
    let mut wanted = inodes.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    match walk_proc(&wanted, boot) {
        Ok(owners) => HelperReplyBody::SocketOwners(owners),
        Err(e) => HelperReplyBody::Err(HelperError::Os(e.raw_os_error())),
    }
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW);

fn walk_proc(wanted: &[u64], boot: BootId) -> rustix::io::Result<Vec<(u64, ProcessKey)>> {
    let proc_fd = rustix::fs::openat(CWD, "/proc", DIR_FLAGS, Mode::empty())?;
    let mut owners = Vec::new();
    let mut link = Vec::with_capacity(64);
    for entry in Dir::read_from(&proc_fd)? {
        let Ok(entry) = entry else { break };
        let Some(pid) = parse_pid(entry.file_name().to_bytes()) else {
            continue;
        };
        // A pid that vanishes or whose fds we cannot read is skipped.
        let _ = scan_pid(&proc_fd, pid, wanted, boot, &mut link, &mut owners);
    }
    owners.sort_unstable_by_key(|&(inode, key)| (inode, key.pid));
    owners.dedup_by_key(|&mut (inode, key)| (inode, key.pid));
    Ok(owners)
}

/// Adds `(inode, key)` for every fd of `pid` that is a wanted socket.
fn scan_pid(
    proc_fd: &OwnedFd,
    pid: u32,
    wanted: &[u64],
    boot: BootId,
    link: &mut Vec<u8>,
    owners: &mut Vec<(u64, ProcessKey)>,
) -> rustix::io::Result<()> {
    // The dirfd pins this process instance: if the pid is reused, lookups through it fail.
    let pid_fd = rustix::fs::openat(proc_fd, pid.to_string(), DIR_FLAGS, Mode::empty())?;
    let mut fd_dir = Dir::new(rustix::fs::openat(&pid_fd, "fd", DIR_FLAGS, Mode::empty())?)?;
    let mut start_ticks = None;
    while let Some(entry) = fd_dir.read() {
        let Ok(entry) = entry else { break };
        let name = entry.file_name();
        if name.to_bytes().first() == Some(&b'.') {
            continue;
        }
        let Ok(target) = rustix::fs::readlinkat(fd_dir.fd()?, name, std::mem::take(link)) else {
            continue;
        };
        let target = target.into_bytes();
        let inode = parse_socket_target(&target).filter(|n| wanted.binary_search(n).is_ok());
        *link = target;
        link.clear();
        let Some(inode) = inode else { continue };
        let start_ticks = match start_ticks {
            Some(t) => t,
            None => *start_ticks.insert(read_start_ticks(&pid_fd)?),
        };
        owners.push((
            inode,
            ProcessKey {
                start_ticks,
                boot_id: boot,
                pid,
            },
        ));
    }
    Ok(())
}

fn parse_pid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(name).ok()?.parse().ok()
}

/// `socket:[<n>]` gives `n`. Any other target gives `None`.
fn parse_socket_target(target: &[u8]) -> Option<u64> {
    let digits = target.strip_prefix(b"socket:[")?.strip_suffix(b"]")?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

fn read_start_ticks(pid_fd: &OwnedFd) -> rustix::io::Result<u64> {
    let stat_fd = rustix::fs::openat(
        pid_fd,
        "stat",
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    loop {
        let n = rustix::io::read(stat_fd.as_fd(), &mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    parse_start_ticks(&buf).ok_or(rustix::io::Errno::INVAL)
}

/// Field 22 of `/proc/<pid>/stat`. Fields count from after the last `)`, which closes `comm`.
fn parse_start_ticks(stat: &[u8]) -> Option<u64> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    let rest = std::str::from_utf8(&stat[close + 1..]).ok()?;
    // After `)` the first field is field 3 (state), so field 22 is index 19.
    rest.split_ascii_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, UdpSocket};
    use std::process::Command;

    const BOOT: BootId = BootId(7);

    fn inode_of(fd: impl AsFd) -> u64 {
        rustix::fs::fstat(fd).unwrap().st_ino
    }

    fn key_of(pid: u32) -> ProcessKey {
        let stat = std::fs::read(format!("/proc/{pid}/stat")).unwrap();
        ProcessKey {
            start_ticks: parse_start_ticks(&stat).unwrap(),
            boot_id: BOOT,
            pid,
        }
    }

    fn owners(reply: HelperReplyBody) -> Vec<(u64, ProcessKey)> {
        match reply {
            HelperReplyBody::SocketOwners(v) => v,
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[test]
    fn resolves_own_sockets() {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (tcp_ino, udp_ino) = (inode_of(&tcp), inode_of(&udp));
        let me = key_of(std::process::id());

        let got = owners(resolve_socket_inodes(&[udp_ino, tcp_ino], BOOT));

        let mine: Vec<_> = got
            .iter()
            .filter(|(_, k)| k.pid == me.pid)
            .copied()
            .collect();
        let mut want = vec![(tcp_ino, me), (udp_ino, me)];
        want.sort_unstable_by_key(|&(i, _)| i);
        assert_eq!(mine, want);
        assert!(got.is_sorted_by_key(|&(i, k)| (i, k.pid)));
    }

    #[test]
    fn shared_socket_lists_both_pids() {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let ino = inode_of(&tcp);
        // Let the child inherit the socket across exec, like a forked server worker.
        rustix::io::fcntl_setfd(&tcp, rustix::io::FdFlags::empty()).unwrap();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        rustix::io::fcntl_setfd(&tcp, rustix::io::FdFlags::CLOEXEC).unwrap();

        let got = owners(resolve_socket_inodes(&[ino], BOOT));
        let child_key = key_of(child.id());
        child.kill().unwrap();
        child.wait().unwrap();

        let me = key_of(std::process::id());
        assert!(got.iter().all(|&(i, _)| i == ino));
        assert!(got.contains(&(ino, me)), "{got:?}");
        assert!(got.contains(&(ino, child_key)), "{got:?}");
    }

    #[test]
    fn unknown_inode_absent() {
        let got = owners(resolve_socket_inodes(&[1], BOOT));
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn empty_request_no_walk() {
        assert_eq!(
            resolve_socket_inodes(&[], BOOT),
            HelperReplyBody::SocketOwners(Vec::new())
        );
    }

    #[test]
    fn parses_socket_targets_only() {
        assert_eq!(parse_socket_target(b"socket:[12345]"), Some(12345));
        assert_eq!(parse_socket_target(b"socket:[]"), None);
        assert_eq!(parse_socket_target(b"pipe:[12345]"), None);
        assert_eq!(parse_socket_target(b"/home/u/socket:[1]"), None);
    }

    #[test]
    fn start_ticks_skips_parens_in_comm() {
        let stat = b"42 (a) b) c) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 999 20 21\n";
        assert_eq!(parse_start_ticks(stat), Some(999));
    }
}
