//! AMD and Intel GPUs through sysfs, hwmon and DRM fdinfo.
//!
//! This file holds card discovery and the PCIe link (PERF-062, PERF-069, PERF-070, ADR-016).
//! Discovery also lists NVIDIA cards so their PCI slot can be matched to NVML; NVIDIA metrics
//! come from `tl-gpu-probe`.

pub mod amdgpu;
pub mod fdinfo;
pub mod intel;

pub use amdgpu::{AmdSample, AmdTemp, GpuMetrics, main_temp, parse_gpu_metrics, read_amd};
pub use fdinfo::{DrmClient, FdinfoScan, engine_busy, engine_busy_cycles, parse_fdinfo, scan};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const VENDOR_NVIDIA: u16 = 0x10de;
pub const VENDOR_AMD: u16 = 0x1002;
pub const VENDOR_INTEL: u16 = 0x8086;

#[derive(Clone, Debug, PartialEq)]
pub struct DrmCard {
    /// N of `/sys/class/drm/cardN`.
    pub card: u32,
    /// `device/vendor`.
    pub vendor: u16,
    /// `device/device`.
    pub device_id: u16,
    /// `PCI_SLOT_NAME` from `device/uevent`, for example `0000:2d:00.0`.
    pub pci_slot: Arc<str>,
    /// Basename of the `device/driver` symlink: `amdgpu`, `i915`, `xe` or `nvidia`.
    pub driver: Option<Arc<str>>,
    /// First `device/hwmon/hwmon*` directory by name.
    pub hwmon: Option<PathBuf>,
}

/// `cardN` directories only (names "card" + digits), vendors NVIDIA, AMD and Intel, sorted by N.
/// Unreadable cards are skipped.
pub fn discover_cards(sys: &Path) -> Vec<DrmCard> {
    let Ok(entries) = fs::read_dir(sys.join("class/drm")) else {
        return Vec::new();
    };
    let mut cards: Vec<DrmCard> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let card = card_number(name.to_str()?)?;
            read_card(&e.path(), card)
        })
        .collect();
    cards.sort_by_key(|c| c.card);
    cards
}

/// `card` followed by decimal digits only, so connectors like `card1-DP-2` give `None`.
fn card_number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("card")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn read_card(dir: &Path, card: u32) -> Option<DrmCard> {
    let device = dir.join("device");
    let vendor = read_hex_u16(&device.join("vendor"))?;
    if !matches!(vendor, VENDOR_NVIDIA | VENDOR_AMD | VENDOR_INTEL) {
        return None;
    }
    let device_id = read_hex_u16(&device.join("device"))?;
    let uevent = fs::read_to_string(device.join("uevent")).ok()?;
    let pci_slot = uevent
        .lines()
        .find_map(|l| l.strip_prefix("PCI_SLOT_NAME="))
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let driver = fs::read_link(device.join("driver"))
        .ok()
        .and_then(|t| t.file_name()?.to_str().map(Arc::from));
    Some(DrmCard {
        card,
        vendor,
        device_id,
        pci_slot: Arc::from(pci_slot),
        driver,
        hwmon: first_hwmon(&device.join("hwmon")),
    })
}

fn read_hex_u16(path: &Path) -> Option<u16> {
    let text = fs::read_to_string(path).ok()?;
    let hex = text.trim().strip_prefix("0x")?;
    u16::from_str_radix(hex, 16).ok()
}

fn first_hwmon(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("hwmon"))
        })
        .map(|e| e.path())
        .min()
}

/// PCIe link from sysfs. `None` fields show "Not reported".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PcieLink {
    pub cur_gen: Option<u8>,
    pub cur_width: Option<u8>,
    pub max_gen: Option<u8>,
    pub max_width: Option<u8>,
}

