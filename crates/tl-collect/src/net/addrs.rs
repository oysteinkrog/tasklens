//! Interface IP addresses from an rtnetlink `RTM_GETADDR` dump (PERF-059, PERF-104, R-052).
//!
//! Parsing is pure: bytes in, addresses out, with bounds checks only. `AddrDump` owns the
//! socket and one 64 KiB receive buffer, kept between dumps. The dump only reads; it never
//! sends a request that changes an address, link or route.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use netlink_sys::{Socket, protocols::NETLINK_ROUTE};
use rustix::io::Errno;

use crate::error::CollectError;

/// Netlink message header length (`struct nlmsghdr`).
const NLMSG_HDR_LEN: usize = 16;
/// Length of `struct ifaddrmsg`.
const IFADDRMSG_LEN: usize = 8;
/// Netlink attribute header length (`struct rtattr`).
const RTA_HDR_LEN: usize = 4;
/// A whole request: netlink header plus `ifaddrmsg`.
const REQUEST_LEN: usize = NLMSG_HDR_LEN + IFADDRMSG_LEN;
/// Receive buffer capacity, kept between dumps.
const RECV_BUF_LEN: usize = 64 * 1024;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const RTM_NEWADDR: u16 = 20;
const RTM_GETADDR: u16 = 22;
/// `NLM_F_REQUEST | NLM_F_DUMP`.
const NLM_F_REQUEST_DUMP: u16 = 0x1 | 0x300;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
/// Masks `NLA_F_NESTED` and `NLA_F_NET_BYTEORDER` off an attribute type.
const NLA_TYPE_MASK: u16 = !(0x8000 | 0x4000);

const MALFORMED: CollectError = CollectError::Parse {
    file: "rtnetlink",
    what: "length",
};

/// One address of one interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfAddr {
    pub ifindex: u32,
    pub family: AddrFamily,
    pub addr: IpAddr,
    pub prefix_len: u8,
    pub scope: AddrScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrFamily {
    V4,
    V6,
}

/// `ifa_scope`: 0 Global, 200 Site, 253 Link, 254 Host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrScope {
    Global,
    Site,
    Link,
    Host,
    Other(u8),
}

impl AddrScope {
    fn from_byte(b: u8) -> Self {
        match b {
            0 => AddrScope::Global,
            200 => AddrScope::Site,
            253 => AddrScope::Link,
            254 => AddrScope::Host,
            other => AddrScope::Other(other),
        }
    }
}

/// A `NETLINK_ROUTE` socket with a reused 64 KiB receive buffer.
pub struct AddrDump {
    sock: Socket,
    buf: Vec<u8>,
    seq: u32,
}

impl AddrDump {
    /// Opens a NETLINK_ROUTE (protocol 0) socket with SOCK_CLOEXEC, bound with port id 0.
    pub fn open() -> Result<Self, CollectError> {
        // `Socket::new` passes SOCK_CLOEXEC; `bind_auto` binds port id 0 so the kernel picks one.
        let mut sock = Socket::new(NETLINK_ROUTE)?;
        sock.bind_auto()?;
        Ok(Self {
            sock,
            buf: Vec::with_capacity(RECV_BUF_LEN),
            seq: 0,
        })
    }

    /// One `AF_UNSPEC` dump of every address. Clears `out` first.
    pub fn dump(&mut self, out: &mut Vec<IfAddr>) -> Result<(), CollectError> {
        out.clear();
        self.seq = self.seq.wrapping_add(1);
        self.sock.send(&build_request(self.seq), 0)?;
        loop {
            self.buf.clear();
            match self.sock.recv(&mut self.buf, 0) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
            // Every message in one datagram carries the sequence number of its request.
            if read_u32(&self.buf, 8) != Some(self.seq) {
                tracing::trace!("rtnetlink: skipped reply with a stale sequence number");
                continue;
            }
            if parse_addr_dump(&self.buf, out)? {
                return Ok(());
            }
        }
    }
}

/// An `RTM_GETADDR` dump request: `nlmsghdr` then a zeroed `ifaddrmsg` (family `AF_UNSPEC`).
fn build_request(seq: u32) -> [u8; REQUEST_LEN] {
    let mut r = [0u8; REQUEST_LEN];
    r[0..4].copy_from_slice(&(REQUEST_LEN as u32).to_ne_bytes());
    r[4..6].copy_from_slice(&RTM_GETADDR.to_ne_bytes());
    r[6..8].copy_from_slice(&NLM_F_REQUEST_DUMP.to_ne_bytes());
    r[8..12].copy_from_slice(&seq.to_ne_bytes());
    // r[12..16]: nlmsg_pid 0, the kernel. r[16..24]: ifaddrmsg, all zero.
    r
}

