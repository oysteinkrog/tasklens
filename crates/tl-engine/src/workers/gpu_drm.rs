//! The `GpuDrm` worker for AMD and Intel cards (ADR-004, ADR-008, ADR-016, PERF-063 to
//! PERF-069, PERF-075).
//!
//! It reads the tl-collect DRM readers at 1 Hz while a GPU page is shown or history records,
//! and not at all otherwise. Cards are enumerated again every 5 s; `amdgpu`, `i915` and `xe`
//! cards are kept (NVIDIA belongs to `GpuNvidia`). When the card set changes, a `Devices` list
//! and a `GpuInfo` go out through `take_extra` before the sample.
//!
//! AMD encode and decode come from the `gpu_metrics` VCN activity when the blob has it, else
//! from two DRM fdinfo scans. Intel engines, memory, encode and decode always come from fdinfo,
//! and Intel power from the hwmon energy counter. One fdinfo walk per tick serves every card and
//! the per-process VRAM rows, which go out while the processes page shows a GPU column or history
//! records. AMD and Intel cards have no UUID, so their identity is `uuid: [0; 16]` plus the PCI slot
//! (R-166). Everything here is read-only.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use smallvec::SmallVec;
use tl_collect::SysRoot;
use tl_collect::counters::Clock;
use tl_collect::gpu_drm::intel::{
    IntelDriver, IntelSample, fdinfo_memory, i915_engine_name, read_intel,
};
use tl_collect::gpu_drm::{
    AmdSample, DrmCard, FdinfoScan, discover_cards, engine_busy, engine_busy_cycles, main_temp,
    read_amd, scan,
};
use tl_collect::parse::udev::{UdevProps, pci_entry, read_udev_props};
use tl_collect::sanitize::sanitize_text;
use tl_collect::sysroot::read_trimmed;
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::system::{GpuTotals, Metric};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{
    DeviceDesc, DeviceKinds, DeviceList, DrmProcessVram, GpuInfoUpdate, GpuSample, GpuStaticInfo,
    SourceUpdate, WorkerError,
};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::EnumClock;

pub const PERIOD: Duration = Duration::from_secs(1);

const AMD_DRIVER: &str = "amdgpu";
const NAME_MAX: usize = 256;
const GPU_UDEV_KEYS: [&str; 1] = ["ID_MODEL_FROM_DATABASE"];

/// The GPU rate table (ADR-008 row "GPU"): 1 Hz while a GPU page is shown, per-process GPU
/// values are shown or recording is on.
pub fn gpu_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.gpus || wants_processes(d) {
        Some(PERIOD)
    } else {
        None
    }
}

/// Per-process VRAM rows are sent while a visible surface shows per-process GPU values or
/// history records, as for NVIDIA.
pub fn wants_processes(d: &Demand) -> bool {
    (d.any_surface_visible && d.gpu_process) || d.recording
}

/// Dedicated memory of each client in `scan` on one of `cards`, one row per client. The scan is
/// already de-duplicated by `drm-client-id`, so a client shared by several fds or processes
/// counts once. A client without a VRAM key is skipped.
///
/// The value is `drm-memory-vram` (amdgpu before the common keys), else the resident size of
/// the dedicated region (`vram`, `vram0` for xe, `local0` for i915), else its total size.
pub fn process_vram(scan: &FdinfoScan, cards: &[DrmCard], out: &mut Vec<DrmProcessVram>) {
    out.clear();
    for c in &scan.clients {
        let Some(card) = cards.iter().find(|k| k.pci_slot == c.pdev) else {
            continue;
        };
        let region = |list: &[(Arc<str>, u64)]| {
            list.iter()
                .find(|(n, _)| VRAM_REGIONS.contains(&&**n))
                .map(|&(_, v)| v)
        };
        let vram = region(&c.memory_bytes)
            .or_else(|| region(&c.resident_bytes))
            .or_else(|| region(&c.total_bytes));
        if let Some(vram_bytes) = vram {
            out.push(DrmProcessVram {
                pdev: Arc::clone(&card.pci_slot),
                pid: c.pid,
                vram_bytes,
            });
        }
    }
}

/// Names of the dedicated memory region: amdgpu, xe, i915 (R-275).
const VRAM_REGIONS: [&str; 3] = ["vram", "vram0", "local0"];

