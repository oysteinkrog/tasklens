//! GPU load and VRAM by app (tasklens-g00wx.1, leader decision 2026-11-10).
//!
//! Per GPU that reports per-process data, the reducer groups the GPU's process rows by memory
//! group (D-036) and publishes the top 20 groups, "Other apps" and "Other / driver" as
//! `GPU_GROUP` and `VRAM_GROUP` series. Each band has its own `DeviceIdentity::MemGroup` device
//! per GPU, with the key `gpu:<pci bus id>:<band key>`, so each GPU page shows that GPU's apps.
//! The two top 20s are ranked apart, as the CPU and memory top 20s are.
//!
//! - A process's load on a GPU is its busiest engine there: the highest of NVML sm, enc and dec,
//!   or the busiest fdinfo engine of its DRM row. An app's load is the sum over its processes,
//!   clamped to 100. Bands are not rescaled.
//! - A process's VRAM on a GPU is the sum of its rows' dedicated memory there.
//! - "Other / driver" is the device total minus all app bands, clamped at 0: the GPU load, or
//!   the device's used VRAM (NVIDIA per-process VRAM leaves out driver and shared allocations).
//!   When the apps sum to more than the device load, the stack is drawn as is and "Other /
//!   driver" is 0.
//! - NVIDIA rows match a GPU by UUID, DRM rows (AMD, Intel) by fdinfo `drm-pdev`. A process
//!   whose group is not known (it started after the last process sweep) is left out of every
//!   app band; its VRAM then counts in "Other / driver".
//! - An app with a row on the GPU but no load reading reads 0, not a gap.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use tl_collect::apps::mem_group::MemGroup;
use tl_model::gpu_proto::GpuProcessSample;
use tl_model::ids::{DeviceIdx, MetricId};
use tl_model::system::WineGroupInfo;

pub use super::mem_groups::GroupInfo;
use super::mem_groups::{MemGroups, OTHER_KEY, OTHER_NAME, TOP_GROUPS};
use crate::update::DrmProcessVram;

/// Prefix of the `DeviceIdentity::MemGroup` key of a GPU band: `gpu:<pci bus id>:<band key>`.
/// The persistent device key is `memgroup:gpu:<pci bus id>:<band key>`.
pub const KEY_PREFIX: &str = "gpu:";
/// Key and name of the band for the device total minus all app bands.
pub const DRIVER_KEY: &str = "driver";
pub const DRIVER_NAME: &str = "Other / driver";
/// The highest load of one app, in percent of the GPU.
pub const APP_MAX_PCT: f64 = 100.0;

/// A GPU band's place in `GpuGroups::devices`: the GPU's PCI bus id and the band key.
pub type BandKey = (Arc<str>, Arc<str>);

/// One process's use of one GPU in one sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcUse {
    pub pid: u32,
    /// Busy percent of its busiest engine on the GPU; 0 without a reading.
    pub load: f64,
    /// Dedicated memory on the GPU, bytes.
    pub vram: u64,
}

/// Which series a band list is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `GPU_GROUP`, percent of the GPU.
    Load,
    /// `VRAM_GROUP`, bytes.
    Vram,
}

impl Kind {
    pub fn metric(self) -> MetricId {
        match self {
            Kind::Load => MetricId::GPU_GROUP,
            Kind::Vram => MetricId::VRAM_GROUP,
        }
    }
}

/// One app group's use of one GPU.
#[derive(Clone, Debug, PartialEq)]
pub struct AppUse {
    pub group: Arc<MemGroup>,
    pub wine: Option<Arc<WineGroupInfo>>,
    /// Percent for `Kind::Load`, bytes for `Kind::Vram`.
    pub value: f64,
    /// Processes of the group with a row on the GPU.
    pub processes: u32,
}

/// The bands of one GPU and one kind in one sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuBands {
    /// At most `TOP_GROUPS`, largest first. Equal values are ordered by key.
    pub top: Vec<AppUse>,
    /// Every group outside `top`.
    pub other: f64,
    pub other_processes: u32,
    /// The device total minus `top` and `other`, clamped at 0. `None` when the device total is
    /// not known.
    pub driver: Option<f64>,
}

/// The NVIDIA rows of the GPU with `uuid`, one entry per pid: the highest of sm, enc and dec,
/// and the sum of `mem_used`.
pub fn nvidia_uses(rows: &[GpuProcessSample], uuid: &[u8; 16]) -> Vec<ProcUse> {
    merge(
        rows.iter()
            .filter(|r| r.uuid == *uuid)
            .map(|r| ProcUse {
                pid: r.pid,
                load: [r.sm_pct, r.enc_pct, r.dec_pct]
                    .into_iter()
                    .filter_map(|p| p.ok())
                    .max()
                    .map_or(0.0, f64::from),
                vram: r.mem_used.ok().unwrap_or(0),
            })
            .collect(),
    )
}