/// Parses one RTM_NEWADDR dump reply buffer (one or more nlmsghdr). Ok(true) when NLMSG_DONE was seen.
pub fn parse_addr_dump(buf: &[u8], out: &mut Vec<IfAddr>) -> Result<bool, CollectError> {
    let mut offset = 0;
    while offset < buf.len() {
        let rest = &buf[offset..];
        let len = read_u32(rest, 0).ok_or(MALFORMED)? as usize;
        if len < NLMSG_HDR_LEN || len > rest.len() {
            return Err(MALFORMED);
        }
        let msg_type = read_u16(rest, 4).unwrap_or(0);
        let payload = &rest[NLMSG_HDR_LEN..len];
        match msg_type {
            NLMSG_DONE => return Ok(true),
            NLMSG_ERROR => {
                let error = read_u32(payload, 0).ok_or(MALFORMED)? as i32;
                if error < 0 {
                    return Err(Errno::from_raw_os_error(error.saturating_neg()).into());
                }
            }
            RTM_NEWADDR => {
                if let Some(a) = parse_ifaddr(payload)? {
                    out.push(a);
                }
            }
            _ => {}
        }
        offset = offset.saturating_add(align4(len));
    }
    Ok(false)
}

/// One `ifaddrmsg` and its attributes. `None` for a family other than IPv4 and IPv6, or when
/// neither `IFA_LOCAL` nor `IFA_ADDRESS` holds an address of the right length.
fn parse_ifaddr(payload: &[u8]) -> Result<Option<IfAddr>, CollectError> {
    if payload.len() < IFADDRMSG_LEN {
        return Err(MALFORMED);
    }
    let family = match payload[0] {
        AF_INET => AddrFamily::V4,
        AF_INET6 => AddrFamily::V6,
        _ => return Ok(None),
    };
    let prefix_len = payload[1];
    // payload[2]: ifa_flags.
    let scope = AddrScope::from_byte(payload[3]);
    let ifindex = read_u32(payload, 4).ok_or(MALFORMED)?;

    let mut address = None;
    let mut local = None;
    let mut attrs = &payload[IFADDRMSG_LEN..];
    while !attrs.is_empty() {
        let (Some(rta_len), Some(rta_type)) = (read_u16(attrs, 0), read_u16(attrs, 2)) else {
            return Err(MALFORMED);
        };
        let rta_len = rta_len as usize;
        if rta_len < RTA_HDR_LEN || rta_len > attrs.len() {
            return Err(MALFORMED);
        }
        let value = &attrs[RTA_HDR_LEN..rta_len];
        match rta_type & NLA_TYPE_MASK {
            IFA_ADDRESS => address = ip(family, value),
            IFA_LOCAL => local = ip(family, value),
            _ => {}
        }
        attrs = attrs.get(align4(rta_len)..).unwrap_or(&[]);
    }

    // On a point-to-point link IFA_ADDRESS is the peer and IFA_LOCAL is this side.
    Ok(local.or(address).map(|addr| IfAddr {
        ifindex,
        family,
        addr,
        prefix_len,
        scope,
    }))
}

fn ip(family: AddrFamily, value: &[u8]) -> Option<IpAddr> {
    match family {
        AddrFamily::V4 => {
            let b: [u8; 4] = value.try_into().ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(b)))
        }
        AddrFamily::V6 => {
            let b: [u8; 16] = value.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(b)))
        }
    }
}