/// The reader state of one card, reused every sample.
#[derive(Clone, Debug)]
enum CardState {
    Amd(AmdSample),
    Intel {
        drv: IntelDriver,
        sample: IntelSample,
        /// The card has `device/tile0` or `device/mem_info_vram_total`.
        discrete: bool,
        /// `energy_uj` and the time of the last sample; `None` makes the next power `WarmingUp`.
        prev_energy: Option<(u64, TimeNs)>,
    },
}

impl CardState {
    /// `None` for a card this worker does not read.
    fn new(root: &SysRoot, card: &DrmCard) -> Option<Self> {
        if card.driver.as_deref() == Some(AMD_DRIVER) {
            return Some(Self::Amd(AmdSample::default()));
        }
        let drv = IntelDriver::from_card(card)?;
        let device = root.sys.join(format!("class/drm/card{}/device", card.card));
        let discrete = device.join("tile0").exists() || device.join("mem_info_vram_total").exists();
        Some(Self::Intel {
            drv,
            sample: IntelSample::default(),
            discrete,
            prev_energy: None,
        })
    }

    fn read(&mut self, card: &DrmCard, root: &SysRoot) {
        match self {
            Self::Amd(amd) => read_amd(card, &root.sys, amd),
            Self::Intel { drv, sample, .. } => read_intel(card, &root.sys, *drv, sample),
        }
    }

    /// An AMD card with VCN activity in `gpu_metrics` needs no fdinfo; every Intel card does.
    fn needs_fdinfo(&self) -> bool {
        match self {
            Self::Amd(amd) => !has_vcn(amd),
            Self::Intel { .. } => true,
        }
    }
}

pub struct GpuDrmSource {
    root: SysRoot,
    /// The `amdgpu`, `i915` and `xe` cards of the last enumeration, sorted by card number.
    cards: Vec<DrmCard>,
    /// When the cards are enumerated again.
    enum_clock: EnumClock,
    /// One per card.
    state: Vec<CardState>,
    /// The last fdinfo scan; `None` until the first scan and after `reset_baselines`.
    prev_scan: Option<FdinfoScan>,
    /// Reused for the next fdinfo scan.
    cur_scan: FdinfoScan,
    /// `Devices` then `GpuInfo`, waiting for `take_extra`.
    pending_extra: VecDeque<SourceUpdate>,
    /// `wants_processes` of the demand `period` saw last.
    processes: Cell<bool>,
    /// Reused for the per-process rows.
    vram: Vec<DrmProcessVram>,
}

