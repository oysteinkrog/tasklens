//! `ResolveSocketInodes`: maps socket inodes to the processes that hold them (SYS-030, R-004).
//!
//! The engine sends the inodes it could not attribute because it cannot read the
//! `/proc/<pid>/fd` of other users. The helper walks `/proc/*/fd` as root. Only `socket:[n]`
//! targets with `n` in the request leave this module; every other fd target is dropped at once.
//!
//! `SockDestroy`: closes one TCP connection with a netlink `SOCK_DESTROY` message (SYS-031,
//! R-004). The kernel also matches the socket cookie, so a reused 4-tuple is never hit.
//!
//! `NetnsSockDiagDump`: dumps the TCP and UDP sockets of another network namespace (SYS-037,
//! R-004). Only a short-lived thread enters that namespace, to open the netlink socket. The
//! socket stays in the namespace it was opened in, so the dump runs on the calling thread and
//! that thread never leaves its own namespace.

use std::net::SocketAddr;
use std::time::Duration;

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{CWD, Dir, Mode, OFlags};
use rustix::io::Errno;
use rustix::net::netlink::{self, SocketAddrNetlink};
use rustix::net::sockopt::{self, Timeout};
use rustix::net::{AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType};
use rustix::thread::LinkNameSpaceType;
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

/// `nlmsghdr` (16 bytes) plus `inet_diag_req_v2` (56 bytes).
const DUMP_MSG_LEN: usize = 72;
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLM_F_DUMP: u16 = 0x300;
const NLMSG_DONE: u16 = 3;
const NLMSG_HDR_LEN: usize = 16;
const IPPROTO_UDP: u8 = 17;
const INET_DIAG_INFO: u8 = 2;
/// One `NetlinkDump` body is at most this many bytes, so its frame stays under 64 KiB.
const NETNS_REPLY_MAX: usize = 60 * 1024;
/// The kernel sends dump datagrams of at most 32 KiB.
const RECV_BUF: usize = 64 * 1024;
/// The four dumps, in the order the engine reads them. Each ends with its own `NLMSG_DONE`.
const NETNS_DUMPS: [(u8, u8); 4] = [
    (AF_INET, IPPROTO_TCP),
    (AF_INET6, IPPROTO_TCP),
    (AF_INET, IPPROTO_UDP),
    (AF_INET6, IPPROTO_UDP),
];

/// Why a namespace dump stopped early.
enum DumpStop {
    /// Sent to the engine as the final `Err` reply.
    Fail(HelperError),
    /// `out` failed: the engine is gone, nothing more is sent.
    Out(std::io::Error),
}

impl From<Errno> for DumpStop {
    fn from(e: Errno) -> Self {
        DumpStop::Fail(dump_error(e))
    }
}

fn dump_error(e: Errno) -> HelperError {
    match e {
        Errno::PERM | Errno::ACCESS => HelperError::PermissionDenied,
        // The receive timeout ran out.
        Errno::AGAIN => HelperError::Os(Errno::TIMEDOUT.raw_os_error()),
        e => HelperError::Os(e.raw_os_error()),
    }
}

/// Answers one `NetnsSockDiagDump` request. Streams the raw netlink dump datagrams of the
/// namespace with inode `netns_ino` as `NetlinkDump` bodies with `last: false`, each under
/// 60 KiB, then an empty `NetlinkDump` with `last: true`. A failure is sent as one `Err` body
/// with `last: true`. Returns an error only when `out` fails. Addresses are never logged.
pub fn netns_sock_diag_dump(
    netns_ino: u64,
    out: &mut dyn FnMut(HelperReplyBody, bool) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match dump_netns(netns_ino, out) {
        Ok(()) => out(HelperReplyBody::NetlinkDump(Vec::new()), true),
        Err(DumpStop::Fail(e)) => out(HelperReplyBody::Err(e), true),
        Err(DumpStop::Out(e)) => Err(e),
    }
}

