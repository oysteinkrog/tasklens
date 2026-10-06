//! Wi-Fi SSID, frequency, signal and bit rates from nl80211 over generic netlink (PERF-058, R-052).
//!
//! NetworkManager has no signal in dBm and no receive bit rate, and some systems run without it.
//! `NL80211_CMD_GET_INTERFACE` gives the SSID and frequency of each Wi-Fi interface, and
//! `NL80211_CMD_GET_STATION` gives the signal and bit rates of the access point a station-mode
//! interface is associated with. Both are dumps that need no privileges and change nothing.
//!
//! Parsing is pure: bytes in, values out, with a bounds check on every length. A message with a
//! bad length or a truncated attribute gives no values. SSIDs are never logged.

use std::io::{self, ErrorKind};
use std::time::Duration;

use netlink_sys::{Socket, protocols::NETLINK_GENERIC};
use rustix::io::Errno;
use rustix::net::sockopt::{Timeout, set_socket_timeout};

/// Netlink message header length (`struct nlmsghdr`).
const NLMSG_HDR_LEN: usize = 16;
/// Generic netlink header length (`struct genlmsghdr`).
const GENL_HDR_LEN: usize = 4;
/// Netlink attribute header length (`struct nlattr`).
const NLA_HDR_LEN: usize = 4;
/// Receive buffer capacity, kept between dumps.
const RECV_BUF_LEN: usize = 64 * 1024;
/// A dump that has not answered within this time fails with `WouldBlock`.
const RECV_TIMEOUT: Duration = Duration::from_secs(1);

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
/// `NLM_F_ROOT | NLM_F_MATCH`.
const NLM_F_DUMP: u16 = 0x300;
/// Masks `NLA_F_NESTED` and `NLA_F_NET_BYTEORDER` off an attribute type.
const NLA_TYPE_MASK: u16 = !(0x8000 | 0x4000);

/// The generic netlink controller (`GENL_ID_CTRL`).
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const FAMILY_NAME: &[u8] = b"nl80211\0";

const NL80211_CMD_GET_INTERFACE: u8 = 5;
const NL80211_CMD_GET_STATION: u8 = 17;
const NL80211_ATTR_IFINDEX: u16 = 3;
const NL80211_ATTR_IFTYPE: u16 = 5;
const NL80211_ATTR_STA_INFO: u16 = 21;
const NL80211_ATTR_WIPHY_FREQ: u16 = 38;
const NL80211_ATTR_SSID: u16 = 52;
const NL80211_IFTYPE_STATION: u32 = 2;
const NL80211_STA_INFO_SIGNAL: u16 = 7;
const NL80211_STA_INFO_TX_BITRATE: u16 = 8;
const NL80211_STA_INFO_RX_BITRATE: u16 = 14;
/// u16, units of 100 kbit/s.
const NL80211_RATE_INFO_BITRATE: u16 = 1;
/// u32, units of 100 kbit/s. Preferred when present.
const NL80211_RATE_INFO_BITRATE32: u16 = 5;
/// An SSID is at most 32 bytes (IEEE 802.11).
const SSID_MAX: usize = 32;

/// One Wi-Fi interface. `None` fields are not reported by the kernel, for example the signal of
/// an interface that is not associated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiLink {
    pub ifindex: u32,
    pub ssid: Option<Vec<u8>>,
    pub freq_mhz: Option<u32>,
    pub signal_dbm: Option<i32>,
    pub rx_kbps: Option<u32>,
    pub tx_kbps: Option<u32>,
}

/// A `NETLINK_GENERIC` socket bound to the nl80211 family, with a reused 64 KiB receive buffer.
pub struct Nl80211 {
    sock: Socket,
    family: u16,
    seq: u32,
    buf: Vec<u8>,
}

