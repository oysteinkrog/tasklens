//! Network interfaces under /sys/class/net/<if>/: identity, kind, connection state and details
//! (ADR-016, PERF-054, PERF-056, R-053, R-162).
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use rustix::io::Errno;
use tl_model::validity::Validity;

use crate::counters::Measured;
use crate::sysroot::{read_i64, read_trimmed, read_u64};
use crate::{CollectError, SysRoot};

const NETNS_FILE: &str = "/proc/self/ns/net";
const IFINDEX_FILE: &str = "net ifindex";

/// ARPHRD_ETHER.
const ARPHRD_ETHER: u32 = 1;
/// ARPHRD_RAWIP, used by wwan modems.
const ARPHRD_RAWIP: u32 = 519;
const ARPHRD_LOOPBACK: u32 = 772;
/// ARPHRD_NONE, used by tun devices such as `tailscale0`.
const ARPHRD_NONE: u32 = 65534;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfaceKind {
    Ethernet,
    WiFi,
    Virtual,
    UsbNetwork,
    Bridge,
    Cellular,
    Tunnel,
    Other,
}

/// The kernel `operstate`, with PERF-054's labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperState {
    Connected,
    Disconnected,
    Testing,
    Dormant,
    NotPresent,
    LowerLayerDown,
    Unknown,
}

/// One interface. Its identity is (`netns_ino`, `ifindex`, `name`) (ADR-003).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetIface {
    pub name: Arc<str>,
    pub ifindex: u32,
    pub netns_ino: u64,
    pub kind: IfaceKind,
    /// The ARPHRD number from `type`.
    pub arphrd_type: u32,
}

/// The PERF-056 details rows of one interface. Read at enumeration time, not per tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IfaceDetails {
    /// Link speed in Mb/s. `Unsupported` ("Not reported") when `speed` reads `-1` or fails with
    /// EINVAL, which the kernel returns for an interface that is down.
    pub speed_mbps: Measured<u32>,
    /// `address`, trimmed. Empty gives `None`. `00:00:00:00:00:00` is kept as is.
    pub mac: Measured<Option<Arc<str>>>,
    /// 0 when `ifindex` cannot be read. The kernel never gives an interface index 0.
    pub ifindex: u32,
    /// The last component of the `device/driver` link. `None` when there is no link.
    pub driver: Option<Arc<str>>,
}

/// Names that are always Virtual, checked before every other rule (PERF-004, R-162).
pub const VIRTUAL_NAME_PREFIXES: [&str; 5] = ["docker", "veth", "br-", "virbr", "vnet"];

/// "net:[4026531840]" -> Some(4026531840). Anything else -> None.
pub fn parse_netns_link(link: &[u8]) -> Option<u64> {
    let digits = link.strip_prefix(b"net:[")?.strip_suffix(b"]")?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// readlink root.proc/self/ns/net -> "net:[4026531840]" -> 4026531840
pub fn read_netns_ino(root: &SysRoot) -> Result<u64, CollectError> {
    let link = std::fs::read_link(root.proc_path("self/ns/net"))?;
    parse_netns_link(link.as_os_str().as_bytes()).ok_or(CollectError::Parse {
        file: NETNS_FILE,
        what: "link",
    })
}

/// Every root.sys/class/net/* except loopback, sorted by name. Clears `out` first.
///
/// Fails only if the directory cannot be listed or the netns link cannot be read. An interface
/// whose name is not UTF-8, or whose `ifindex` or `type` cannot be read, is left out: it may
/// have gone away during the scan.
pub fn enumerate(root: &SysRoot, out: &mut Vec<NetIface>) -> Result<(), CollectError> {
    out.clear();
    let class = root.sys_path("class/net");
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&class)? {
        let entry = entry?;
        if let Ok(name) = entry.file_name().into_string()
            && name != "lo"
        {
            names.push(name);
        }
    }
    names.sort_unstable();
    let netns_ino = read_netns_ino(root)?;

    let mut buf = Vec::new();
    for name in names {
        let dir = class.join(&name);
        let Ok(arphrd_type) = read_u32(&dir.join("type"), &mut buf) else {
            continue;
        };
        if arphrd_type == ARPHRD_LOOPBACK {
            continue;
        }
        let Ok(ifindex) = read_u32(&dir.join("ifindex"), &mut buf) else {
            continue;
        };
        let kind = classify(root, &name, arphrd_type);
        out.push(NetIface {
            name: Arc::from(name),
            ifindex,
            netns_ino,
            kind,
            arphrd_type,
        });
    }
    Ok(())
}