/// The DRM rows of the card with PCI slot `pdev`, one entry per pid: the busiest engine and the
/// sum of dedicated memory.
pub fn drm_uses(rows: &[DrmProcessVram], pdev: &str) -> Vec<ProcUse> {
    merge(
        rows.iter()
            .filter(|r| *r.pdev == *pdev)
            .map(|r| ProcUse {
                pid: r.pid,
                load: f64::from(r.engines.iter().map(|(_, p)| *p).fold(0.0_f32, f32::max)),
                vram: r.vram_bytes,
            })
            .collect(),
    )
}

/// One entry per pid, by pid: the highest load and the sum of VRAM.
fn merge(mut uses: Vec<ProcUse>) -> Vec<ProcUse> {
    uses.sort_unstable_by_key(|u| u.pid);
    let mut out: Vec<ProcUse> = Vec::with_capacity(uses.len());
    for u in uses {
        match out.last_mut() {
            Some(last) if last.pid == u.pid => {
                last.load = last.load.max(u.load);
                last.vram = last.vram.saturating_add(u.vram);
            }
            _ => out.push(u),
        }
    }
    out
}

/// The group of each of `pids` that the last process sweep saw, through `MemGroups::keys` and
/// `MemGroups::by_key`. A pid the sweep did not see is missing.
pub fn pid_groups(groups: &MemGroups, pids: &FxHashSet<u32>) -> FxHashMap<u32, GroupInfo> {
    let mut out = FxHashMap::default();
    if pids.is_empty() {
        return out;
    }
    for (key, group) in &groups.keys {
        if pids.contains(&key.pid)
            && let Some(info) = groups.by_key.get(group)
        {
            out.insert(key.pid, info.clone());
        }
    }
    out
}

/// The bands of one GPU and one kind (see the module doc). `total` is the device's GPU load or
/// used VRAM, when known.
pub fn bands(
    uses: &[ProcUse],
    groups: &FxHashMap<u32, GroupInfo>,
    kind: Kind,
    total: Option<f64>,
) -> GpuBands {
    let mut apps: Vec<AppUse> = Vec::new();
    let mut index: FxHashMap<&str, usize> = FxHashMap::default();
    for u in uses {
        let Some((group, wine)) = groups.get(&u.pid) else {
            continue;
        };
        let slot = *index.entry(&group.key).or_insert_with(|| {
            apps.push(AppUse {
                group: Arc::clone(group),
                wine: wine.clone(),
                value: 0.0,
                processes: 0,
            });
            apps.len() - 1
        });
        let app = &mut apps[slot];
        app.value += match kind {
            Kind::Load => u.load,
            Kind::Vram => u.vram as f64,
        };
        app.processes += 1;
    }
    if kind == Kind::Load {
        for app in &mut apps {
            app.value = app.value.min(APP_MAX_PCT);
        }
    }
    apps.sort_unstable_by(|a, b| {
        b.value
            .total_cmp(&a.value)
            .then_with(|| a.group.key.cmp(&b.group.key))
    });
    let rest = apps.split_off(apps.len().min(TOP_GROUPS));
    let other: f64 = rest.iter().map(|a| a.value).sum();
    let sum = apps.iter().map(|a| a.value).sum::<f64>() + other;
    GpuBands {
        top: apps,
        other,
        other_processes: rest.iter().map(|a| a.processes).sum(),
        driver: total.map(|t| (t - sum).max(0.0)),
    }
}

/// The GPU band devices the reducer has added, and the bands each GPU showed last.
pub struct GpuGroups {
    /// Device and display name per (GPU PCI bus id, band key), so a sample in steady state does
    /// not touch the catalog.
    pub devices: FxHashMap<BandKey, (DeviceIdx, Arc<str>)>,
    /// Per (GPU device, `GPU_GROUP` or `VRAM_GROUP`): the band devices of the last sample with
    /// process rows. One that drops out gets a gap; a sample without rows gaps them all and
    /// keeps them, so the next one gaps them again.
    pub shown: FxHashMap<(DeviceIdx, MetricId), Vec<DeviceIdx>>,
    pub other_key: Arc<str>,
    pub other_name: Arc<str>,
    pub driver_key: Arc<str>,
    pub driver_name: Arc<str>,
}