impl Nl80211 {
    /// Resolves the "nl80211" family with CTRL_CMD_GETFAMILY. `Ok(None)` when the family does not exist (no Wi-Fi driver loaded).
    pub fn open() -> io::Result<Option<Self>> {
        // `Socket::new` passes SOCK_CLOEXEC; `bind_auto` binds port id 0 so the kernel picks one.
        let mut sock = Socket::new(NETLINK_GENERIC)?;
        sock.bind_auto()?;
        set_socket_timeout(&sock, Timeout::Recv, Some(RECV_TIMEOUT))?;
        let mut nl = Self {
            sock,
            family: GENL_ID_CTRL,
            seq: 0,
            buf: Vec::with_capacity(RECV_BUF_LEN),
        };
        let mut family = None;
        let name = attr(CTRL_ATTR_FAMILY_NAME, FAMILY_NAME);
        match nl.exchange(CTRL_CMD_GETFAMILY, NLM_F_REQUEST, &name, |msg| {
            family = family.or_else(|| parse_family(msg));
        }) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(Errno::NOENT.raw_os_error()) => return Ok(None),
            Err(e) => return Err(e),
        }
        match family {
            Some(id) => {
                nl.family = id;
                Ok(Some(nl))
            }
            None => Err(io::Error::new(
                ErrorKind::InvalidData,
                "no family id in the CTRL_CMD_GETFAMILY reply",
            )),
        }
    }

    /// One GET_INTERFACE dump, then one GET_STATION dump per station-mode interface. 64 KiB receive buffer, 1 s socket timeout.
    pub fn links(&mut self) -> io::Result<Vec<WifiLink>> {
        let mut ifaces = Vec::new();
        self.exchange(
            NL80211_CMD_GET_INTERFACE,
            NLM_F_REQUEST | NLM_F_DUMP,
            &[],
            |msg| ifaces.extend(parse_interface(msg)),
        )?;
        let mut out = Vec::with_capacity(ifaces.len());
        for (ifindex, iftype, ssid, freq_mhz) in ifaces {
            let mut link = WifiLink {
                ifindex,
                ssid,
                freq_mhz,
                signal_dbm: None,
                rx_kbps: None,
                tx_kbps: None,
            };
            if iftype == NL80211_IFTYPE_STATION {
                // A station-mode interface has at most one station: its access point.
                let mut station = None;
                let index = attr(NL80211_ATTR_IFINDEX, &ifindex.to_ne_bytes());
                match self.exchange(
                    NL80211_CMD_GET_STATION,
                    NLM_F_REQUEST | NLM_F_DUMP,
                    &index,
                    |msg| station = station.or(Some(parse_station(msg))),
                ) {
                    Ok(()) => {}
                    // The interface went away between the two dumps.
                    Err(e) if e.raw_os_error() == Some(Errno::NODEV.raw_os_error()) => {}
                    Err(e) => return Err(e),
                }
                if let Some((signal_dbm, rx_kbps, tx_kbps)) = station {
                    link.signal_dbm = signal_dbm;
                    link.rx_kbps = rx_kbps;
                    link.tx_kbps = tx_kbps;
                }
            }
            out.push(link);
        }
        Ok(out)
    }

    /// Sends one request to the current family and hands each reply message of that family to
    /// `on_msg`, whole (netlink header included). A dump ends at `NLMSG_DONE`, a plain request
    /// after the first reply datagram. A negative `NLMSG_ERROR` is returned as its errno.
    fn exchange(
        &mut self,
        cmd: u8,
        flags: u16,
        attrs: &[u8],
        mut on_msg: impl FnMut(&[u8]),
    ) -> io::Result<()> {
        self.seq = self.seq.wrapping_add(1);
        self.sock
            .send(&request(self.family, flags, self.seq, cmd, attrs), 0)?;
        let dump = flags & NLM_F_DUMP != 0;
        loop {
            self.buf.clear();
            match self.sock.recv(&mut self.buf, 0) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            // Every message in one datagram carries the sequence number of its request.
            if read_u32(&self.buf, 8) != Some(self.seq) {
                tracing::trace!("nl80211: skipped reply with a stale sequence number");
                continue;
            }
            let mut rest = &self.buf[..];
            while let Some(len) = msg_len(rest) {
                let msg = &rest[..len];
                match read_u16(msg, 4) {
                    Some(NLMSG_DONE) => return Ok(()),
                    Some(NLMSG_ERROR) => {
                        let error = read_u32(msg, NLMSG_HDR_LEN).map_or(0, |e| e as i32);
                        if error < 0 {
                            return Err(io::Error::from_raw_os_error(error.saturating_neg()));
                        }
                        // 0 is an acknowledgement.
                    }
                    Some(t) if t == self.family => on_msg(msg),
                    _ => {}
                }
                rest = rest.get(align4(len)..).unwrap_or(&[]);
            }
            if !dump {
                return Ok(());
            }
        }
    }
}

