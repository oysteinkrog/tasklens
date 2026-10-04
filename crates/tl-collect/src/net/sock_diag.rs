//! `NETLINK_SOCK_DIAG` socket records and the parser for dump replies (SYS-028, PROC-032).
//!
//! Parsing is pure: bytes in, records out. The caller owns the socket and reuses `out` between
//! dumps, so the parser allocates nothing except pushes to `out`.

use std::net::IpAddr;

use netlink_packet_core::{NLA_TYPE_MASK, NLMSG_DONE, NLMSG_ERROR, Parseable};
use netlink_packet_sock_diag::{
    AF_INET, AF_INET6, INET_DIAG_INFO, SOCK_DIAG_BY_FAMILY, inet::InetResponseHeader,
};

use crate::error::CollectError;

/// Netlink message header length (`struct nlmsghdr`).
const NLMSG_HDR_LEN: usize = 16;
/// Fixed length of `struct inet_diag_msg`.
const INET_DIAG_MSG_LEN: usize = 72;
/// Netlink attribute header length (`struct nlattr`).
const NLA_HDR_LEN: usize = 4;

// Offsets in `struct tcp_info` (include/uapi/linux/tcp.h).
const TCPI_LAST_DATA_RECV: usize = 52;
const TCPI_RTT: usize = 68;
const TCPI_RTTVAR: usize = 72;
const TCPI_SND_CWND: usize = 80;
const TCPI_TOTAL_RETRANS: usize = 100;
const TCPI_BYTES_ACKED: usize = 120;
const TCPI_BYTES_RECEIVED: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    fn af(self) -> u8 {
        match self {
            Family::V4 => AF_INET,
            Family::V6 => AF_INET6,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

/// The `struct tcp_info` fields tasklens uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TcpInfoLite {
    /// `tcpi_bytes_acked`: bytes sent and acknowledged.
    pub bytes_acked: u64,
    /// `tcpi_bytes_received`.
    pub bytes_received: u64,
    /// `tcpi_rtt`.
    pub rtt_us: u32,
    /// `tcpi_rttvar`.
    pub rttvar_us: u32,
    /// `tcpi_snd_cwnd`.
    pub snd_cwnd: u32,
    /// `tcpi_total_retrans`.
    pub total_retrans: u32,
    /// `tcpi_last_data_recv`.
    pub last_data_recv_ms: u32,
}

/// One socket from a sock_diag dump.
#[derive(Clone, Debug, PartialEq)]
pub struct SockRecord {
    pub family: Family,
    pub proto: Proto,
    /// Kernel TCP state number (1 ESTABLISHED to 12 NEW_SYN_RECV). For UDP, 7 CLOSE means
    /// unconnected and 1 ESTABLISHED means connected.
    pub state: u8,
    /// `id.idiag_src` (the first 4 bytes for V4).
    pub src: IpAddr,
    /// `id.idiag_dst`.
    pub dst: IpAddr,
    /// Host byte order.
    pub sport: u16,
    pub dport: u16,
    /// `id.idiag_if`.
    pub ifindex: u32,
    /// `id.idiag_cookie`, the two u32 words as `(hi << 32) | lo`.
    pub cookie: u64,
    pub rqueue: u32,
    pub wqueue: u32,
    pub uid: u32,
    /// 0 for TIME-WAIT and orphaned sockets.
    pub inode: u64,
    /// Only for TCP when `INET_DIAG_INFO` was present and long enough.
    pub tcp_info: Option<TcpInfoLite>,
}

/// Appends every record in one or more netlink messages in `buf` to `out`.
/// Returns Ok(true) when an NLMSG_DONE was seen, Ok(false) when more messages are expected.
pub fn parse_dump(
    buf: &[u8],
    family: Family,
    proto: Proto,
    out: &mut Vec<SockRecord>,
) -> Result<bool, CollectError> {
    let mut offset = 0;
    while offset < buf.len() {
        let rest = &buf[offset..];
        let len = match read_u32(rest, 0) {
            Some(len) => len as usize,
            None => return Err(CollectError::NetlinkMalformed("short netlink header")),
        };
        if len < NLMSG_HDR_LEN || len > rest.len() {
            return Err(CollectError::NetlinkMalformed("short netlink header"));
        }
        let msg_type = read_u16(rest, 4).unwrap_or(0);
        let payload = &rest[NLMSG_HDR_LEN..len];
        match msg_type {
            NLMSG_DONE => return Ok(true),
            NLMSG_ERROR => {
                let error = read_i32(payload, 0)
                    .ok_or(CollectError::NetlinkMalformed("short error message"))?;
                if error < 0 {
                    return Err(CollectError::SockDiagErrno(error.saturating_neg()));
                }
            }
            SOCK_DIAG_BY_FAMILY => out.push(parse_inet_diag(payload, family, proto)?),
            _ => {}
        }
        offset = offset.saturating_add(align4(len));
    }
    Ok(false)
}

/// Parses one `inet_diag_msg` and its attributes.
fn parse_inet_diag(
    payload: &[u8],
    family: Family,
    proto: Proto,
) -> Result<SockRecord, CollectError> {
    if payload.len() < INET_DIAG_MSG_LEN {
        return Err(CollectError::NetlinkMalformed("short inet_diag_msg"));
    }
    // Check the length and family first so the crate parser never reaches its error paths,
    // which allocate.
    if payload[0] != family.af() {
        return Err(CollectError::NetlinkMalformed("unexpected address family"));
    }
    let header = InetResponseHeader::parse(&payload[..INET_DIAG_MSG_LEN])
        .map_err(|_| CollectError::NetlinkMalformed("bad inet_diag_msg"))?;
    let id = &header.socket_id;
    let cookie_lo = u32::from_ne_bytes([id.cookie[0], id.cookie[1], id.cookie[2], id.cookie[3]]);
    let cookie_hi = u32::from_ne_bytes([id.cookie[4], id.cookie[5], id.cookie[6], id.cookie[7]]);

    let mut tcp_info = None;
    let mut attrs = &payload[INET_DIAG_MSG_LEN..];
    while !attrs.is_empty() {
        let (Some(nla_len), Some(nla_type)) = (read_u16(attrs, 0), read_u16(attrs, 2)) else {
            return Err(CollectError::NetlinkMalformed("truncated attribute"));
        };
        let nla_len = nla_len as usize;
        if nla_len < NLA_HDR_LEN || nla_len > attrs.len() {
            return Err(CollectError::NetlinkMalformed("truncated attribute"));
        }
        if proto == Proto::Tcp && nla_type & NLA_TYPE_MASK == INET_DIAG_INFO {
            tcp_info = parse_tcp_info(&attrs[NLA_HDR_LEN..nla_len]);
        }
        attrs = attrs.get(align4(nla_len)..).unwrap_or(&[]);
    }

    Ok(SockRecord {
        family,
        proto,
        state: header.state,
        src: id.source_address,
        dst: id.destination_address,
        sport: id.source_port,
        dport: id.destination_port,
        ifindex: id.interface_id,
        cookie: (u64::from(cookie_hi) << 32) | u64::from(cookie_lo),
        rqueue: header.recv_queue,
        wqueue: header.send_queue,
        uid: header.uid,
        inode: u64::from(header.inode),
        tcp_info,
    })
}

/// Reads the used `tcp_info` fields. A payload from an old kernel that ends before
/// `tcpi_bytes_received` gives None.
fn parse_tcp_info(p: &[u8]) -> Option<TcpInfoLite> {
    Some(TcpInfoLite {
        bytes_acked: read_u64(p, TCPI_BYTES_ACKED)?,
        bytes_received: read_u64(p, TCPI_BYTES_RECEIVED)?,
        rtt_us: read_u32(p, TCPI_RTT)?,
        rttvar_us: read_u32(p, TCPI_RTTVAR)?,
        snd_cwnd: read_u32(p, TCPI_SND_CWND)?,
        total_retrans: read_u32(p, TCPI_TOTAL_RETRANS)?,
        last_data_recv_ms: read_u32(p, TCPI_LAST_DATA_RECV)?,
    })
}

/// SYS-029 label for a kernel TCP state number.
pub fn tcp_state_label(state: u8) -> &'static str {
    match state {
        1 => "ESTABLISHED",
        2 => "SYN-SENT",
        3 | 12 => "SYN-RECEIVED",
        4 => "FIN-WAIT-1",
        5 => "FIN-WAIT-2",
        6 => "TIME-WAIT",
        7 => "CLOSED",
        8 => "CLOSE-WAIT",
        9 => "LAST-ACK",
        10 => "LISTENING",
        11 => "CLOSING",
        _ => "UNKNOWN",
    }
}