impl Default for GpuGroups {
    fn default() -> Self {
        GpuGroups {
            devices: FxHashMap::default(),
            shown: FxHashMap::default(),
            other_key: Arc::from(OTHER_KEY),
            other_name: Arc::from(OTHER_NAME),
            driver_key: Arc::from(DRIVER_KEY),
            driver_name: Arc::from(DRIVER_NAME),
        }
    }
}

impl GpuGroups {
    /// Drops the devices no GPU shows any more.
    pub fn prune(&mut self) {
        let keep: FxHashSet<DeviceIdx> = self.shown.values().flatten().copied().collect();
        if self.devices.len() > keep.len() {
            self.devices.retain(|_, (idx, _)| keep.contains(idx));
        }
    }
}

#[cfg(test)]
mod tests {
    use smallvec::smallvec;
    use tl_collect::apps::mem_group::MemGroupKind;
    use tl_model::gpu_proto::Reading;

    use super::*;

    fn group(key: &str) -> GroupInfo {
        let group = MemGroup {
            key: Arc::from(key),
            name: Arc::from(key.trim_start_matches("exe:")),
            kind: MemGroupKind::Program,
            app_id: None,
            wine_prefix: None,
        };
        (Arc::new(group), None)
    }

    /// pid → group: pid `p` is in `exe:<names[p - 1]>`.
    fn groups(names: &[&str]) -> FxHashMap<u32, GroupInfo> {
        (1..)
            .zip(names)
            .map(|(pid, name)| (pid, group(&format!("exe:{name}"))))
            .collect()
    }

    fn use_of(pid: u32, load: f64, vram: u64) -> ProcUse {
        ProcUse { pid, load, vram }
    }

    fn nv(uuid: u8, pid: u32, sm: Reading<u32>, enc: u32, mem: u64) -> GpuProcessSample {
        GpuProcessSample {
            uuid: [uuid; 16],
            pid,
            sm_pct: sm,
            mem_pct: Reading::Ok(0),
            enc_pct: Reading::Ok(enc),
            dec_pct: Reading::Unsupported,
            mem_used: Reading::Ok(mem),
        }
    }

    #[test]
    fn nvidia_rows_by_device_busiest_engine_and_vram_sum() {
        let rows = [
            nv(1, 7, Reading::Ok(30), 55, 100),
            // The same pid twice on one GPU (graphics and compute): the higher load, the VRAM
            // summed.
            nv(1, 7, Reading::Ok(60), 0, 50),
            nv(2, 7, Reading::Ok(90), 0, 1000),
            // An idle process: no load reading, so 0.
            nv(1, 3, Reading::Unsupported, 0, 20),
        ];
        assert_eq!(
            nvidia_uses(&rows, &[1; 16]),
            [use_of(3, 0.0, 20), use_of(7, 60.0, 150)]
        );
        assert_eq!(nvidia_uses(&rows, &[2; 16]), [use_of(7, 90.0, 1000)]);
        assert!(nvidia_uses(&rows, &[3; 16]).is_empty());
    }

    #[test]
    fn drm_rows_by_pdev() {
        let mut a = DrmProcessVram::new(Arc::from("0000:03:00.0"), 4, 300);
        a.engines = smallvec![(Arc::from("gfx"), 40.0), (Arc::from("compute"), 70.0)];
        let mut b = DrmProcessVram::new(Arc::from("0000:00:02.0"), 4, 0);
        b.engines = smallvec![(Arc::from("render"), 10.0)];
        // A first scan: no engine yet, so load 0.
        let c = DrmProcessVram::new(Arc::from("0000:03:00.0"), 9, 64);
        let rows = [a, b, c];
        assert_eq!(
            drm_uses(&rows, "0000:03:00.0"),
            [use_of(4, 70.0, 300), use_of(9, 0.0, 64)]
        );
        assert_eq!(drm_uses(&rows, "0000:00:02.0"), [use_of(4, 10.0, 0)]);
    }

