//! Intel GPU metrics for the i915 and xe drivers from sysfs, hwmon and DRM fdinfo (PERF-064,
//! PERF-066 to PERF-069, PERF-075).
//!
//! Untested on real Intel hardware; the `drm-intel` fixture carries the tests. Engines use the
//! fdinfo fallback of PERF-075 (`engine_busy` for i915, `engine_busy_cycles` for xe); the i915 PMU
//! is not read in v1 (R-144). A file that is missing, unreadable or does not parse leaves its
//! field `None`, which the UI shows as "Not supported". The xe VRAM total is left out until U-53
//! names its sysfs file (R-164).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::DrmCard;
use super::fdinfo::{DrmClient, FdinfoScan};
use crate::sysroot::{read_i64, read_u64};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelDriver {
    I915,
    Xe,
}

impl IntelDriver {
    /// From `DrmCard::driver`: `i915` or `xe`. Other drivers give `None` and are not read.
    pub fn from_card(card: &DrmCard) -> Option<Self> {
        match card.driver.as_deref()? {
            "i915" => Some(Self::I915),
            "xe" => Some(Self::Xe),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntelSample {
    /// i915 `gt_act_freq_mhz`, xe `device/tile0/gt0/freq0/act_freq`.
    pub act_freq_mhz: Option<u32>,
    /// i915 `gt_max_freq_mhz`, xe `device/tile0/gt0/freq0/max_freq`.
    pub max_freq_mhz: Option<u32>,
    /// hwmon `energy1_input`, microjoules. The engine computes power from the delta.
    pub energy_uj: Option<u64>,
    /// hwmon `power1_max`, microwatts.
    pub power_max_uw: Option<u64>,
    /// hwmon `temp1_input`, millidegrees C. Only a dGPU has it.
    pub temp_mc: Option<i32>,
}

/// Fills `out` for one Intel card. Every field is overwritten, so a file that stops being
/// readable clears its field.
pub fn read_intel(card: &DrmCard, sys: &Path, drv: IntelDriver, out: &mut IntelSample) {
    let mut buf = Vec::with_capacity(64);
    let mut path = sys.join(format!("class/drm/card{}", card.card));
    let (act, max) = match drv {
        IntelDriver::I915 => ("gt_act_freq_mhz", "gt_max_freq_mhz"),
        IntelDriver::Xe => {
            path.push("device/tile0/gt0/freq0");
            ("act_freq", "max_freq")
        }
    };
    out.act_freq_mhz = read_num(&mut path, act, &mut buf);
    out.max_freq_mhz = read_num(&mut path, max, &mut buf);

    out.energy_uj = None;
    out.power_max_uw = None;
    out.temp_mc = None;
    let Some(hwmon) = &card.hwmon else {
        return;
    };
    let mut path = hwmon.clone();
    out.energy_uj = read_num(&mut path, "energy1_input", &mut buf);
    out.power_max_uw = read_num(&mut path, "power1_max", &mut buf);
    path.push("temp1_input");
    out.temp_mc = read_i64(&path, &mut buf)
        .ok()
        .and_then(|v| i32::try_from(v).ok());
}

/// Reads `dir/name` as one unsigned number that fits `T`.
fn read_num<T: TryFrom<u64>>(dir: &mut PathBuf, name: &str, buf: &mut Vec<u8>) -> Option<T> {
    dir.push(name);
    let value = read_u64(dir, buf).ok();
    dir.pop();
    value.and_then(|v| T::try_from(v).ok())
}

/// Engine display names for i915 fdinfo engine keys (PERF-075).
pub fn i915_engine_name(key: &str) -> Option<&'static str> {
    match key {
        "render" => Some("render"),
        "copy" => Some("copy"),
        "video" => Some("video"),
        "video-enhance" => Some("video-enhance"),
        "compute" => Some("compute"),
        _ => None,
    }
}

/// Memory from fdinfo, summed over the clients on `pdev`: (dedicated resident, dedicated total,
/// shared resident) in bytes. A value is `None` when no client on `pdev` reports its key. The
/// shared value is the iGPU "Shared" memory.
///
/// Region names per R-274: dedicated is `local0` (i915) or `vram0` (xe), shared is `system0`
/// (i915) or `system` (xe). Other regions (`gtt`, `stolen`) are ignored.
pub fn fdinfo_memory(scan: &FdinfoScan, pdev: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    (
        sum_region(scan, pdev, |c| &c.resident_bytes, DEDICATED),
        sum_region(scan, pdev, |c| &c.total_bytes, DEDICATED),
        sum_region(scan, pdev, |c| &c.resident_bytes, SHARED),
    )
}

/// i915 and xe names for the dedicated (VRAM) region.
const DEDICATED: &[&str] = &["local0", "vram0"];
/// i915 and xe names for the shared (system memory) region.
const SHARED: &[&str] = &["system0", "system"];

/// Sum over the clients on `pdev` of the first of `regions` in the list `field` picks; `None`
/// when no client has one.
fn sum_region<'a, L: AsRef<[(Arc<str>, u64)]> + 'a>(
    scan: &'a FdinfoScan,
    pdev: &str,
    field: impl Fn(&'a DrmClient) -> &'a L,
    regions: &[&str],
) -> Option<u64> {
    scan.clients
        .iter()
        .filter(|c| &*c.pdev == pdev)
        .filter_map(|c| {
            field(c)
                .as_ref()
                .iter()
                .find(|(n, _)| regions.contains(&&**n))
                .map(|&(_, v)| v)
        })
        .reduce(u64::saturating_add)
}

/// Source text for the temperature row (PERF-069).
pub fn temp_source_text(drv: IntelDriver) -> &'static str {
    match drv {
        IntelDriver::I915 => "hwmon i915",
        IntelDriver::Xe => "hwmon xe: pkg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_drm::{VENDOR_INTEL, discover_cards};

    fn fixture_sys() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-intel/sys")
    }

    /// Card N of the fixture and the sample read from it.
    fn sample(n: usize) -> (DrmCard, IntelSample) {
        let sys = fixture_sys();
        let cards = discover_cards(&sys);
        assert_eq!(cards.len(), 2, "{cards:?}");
        let card = cards[n].clone();
        assert_eq!(card.vendor, VENDOR_INTEL);
        let drv = IntelDriver::from_card(&card).expect("intel driver");
        let mut out = IntelSample::default();
        read_intel(&card, &sys, drv, &mut out);
        (card, out)
    }

    fn client(id: u64, pdev: &str, resident: &[(&str, u64)], total: &[(&str, u64)]) -> DrmClient {
        DrmClient {
            pid: 1,
            driver: Arc::from("i915"),
            pdev: Arc::from(pdev),
            client_id: id,
            resident_bytes: resident.iter().map(|&(n, v)| (Arc::from(n), v)).collect(),
            total_bytes: total.iter().map(|&(n, v)| (Arc::from(n), v)).collect(),
            ..DrmClient::default()
        }
    }

    #[test]
    fn i915_freq() {
        let (card, s) = sample(0);
        assert_eq!(IntelDriver::from_card(&card), Some(IntelDriver::I915));
        assert_eq!(
            s,
            IntelSample {
                act_freq_mhz: Some(1300),
                max_freq_mhz: Some(2200),
                energy_uj: Some(123_456_789),
                power_max_uw: Some(28_000_000),
                // An iGPU has no temp1_input.
                temp_mc: None,
            }
        );
    }

    #[test]
    fn xe_freq() {
        let (card, s) = sample(1);
        assert_eq!(IntelDriver::from_card(&card), Some(IntelDriver::Xe));
        assert_eq!(
            s,
            IntelSample {
                act_freq_mhz: Some(900),
                max_freq_mhz: Some(2850),
                energy_uj: Some(987_654_321),
                power_max_uw: Some(190_000_000),
                temp_mc: Some(48_000),
            }
        );

        // Reading the xe card as i915 finds no gt_*_freq_mhz files, and the old values are cleared.
        let mut out = s;
        read_intel(&card, &fixture_sys(), IntelDriver::I915, &mut out);
        assert_eq!((out.act_freq_mhz, out.max_freq_mhz), (None, None));

        // Other drivers are not Intel cards to read.
        let other = DrmCard {
            driver: Some(Arc::from("amdgpu")),
            ..card.clone()
        };
        assert_eq!(IntelDriver::from_card(&other), None);
        let none = DrmCard {
            driver: None,
            ..card
        };
        assert_eq!(IntelDriver::from_card(&none), None);
    }

    #[test]
    fn fdinfo_local_memory() {
        let p = "0000:03:00.0";
        let scan = FdinfoScan {
            clients: vec![
                client(
                    1,
                    p,
                    &[("local0", 1024 * 1024), ("system0", 4096)],
                    &[("local0", 8 * 1024 * 1024)],
                ),
                client(2, p, &[("local0", 2048 * 1024)], &[]),
                // Another pdev is not counted.
                client(3, "0000:00:02.0", &[("local0", 999), ("system0", 1)], &[]),
            ],
            partial: false,
            t_ns: 0,
        };
        assert_eq!(
            fdinfo_memory(&scan, p),
            (Some(3_145_728), Some(8 * 1024 * 1024), Some(4096))
        );
        assert_eq!(
            fdinfo_memory(&scan, "0000:00:02.0"),
            (Some(999), None, Some(1))
        );
        assert_eq!(fdinfo_memory(&scan, "none"), (None, None, None));

        // Parsed from fdinfo text, KiB units.
        let text = |id: u64, kib: u64| {
            format!(
                "drm-driver: i915\ndrm-client-id: {id}\ndrm-pdev: {p}\n\
                 drm-resident-local0: {kib} KiB\n"
            )
        };
        let parse = |id, kib| {
            super::super::fdinfo::parse_fdinfo(1, text(id, kib).as_bytes()).expect("client")
        };
        let scan = FdinfoScan {
            clients: vec![parse(1, 1024), parse(2, 2048)],
            partial: false,
            t_ns: 0,
        };
        assert_eq!(fdinfo_memory(&scan, p).0, Some(3_145_728));
    }

    #[test]
    fn xe_fdinfo_regions() {
        let p = "0000:03:00.0";
        let xe = |id: u64| {
            let text = format!(
                "drm-driver: xe\ndrm-client-id: {id}\ndrm-pdev: {p}\n\
                 drm-total-system: 8 MiB\ndrm-resident-system: 2 MiB\n\
                 drm-total-gtt: 64 MiB\ndrm-resident-gtt: 64 MiB\n\
                 drm-total-stolen: 32 MiB\n\
                 drm-total-vram0: 4 MiB\ndrm-resident-vram0: 3 MiB\n"
            );
            super::super::fdinfo::parse_fdinfo(1, text.as_bytes()).expect("client")
        };
        let scan = FdinfoScan {
            clients: vec![xe(1), xe(2)],
            partial: false,
            t_ns: 0,
        };
        // gtt and stolen are not counted.
        assert_eq!(
            fdinfo_memory(&scan, p),
            (
                Some(6 * 1024 * 1024),
                Some(8 * 1024 * 1024),
                Some(4 * 1024 * 1024)
            )
        );
    }

    #[test]
    fn engine_names() {
        assert_eq!(i915_engine_name("render"), Some("render"));
        assert_eq!(i915_engine_name("foo"), None);
        for key in ["copy", "video", "video-enhance", "compute"] {
            assert_eq!(i915_engine_name(key), Some(key));
        }
        assert_eq!(i915_engine_name("capacity-vcs"), None);
    }

    #[test]
    fn temp_source() {
        assert_eq!(temp_source_text(IntelDriver::Xe), "hwmon xe: pkg");
        assert_eq!(temp_source_text(IntelDriver::I915), "hwmon i915");
    }
}