/// SYS-029 label for a UDP socket state.
pub fn udp_state_label(state: u8) -> &'static str {
    match state {
        1 => "CONNECTED",
        _ => "UNCONNECTED",
    }
}

fn align4(len: usize) -> usize {
    len.saturating_add(3) & !3
}

fn read_u16(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_ne_bytes([s[0], s[1]]))
}

fn read_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_i32(b: &[u8], at: usize) -> Option<i32> {
    read_u32(b, at).map(|v| v as i32)
}

fn read_u64(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    let mut w = [0u8; 8];
    w.copy_from_slice(s);
    Some(u64::from_ne_bytes(w))
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use netlink_packet_core::{Emitable, NetlinkHeader};
    use netlink_packet_sock_diag::inet::SocketId;

    use super::*;

    /// One netlink message of `msg_type` around `payload`, padded to 4 bytes.
    fn msg(msg_type: u16, payload: &[u8]) -> Vec<u8> {
        let mut header = NetlinkHeader::default();
        header.length = (NLMSG_HDR_LEN + payload.len()) as u32;
        header.message_type = msg_type;
        let mut out = vec![0u8; align4(NLMSG_HDR_LEN + payload.len())];
        header.emit(&mut out[..NLMSG_HDR_LEN]);
        out[NLMSG_HDR_LEN..NLMSG_HDR_LEN + payload.len()].copy_from_slice(payload);
        out
    }

    fn done() -> Vec<u8> {
        msg(NLMSG_DONE, &0i32.to_ne_bytes())
    }

    fn error(errno: i32) -> Vec<u8> {
        // nlmsgerr: error code, then the request header.
        let mut p = errno.to_ne_bytes().to_vec();
        p.extend_from_slice(&[0u8; NLMSG_HDR_LEN]);
        msg(NLMSG_ERROR, &p)
    }

    fn header(af: u8, state: u8, id: SocketId, inode: u32) -> Vec<u8> {
        let h = InetResponseHeader {
            family: af,
            state,
            timer: None,
            socket_id: id,
            recv_queue: 3,
            send_queue: 4,
            uid: 1000,
            inode,
        };
        let mut p = vec![0u8; h.buffer_len()];
        h.emit(&mut p);
        p
    }

    fn attr(nla_type: u16, value: &[u8]) -> Vec<u8> {
        let len = NLA_HDR_LEN + value.len();
        let mut a = vec![0u8; align4(len)];
        a[0..2].copy_from_slice(&(len as u16).to_ne_bytes());
        a[2..4].copy_from_slice(&nla_type.to_ne_bytes());
        a[NLA_HDR_LEN..len].copy_from_slice(value);
        a
    }

    fn tcp_info_bytes(len: usize, acked: u64, received: u64) -> Vec<u8> {
        let mut t = vec![0u8; len];
        if len >= TCPI_BYTES_RECEIVED + 8 {
            t[TCPI_BYTES_ACKED..TCPI_BYTES_ACKED + 8].copy_from_slice(&acked.to_ne_bytes());
            t[TCPI_BYTES_RECEIVED..TCPI_BYTES_RECEIVED + 8]
                .copy_from_slice(&received.to_ne_bytes());
            t[TCPI_RTT..TCPI_RTT + 4].copy_from_slice(&250u32.to_ne_bytes());
        }
        t
    }

    fn v6_id() -> SocketId {
        let mut id = SocketId::new_v6();
        id.source_address = IpAddr::V6(Ipv6Addr::LOCALHOST);
        id.source_port = 443;
        id.destination_address = IpAddr::V6(Ipv6Addr::LOCALHOST);
        id.destination_port = 50000;
        id
    }

    #[test]
    fn parse_tcp4_listener() {
        let mut id = SocketId::new_v4();
        id.source_address = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        id.source_port = 631;
        id.cookie[..4].copy_from_slice(&7u32.to_ne_bytes());
        id.cookie[4..].copy_from_slice(&1u32.to_ne_bytes());
        let mut buf = msg(SOCK_DIAG_BY_FAMILY, &header(AF_INET, 10, id, 4242));
        buf.extend(done());

        let mut out = Vec::new();
        assert!(parse_dump(&buf, Family::V4, Proto::Tcp, &mut out).unwrap());
        assert_eq!(out.len(), 1);
        let r = &out[0];
        assert_eq!(r.family, Family::V4);
        assert_eq!(r.proto, Proto::Tcp);
        assert_eq!(r.state, 10);
        assert_eq!(r.src, IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(r.sport, 631);
        assert_eq!(r.dst, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(r.dport, 0);
        assert_eq!(r.inode, 4242);
        assert_eq!(r.cookie, (1u64 << 32) | 7);
        assert_eq!((r.rqueue, r.wqueue, r.uid), (3, 4, 1000));
        assert_eq!(r.tcp_info, None);
    }

    #[test]
    fn parse_tcp6_with_info() {
        let mut p = header(AF_INET6, 1, v6_id(), 99);
        p.extend(attr(1, &[1, 2, 3])); // INET_DIAG_MEMINFO-like attribute, skipped
        p.extend(attr(INET_DIAG_INFO, &tcp_info_bytes(232, 1000, 2000)));
        let buf = msg(SOCK_DIAG_BY_FAMILY, &p);

        let mut out = Vec::new();
        assert!(!parse_dump(&buf, Family::V6, Proto::Tcp, &mut out).unwrap());
        let info = out[0].tcp_info.expect("tcp_info");
        assert_eq!(info.bytes_acked, 1000);
        assert_eq!(info.bytes_received, 2000);
        assert_eq!(info.rtt_us, 250);
        assert_eq!(out[0].sport, 443);
        assert_eq!(out[0].dport, 50000);
    }

    #[test]
    fn short_tcp_info_is_none() {
        let mut p = header(AF_INET6, 1, v6_id(), 99);
        p.extend(attr(
            INET_DIAG_INFO,
            &tcp_info_bytes(TCPI_BYTES_RECEIVED, 1, 2),
        ));
        let buf = msg(SOCK_DIAG_BY_FAMILY, &p);

        let mut out = Vec::new();
        parse_dump(&buf, Family::V6, Proto::Tcp, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tcp_info, None);
    }

    #[test]
    fn nlmsg_error_maps_errno() {
        let mut out = Vec::new();
        let err = parse_dump(&error(-13), Family::V4, Proto::Tcp, &mut out).unwrap_err();
        assert!(matches!(err, CollectError::SockDiagErrno(13)));

        let mut ack = error(0);
        ack.extend(done());
        assert!(parse_dump(&ack, Family::V4, Proto::Tcp, &mut out).unwrap());
    }

    #[test]
    fn overlong_header_is_malformed() {
        let mut buf = msg(
            SOCK_DIAG_BY_FAMILY,
            &header(AF_INET, 10, SocketId::new_v4(), 1),
        );
        let too_long = (buf.len() + 8) as u32;
        buf[0..4].copy_from_slice(&too_long.to_ne_bytes());
        let mut out = Vec::new();
        let err = parse_dump(&buf, Family::V4, Proto::Tcp, &mut out).unwrap_err();
        assert!(matches!(err, CollectError::NetlinkMalformed(_)));
    }

    #[test]
    fn truncated_attribute_is_malformed() {
        let mut p = header(AF_INET, 1, SocketId::new_v4(), 1);
        p.extend_from_slice(&200u16.to_ne_bytes());
        p.extend_from_slice(&INET_DIAG_INFO.to_ne_bytes());
        let buf = msg(SOCK_DIAG_BY_FAMILY, &p);
        let mut out = Vec::new();
        let err = parse_dump(&buf, Family::V4, Proto::Tcp, &mut out).unwrap_err();
        assert!(matches!(
            err,
            CollectError::NetlinkMalformed("truncated attribute")
        ));
    }

    #[test]
    fn state_labels_match_sys029() {
        let tcp = [
            (10, "LISTENING"),
            (2, "SYN-SENT"),
            (3, "SYN-RECEIVED"),
            (12, "SYN-RECEIVED"),
            (1, "ESTABLISHED"),
            (4, "FIN-WAIT-1"),
            (5, "FIN-WAIT-2"),
            (8, "CLOSE-WAIT"),
            (11, "CLOSING"),
            (9, "LAST-ACK"),
            (6, "TIME-WAIT"),
            (7, "CLOSED"),
            (0, "UNKNOWN"),
            (99, "UNKNOWN"),
        ];
        for (state, label) in tcp {
            assert_eq!(tcp_state_label(state), label, "state {state}");
        }
        assert_eq!(udp_state_label(1), "CONNECTED");
        assert_eq!(udp_state_label(7), "UNCONNECTED");
        assert_eq!(udp_state_label(0), "UNCONNECTED");
    }
}