/// Reads `current_link_speed`, `current_link_width`, `max_link_speed` and `max_link_width` from
/// `class/drm/cardN/device`.
pub fn read_pcie_link(sys: &Path, card: &DrmCard) -> PcieLink {
    let device = sys.join(format!("class/drm/card{}/device", card.card));
    let speed = |file: &str| read_trimmed(&device.join(file)).and_then(|s| parse_link_speed(&s));
    let width = |file: &str| read_trimmed(&device.join(file)).and_then(|s| s.parse().ok());
    PcieLink {
        cur_gen: speed("current_link_speed"),
        cur_width: width("current_link_width"),
        max_gen: speed("max_link_speed"),
        max_width: width("max_link_width"),
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

/// Text like `16.0 GT/s PCIe`: the number before ` GT/s`, as a PCIe generation.
fn parse_link_speed(text: &str) -> Option<u8> {
    let (num, _) = text.split_once(" GT/s")?;
    gts_to_gen(num.trim().parse().ok()?)
}

/// 2.5 GT/s is gen 1, 5 is 2, 8 is 3, 16 is 4, 32 is 5 and 64 is 6, within 0.1. Other values
/// give `None`.
pub fn gts_to_gen(gts: f32) -> Option<u8> {
    const GENS: [(f32, u8); 6] = [
        (2.5, 1),
        (5.0, 2),
        (8.0, 3),
        (16.0, 4),
        (32.0, 5),
        (64.0, 6),
    ];
    GENS.iter()
        .find(|(rate, _)| (gts - rate).abs() < 0.1)
        .map(|&(_, generation)| generation)
}

/// True when a sysfs `PCI_SLOT_NAME` and an NVML bus id name the same slot. NVML uses an 8 digit
/// domain (`00000000:2D:00.0`); it is cut to its last 4 digits and compared case-insensitively.
pub fn same_pci_slot(sysfs: &str, nvml: &str) -> bool {
    fn normalize(id: &str) -> Option<(&str, &str)> {
        let (domain, rest) = id.trim().split_once(':')?;
        let domain = domain.get(domain.len().checked_sub(4)?..)?;
        Some((domain, rest))
    }
    match (normalize(sysfs), normalize(nvml)) {
        (Some((da, ra)), Some((db, rb))) => {
            da.eq_ignore_ascii_case(db) && ra.eq_ignore_ascii_case(rb)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_sys() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-basic/sys")
    }

    #[test]
    fn discover_two_cards() {
        let sys = fixture_sys();
        let cards = discover_cards(&sys);
        assert_eq!(cards.len(), 2, "{cards:?}");
        assert_eq!(cards[0].card, 0);
        assert_eq!(cards[0].vendor, VENDOR_INTEL);
        assert_eq!(cards[0].device_id, 0x56a0);
        assert_eq!(&*cards[0].pci_slot, "0000:00:02.0");
        assert_eq!(cards[0].driver.as_deref(), Some("i915"));
        assert_eq!(
            cards[0].hwmon,
            Some(sys.join("class/drm/card0/device/hwmon/hwmon5"))
        );
        assert_eq!(cards[1].card, 1);
        assert_eq!(cards[1].vendor, VENDOR_NVIDIA);
        assert_eq!(&*cards[1].pci_slot, "0000:2d:00.0");
        assert_eq!(cards[1].driver.as_deref(), Some("nvidia"));
        assert_eq!(cards[1].hwmon, None);
        assert_eq!(card_number("card1-DP-2"), None);
    }

    #[test]
    fn pcie_link_gen() {
        let sys = fixture_sys();
        let cards = discover_cards(&sys);
        let nvidia = cards.iter().find(|c| c.card == 1).expect("card1");
        assert_eq!(
            read_pcie_link(&sys, nvidia),
            PcieLink {
                cur_gen: Some(4),
                cur_width: Some(16),
                max_gen: Some(5),
                max_width: Some(16),
            }
        );
        // card0 has no link files.
        assert_eq!(read_pcie_link(&sys, &cards[0]), PcieLink::default());
        assert_eq!(gts_to_gen(2.5), Some(1));
        assert_eq!(gts_to_gen(64.0), Some(6));
        assert_eq!(gts_to_gen(8.05), Some(3));
        assert_eq!(gts_to_gen(10.0), None);
        assert_eq!(parse_link_speed("Unknown"), None);
    }

    #[test]
    fn nvml_slot_match() {
        assert!(same_pci_slot("0000:2d:00.0", "00000000:2D:00.0"));
        assert!(same_pci_slot("0000:2d:00.0", "0000:2d:00.0"));
        assert!(!same_pci_slot("0000:2d:00.0", "00000000:2E:00.0"));
        assert!(!same_pci_slot("0000:2d:00.0", "00000001:2D:00.0"));
        assert!(!same_pci_slot("0000:2d:00.0", ""));
    }

    #[test]
    fn other_vendor_skipped() {
        let sys = fixture_sys();
        // card2 in the fixture is a virtio GPU, vendor 0x1af4.
        assert!(sys.join("class/drm/card2/device/vendor").exists());
        let cards = discover_cards(&sys);
        assert!(cards.iter().all(|c| c.card != 2), "{cards:?}");
    }
}