fn dump_netns(
    netns_ino: u64,
    out: &mut dyn FnMut(HelperReplyBody, bool) -> std::io::Result<()>,
) -> Result<(), DumpStop> {
    let pid = find_netns_pid(netns_ino)?.ok_or(DumpStop::Fail(HelperError::NoSuchProcess))?;
    let ns = open_netns(pid, netns_ino).map_err(DumpStop::Fail)?;
    let fd = netns_socket(ns, netns_ino)?;
    sockopt::set_socket_timeout(&fd, Timeout::Recv, Some(ACK_TIMEOUT))?;
    let mut buf = vec![0u8; RECV_BUF];
    // One dump at a time: a netlink socket refuses a second dump while one runs.
    for (seq, (af, proto)) in (1..).zip(NETNS_DUMPS) {
        let msg = build_dump_msg(af, proto, seq);
        rustix::net::sendto(&fd, &msg, SendFlags::empty(), &SocketAddrNetlink::new(0, 0))?;
        loop {
            let (_, n) = rustix::net::recv(&fd, &mut buf, RecvFlags::empty())?;
            if n > buf.len() {
                // The datagram did not fit and its tail is lost.
                return Err(Errno::MSGSIZE.into());
            }
            let (chunks, ended) = split_messages(&buf[..n], NETNS_REPLY_MAX)?;
            for chunk in chunks {
                out(HelperReplyBody::NetlinkDump(chunk.to_vec()), false).map_err(DumpStop::Out)?;
            }
            if ended {
                break;
            }
        }
    }
    Ok(())
}

/// The lowest pid whose `ns/net` link is `net:[ino]`. Pids we cannot read are skipped.
fn find_netns_pid(ino: u64) -> rustix::io::Result<Option<u32>> {
    let proc_fd = rustix::fs::openat(CWD, "/proc", DIR_FLAGS, Mode::empty())?;
    let mut found: Option<u32> = None;
    let mut link = Vec::with_capacity(32);
    for entry in Dir::read_from(&proc_fd)? {
        let Ok(entry) = entry else { break };
        let Some(pid) = parse_pid(entry.file_name().to_bytes()) else {
            continue;
        };
        if found.is_some_and(|f| f < pid) {
            continue;
        }
        let Ok(target) =
            rustix::fs::readlinkat(&proc_fd, format!("{pid}/ns/net"), std::mem::take(&mut link))
        else {
            continue;
        };
        let target = target.into_bytes();
        if parse_net_target(&target) == Some(ino) {
            found = Some(pid);
        }
        link = target;
        link.clear();
    }
    Ok(found)
}