    #[test]
    fn load_sums_per_app_clamps_at_100_and_fills_driver() {
        // pids 1 and 2 are one app.
        let g = groups(&["game", "game", "obs"]);
        let uses = [
            use_of(1, 30.0, 0),
            use_of(2, 25.0, 0),
            use_of(3, 10.0, 0),
            // Unknown to the last sweep: left out.
            use_of(99, 50.0, 0),
        ];
        let b = bands(&uses, &g, Kind::Load, Some(80.0));
        let keys: Vec<&str> = b.top.iter().map(|a| &*a.group.key).collect();
        assert_eq!(keys, ["exe:game", "exe:obs"]);
        assert_eq!(b.top[0].value, 55.0);
        assert_eq!(b.top[0].processes, 2);
        assert_eq!(b.other, 0.0);
        assert_eq!(b.driver, Some(15.0));
        let stack = b.top.iter().map(|a| a.value).sum::<f64>() + b.other + b.driver.unwrap();
        assert_eq!(stack, 80.0, "the stack sums to the device load");

        // One app at 70% 3D and one at 60% video while the device reads 70%: drawn as is,
        // no rescale, and "Other / driver" is 0.
        let uses = [use_of(1, 70.0, 0), use_of(3, 60.0, 0)];
        let b = bands(&uses, &g, Kind::Load, Some(70.0));
        assert_eq!(b.top[0].value, 70.0);
        assert_eq!(b.top[1].value, 60.0);
        assert_eq!(b.driver, Some(0.0));

        // Two processes of one app at 80% each: the app is clamped to 100.
        let uses = [use_of(1, 80.0, 0), use_of(2, 80.0, 0)];
        let b = bands(&uses, &g, Kind::Load, None);
        assert_eq!(b.top[0].value, APP_MAX_PCT);
        assert_eq!(b.driver, None, "no device load, no driver band value");
    }

    #[test]
    fn idle_app_reads_zero() {
        let g = groups(&["game", "editor"]);
        let uses = [use_of(1, 40.0, 500), use_of(2, 0.0, 100)];
        let b = bands(&uses, &g, Kind::Load, Some(40.0));
        assert_eq!(b.top.len(), 2);
        assert_eq!(&*b.top[1].group.key, "exe:editor");
        assert_eq!(b.top[1].value, 0.0);
    }

    #[test]
    fn vram_driver_band_is_used_minus_all_processes() {
        let g = groups(&["game", "obs"]);
        // pid 9 is unknown: its 100 bytes stay in "Other / driver".
        let uses = [
            use_of(1, 0.0, 600),
            use_of(2, 0.0, 200),
            use_of(9, 0.0, 100),
        ];
        let b = bands(&uses, &g, Kind::Vram, Some(1000.0));
        assert_eq!(b.top[0].value, 600.0);
        assert_eq!(b.top[1].value, 200.0);
        assert_eq!(b.driver, Some(200.0));
        let stack = b.top.iter().map(|a| a.value).sum::<f64>() + b.other + b.driver.unwrap();
        assert_eq!(stack, 1000.0, "the stack sums to the device's used VRAM");
        // The processes sum to more than used: "Other / driver" is 0, nothing is rescaled.
        let b = bands(&uses, &g, Kind::Vram, Some(700.0));
        assert_eq!(b.driver, Some(0.0));
        assert_eq!(b.top[0].value, 600.0);
    }

    #[test]
    fn top_20_then_other_apps() {
        let names: Vec<String> = (0..25).map(|i| format!("p{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let g = groups(&refs);
        let uses: Vec<ProcUse> = (1..=25u32)
            .map(|pid| use_of(pid, f64::from(pid), u64::from(26 - pid)))
            .collect();
        let load = bands(&uses, &g, Kind::Load, Some(400.0));
        assert_eq!(load.top.len(), TOP_GROUPS);
        assert_eq!(&*load.top[0].group.key, "exe:p24");
        // pids 1 to 5 are outside the load top 20: 1 + 2 + 3 + 4 + 5.
        assert_eq!(load.other, 15.0);
        assert_eq!(load.other_processes, 5);
        // The VRAM top 20 is ranked apart: pid 1 has the most VRAM.
        let vram = bands(&uses, &g, Kind::Vram, None);
        assert_eq!(&*vram.top[0].group.key, "exe:p00");
        assert_eq!(vram.other, 15.0);
    }

    #[test]
    fn prune_keeps_shown_devices() {
        let mut state = GpuGroups::default();
        let key = |k: &str| (Arc::<str>::from("0000:01:00.0"), Arc::<str>::from(k));
        state
            .devices
            .insert(key("a"), (DeviceIdx(5), Arc::from("a")));
        state
            .devices
            .insert(key("b"), (DeviceIdx(6), Arc::from("b")));
        state
            .shown
            .insert((DeviceIdx(1), MetricId::GPU_GROUP), vec![DeviceIdx(5)]);
        state
            .shown
            .insert((DeviceIdx(1), MetricId::VRAM_GROUP), vec![DeviceIdx(5)]);
        state.prune();
        assert_eq!(state.devices.len(), 1);
        assert!(state.devices.contains_key(&key("a")));
    }
}