/// "192.168.1.10/24", "fe80::1/64" (std Display of the address plus "/prefix").
pub fn format_cidr(a: &IfAddr) -> String {
    format!("{}/{}", a.addr, a.prefix_len)
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// One netlink message of `msg_type` around `payload`, padded to 4 bytes.
    fn msg(msg_type: u16, payload: &[u8]) -> Vec<u8> {
        let len = NLMSG_HDR_LEN + payload.len();
        let mut out = vec![0u8; align4(len)];
        out[0..4].copy_from_slice(&(len as u32).to_ne_bytes());
        out[4..6].copy_from_slice(&msg_type.to_ne_bytes());
        out[NLMSG_HDR_LEN..len].copy_from_slice(payload);
        out
    }

    fn done() -> Vec<u8> {
        msg(NLMSG_DONE, &0i32.to_ne_bytes())
    }

    fn attr(rta_type: u16, value: &[u8]) -> Vec<u8> {
        let len = RTA_HDR_LEN + value.len();
        let mut a = vec![0u8; align4(len)];
        a[0..2].copy_from_slice(&(len as u16).to_ne_bytes());
        a[2..4].copy_from_slice(&rta_type.to_ne_bytes());
        a[RTA_HDR_LEN..len].copy_from_slice(value);
        a
    }

    /// An `RTM_NEWADDR` message with an `ifaddrmsg` and the given attributes.
    fn newaddr(af: u8, prefix: u8, scope: u8, ifindex: u32, attrs: &[Vec<u8>]) -> Vec<u8> {
        let mut p = vec![af, prefix, 0, scope];
        p.extend_from_slice(&ifindex.to_ne_bytes());
        for a in attrs {
            p.extend_from_slice(a);
        }
        msg(RTM_NEWADDR, &p)
    }

    fn v6(s: &str) -> Ipv6Addr {
        s.parse().unwrap()
    }

    #[test]
    fn parse_three_addresses() {
        let mut buf = newaddr(AF_INET, 24, 0, 2, &[attr(IFA_ADDRESS, &[192, 168, 1, 10])]);
        buf.extend(newaddr(
            AF_INET6,
            64,
            0,
            2,
            &[attr(IFA_ADDRESS, &v6("2001:db8::5").octets())],
        ));
        buf.extend(newaddr(
            AF_INET6,
            64,
            253,
            2,
            &[attr(IFA_ADDRESS, &v6("fe80::1").octets())],
        ));
        buf.extend(done());

        let mut out = Vec::new();
        assert!(parse_addr_dump(&buf, &mut out).unwrap());
        assert_eq!(
            out,
            [
                IfAddr {
                    ifindex: 2,
                    family: AddrFamily::V4,
                    addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                    prefix_len: 24,
                    scope: AddrScope::Global,
                },
                IfAddr {
                    ifindex: 2,
                    family: AddrFamily::V6,
                    addr: IpAddr::V6(v6("2001:db8::5")),
                    prefix_len: 64,
                    scope: AddrScope::Global,
                },
                IfAddr {
                    ifindex: 2,
                    family: AddrFamily::V6,
                    addr: IpAddr::V6(v6("fe80::1")),
                    prefix_len: 64,
                    scope: AddrScope::Link,
                },
            ]
        );
        let cidrs: Vec<String> = out.iter().map(format_cidr).collect();
        assert_eq!(cidrs, ["192.168.1.10/24", "2001:db8::5/64", "fe80::1/64"]);
    }

    #[test]
    fn local_wins_for_ptp() {
        let buf = newaddr(
            AF_INET,
            32,
            0,
            5,
            &[
                attr(IFA_ADDRESS, &[10, 0, 0, 2]),
                attr(IFA_LOCAL, &[10, 0, 0, 1]),
            ],
        );
        let mut out = Vec::new();
        assert!(!parse_addr_dump(&buf, &mut out).unwrap());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].addr, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn other_family_and_short_address_are_skipped() {
        // AF_PACKET (17) and an IPv4 address attribute of the wrong length.
        let mut buf = newaddr(17, 0, 0, 1, &[attr(IFA_ADDRESS, &[1, 2, 3, 4])]);
        buf.extend(newaddr(AF_INET, 8, 0, 1, &[attr(IFA_ADDRESS, &[1, 2, 3])]));
        let mut out = Vec::new();
        assert!(!parse_addr_dump(&buf, &mut out).unwrap());
        assert!(out.is_empty());
    }

    #[test]
    fn bad_lengths_are_parse_errors() {
        let mut out = Vec::new();
        let mut buf = done();
        let too_long = (buf.len() + 8) as u32;
        buf[0..4].copy_from_slice(&too_long.to_ne_bytes());
        assert!(matches!(
            parse_addr_dump(&buf, &mut out),
            Err(CollectError::Parse {
                file: "rtnetlink",
                what: "length"
            })
        ));

        let mut p = vec![AF_INET, 24, 0, 0];
        p.extend_from_slice(&2u32.to_ne_bytes());
        p.extend_from_slice(&200u16.to_ne_bytes());
        p.extend_from_slice(&IFA_ADDRESS.to_ne_bytes());
        assert!(matches!(
            parse_addr_dump(&msg(RTM_NEWADDR, &p), &mut out),
            Err(CollectError::Parse { .. })
        ));
        assert!(matches!(
            parse_addr_dump(&msg(RTM_NEWADDR, &[AF_INET, 24]), &mut out),
            Err(CollectError::Parse { .. })
        ));
    }

    #[test]
    fn nlmsg_error_maps_errno() {
        let mut p = (-1i32).to_ne_bytes().to_vec();
        p.extend_from_slice(&[0u8; NLMSG_HDR_LEN]);
        let mut out = Vec::new();
        assert!(matches!(
            parse_addr_dump(&msg(NLMSG_ERROR, &p), &mut out),
            Err(CollectError::PermissionDenied)
        ));
    }

    #[test]
    fn request_layout() {
        let r = build_request(9);
        assert_eq!(read_u32(&r, 0), Some(24));
        assert_eq!(read_u16(&r, 4), Some(22));
        assert_eq!(read_u16(&r, 6), Some(0x301));
        assert_eq!(read_u32(&r, 8), Some(9));
        assert!(r[12..].iter().all(|&b| b == 0));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn prop_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=512)) {
            let mut out = Vec::new();
            let _ = parse_addr_dump(&bytes, &mut out);
        }
    }

    #[test]
    #[ignore = "real system: opens a NETLINK_ROUTE socket"]
    fn real_loopback() {
        let mut dump = AddrDump::open().unwrap();
        let mut out = Vec::new();
        dump.dump(&mut out).unwrap();
        let lo = out
            .iter()
            .find(|a| a.addr == IpAddr::V4(Ipv4Addr::LOCALHOST))
            .expect("127.0.0.1 listed");
        assert_eq!(format_cidr(lo), "127.0.0.1/8");
        assert_eq!(lo.scope, AddrScope::Host);

        // A second dump clears `out` and reuses the buffer.
        let first = out.len();
        dump.dump(&mut out).unwrap();
        assert_eq!(out.len(), first);
        assert_eq!(dump.buf.capacity(), RECV_BUF_LEN);
    }
}