/// The first PERF-054 rule that matches, in order: name prefix, Wi-Fi, bridge, tunnel, USB,
/// cellular, Ethernet, Other.
pub fn classify(root: &SysRoot, name: &str, arphrd_type: u32) -> IfaceKind {
    if VIRTUAL_NAME_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return IfaceKind::Virtual;
    }
    let dir = root.sys_path("class/net").join(name);
    if has_entry(&dir, "wireless") || has_entry(&dir, "phy80211") {
        return IfaceKind::WiFi;
    }
    if has_entry(&dir, "bridge") {
        return IfaceKind::Bridge;
    }
    if has_entry(&dir, "tun_flags") || arphrd_type == ARPHRD_NONE {
        return IfaceKind::Tunnel;
    }
    if has_entry(&dir, "device/driver") && under_usb(root, &dir.join("device")) {
        return IfaceKind::UsbNetwork;
    }
    if name.starts_with("wwan") || arphrd_type == ARPHRD_RAWIP {
        return IfaceKind::Cellular;
    }
    if arphrd_type == ARPHRD_ETHER && has_entry(&dir, "device") {
        return IfaceKind::Ethernet;
    }
    IfaceKind::Other
}

/// operstate + carrier mapped to PERF-054's labels.
///
/// `unknown` is `Connected` when `carrier` reads `1`, else `Unknown`. A read error gives
/// `err.validity()`; text the kernel does not write gives `Failed`. The value is `Unknown`
/// whenever the validity is not `Valid`.
pub fn read_oper_state(root: &SysRoot, name: &str, buf: &mut Vec<u8>) -> Measured<OperState> {
    let dir = root.sys_path("class/net").join(name);
    let state = match read_trimmed(&dir.join("operstate"), buf) {
        Ok(text) => parse_operstate(text),
        Err(err) => return unknown(err.validity()),
    };
    match state {
        Some(Some(state)) => Measured::valid(state),
        Some(None) => {
            let carrier = matches!(read_trimmed(&dir.join("carrier"), buf), Ok(b"1"));
            Measured::valid(if carrier {
                OperState::Connected
            } else {
                OperState::Unknown
            })
        }
        None => unknown(Validity::Failed),
    }
}

/// Link speed, MAC address, interface index and driver of root.sys/class/net/<name>
/// (PERF-056). Each field carries its own failure; this never fails as a whole.
pub fn read_details(root: &SysRoot, name: &str, buf: &mut Vec<u8>) -> IfaceDetails {
    let dir = root.sys_path("class/net").join(name);
    let speed_mbps = match read_i64(&dir.join("speed"), buf) {
        Ok(-1) | Err(CollectError::Os(Errno::INVAL)) => Measured::invalid(Validity::Unsupported),
        Ok(mbps) => u32::try_from(mbps)
            .map_or_else(|_| Measured::invalid(Validity::Failed), Measured::valid),
        Err(err) => Measured::invalid(err.validity()),
    };
    let mac = match read_trimmed(&dir.join("address"), buf) {
        Ok(b"") => Measured::valid(None),
        Ok(text) => std::str::from_utf8(text).map_or_else(
            |_| Measured::invalid(Validity::Failed),
            |mac| Measured::valid(Some(Arc::from(mac))),
        ),
        Err(err) => Measured::invalid(err.validity()),
    };
    let ifindex = read_u32(&dir.join("ifindex"), buf).unwrap_or(0);
    let driver = std::fs::read_link(dir.join("device/driver"))
        .ok()
        .and_then(|link| link.file_name()?.to_str().map(Arc::from));
    IfaceDetails {
        speed_mbps,
        mac,
        ifindex,
        driver,
    }
}

/// `Some(Some(state))` for a known state, `Some(None)` for `unknown` (the caller checks
/// `carrier`), `None` for any other text.
fn parse_operstate(text: &[u8]) -> Option<Option<OperState>> {
    Some(Some(match text {
        b"up" => OperState::Connected,
        b"down" => OperState::Disconnected,
        b"testing" => OperState::Testing,
        b"dormant" => OperState::Dormant,
        b"notpresent" => OperState::NotPresent,
        b"lowerlayerdown" => OperState::LowerLayerDown,
        b"unknown" => return Some(None),
        _ => return None,
    }))
}

fn unknown(validity: Validity) -> Measured<OperState> {
    Measured {
        value: OperState::Unknown,
        validity,
    }
}

fn read_u32(path: &Path, buf: &mut Vec<u8>) -> Result<u32, CollectError> {
    u32::try_from(read_u64(path, buf)?).map_err(|_| CollectError::Parse {
        file: IFINDEX_FILE,
        what: "out of range",
    })
}

/// True when `dir/rel` exists. The last component is not followed, so a dangling symlink counts.
fn has_entry(dir: &Path, rel: &str) -> bool {
    std::fs::symlink_metadata(dir.join(rel)).is_ok()
}