impl GpuDrmSource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            cards: Vec::new(),
            enum_clock: EnumClock::default(),
            state: Vec::new(),
            prev_scan: None,
            cur_scan: FdinfoScan::default(),
            pending_extra: VecDeque::new(),
            processes: Cell::new(false),
            vram: Vec::new(),
        }
    }

    /// Sends per-process rows from the next sample on. `period` sets it from the demand.
    pub fn set_processes(&mut self, on: bool) {
        self.processes.set(on);
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        let t_ns = clock.boot_ns;
        if self.enum_clock.due(t_ns) {
            self.enumerate(t_ns);
        }
        if self.cards.is_empty() {
            // No AMD or Intel card: an empty `Gpu` sample would clear the NVIDIA rows in the
            // reducer.
            let idle = SourceUpdate::Health {
                id: SourceId::GpuDrm,
                state: SourceState::Off,
                last_error: None,
                t_ns,
            };
            return (self, idle);
        }

        for (card, state) in self.cards.iter().zip(&mut self.state) {
            state.read(card, &self.root);
        }

        // One fdinfo walk serves every card that needs it and the process rows.
        let processes = self.processes.get();
        let need_fdinfo = processes || self.state.iter().any(CardState::needs_fdinfo);
        if need_fdinfo {
            scan(&self.root, &mut self.cur_scan);
        }
        let drm_processes = processes.then(|| {
            process_vram(&self.cur_scan, &self.cards, &mut self.vram);
            self.vram.clone()
        });

        let mut raw = SmallVec::<[(Arc<str>, f32); 8]>::new();
        let mut busy = SmallVec::<[(Arc<str>, f32); 8]>::new();
        let mut devices = Vec::with_capacity(self.cards.len());
        for (card, state) in self.cards.iter().zip(&mut self.state) {
            let pdev = &*card.pci_slot;
            let t = match state {
                CardState::Amd(amd) => {
                    let fdinfo = match (&self.prev_scan, has_vcn(amd)) {
                        (Some(prev), false) => {
                            engine_busy(prev, &self.cur_scan, pdev, &mut busy);
                            Some(&busy[..])
                        }
                        _ => None,
                    };
                    totals(amd, fdinfo)
                }
                CardState::Intel {
                    drv,
                    sample,
                    discrete,
                    prev_energy,
                } => {
                    let fdinfo = match &self.prev_scan {
                        Some(prev) => {
                            intel_busy(*drv, prev, &self.cur_scan, pdev, &mut raw, &mut busy);
                            Some(&busy[..])
                        }
                        None => None,
                    };
                    let mem = fdinfo_memory(&self.cur_scan, pdev);
                    let discrete = *discrete || mem.1.is_some();
                    let t = intel(sample, *prev_energy, t_ns, fdinfo, mem, discrete);
                    *prev_energy = sample.energy_uj.map(|e| (e, t_ns));
                    t
                }
            };
            devices.push((identity(card), t));
        }
        if need_fdinfo {
            let cur = std::mem::take(&mut self.cur_scan);
            self.cur_scan = self.prev_scan.replace(cur).unwrap_or_default();
        }

        let sample = GpuSample {
            t_ns,
            devices,
            processes: None,
            drm_processes,
        };
        (self, SourceUpdate::Gpu(sample))
    }

    /// Re-reads the card list. When the `amdgpu`, `i915` and `xe` set changed, queues `Devices`
    /// and `GpuInfo`.
    fn enumerate(&mut self, t_ns: u64) {
        let (cards, state): (Vec<DrmCard>, Vec<CardState>) = discover_cards(&self.root.sys)
            .into_iter()
            .filter_map(|c| {
                let state = CardState::new(&self.root, &c)?;
                Some((c, state))
            })
            .unzip();
        if cards == self.cards {
            return;
        }
        self.cards = cards;
        self.state = state;

        let mut props = UdevProps::default();
        let mut entries = Vec::with_capacity(self.cards.len());
        let mut infos = Vec::with_capacity(self.cards.len());
        for (card, state) in self.cards.iter().zip(&mut self.state) {
            state.read(card, &self.root);
            let (vendor, mem_total, power_limit_uw) = match state {
                CardState::Amd(amd) => ("AMD", amd.vram_total.unwrap_or(0), amd.power_cap_uw),
                // No sysfs file gives the Intel VRAM total (R-164); the samples carry fdinfo's.
                CardState::Intel { sample, .. } => ("Intel", 0, sample.power_max_uw),
            };
            let name = display_name(&self.root, card, vendor, &mut props);
            let key: Arc<str> = Arc::from(format!("gpu:{}", card.pci_slot));
            entries.push(DeviceDesc {
                identity: identity(card),
                display_name: Arc::clone(&name),
                parent: None,
                persistent_key: Arc::clone(&key),
                net_kind: None,
            });
            infos.push(GpuStaticInfo {
                identity: identity(card),
                name,
                mem_total,
                power_limit_mw: power_limit_uw.and_then(|uw| u32::try_from(uw / 1000).ok()),
                driver: driver_text(&self.root, card.driver.as_deref().unwrap_or_default()),
                nvml: Arc::from(""),
                supports_process_util: false,
                persistent_key: key,
            });
        }
        self.pending_extra
            .push_back(SourceUpdate::Devices(DeviceList {
                t_ns,
                kinds: DeviceKinds::GPUS,
                entries,
            }));
        self.pending_extra
            .push_back(SourceUpdate::GpuInfo(GpuInfoUpdate { devices: infos }));
    }
}

