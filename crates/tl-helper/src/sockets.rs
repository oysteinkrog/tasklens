//! `ResolveSocketInodes`: maps socket inodes to the processes that hold them (SYS-030, R-004).
//!
//! The engine sends the inodes it could not attribute because it cannot read the
//! `/proc/<pid>/fd` of other users. The helper walks `/proc/*/fd` as root. Only `socket:[n]`
//! targets with `n` in the request leave this module; every other fd target is dropped at once.
//!
//! `SockDestroy`: closes one TCP connection with a netlink `SOCK_DESTROY` message (SYS-031,
//! R-004). The kernel also matches the socket cookie, so a reused 4-tuple is never hit.

use std::net::SocketAddr;
use std::time::Duration;

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{CWD, Dir, Mode, OFlags};
use rustix::io::Errno;
use rustix::net::netlink::{self, SocketAddrNetlink};
use rustix::net::sockopt::{self, Timeout};
use rustix::net::{AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType};
use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::helper_proto::{HelperError, HelperReplyBody};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

/// Answers one `ResolveSocketInodes` request. Fails only when `/proc` itself cannot be opened.
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

/// `nlmsghdr` (16 bytes) plus `inet_diag_req_v2` (56 bytes).
const DESTROY_MSG_LEN: usize = 72;
const SOCK_DESTROY: u16 = 21;
const NLM_F_REQUEST: u16 = 0x0001;
const NLM_F_ACK: u16 = 0x0004;
const NLMSG_ERROR: u16 = 2;
const DESTROY_SEQ: u32 = 1;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;
const IPPROTO_TCP: u8 = 6;
const ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// Answers one `SockDestroy` request. Opens one netlink socket and closes it on return.
pub fn sock_destroy(family: Family, proto: Proto, t: &FourTuple, cookie: u64) -> HelperReplyBody {
    // Rule "only tcp": the kernel can destroy UDP sockets too, but the helper never does.
    if proto != Proto::Tcp {
        return HelperReplyBody::Err(HelperError::NotAllowed);
    }
    // Rule "family matches addresses": both ends must be of the requested family.
    let v6 = family == Family::Inet6;
    if t.local.is_ipv6() != v6 || t.remote.is_ipv6() != v6 {
        return HelperReplyBody::Err(HelperError::NotAllowed);
    }
    let msg = build_destroy_msg(family, t, cookie);
    match send_destroy(&msg) {
        Ok(0) => HelperReplyBody::Ok,
        Ok(errno) if errno == Errno::NOENT.raw_os_error() => {
            HelperReplyBody::Err(HelperError::NoSuchProcess)
        }
        Ok(errno) => HelperReplyBody::Err(HelperError::Os(errno)),
        Err(e) if e == Errno::AGAIN => {
            HelperReplyBody::Err(HelperError::Os(Errno::TIMEDOUT.raw_os_error()))
        }
        Err(e) => HelperReplyBody::Err(HelperError::Os(e.raw_os_error())),
    }
}

/// Sends `msg` to the kernel and returns the positive errno of its `NLMSG_ERROR` ack, 0 for
/// success.
fn send_destroy(msg: &[u8; DESTROY_MSG_LEN]) -> rustix::io::Result<i32> {
    let fd = rustix::net::socket_with(
        AddressFamily::NETLINK,
        SocketType::RAW,
        SocketFlags::CLOEXEC,
        Some(netlink::SOCK_DIAG),
    )?;
    sockopt::set_socket_timeout(&fd, Timeout::Recv, Some(ACK_TIMEOUT))?;
    rustix::net::sendto(&fd, msg, SendFlags::empty(), &SocketAddrNetlink::new(0, 0))?;
    // An error ack echoes our 72-byte request after its own 20 bytes.
    let mut buf = [0u8; 256];
    let (_, n) = rustix::net::recv(&fd, &mut buf, RecvFlags::empty())?;
    parse_ack(&buf[..n.min(buf.len())]).ok_or(Errno::PROTO)
}

