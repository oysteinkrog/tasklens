//! Display connectors of one DRM card from sysfs, with the monitor name from EDID, for the GPU
//! page's Displays list and Information > Graphics > Resolution (PERF-072, PERF-073, R-057).
//!
//! This is the sysfs path; the KMS ioctl path (current mode, refresh, `max bpc`) comes later.
//! All connectors are returned: the engine hides disconnected and disabled ones (PERF-072).
//! EDID serial fields are never parsed or kept.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;

/// Read limit for the `status`, `enabled` and `modes` files, in bytes.
const TEXT_MAX: u64 = 4 * 1024;
/// Read limit for the `edid` file, in bytes.
const EDID_MAX: u64 = 32 * 1024;
/// EDID base block header.
const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
/// Offsets of the four 18-byte descriptors in the EDID base block.
const DESCRIPTOR_OFFSETS: [usize; 4] = [54, 72, 90, 108];
/// Display descriptor tag for the monitor name.
const TAG_MONITOR_NAME: u8 = 0xFC;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectorType {
    Unknown,
    Vga,
    DviI,
    DviD,
    DviA,
    Composite,
    SVideo,
    Lvds,
    Component,
    Din,
    DisplayPort,
    Hdmi,
    Tv,
    Edp,
    Virtual,
    Dsi,
    Dpi,
    Writeback,
    Spi,
    Usb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorInfo {
    /// `N` of `cardN`.
    pub card: u32,
    /// Directory name, for example "card1-DP-2".
    pub sysfs_name: Arc<str>,
    pub connector_type: ConnectorType,
    /// `status` is `connected`.
    pub connected: bool,
    /// `enabled` file: `enabled` or `disabled`. `None` when the file is absent or unreadable.
    pub enabled: Option<bool>,
    /// EDID display descriptor 0xFC.
    pub monitor_name: Option<Arc<str>>,
    /// `modes` lines `WxH` in file order. The first is the preferred mode.
    pub modes: SmallVec<[(u32, u32); 4]>,
}

/// Every connector directory `<sys>/class/drm/card<N>-<NAME>` for one card, sorted by sysfs name.
/// Never fails: unreadable files leave fields empty, a missing directory gives an empty list.
pub fn read_connectors(sys: &Path, card: u32) -> Vec<ConnectorInfo> {
    let drm = sys.join("class/drm");
    let prefix = format!("card{card}-");
    let Ok(entries) = std::fs::read_dir(&drm) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.len() > prefix.len() && name.starts_with(&prefix))
        .collect();
    names.sort_unstable();
    let mut buf = Vec::new();
    names
        .into_iter()
        .map(|name| {
            let dir = drm.join(&name);
            let connected = read_capped(&dir.join("status"), TEXT_MAX, &mut buf)
                && buf.trim_ascii() == b"connected";
            let enabled = if read_capped(&dir.join("enabled"), TEXT_MAX, &mut buf) {
                match buf.trim_ascii() {
                    b"enabled" => Some(true),
                    b"disabled" => Some(false),
                    _ => None,
                }
            } else {
                None
            };
            let modes = if read_capped(&dir.join("modes"), TEXT_MAX, &mut buf) {
                parse_modes(&buf)
            } else {
                SmallVec::new()
            };
            let monitor_name = if read_capped(&dir.join("edid"), EDID_MAX, &mut buf) {
                edid_monitor_name(&buf)
            } else {
                None
            };
            ConnectorInfo {
                card,
                connector_type: connector_type_from_sysfs(&name[prefix.len()..]),
                sysfs_name: Arc::from(name),
                connected,
                enabled,
                monitor_name,
                modes,
            }
        })
        .collect()
}

/// Connector type from the sysfs name after `cardN-`, for example `DP-2` or `HDMI-A-1`. The
/// trailing `-<index>` is dropped and the rest matched against the kernel's type names.
pub fn connector_type_from_sysfs(name_after_card: &str) -> ConnectorType {
    let kind = match name_after_card.rsplit_once('-') {
        Some((kind, index)) if !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()) => {
            kind
        }
        _ => name_after_card,
    };
    match kind {
        "VGA" => ConnectorType::Vga,
        "DVI-I" => ConnectorType::DviI,
        "DVI-D" => ConnectorType::DviD,
        "DVI-A" => ConnectorType::DviA,
        "Composite" => ConnectorType::Composite,
        "SVIDEO" => ConnectorType::SVideo,
        "LVDS" => ConnectorType::Lvds,
        "Component" => ConnectorType::Component,
        "DIN" => ConnectorType::Din,
        "DP" => ConnectorType::DisplayPort,
        "HDMI-A" | "HDMI-B" => ConnectorType::Hdmi,
        "TV" => ConnectorType::Tv,
        "eDP" => ConnectorType::Edp,
        "Virtual" => ConnectorType::Virtual,
        "DSI" => ConnectorType::Dsi,
        "DPI" => ConnectorType::Dpi,
        "Writeback" => ConnectorType::Writeback,
        "SPI" => ConnectorType::Spi,
        "USB" => ConnectorType::Usb,
        _ => ConnectorType::Unknown,
    }
}