/// A request: `nlmsghdr`, `genlmsghdr` (version 0) and the attributes.
fn request(family: u16, flags: u16, seq: u32, cmd: u8, attrs: &[u8]) -> Vec<u8> {
    let len = NLMSG_HDR_LEN + GENL_HDR_LEN + attrs.len();
    let mut r = Vec::with_capacity(len);
    r.extend_from_slice(&(len as u32).to_ne_bytes());
    r.extend_from_slice(&family.to_ne_bytes());
    r.extend_from_slice(&flags.to_ne_bytes());
    r.extend_from_slice(&seq.to_ne_bytes());
    // nlmsg_pid 0: the kernel.
    r.extend_from_slice(&0u32.to_ne_bytes());
    r.extend_from_slice(&[cmd, 0, 0, 0]);
    r.extend_from_slice(attrs);
    r
}

/// One attribute, padded to 4 bytes.
fn attr(nla_type: u16, value: &[u8]) -> Vec<u8> {
    let len = NLA_HDR_LEN + value.len();
    let mut a = vec![0u8; align4(len)];
    a[0..2].copy_from_slice(&(len as u16).to_ne_bytes());
    a[2..4].copy_from_slice(&nla_type.to_ne_bytes());
    a[NLA_HDR_LEN..len].copy_from_slice(value);
    a
}

/// The length of the first message in `b`. `None` when it is shorter than a header or longer
/// than the bytes left.
fn msg_len(b: &[u8]) -> Option<usize> {
    let len = read_u32(b, 0)? as usize;
    (NLMSG_HDR_LEN..=b.len()).contains(&len).then_some(len)
}

/// The attributes of one generic netlink message (after both headers). `None` for a bad length.
fn genl_attrs(msg: &[u8]) -> Option<&[u8]> {
    let len = msg_len(msg)?;
    msg.get(NLMSG_HDR_LEN + GENL_HDR_LEN..len)
}

/// Calls `f` with the type (flags masked off) and value of each attribute. `None` when an
/// attribute header or value runs past the end.
fn for_each_attr<'a>(mut b: &'a [u8], mut f: impl FnMut(u16, &'a [u8])) -> Option<()> {
    while !b.is_empty() {
        let len = read_u16(b, 0)? as usize;
        let nla_type = read_u16(b, 2)?;
        if len < NLA_HDR_LEN {
            return None;
        }
        f(nla_type & NLA_TYPE_MASK, b.get(NLA_HDR_LEN..len)?);
        // The last attribute may lack its padding.
        b = b.get(align4(len)..).unwrap_or(&[]);
    }
    Some(())
}

/// `CTRL_ATTR_FAMILY_ID` of a `CTRL_CMD_NEWFAMILY` reply.
fn parse_family(msg: &[u8]) -> Option<u16> {
    let mut id = None;
    for_each_attr(genl_attrs(msg)?, |t, v| {
        if t == CTRL_ATTR_FAMILY_ID {
            id = read_u16(v, 0);
        }
    })?;
    id
}

/// One `NL80211_CMD_NEW_INTERFACE` message: (ifindex, iftype, SSID, frequency in MHz). `None`
/// for a bad length, a truncated attribute, or no ifindex or iftype.
pub fn parse_interface(msg: &[u8]) -> Option<(u32, u32, Option<Vec<u8>>, Option<u32>)> {
    let (mut ifindex, mut iftype, mut ssid, mut freq) = (None, None, None, None);
    for_each_attr(genl_attrs(msg)?, |t, v| match t {
        NL80211_ATTR_IFINDEX => ifindex = read_u32(v, 0),
        NL80211_ATTR_IFTYPE => iftype = read_u32(v, 0),
        NL80211_ATTR_WIPHY_FREQ => freq = read_u32(v, 0),
        NL80211_ATTR_SSID if v.len() <= SSID_MAX => ssid = Some(v.to_vec()),
        _ => {}
    })?;
    Some((ifindex?, iftype?, ssid, freq))
}