/// Reads the errno from an `NLMSG_ERROR` ack to our request. `None` for anything else.
fn parse_ack(buf: &[u8]) -> Option<i32> {
    let u16_at = |at: usize| Some(u16::from_ne_bytes(buf.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_ne_bytes(buf.get(at..at + 4)?.try_into().ok()?));
    if u16_at(4)? != NLMSG_ERROR || u32_at(8)? != DESTROY_SEQ {
        return None;
    }
    let error = i32::from_ne_bytes(u32_at(16)?.to_ne_bytes());
    error.checked_neg()
}

/// Builds the `SOCK_DESTROY` request. Netlink fields are in host order; ports and addresses are
/// in network order.
fn build_destroy_msg(family: Family, t: &FourTuple, cookie: u64) -> [u8; DESTROY_MSG_LEN] {
    let mut m = [0u8; DESTROY_MSG_LEN];
    // nlmsghdr
    m[0..4].copy_from_slice(&(DESTROY_MSG_LEN as u32).to_ne_bytes());
    m[4..6].copy_from_slice(&SOCK_DESTROY.to_ne_bytes());
    m[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    m[8..12].copy_from_slice(&DESTROY_SEQ.to_ne_bytes());
    // nlmsg_pid stays 0.
    // inet_diag_req_v2
    m[16] = match family {
        Family::Inet => AF_INET,
        Family::Inet6 => AF_INET6,
    };
    m[17] = IPPROTO_TCP;
    // idiag_ext and pad stay 0.
    m[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
    // inet_diag_sockid: src and sport are the local end.
    m[24..26].copy_from_slice(&t.local.port().to_be_bytes());
    m[26..28].copy_from_slice(&t.remote.port().to_be_bytes());
    write_addr(&mut m[28..44], t.local);
    write_addr(&mut m[44..60], t.remote);
    // A link-local socket is bound to its interface and is found only with that index (R-270).
    let ifindex = scope_id(t.local)
        .or_else(|| scope_id(t.remote))
        .unwrap_or(0);
    m[60..64].copy_from_slice(&ifindex.to_ne_bytes());
    m[64..68].copy_from_slice(&(cookie as u32).to_ne_bytes());
    m[68..72].copy_from_slice(&((cookie >> 32) as u32).to_ne_bytes());
    m
}

/// IPv4 fills the first word of `out`; IPv6 fills all four.
fn write_addr(out: &mut [u8], addr: SocketAddr) {
    match addr {
        SocketAddr::V4(a) => out[..4].copy_from_slice(&a.ip().octets()),
        SocketAddr::V6(a) => out.copy_from_slice(&a.ip().octets()),
    }
}

fn scope_id(addr: SocketAddr) -> Option<u32> {
    match addr {
        SocketAddr::V6(a) if a.scope_id() != 0 => Some(a.scope_id()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv6Addr, SocketAddrV6, TcpListener, TcpStream, UdpSocket};
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

    fn tuple(local: &str, remote: &str) -> FourTuple {
        FourTuple {
            local: local.parse().unwrap(),
            remote: remote.parse().unwrap(),
        }
    }

    #[test]
    fn destroy_msg_v4_bytes() {
        let t = tuple("192.168.1.5:40000", "1.2.3.4:443");
        let got = build_destroy_msg(Family::Inet, &t, 0x0000_0001_0000_00AB);
        #[rustfmt::skip]
        let want: [u8; 72] = [
            // nlmsghdr: len 72, type 21, flags 5, seq 1, pid 0
            72, 0, 0, 0, 21, 0, 5, 0, 1, 0, 0, 0, 0, 0, 0, 0,
            // family 2, protocol 6, ext 0, pad 0, states 0xFFFF_FFFF
            2, 6, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF,
            // sport 40000, dport 443, big-endian
            0x9C, 0x40, 0x01, 0xBB,
            // src 192.168.1.5
            192, 168, 1, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            // dst 1.2.3.4
            1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            // if 0
            0, 0, 0, 0,
            // cookie words 0xAB and 1
            0xAB, 0, 0, 0, 1, 0, 0, 0,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn destroy_msg_v6_bytes() {
        let t = tuple("[::1]:40000", "[::1]:443");
        let got = build_destroy_msg(Family::Inet6, &t, 7);
        assert_eq!(got[16], 10);
        assert_eq!(got[17], 6);
        let one = Ipv6Addr::LOCALHOST.octets();
        assert_eq!(got[28..44], one);
        assert_eq!(got[44..60], one);
        assert_eq!(got[60..64], [0; 4]);
        assert_eq!(got[64..72], [7, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn destroy_msg_v6_link_local_uses_scope_id() {
        let ip: Ipv6Addr = "fe80::1".parse().unwrap();
        let t = FourTuple {
            local: SocketAddrV6::new(ip, 40000, 0, 3).into(),
            remote: SocketAddrV6::new(ip, 443, 0, 3).into(),
        };
        let got = build_destroy_msg(Family::Inet6, &t, 7);
        assert_eq!(got[60..64], 3u32.to_ne_bytes());
    }

    #[test]
    fn destroy_udp_refused() {
        // Security regression: only TCP. The refusal happens before any socket is opened.
        let t = tuple("127.0.0.1:40000", "127.0.0.1:443");
        assert_eq!(
            sock_destroy(Family::Inet, Proto::Udp, &t, 1),
            HelperReplyBody::Err(HelperError::NotAllowed)
        );
    }

    #[test]
    fn destroy_family_mismatch_refused() {
        let t = tuple("127.0.0.1:40000", "127.0.0.1:443");
        assert_eq!(
            sock_destroy(Family::Inet6, Proto::Tcp, &t, 1),
            HelperReplyBody::Err(HelperError::NotAllowed)
        );
    }

    #[test]
    fn ack_parse() {
        let mut ack = [0u8; 36];
        ack[4..6].copy_from_slice(&NLMSG_ERROR.to_ne_bytes());
        ack[8..12].copy_from_slice(&DESTROY_SEQ.to_ne_bytes());
        assert_eq!(parse_ack(&ack), Some(0));
        ack[16..20].copy_from_slice(&(-95i32).to_ne_bytes());
        assert_eq!(parse_ack(&ack), Some(95));
        ack[8..12].copy_from_slice(&2u32.to_ne_bytes());
        assert_eq!(parse_ack(&ack), None);
        assert_eq!(parse_ack(&ack[..10]), None);
    }

    #[test]
    fn destroy_without_cap_fails() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: runs as root; the root case is in real_helper_root.rs");
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_server, _) = listener.accept().unwrap();
        let t = FourTuple {
            local: client.local_addr().unwrap(),
            remote: client.peer_addr().unwrap(),
        };
        let cookie = sockopt::socket_cookie(&client).unwrap();

        let got = sock_destroy(Family::Inet, Proto::Tcp, &t, cookie);

        assert!(
            matches!(
                got,
                HelperReplyBody::Err(HelperError::PermissionDenied | HelperError::Os(1))
            ),
            "{got:?}"
        );
    }
}