/// `net:[<n>]` gives `n`. Any other target gives `None`.
fn parse_net_target(target: &[u8]) -> Option<u64> {
    let digits = target.strip_prefix(b"net:[")?.strip_suffix(b"]")?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Opens the namespace of `pid` and checks that it is still the one with inode `ino`.
fn open_netns(pid: u32, ino: u64) -> Result<OwnedFd, HelperError> {
    // No NOFOLLOW: `ns/net` is a magic link and opening it is the point.
    let ns = rustix::fs::openat(
        CWD,
        format!("/proc/{pid}/ns/net"),
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| match e {
        Errno::NOENT | Errno::SRCH => HelperError::NoSuchProcess,
        e => dump_error(e),
    })?;
    // The pid may have exited and been reused by a process in another namespace.
    if rustix::fs::fstat(&ns).map_err(dump_error)?.st_ino != ino {
        return Err(HelperError::IdentityChanged);
    }
    Ok(ns)
}

/// Opens a `NETLINK_SOCK_DIAG` socket inside the namespace `ns` (inode `ino`). Only a new
/// thread enters the namespace, and it ends right after, so the caller's thread keeps its own.
fn netns_socket(ns: OwnedFd, ino: u64) -> rustix::io::Result<OwnedFd> {
    let worker = std::thread::Builder::new()
        .name("tl-netns".into())
        .spawn(move || {
            // Entering a network namespace needs CAP_SYS_ADMIN even when it is our own, so
            // skip it then.
            let own = rustix::fs::stat("/proc/thread-self/ns/net")?.st_ino;
            if own != ino {
                rustix::thread::move_into_link_name_space(
                    ns.as_fd(),
                    Some(LinkNameSpaceType::Network),
                )?;
            }
            rustix::net::socket_with(
                AddressFamily::NETLINK,
                SocketType::RAW,
                SocketFlags::CLOEXEC,
                Some(netlink::SOCK_DIAG),
            )
        })
        .map_err(|_| Errno::IO)?;
    worker.join().map_err(|_| Errno::IO)?
}

/// Builds one `SOCK_DIAG_BY_FAMILY` dump request for all states, with `INET_DIAG_INFO`.
fn build_dump_msg(af: u8, proto: u8, seq: u32) -> [u8; DUMP_MSG_LEN] {
    let mut m = [0u8; DUMP_MSG_LEN];
    // nlmsghdr
    m[0..4].copy_from_slice(&(DUMP_MSG_LEN as u32).to_ne_bytes());
    m[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    m[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
    m[8..12].copy_from_slice(&seq.to_ne_bytes());
    // nlmsg_pid stays 0.
    // inet_diag_req_v2
    m[16] = af;
    m[17] = proto;
    m[18] = 1 << (INET_DIAG_INFO - 1);
    // pad stays 0.
    m[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
    // Ports, addresses and interface stay 0; the cookie is INET_DIAG_NOCOOKIE.
    m[64..72].fill(0xFF);
    m
}

/// Splits one datagram into chunks of whole netlink messages, each at most `max` bytes. The
/// second value is true when the dump ended: the last chunk then ends with its `NLMSG_DONE` or
/// `NLMSG_ERROR`, and any bytes after it are dropped. A bad message length gives `PROTO`.
fn split_messages(buf: &[u8], max: usize) -> rustix::io::Result<(Vec<&[u8]>, bool)> {
    let mut chunks = Vec::new();
    let (mut start, mut at) = (0, 0);
    let mut ended = false;
    while at < buf.len() {
        let rest = &buf[at..];
        let len = rest
            .get(0..4)
            .map_or(0, |b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize);
        if len < NLMSG_HDR_LEN || len > rest.len() {
            return Err(Errno::PROTO);
        }
        let end = at + len.next_multiple_of(4).min(rest.len());
        if end - at > max {
            return Err(Errno::PROTO);
        }
        if end - start > max {
            chunks.push(&buf[start..at]);
            start = at;
        }
        at = end;
        let kind = u16::from_ne_bytes([rest[4], rest[5]]);
        if kind == NLMSG_DONE || kind == NLMSG_ERROR {
            ended = true;
            break;
        }
    }
    if at > start {
        chunks.push(&buf[start..at]);
    }
    Ok((chunks, ended))
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

    fn netns_link() -> Vec<u8> {
        std::fs::read_link("/proc/thread-self/ns/net")
            .unwrap()
            .into_os_string()
            .into_encoded_bytes()
    }

    fn run_netns_dump(ino: u64) -> Vec<(HelperReplyBody, bool)> {
        let mut calls = Vec::new();
        netns_sock_diag_dump(ino, &mut |body, last| {
            calls.push((body, last));
            Ok(())
        })
        .unwrap();
        calls
    }

    /// A minimal reader of whole netlink messages: `(type, payload)` for each.
    fn nl_messages(mut bytes: &[u8]) -> Vec<(u16, &[u8])> {
        let mut msgs = Vec::new();
        while !bytes.is_empty() {
            let len = u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as usize;
            let kind = u16::from_ne_bytes(bytes[4..6].try_into().unwrap());
            msgs.push((kind, &bytes[NLMSG_HDR_LEN..len]));
            bytes = bytes.get(len.next_multiple_of(4)..).unwrap_or(&[]);
        }
        msgs
    }

    /// Checks the shape of a successful dump and returns its message bodies.
    fn dump_bodies(calls: &[(HelperReplyBody, bool)]) -> Vec<&[u8]> {
        let (final_call, rest) = calls.split_last().expect("no reply");
        assert_eq!(
            final_call,
            &(HelperReplyBody::NetlinkDump(Vec::new()), true)
        );
        let bodies: Vec<&[u8]> = rest
            .iter()
            .map(|(body, last)| {
                assert!(!last);
                match body {
                    HelperReplyBody::NetlinkDump(b) if !b.is_empty() => b.as_slice(),
                    other => panic!("unexpected reply {other:?}"),
                }
            })
            .collect();
        assert!(bodies.iter().all(|b| b.len() <= NETNS_REPLY_MAX));
        let done = bodies
            .iter()
            .flat_map(|b| nl_messages(b))
            .filter(|&(kind, _)| kind == NLMSG_DONE)
            .count();
        assert_eq!(done, 4, "one NLMSG_DONE per dump");
        bodies
    }

    #[test]
    fn netns_dump_own_namespace() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ino = rustix::fs::stat("/proc/self/ns/net").unwrap().st_ino;

        let calls = run_netns_dump(ino);

        // inet_diag_msg: family, state, timer, retrans, then sport big-endian.
        const TCP_LISTEN: u8 = 10;
        let found = dump_bodies(&calls)
            .iter()
            .flat_map(|b| nl_messages(b))
            .any(|(kind, p)| {
                kind == SOCK_DIAG_BY_FAMILY
                    && p[0] == AF_INET
                    && p[1] == TCP_LISTEN
                    && u16::from_be_bytes([p[4], p[5]]) == port
            });
        assert!(found, "listener on port {port} not in the dump");
    }

    #[test]
    fn netns_unknown_inode() {
        assert_eq!(
            run_netns_dump(1),
            vec![(HelperReplyBody::Err(HelperError::NoSuchProcess), true)]
        );
    }

    #[test]
    fn netns_main_thread_unchanged() {
        // Security regression: the namespace switch stays on the short-lived worker thread.
        let before = netns_link();
        let ino = rustix::fs::stat("/proc/self/ns/net").unwrap().st_ino;
        run_netns_dump(ino);
        run_netns_dump(1);
        assert_eq!(netns_link(), before);
    }

    #[test]
    fn netns_dump_other_namespace_as_root() {
        if !rustix::process::geteuid().is_root() {
            eprintln!("skipped: entering another network namespace needs root");
            return;
        }
        let before = netns_link();
        let own = rustix::fs::stat("/proc/self/ns/net").unwrap().st_ino;
        let mut child = Command::new("unshare")
            .args(["--net", "sleep", "30"])
            .spawn()
            .unwrap();
        let path = format!("/proc/{}/ns/net", child.id());
        // `unshare` enters the new namespace just after it starts.
        let mut ino = own;
        for _ in 0..200 {
            ino = rustix::fs::stat(path.as_str()).unwrap().st_ino;
            if ino != own {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let calls = run_netns_dump(ino);
        child.kill().unwrap();
        child.wait().unwrap();

        assert_ne!(ino, own, "the child never left our namespace");
        dump_bodies(&calls);
        assert_eq!(netns_link(), before);
    }

    #[test]
    fn dump_msg_bytes() {
        let got = build_dump_msg(AF_INET6, IPPROTO_UDP, 4);
        #[rustfmt::skip]
        let want: [u8; 72] = [
            // nlmsghdr: len 72, type 20, flags 0x301, seq 4, pid 0
            72, 0, 0, 0, 20, 0, 0x01, 0x03, 4, 0, 0, 0, 0, 0, 0, 0,
            // family 10, protocol 17, ext INET_DIAG_INFO bit, pad 0, states 0xFFFF_FFFF
            10, 17, 2, 0, 0xFF, 0xFF, 0xFF, 0xFF,
            // ports, addresses and interface 0
            0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
            // cookie INET_DIAG_NOCOOKIE
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ];
        assert_eq!(got, want);
    }

    fn nlmsg(kind: u16, len: u32) -> Vec<u8> {
        let mut m = vec![0u8; (len as usize).next_multiple_of(4)];
        m[0..4].copy_from_slice(&len.to_ne_bytes());
        m[4..6].copy_from_slice(&kind.to_ne_bytes());
        m
    }

    #[test]
    fn split_keeps_whole_messages() {
        let msg = nlmsg(SOCK_DIAG_BY_FAMILY, 20);
        let three = [msg.clone(), msg.clone(), msg.clone()].concat();
        let (chunks, ended) = split_messages(&three, 40).unwrap();
        assert_eq!(chunks, vec![&three[..40], &three[40..]]);
        assert!(!ended);

        // DONE ends the dump and what follows it is dropped.
        let done = nlmsg(NLMSG_DONE, 20);
        let buf = [msg.clone(), done, msg.clone()].concat();
        let (chunks, ended) = split_messages(&buf, NETNS_REPLY_MAX).unwrap();
        assert_eq!(chunks, vec![&buf[..40]]);
        assert!(ended);

        // An unpadded last message ends at the buffer end.
        let short = nlmsg(SOCK_DIAG_BY_FAMILY, 18);
        let (chunks, _) = split_messages(&short[..18], NETNS_REPLY_MAX).unwrap();
        assert_eq!(chunks, vec![&short[..18]]);

        assert_eq!(split_messages(&nlmsg(20, 8), 1024), Err(Errno::PROTO));
        assert_eq!(split_messages(&msg[..12], 1024), Err(Errno::PROTO));
        let mut long = msg.clone();
        long[0..4].copy_from_slice(&64u32.to_ne_bytes());
        assert_eq!(split_messages(&long, 1024), Err(Errno::PROTO));
    }

    #[test]
    fn parses_net_targets_only() {
        assert_eq!(parse_net_target(b"net:[4026531840]"), Some(4026531840));
        assert_eq!(parse_net_target(b"net:[]"), None);
        assert_eq!(parse_net_target(b"socket:[4026531840]"), None);
    }
}