/// One `NL80211_CMD_NEW_STATION` message: (signal in dBm, RX kbit/s, TX kbit/s). All `None` for
/// a bad length or a truncated attribute.
pub fn parse_station(msg: &[u8]) -> (Option<i32>, Option<u32>, Option<u32>) {
    station(msg).unwrap_or((None, None, None))
}

fn station(msg: &[u8]) -> Option<(Option<i32>, Option<u32>, Option<u32>)> {
    let mut info = None;
    for_each_attr(genl_attrs(msg)?, |t, v| {
        if t == NL80211_ATTR_STA_INFO {
            info = Some(v);
        }
    })?;
    let (mut signal, mut rx, mut tx) = (None, None, None);
    let mut truncated = false;
    for_each_attr(info.unwrap_or(&[]), |t, v| match t {
        NL80211_STA_INFO_SIGNAL => signal = v.first().map(|&b| i32::from(b as i8)),
        NL80211_STA_INFO_RX_BITRATE => match rate_kbps(v) {
            Some(r) => rx = r,
            None => truncated = true,
        },
        NL80211_STA_INFO_TX_BITRATE => match rate_kbps(v) {
            Some(r) => tx = r,
            None => truncated = true,
        },
        _ => {}
    })?;
    (!truncated).then_some((signal, rx, tx))
}