/// Monitor name from the first EDID display descriptor with tag 0xFC. `None` for a block shorter
/// than 128 bytes, a wrong header, no such descriptor or an empty name. The checksum is not
/// checked. Only bytes 0x20 to 0x7E are kept.
pub fn edid_monitor_name(edid: &[u8]) -> Option<Arc<str>> {
    if edid.len() < 128 || edid[..8] != EDID_HEADER {
        return None;
    }
    let desc = DESCRIPTOR_OFFSETS
        .iter()
        .map(|&off| &edid[off..off + 18])
        .find(|d| d[..3] == [0, 0, 0] && d[3] == TAG_MONITOR_NAME)?;
    let raw = &desc[5..18];
    let raw = raw
        .iter()
        .position(|&b| b == 0x0A)
        .map_or(raw, |end| &raw[..end]);
    let name: String = raw
        .iter()
        .filter(|b| (0x20..=0x7E).contains(*b))
        .map(|&b| char::from(b))
        .collect();
    let name = name.trim_end_matches(' ');
    (!name.is_empty()).then(|| Arc::from(name))
}

/// Parses `WxH` lines. A line without a width and height is skipped; a suffix after the height
/// (for example `i` for interlaced) is ignored.
fn parse_modes(raw: &[u8]) -> SmallVec<[(u32, u32); 4]> {
    let mut modes = SmallVec::new();
    for line in raw.split(|&b| b == b'\n') {
        let Some(x) = line.iter().position(|&b| b == b'x') else {
            continue;
        };
        let height = &line[x + 1..];
        let digits = height.iter().take_while(|b| b.is_ascii_digit()).count();
        if let (Some(w), Some(h)) = (parse_u32(&line[..x]), parse_u32(&height[..digits])) {
            modes.push((w, h));
        }
    }
    modes
}

fn parse_u32(digits: &[u8]) -> Option<u32> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Reads at most `max` bytes of `path` into `buf`. False when the file cannot be read.
fn read_capped(path: &Path, max: u64, buf: &mut Vec<u8>) -> bool {
    buf.clear();
    match File::open(path) {
        Ok(f) => f.take(max).read_to_end(buf).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dmi-desktop/sys")
    }

    fn connector(name: &str) -> ConnectorInfo {
        read_connectors(&fixture(), 1)
            .into_iter()
            .find(|c| &*c.sysfs_name == name)
            .unwrap()
    }

    /// A 128-byte EDID base block with `name` (at most 13 bytes) in a 0xFC descriptor at offset
    /// 72. No serial number.
    fn edid_with_name(name: &[u8]) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&EDID_HEADER);
        e[72 + 3] = TAG_MONITOR_NAME;
        e[72 + 5..72 + 5 + name.len()].copy_from_slice(name);
        e
    }

    #[test]
    fn dp_connected_with_edid() {
        let c = connector("card1-DP-2");
        assert_eq!(c.card, 1);
        assert_eq!(c.connector_type, ConnectorType::DisplayPort);
        assert!(c.connected);
        assert_eq!(c.enabled, Some(true));
        assert_eq!(c.monitor_name.as_deref(), Some("LG ULTRAGEAR"));
        assert_eq!(c.modes.as_slice(), &[(3840, 2160), (1920, 1080)]);
    }

    #[test]
    fn hdmi_disconnected() {
        let c = connector("card1-HDMI-A-1");
        assert_eq!(c.connector_type, ConnectorType::Hdmi);
        assert!(!c.connected);
        assert_eq!(c.monitor_name, None);
        assert!(c.modes.is_empty());
    }

    #[test]
    fn connectors_sorted_and_card_filtered() {
        let names: Vec<_> = read_connectors(&fixture(), 1)
            .into_iter()
            .map(|c| c.sysfs_name)
            .collect();
        assert_eq!(
            names,
            [Arc::from("card1-DP-2"), Arc::from("card1-HDMI-A-1")]
        );
        assert!(read_connectors(&fixture(), 0).is_empty());
        assert!(read_connectors(Path::new("/nonexistent"), 1).is_empty());
    }

    #[test]
    fn edid_bad_header() {
        let mut e = edid_with_name(b"Monitor\n");
        assert_eq!(edid_monitor_name(&e).as_deref(), Some("Monitor"));
        e[0] = 0x01;
        assert_eq!(edid_monitor_name(&e), None);
        assert_eq!(edid_monitor_name(&edid_with_name(b"Monitor")[..127]), None);
        assert_eq!(edid_monitor_name(&[]), None);
    }

    #[test]
    fn edid_control_byte_dropped() {
        let e = edid_with_name(b"AB\x1bCD  \n    ");
        assert_eq!(edid_monitor_name(&e).as_deref(), Some("ABCD"));
        assert_eq!(edid_monitor_name(&edid_with_name(b"  \n")), None);
        assert_eq!(
            edid_monitor_name(&edid_with_name(b"\xffXYZ")).as_deref(),
            Some("XYZ")
        );
    }

    #[test]
    fn connector_type_names() {
        assert_eq!(
            connector_type_from_sysfs("Writeback-1"),
            ConnectorType::Writeback
        );
        assert_eq!(connector_type_from_sysfs("XYZ-1"), ConnectorType::Unknown);
        assert_eq!(
            connector_type_from_sysfs("DP-2"),
            ConnectorType::DisplayPort
        );
        assert_eq!(connector_type_from_sysfs("HDMI-A-1"), ConnectorType::Hdmi);
        assert_eq!(connector_type_from_sysfs("HDMI-B-3"), ConnectorType::Hdmi);
        assert_eq!(connector_type_from_sysfs("DVI-I-1"), ConnectorType::DviI);
        assert_eq!(connector_type_from_sysfs("eDP-1"), ConnectorType::Edp);
        assert_eq!(connector_type_from_sysfs(""), ConnectorType::Unknown);
    }

    #[test]
    fn modes_parsed_in_order() {
        let modes = parse_modes(b"1920x1080\n1920x1080i\nbad\n800x\n640x480\n");
        assert_eq!(modes.as_slice(), &[(1920, 1080), (1920, 1080), (640, 480)]);
    }
}