/// True when the canonical path of `device`, below the canonical sysfs root, has a component
/// that starts with `usb`.
fn under_usb(root: &SysRoot, device: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(device) else {
        return false;
    };
    let below = std::fs::canonicalize(&root.sys)
        .ok()
        .and_then(|sys| real.strip_prefix(sys).ok().map(Path::to_path_buf))
        .unwrap_or(real);
    below
        .components()
        .any(|c| c.as_os_str().as_bytes().starts_with(b"usb"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    proptest! {
        #[test]
        fn prop_netns_link_parse_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=64)) {
            let _ = parse_netns_link(&bytes);
        }

        #[test]
        fn prop_netns_link_round_trip(n in any::<u64>()) {
            prop_assert_eq!(parse_netns_link(format!("net:[{n}]").as_bytes()), Some(n));
        }
    }

    #[test]
    fn netns_link_rejects() {
        for bad in [
            &b""[..],
            b"net:[]",
            b"net:[12",
            b"net:12]",
            b"mnt:[4026531840]",
            b"net:[+1]",
            b"net:[ 1]",
            b"net:[18446744073709551616]",
        ] {
            assert_eq!(parse_netns_link(bad), None, "{bad:?}");
        }
        assert_eq!(parse_netns_link(b"net:[4026531840]"), Some(4_026_531_840));
    }

    #[test]
    fn operstate_text() {
        assert_eq!(parse_operstate(b"up"), Some(Some(OperState::Connected)));
        assert_eq!(
            parse_operstate(b"notpresent"),
            Some(Some(OperState::NotPresent))
        );
        assert_eq!(parse_operstate(b"unknown"), Some(None));
        assert_eq!(parse_operstate(b"UP"), None);
        assert_eq!(parse_operstate(b""), None);
    }

    #[test]
    fn bad_netns_link_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let ns = dir.path().join("proc/self/ns");
        fs::create_dir_all(&ns).unwrap();
        symlink("pid:[1]", ns.join("net")).unwrap();
        let root = SysRoot::at(dir.path());
        assert!(matches!(
            read_netns_ino(&root),
            Err(CollectError::Parse {
                file: "/proc/self/ns/net",
                what: "link"
            })
        ));
    }

    #[test]
    fn unlisted_class_dir_fails_and_bad_iface_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        let mut out = Vec::new();
        assert!(matches!(
            enumerate(&root, &mut out),
            Err(CollectError::NotFound)
        ));

        let ns = dir.path().join("proc/self/ns");
        fs::create_dir_all(&ns).unwrap();
        symlink("net:[7]", ns.join("net")).unwrap();
        let net = dir.path().join("sys/class/net");
        for (name, ty, ifindex) in [
            ("a0", "1", "2"),
            ("b0", "x", "3"),
            ("c0", "1", "4294967296"),
        ] {
            fs::create_dir_all(net.join(name)).unwrap();
            fs::write(net.join(name).join("type"), ty).unwrap();
            fs::write(net.join(name).join("ifindex"), ifindex).unwrap();
        }
        enumerate(&root, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(&*out[0].name, "a0");
        assert_eq!(out[0].netns_ino, 7);
        assert_eq!(out[0].kind, IfaceKind::Other);
    }

    #[test]
    fn details_bad_values() {
        let dir = tempfile::tempdir().unwrap();
        let iface = dir.path().join("sys/class/net/x0");
        fs::create_dir_all(&iface).unwrap();
        fs::write(iface.join("speed"), "-2\n").unwrap();
        fs::write(iface.join("address"), "\n").unwrap();
        let root = SysRoot::at(dir.path());
        let mut buf = Vec::new();
        let got = read_details(&root, "x0", &mut buf);
        assert_eq!(got.speed_mbps.validity, Validity::Failed);
        assert_eq!(got.mac, Measured::valid(None));
        assert_eq!(got.ifindex, 0);
        assert_eq!(got.driver, None);

        fs::write(iface.join("speed"), "4294967296").unwrap();
        fs::write(iface.join("address"), "00:00:00:00:00:00").unwrap();
        let got = read_details(&root, "x0", &mut buf);
        assert_eq!(got.speed_mbps.validity, Validity::Failed);
        assert_eq!(
            got.mac,
            Measured::valid(Some(Arc::from("00:00:00:00:00:00")))
        );
    }

    #[test]
    fn unknown_operstate_text_is_failed() {
        let dir = tempfile::tempdir().unwrap();
        let iface = dir.path().join("sys/class/net/x0");
        fs::create_dir_all(&iface).unwrap();
        fs::write(iface.join("operstate"), "sideways\n").unwrap();
        let root = SysRoot::at(dir.path());
        let mut buf = Vec::new();
        let state = read_oper_state(&root, "x0", &mut buf);
        assert_eq!(state.validity, Validity::Failed);
        assert_eq!(state.value, OperState::Unknown);
    }
}