/// A nested `nl80211_rate_info` in kbit/s: `BITRATE32`, else `BITRATE`. `Some(None)` when
/// neither is there, `None` when the nest is truncated.
fn rate_kbps(nest: &[u8]) -> Option<Option<u32>> {
    let (mut r32, mut r16) = (None, None);
    for_each_attr(nest, |t, v| match t {
        NL80211_RATE_INFO_BITRATE32 => r32 = read_u32(v, 0),
        NL80211_RATE_INFO_BITRATE => r16 = read_u16(v, 0).map(u32::from),
        _ => {}
    })?;
    Some(r32.or(r16).map(|r| r.saturating_mul(100)))
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

    /// Hex to bytes; whitespace is ignored.
    fn hex(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }

    /// A `NEW_INTERFACE` reply (little endian, family id 0x1c): ifindex 3, ifname "wlan0",
    /// iftype 2, wiphy freq 5180, SSID "Home". Built by hand from `linux/nl80211.h`.
    const INTERFACE_VECTOR: &str = "
        40000000 1c00 0200 07000000 00000000
        07 01 0000
        0800 0300 03000000
        0a00 0400 776c616e3000 0000
        0800 0500 02000000
        0800 2600 3c140000
        0800 3400 486f6d65
    ";

    /// A `NEW_STATION` reply: MAC, then `STA_INFO` with signal 0xBA (-70 dBm), TX `BITRATE32`
    /// 8660 and `BITRATE` 8660 (u16), RX `BITRATE` 4000 only.
    const STATION_VECTOR: &str = "
        54000000 1c00 0200 08000000 00000000
        13 01 0000
        0800 0300 03000000
        0a00 0600 020000000001 0000
        2c00 1500
            0500 0700 ba 000000
            1400 0800
                0600 0100 d421 0000
                0800 0500 d4210000
            0c00 0e00
                0600 0100 a00f 0000
    ";

    #[test]
    fn parse_interface_vector() {
        let msg = hex(INTERFACE_VECTOR);
        assert_eq!(msg.len(), 0x40);
        assert_eq!(
            parse_interface(&msg),
            Some((3, 2, Some(b"Home".to_vec()), Some(5180)))
        );
    }

    #[test]
    fn parse_station_vector() {
        let msg = hex(STATION_VECTOR);
        assert_eq!(msg.len(), 0x54);
        assert_eq!(
            parse_station(&msg),
            (Some(-70), Some(400_000), Some(866_000))
        );
    }

    #[test]
    fn truncated_is_safe() {
        let station = hex(STATION_VECTOR);
        // Cut in the middle of STA_INFO, with the original nlmsg_len.
        let cut = &station[..60];
        assert_eq!(parse_station(cut), (None, None, None));
        // The same cut with nlmsg_len fixed up: STA_INFO now runs past the end.
        let mut fixed = cut.to_vec();
        fixed[0..4].copy_from_slice(&60u32.to_ne_bytes());
        assert_eq!(parse_station(&fixed), (None, None, None));
        // STA_INFO cut short, so the RX rate nest runs past its end.
        let mut inner = station.clone();
        let sta_len = 0x2c - 8;
        inner[40..42].copy_from_slice(&(sta_len as u16).to_ne_bytes());
        assert_eq!(parse_station(&inner), (None, None, None));

        let iface = hex(INTERFACE_VECTOR);
        for end in 0..iface.len() {
            let mut b = iface[..end].to_vec();
            assert_eq!(parse_interface(&b), None, "cut at {end}");
            if end >= 4 {
                b[0..4].copy_from_slice(&(end as u32).to_ne_bytes());
                // Short but self-consistent: either no ifindex and iftype, or a truncated
                // attribute. Never a panic.
                let _ = parse_interface(&b);
            }
        }
        for end in 0..station.len() {
            let _ = parse_station(&station[..end]);
        }
    }

    #[test]
    fn missing_ifindex_or_long_ssid() {
        // No ifindex: P2P-device interfaces have no netdev.
        let mut msg = vec![0u8; NLMSG_HDR_LEN];
        msg.extend_from_slice(&[NL80211_CMD_GET_INTERFACE, 1, 0, 0]);
        msg.extend(attr(NL80211_ATTR_IFTYPE, &10u32.to_ne_bytes()));
        let len = msg.len() as u32;
        msg[0..4].copy_from_slice(&len.to_ne_bytes());
        assert_eq!(parse_interface(&msg), None);

        msg.extend(attr(NL80211_ATTR_IFINDEX, &4u32.to_ne_bytes()));
        msg.extend(attr(NL80211_ATTR_SSID, &[b'x'; 33]));
        let len = msg.len() as u32;
        msg[0..4].copy_from_slice(&len.to_ne_bytes());
        assert_eq!(parse_interface(&msg), Some((4, 10, None, None)));
    }

    #[test]
    fn family_reply() {
        let mut msg = vec![0u8; NLMSG_HDR_LEN];
        msg.extend_from_slice(&[1, 2, 0, 0]);
        msg.extend(attr(CTRL_ATTR_FAMILY_NAME, FAMILY_NAME));
        msg.extend(attr(CTRL_ATTR_FAMILY_ID, &0x1cu16.to_ne_bytes()));
        let len = msg.len() as u32;
        msg[0..4].copy_from_slice(&len.to_ne_bytes());
        assert_eq!(parse_family(&msg), Some(0x1c));
    }

    #[test]
    fn request_layout() {
        let r = request(
            0x1c,
            NLM_F_REQUEST | NLM_F_DUMP,
            9,
            NL80211_CMD_GET_STATION,
            &attr(NL80211_ATTR_IFINDEX, &3u32.to_ne_bytes()),
        );
        assert_eq!(read_u32(&r, 0), Some(28));
        assert_eq!(r.len(), 28);
        assert_eq!(read_u16(&r, 4), Some(0x1c));
        assert_eq!(read_u16(&r, 6), Some(0x301));
        assert_eq!(read_u32(&r, 8), Some(9));
        assert_eq!(read_u32(&r, 12), Some(0));
        assert_eq!(&r[16..20], &[17, 0, 0, 0]);
        assert_eq!(read_u16(&r, 20), Some(8));
        assert_eq!(read_u16(&r, 22), Some(3));
        assert_eq!(read_u32(&r, 24), Some(3));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn prop_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=512)) {
            let _ = parse_interface(&bytes);
            let _ = parse_station(&bytes);
            let _ = parse_family(&bytes);
        }
    }
}