impl Source for GpuDrmSource {
    fn id(&self) -> SourceId {
        SourceId::GpuDrm
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        self.processes.set(wants_processes(d));
        gpu_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    /// The next fdinfo-based values and the next Intel power are `WarmingUp`.
    fn reset_baselines(&mut self) {
        self.prev_scan = None;
        for state in &mut self.state {
            if let CardState::Intel { prev_energy, .. } = state {
                *prev_energy = None;
            }
        }
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending_extra.pop_front()
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::GpuDrm,
        name: "gpu-drm",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `GpuDrmSource` per generation, built on the blocking pool, then `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| GpuDrmSource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(src) => src,
                Err(JoinError::Cancelled(reason)) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    return Outcome::Cancelled(reason);
                }
                Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
                Err(JoinError::PolledAfterCompletion) => {
                    return Outcome::Err(WorkerError::Other("job polled after completion".into()));
                }
            };
            source::run(cx, io, src).await
        }
    })
}

/// AMD and Intel cards have no UUID (R-166): the PCI slot in sysfs form names the card.
pub fn identity(card: &DrmCard) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid: [0; 16],
        pci_bus_id: Arc::clone(&card.pci_slot),
    }
}

/// Device totals from one AMD sample and the fdinfo busy percent per engine. An empty `busy`
/// list is the first fdinfo scan: encode and decode are `WarmingUp` unless VCN activity is in
/// the `gpu_metrics` blob.
pub fn amd_totals(s: &AmdSample, busy: &[(Arc<str>, f32)]) -> GpuTotals {
    totals(s, (!busy.is_empty()).then_some(busy))
}

/// `busy` is `None` before the second fdinfo scan. Once warm, an engine with no clients is 0.
fn totals(s: &AmdSample, busy: Option<&[(Arc<str>, f32)]>) -> GpuTotals {
    let vcn = s
        .metrics
        .as_ref()
        .and_then(|m| m.vcn_activity.first())
        .map(|&v| f32::from(v) / 100.0);
    let (enc_pct, dec_pct) = match (vcn, busy) {
        // VCN is one unified engine on RDNA 2 and later (PERF-065).
        (Some(v), _) => (Metric::valid(v), Metric::valid(v)),
        (None, Some(busy)) => (engine(busy, "enc"), engine(busy, "dec")),
        (None, None) => (
            Metric::with(Validity::WarmingUp),
            Metric::with(Validity::WarmingUp),
        ),
    };
    GpuTotals {
        device: DeviceIdx(0),
        util_pct: opt(s.gpu_busy_pct.map(f32::from)),
        mem_util_pct: opt(s.mem_busy_pct.map(f32::from)),
        mem_used: opt(s.vram_used),
        mem_total: opt(s.vram_total),
        shared_used: opt(s.gtt_used),
        shared_total: opt(s.gtt_total),
        enc_pct,
        dec_pct,
        temp_c: opt(main_temp(s).map(|(_, milli)| milli as f32 / 1000.0)),
        power_w: opt(s.power_uw.map(|uw| uw as f32 / 1e6)),
        power_limit_w: opt(s.power_cap_uw.map(|uw| uw as f32 / 1e6)),
        clock_gfx_mhz: opt(s.sclk_mhz),
        clock_mem_mhz: opt(s.mclk_mhz),
        fan_pct: opt(s.fan_pwm.map(|pwm| f32::from(pwm) / 255.0 * 100.0)),
        pcie_rx_bps: Metric::with(Validity::Unsupported),
        pcie_tx_bps: Metric::with(Validity::Unsupported),
    }
}

/// Device totals for one Intel card. `prev_energy` is the energy counter and time of the last
/// sample. `busy` holds the busy percent per engine under the i915 names (render, copy, video,
/// video-enhance, compute) and is empty before the second fdinfo scan. `mem` is
/// `fdinfo_memory`: (local resident, local total, system resident).
pub fn intel_totals(
    s: &IntelSample,
    prev_energy: Option<(u64, TimeNs)>,
    now: TimeNs,
    busy: &[(Arc<str>, f32)],
    mem: (Option<u64>, Option<u64>, Option<u64>),
    discrete: bool,
) -> GpuTotals {
    intel(
        s,
        prev_energy,
        now,
        (!busy.is_empty()).then_some(busy),
        mem,
        discrete,
    )
}

/// `busy` is `None` before the second fdinfo scan. Once warm, an engine with no clients is 0.
fn intel(
    s: &IntelSample,
    prev_energy: Option<(u64, TimeNs)>,
    now: TimeNs,
    busy: Option<&[(Arc<str>, f32)]>,
    (local_used, local_total, system_used): (Option<u64>, Option<u64>, Option<u64>),
    discrete: bool,
) -> GpuTotals {
    let (util_pct, video) = match busy {
        // Utilization is the busiest engine (PERF-063).
        Some(busy) => (
            Metric::valid(busy.iter().map(|&(_, p)| p).fold(0.0, f32::max)),
            engine(busy, "video"),
        ),
        None => (
            Metric::with(Validity::WarmingUp),
            Metric::with(Validity::WarmingUp),
        ),
    };
    let power_w = match (s.energy_uj, prev_energy) {
        (None, _) => Metric::with(Validity::Unsupported),
        // µJ over µs is watts. A counter that went backwards waits one more sample.
        (Some(e), Some((prev, t))) if e >= prev && now > t => {
            Metric::valid(((e - prev) as f64 / ((now - t) as f64 / 1000.0)) as f32)
        }
        (Some(_), _) => Metric::with(Validity::WarmingUp),
    };
    let (mem_used, mem_total, temp_c) = if discrete {
        (
            opt(local_used),
            opt(local_total),
            opt(s.temp_mc.map(|mc| mc as f32 / 1000.0)),
        )
    } else {
        (
            Metric::with(Validity::Unsupported),
            Metric::with(Validity::Unsupported),
            Metric::with(Validity::Unsupported),
        )
    };
    GpuTotals {
        device: DeviceIdx(0),
        util_pct,
        mem_util_pct: Metric::with(Validity::Unsupported),
        mem_used,
        mem_total,
        shared_used: opt(system_used),
        shared_total: Metric::with(Validity::Unsupported),
        // Intel video engines both encode and decode (PERF-065).
        enc_pct: video,
        dec_pct: video,
        temp_c,
        power_w,
        power_limit_w: opt(s.power_max_uw.map(|uw| uw as f32 / 1e6)),
        clock_gfx_mhz: opt(s.act_freq_mhz),
        clock_mem_mhz: Metric::with(Validity::Unsupported),
        fan_pct: Metric::with(Validity::Unsupported),
        pcie_rx_bps: Metric::with(Validity::Unsupported),
        pcie_tx_bps: Metric::with(Validity::Unsupported),
    }
}

/// Busy percent per engine of one Intel card under the i915 engine names: `drm-engine-*` time
/// for i915, `drm-cycles-*` for xe (PERF-075). Other engine keys are dropped.
fn intel_busy(
    drv: IntelDriver,
    prev: &FdinfoScan,
    cur: &FdinfoScan,
    pdev: &str,
    raw: &mut SmallVec<[(Arc<str>, f32); 8]>,
    out: &mut SmallVec<[(Arc<str>, f32); 8]>,
) {
    out.clear();
    match drv {
        IntelDriver::I915 => {
            engine_busy(prev, cur, pdev, raw);
            out.extend(raw.drain(..).filter(|(k, _)| i915_engine_name(k).is_some()));
        }
        IntelDriver::Xe => {
            engine_busy_cycles(prev, cur, pdev, raw);
            out.extend(
                raw.drain(..)
                    .filter_map(|(k, p)| Some((Arc::from(xe_engine_name(&k)?), p))),
            );
        }
    }
}

/// The i915 engine name of an xe engine class.
fn xe_engine_name(class: &str) -> Option<&'static str> {
    match class {
        "rcs" => Some("render"),
        "bcs" => Some("copy"),
        "vcs" => Some("video"),
        "vecs" => Some("video-enhance"),
        "ccs" => Some("compute"),
        _ => None,
    }
}

fn engine(busy: &[(Arc<str>, f32)], name: &str) -> Metric<f32> {
    let pct = busy
        .iter()
        .find(|(n, _)| &**n == name)
        .map_or(0.0, |&(_, p)| p);
    Metric::valid(pct)
}

fn has_vcn(s: &AmdSample) -> bool {
    s.metrics
        .as_ref()
        .is_some_and(|m| !m.vcn_activity.is_empty())
}

fn opt<T: Copy + Default>(v: Option<T>) -> Metric<T> {
    v.map_or(Metric::with(Validity::Unsupported), Metric::valid)
}

/// `ID_MODEL_FROM_DATABASE` of the card's PCI udev entry, else "<vendor> GPU <device id>".
fn display_name(root: &SysRoot, card: &DrmCard, vendor: &str, props: &mut UdevProps) -> Arc<str> {
    let path = root.run_path(&format!("udev/data/{}", pci_entry(&card.pci_slot)));
    let mut clean = String::new();
    if read_udev_props(&path, &GPU_UDEV_KEYS, props).is_ok()
        && let Some(model) = props.get("ID_MODEL_FROM_DATABASE")
    {
        sanitize_text(model.as_bytes(), NAME_MAX, &mut clean);
    }
    if clean.is_empty() {
        return Arc::from(format!("{vendor} GPU {:04x}", card.device_id));
    }
    Arc::from(clean)
}

/// "<driver> <module version>", or the driver alone when the module has no version file.
fn driver_text(root: &SysRoot, driver: &str) -> Arc<str> {
    let mut buf = Vec::new();
    let file = root.sys.join(format!("module/{driver}/version"));
    let mut clean = String::new();
    if let Ok(text) = read_trimmed(&file, &mut buf) {
        sanitize_text(text, NAME_MAX, &mut clean);
    }
    if clean.is_empty() {
        Arc::from(driver)
    } else {
        Arc::from(format!("{driver} {clean}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_collect::gpu_drm::{DrmClient, GpuMetrics};

    #[test]
    fn period_rules() {
        let s = EngineSettings::default();
        assert_eq!(gpu_period(&Demand::default(), &s), None);
        let gpus = Demand {
            gpus: true,
            ..Demand::default()
        };
        assert_eq!(gpu_period(&gpus, &s), Some(Duration::from_secs(1)));
        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        assert_eq!(gpu_period(&recording, &s), Some(Duration::from_secs(1)));
    }

    #[test]
    fn processes_need_visible_gpu_column() {
        let s = EngineSettings::default();
        let hidden = Demand {
            gpu_process: true,
            ..Demand::default()
        };
        assert!(!wants_processes(&hidden));
        assert_eq!(gpu_period(&hidden, &s), None);
        let shown = Demand {
            any_surface_visible: true,
            ..hidden
        };
        assert!(wants_processes(&shown));
        assert_eq!(gpu_period(&shown, &s), Some(PERIOD));
        // A GPU page alone does not send process rows.
        let gpus = Demand {
            any_surface_visible: true,
            gpus: true,
            ..Demand::default()
        };
        assert!(!wants_processes(&gpus));
    }

    fn card(pci_slot: &str, driver: &str) -> DrmCard {
        DrmCard {
            card: 0,
            vendor: 0,
            device_id: 0,
            pci_slot: Arc::from(pci_slot),
            driver: Some(Arc::from(driver)),
            hwmon: None,
        }
    }

    /// `process_vram` over the `drm-vram` fixture, sorted by (pdev, pid, bytes).
    fn fixture_vram(cards: &[DrmCard]) -> Vec<(String, u32, u64)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-vram");
        let mut fd_scan = FdinfoScan::default();
        scan(&SysRoot::at(&dir), &mut fd_scan);
        assert!(!fd_scan.partial);
        let mut out = Vec::new();
        process_vram(&fd_scan, cards, &mut out);
        let mut rows: Vec<_> = out
            .into_iter()
            .map(|r| (r.pdev.to_string(), r.pid, r.vram_bytes))
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn amdgpu_fdinfo_vram() {
        let rows = fixture_vram(&[card("0000:03:00.0", "amdgpu")]);
        // Client 40 is open on fds 3 and 4 of pid 3000 and fd 7 of pid 3001: it counts once,
        // with `drm-memory-vram`. Client 41 has only the common keys: resident wins over total.
        // pid 3004 is on a card this worker does not read.
        let shared = rows.iter().filter(|r| r.2 == 64 * MIB).count();
        assert_eq!(shared, 1, "{rows:?}");
        assert!(
            rows.contains(&("0000:03:00.0".into(), 3000, 16 * MIB)),
            "{rows:?}"
        );
        assert_eq!(rows.len(), 2, "{rows:?}");
        let total: u64 = rows.iter().map(|r| r.2).sum();
        assert_eq!(total, 80 * MIB);
    }

    #[test]
    fn intel_fdinfo_vram() {
        let rows = fixture_vram(&[card("0000:00:02.0", "i915"), card("0000:04:00.0", "xe")]);
        // i915 `local0` and xe `vram0`; the xe client with system memory only has no row.
        assert_eq!(
            rows,
            [
                ("0000:00:02.0".into(), 3002, 8 * MIB),
                ("0000:04:00.0".into(), 3003, 20 * MIB),
            ]
        );
    }

    #[test]
    fn process_rows_follow_demand() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-vram");
        let mut src = GpuDrmSource::new(SysRoot::at(&dir));
        let clock = |boot_ns| Clock {
            boot_ns,
            ..Clock::now()
        };
        src.cards = vec![card("0000:04:00.0", "xe")];
        src.state = vec![CardState::Intel {
            drv: IntelDriver::Xe,
            sample: IntelSample::default(),
            discrete: true,
            prev_energy: None,
        }];
        // The fixture has no sysfs cards: mark the enumeration done to keep the hand-made list.
        assert!(src.enum_clock.due(SEC));
        let shown = Demand {
            any_surface_visible: true,
            gpu_process: true,
            ..Demand::default()
        };
        src.period(&shown, &EngineSettings::default());
        let (mut src, up) = src.sample_at(clock(SEC));
        let SourceUpdate::Gpu(g) = up else {
            panic!("{up:?}");
        };
        let rows = g.drm_processes.expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].pid, rows[0].vram_bytes), (3003, 20 * MIB));

        src.set_processes(false);
        let (_, up) = src.sample_at(clock(2 * SEC));
        let SourceUpdate::Gpu(g) = up else {
            panic!("{up:?}");
        };
        assert!(g.drm_processes.is_none());
    }

    #[test]
    fn vcn_both_enc_dec() {
        let s = AmdSample {
            metrics: Some(GpuMetrics {
                vcn_activity: SmallVec::from_slice(&[2500]),
                ..GpuMetrics::default()
            }),
            ..AmdSample::default()
        };
        let t = amd_totals(&s, &[]);
        assert_eq!(t.enc_pct, Metric::valid(25.0));
        assert_eq!(t.dec_pct, Metric::valid(25.0));
        // Missing inputs are `Unsupported`, never 0.
        assert_eq!(t.util_pct.validity, Validity::Unsupported);
        assert_eq!(t.temp_c.validity, Validity::Unsupported);
    }

    #[test]
    fn fdinfo_enc_dec_fallback() {
        let s = AmdSample::default();
        let busy = [(Arc::<str>::from("enc"), 5.0)];
        let t = amd_totals(&s, &busy);
        assert_eq!(t.enc_pct, Metric::valid(5.0));
        assert_eq!(t.dec_pct, Metric::valid(0.0));

        let t = amd_totals(&s, &[]);
        assert_eq!(t.enc_pct.validity, Validity::WarmingUp);
        assert_eq!(t.dec_pct.validity, Validity::WarmingUp);

        // Warm, but no client on the card: both 0.
        let t = totals(&s, Some(&[]));
        assert_eq!(t.enc_pct, Metric::valid(0.0));
        assert_eq!(t.dec_pct, Metric::valid(0.0));
    }

    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;
    const SEC: TimeNs = 1_000_000_000;

    fn busy(list: &[(&str, f32)]) -> Vec<(Arc<str>, f32)> {
        list.iter().map(|&(n, p)| (Arc::from(n), p)).collect()
    }

    #[test]
    fn intel_util_is_max_engine() {
        let s = IntelSample::default();
        let b = busy(&[("render", 40.0), ("video", 10.0), ("copy", 5.0)]);
        let t = intel_totals(&s, None, SEC, &b, (None, None, None), false);
        assert_eq!(t.util_pct, Metric::valid(40.0));
        assert_eq!(t.enc_pct, Metric::valid(10.0));
        assert_eq!(t.dec_pct, Metric::valid(10.0));
        assert_eq!(t.mem_util_pct.validity, Validity::Unsupported);
        assert_eq!(t.fan_pct.validity, Validity::Unsupported);
        assert_eq!(t.clock_mem_mhz.validity, Validity::Unsupported);

        // No engine values yet.
        let t = intel_totals(&s, None, SEC, &[], (None, None, None), false);
        assert_eq!(t.util_pct.validity, Validity::WarmingUp);
        assert_eq!(t.enc_pct.validity, Validity::WarmingUp);

        // Warm, but no client on the card.
        let t = intel(&s, None, SEC, Some(&[]), (None, None, None), false);
        assert_eq!(t.util_pct, Metric::valid(0.0));
        assert_eq!(t.dec_pct, Metric::valid(0.0));
    }

    #[test]
    fn intel_xe_engine_names() {
        let client = |id: u64, cycles: u64, total: u64| DrmClient {
            driver: Arc::from("xe"),
            pdev: Arc::from("0000:03:00.0"),
            client_id: id,
            cycles: [("rcs", cycles), ("vcs", cycles / 2), ("xyz", cycles)]
                .iter()
                .map(|&(n, v)| (Arc::from(n), v))
                .collect(),
            total_cycles: [("rcs", total), ("vcs", total), ("xyz", total)]
                .iter()
                .map(|&(n, v)| (Arc::from(n), v))
                .collect(),
            ..DrmClient::default()
        };
        let scan = |cycles, total| FdinfoScan {
            clients: vec![client(1, cycles, total)],
            partial: false,
            t_ns: 0,
        };
        let (mut raw, mut out) = (SmallVec::new(), SmallVec::new());
        intel_busy(
            IntelDriver::Xe,
            &scan(0, 0),
            &scan(600, 1000),
            "0000:03:00.0",
            &mut raw,
            &mut out,
        );
        assert_eq!(&out[..], &busy(&[("render", 60.0), ("video", 30.0)])[..]);
    }

    #[test]
    fn intel_power_from_energy() {
        let s = |uj| IntelSample {
            energy_uj: Some(uj),
            power_max_uw: Some(28_000_000),
            ..IntelSample::default()
        };
        let mem = (None, None, None);
        // First sample.
        let t = intel_totals(&s(5_000_000), None, SEC, &[], mem, false);
        assert_eq!(t.power_w.validity, Validity::WarmingUp);
        assert_eq!(t.power_limit_w, Metric::valid(28.0));

        let prev = Some((5_000_000, SEC));
        let t = intel_totals(&s(17_000_000), prev, 2 * SEC, &[], mem, false);
        assert_eq!(t.power_w.validity, Validity::Valid);
        assert!((t.power_w.value - 12.0).abs() < 1e-4, "{:?}", t.power_w);

        // Half a second at 12 W is 6 J.
        let t = intel_totals(&s(11_000_000), prev, SEC + SEC / 2, &[], mem, false);
        assert!((t.power_w.value - 12.0).abs() < 1e-4, "{:?}", t.power_w);

        // Counter wrap.
        let t = intel_totals(&s(1_000), prev, 2 * SEC, &[], mem, false);
        assert_eq!(t.power_w.validity, Validity::WarmingUp);

        // No counter.
        let t = intel_totals(&IntelSample::default(), prev, 2 * SEC, &[], mem, false);
        assert_eq!(t.power_w.validity, Validity::Unsupported);
        assert_eq!(t.power_limit_w.validity, Validity::Unsupported);
    }

    #[test]
    fn intel_discrete_memory() {
        let s = IntelSample {
            temp_mc: Some(48_000),
            ..IntelSample::default()
        };
        let mem = (Some(3_145_728), Some(8 * GIB), None);
        let t = intel_totals(&s, None, SEC, &[], mem, true);
        assert_eq!(t.mem_used, Metric::valid(3_145_728));
        assert_eq!(t.mem_total, Metric::valid(8 * GIB));
        assert_eq!(t.temp_c, Metric::valid(48.0));
        assert_eq!(t.shared_used.validity, Validity::Unsupported);
        assert_eq!(t.shared_total.validity, Validity::Unsupported);
    }

    #[test]
    fn intel_igpu_shared() {
        // An iGPU hwmon has no temperature, and one that had would still not be shown.
        let s = IntelSample {
            temp_mc: Some(48_000),
            act_freq_mhz: Some(1300),
            ..IntelSample::default()
        };
        let t = intel_totals(&s, None, SEC, &[], (None, None, Some(512 * MIB)), false);
        assert_eq!(t.shared_used, Metric::valid(512 * MIB));
        assert_eq!(t.shared_total.validity, Validity::Unsupported);
        assert_eq!(t.mem_used.validity, Validity::Unsupported);
        assert_eq!(t.mem_total.validity, Validity::Unsupported);
        assert_eq!(t.temp_c.validity, Validity::Unsupported);
        assert_eq!(t.clock_gfx_mhz, Metric::valid(1300));
    }
}
