//! The `GpuDrm` worker for AMD and Intel cards (ADR-004, ADR-008, ADR-016, PERF-063 to
//! PERF-069, PERF-075).
//!
//! It reads the tl-collect DRM readers at 1 Hz while a GPU page is shown or history records,
//! and not at all otherwise. Cards are enumerated again every 5 s; `amdgpu`, `i915` and `xe`
//! cards are kept (NVIDIA belongs to `GpuNvidia`). When the card set changes, a `Devices` list
//! and a `GpuInfo` go out through `take_extra` before the sample.
//!
//! AMD encode and decode come from the `gpu_metrics` VCN activity when the blob has it, else
//! from two DRM fdinfo scans. AMD compute, DMA and JPEG always come from fdinfo (PERF-074). Intel
//! engines, memory, encode and decode always come from fdinfo, and Intel power from the hwmon
//! energy counter. One fdinfo walk per tick serves every card and the per-process VRAM rows, which
//! go out while the processes page shows a GPU column or history records, and with every 10th
//! sample while device history alone needs them (`HistoryRows`). AMD and Intel cards
//! have no UUID, so their identity is `uuid: [0; 16]` plus the PCI slot (R-166). Everything here
//! is read-only.

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
    AmdSample, AmdTemp, DrmCard, FdinfoScan, PcieLink, discover_cards, engine_busy,
    engine_busy_cycles, main_temp, read_amd, read_pcie_link, scan,
};
use tl_collect::parse::udev::{UdevProps, pci_entry, read_udev_props};
use tl_collect::sanitize::sanitize_text;
use tl_collect::sysroot::{read_trimmed, read_u64};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::system::GpuStaticInfo as DeviceStatic;
use tl_model::system::{GpuEngine, GpuEngines, GpuTotals, Metric};
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
/// values are shown, recording or device history is on.
pub fn gpu_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.gpus || d.device_history || wants_processes(d) {
        Some(PERIOD)
    } else {
        None
    }
}

/// Per-process VRAM rows are sent while a visible surface shows per-process GPU values or
/// history records, as for NVIDIA. Device history alone gets them through `HistoryRows`.
pub fn wants_processes(d: &Demand) -> bool {
    (d.any_surface_visible && d.gpu_process) || d.recording
}

/// While only device history needs process rows, a GPU worker sends them with every this many
/// samples: the process worker's history sweep period (`processes::HISTORY_PERIOD`, 10 s) over
/// the 1 s GPU period.
pub const HISTORY_ROWS_EVERY: u32 =
    (super::processes::HISTORY_PERIOD.as_secs() / PERIOD.as_secs()) as u32;

/// Process rows for device history alone (tasklens-g00wx.1, leader decision 4). The
/// `GPU_GROUP` and `VRAM_GROUP` series are stored like `CPU_GROUP`, so the 10 s and 1 min
/// tiers need a point in every 10 s with no window open. While `Demand.device_history` is on
/// and no view or recording asks for rows, `due` turns true on every `HISTORY_ROWS_EVERY`th
/// sample, the first one included. Both GPU workers use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryRows {
    /// Samples since the last one with rows.
    since: u32,
}

impl Default for HistoryRows {
    fn default() -> Self {
        HistoryRows {
            since: HISTORY_ROWS_EVERY - 1,
        }
    }
}

impl HistoryRows {
    /// Whether this sample carries process rows. `wanted`: a view or recording asks for them
    /// (`wants_processes`); `history`: `Demand.device_history`. Call it once per sample.
    pub fn due(&mut self, wanted: bool, history: bool) -> bool {
        if wanted || !history {
            *self = HistoryRows::default();
            return wanted;
        }
        self.since += 1;
        if self.since >= HISTORY_ROWS_EVERY {
            self.since = 0;
            true
        } else {
            false
        }
    }
}

/// Memory and engine use of each process in `scan` on one of `cards`: one row per (card, pid),
/// summed over that pid's clients (PROC-033). The scan is already de-duplicated by
/// `drm-client-id`, so a client shared by several fds or processes counts once. A pid whose
/// clients have no dedicated memory, no shared memory and no engine key has no row.
///
/// Dedicated memory is `drm-memory-vram` (amdgpu before the common keys), else the resident size
/// of the dedicated region (`vram`, `vram0` for xe, `local0` for i915), else its total size, else
/// 0. Shared memory is `drm-memory-gtt` (amdgpu), else the resident size of `gtt`, `system` (xe)
/// or `system0` (i915), else 0.
///
/// Engine busy percent comes from `prev`, the scan before `scan`: the delta of each
/// `drm-engine-<name>` ns over the delta of the scan time, times 100, summed over the pid's
/// clients and clamped to 0..=100. The previous ns are those of the client with the same pdev,
/// client id and pid. Without `prev`, or for a client new since `prev`, the client adds no
/// engines. xe reports cycles, not ns, so xe rows have no engines here. Encode and decode are
/// the busiest of the pid's encode and decode engines (`engine_role`).
pub fn process_vram(
    scan: &FdinfoScan,
    prev: Option<&FdinfoScan>,
    cards: &[DrmCard],
    out: &mut Vec<DrmProcessVram>,
) {
    out.clear();
    let dt = prev.map_or(0, |p| scan.t_ns.saturating_sub(p.t_ns));
    for c in &scan.clients {
        let Some(card) = cards.iter().find(|k| k.pci_slot == c.pdev) else {
            continue;
        };
        let region = |list: &[(Arc<str>, u64)], names: &[&str]| {
            list.iter()
                .find(|(n, _)| names.contains(&&**n))
                .map(|&(_, v)| v)
        };
        let vram = region(&c.memory_bytes, &VRAM_REGIONS)
            .or_else(|| region(&c.resident_bytes, &VRAM_REGIONS))
            .or_else(|| region(&c.total_bytes, &VRAM_REGIONS));
        let shared = region(&c.memory_bytes, &SHARED_REGIONS)
            .or_else(|| region(&c.resident_bytes, &SHARED_REGIONS));
        if vram.is_none() && shared.is_none() && c.engine_ns.is_empty() {
            continue;
        }
        let i = match out
            .iter()
            .position(|r| r.pid == c.pid && r.pdev == card.pci_slot)
        {
            Some(i) => i,
            None => {
                out.push(DrmProcessVram::new(Arc::clone(&card.pci_slot), c.pid, 0));
                out.len() - 1
            }
        };
        let row = &mut out[i];
        row.vram_bytes = row.vram_bytes.saturating_add(vram.unwrap_or(0));
        row.shared_bytes = row.shared_bytes.saturating_add(shared.unwrap_or(0));
        let before = prev.filter(|_| dt > 0).and_then(|p| {
            p.clients
                .iter()
                .find(|p| p.client_id == c.client_id && p.pid == c.pid && p.pdev == c.pdev)
        });
        let Some(before) = before else {
            continue;
        };
        for (name, now) in &c.engine_ns {
            let then = before
                .engine_ns
                .iter()
                .find(|(n, _)| n == name)
                .map_or(*now, |&(_, v)| v);
            // A counter that went backwards adds nothing (ADR-008).
            let pct = (now.saturating_sub(then) as f64 / dt as f64 * 100.0) as f32;
            match row.engines.iter_mut().find(|(n, _)| n == name) {
                Some((_, sum)) => *sum += pct,
                None => row.engines.push((Arc::clone(name), pct)),
            }
        }
    }
    for row in out.iter_mut() {
        for (name, pct) in &mut row.engines {
            *pct = pct.clamp(0.0, 100.0);
            let (enc, dec) = engine_role(name);
            if enc {
                row.encode_pct = Some(row.encode_pct.map_or(*pct, |e| e.max(*pct)));
            }
            if dec {
                row.decode_pct = Some(row.decode_pct.map_or(*pct, |d| d.max(*pct)));
            }
        }
    }
}

/// Whether the fdinfo engine `name` counts as (encode, decode). PROC-033 takes both from
/// `drm-engine-video*` (Intel `video`, `video-enhance`). Leader decision 2026-11-06: amdgpu
/// `drm-engine-enc` is encode and `drm-engine-dec` is decode; `enc_1` and further numbered rings
/// count as `enc`.
pub fn engine_role(name: &str) -> (bool, bool) {
    if name.starts_with("video") {
        return (true, true);
    }
    let ring = |base: &str| {
        name.strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
    };
    (ring("enc"), ring("dec"))
}

/// Names of the dedicated memory region: amdgpu, xe, i915 (R-275).
const VRAM_REGIONS: [&str; 3] = ["vram", "vram0", "local0"];

/// Names of the shared memory region: amdgpu, xe, i915 (PROC-033).
const SHARED_REGIONS: [&str; 3] = ["gtt", "system", "system0"];

/// The reader state of one card, reused every sample.
#[derive(Clone, Debug)]
enum CardState {
    Amd {
        sample: AmdSample,
        /// The fdinfo engines seen on this card so far, as a mask of `bit`.
        seen: u16,
    },
    Intel {
        drv: IntelDriver,
        sample: IntelSample,
        /// The card has `device/tile0` or `device/mem_info_vram_total`.
        discrete: bool,
        /// `energy_uj` and the time of the last sample; `None` makes the next power `WarmingUp`.
        prev_energy: Option<(u64, TimeNs)>,
        /// The fdinfo engines seen on this card so far, as a mask of `bit`.
        seen: u16,
    },
}

impl CardState {
    /// `None` for a card this worker does not read.
    fn new(root: &SysRoot, card: &DrmCard) -> Option<Self> {
        if card.driver.as_deref() == Some(AMD_DRIVER) {
            return Some(Self::Amd {
                sample: AmdSample::default(),
                seen: 0,
            });
        }
        let drv = IntelDriver::from_card(card)?;
        let device = root.sys.join(format!("class/drm/card{}/device", card.card));
        let discrete = device.join("tile0").exists() || device.join("mem_info_vram_total").exists();
        Some(Self::Intel {
            drv,
            sample: IntelSample::default(),
            discrete,
            prev_energy: None,
            seen: 0,
        })
    }

    fn read(&mut self, card: &DrmCard, root: &SysRoot) {
        match self {
            Self::Amd { sample, .. } => read_amd(card, &root.sys, sample),
            Self::Intel { drv, sample, .. } => read_intel(card, &root.sys, *drv, sample),
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
    /// One per card: its static values, built at each card scan (PERF-066 to PERF-071).
    statics: Vec<Arc<DeviceStatic>>,
    /// The last fdinfo scan; `None` until the first scan and after `reset_baselines`.
    prev_scan: Option<FdinfoScan>,
    /// Reused for the next fdinfo scan.
    cur_scan: FdinfoScan,
    /// `Devices` then `GpuInfo`, waiting for `take_extra`.
    pending_extra: VecDeque<SourceUpdate>,
    /// `wants_processes` of the demand `period` saw last.
    processes: Cell<bool>,
    /// `Demand.device_history` of the demand `period` saw last.
    history: Cell<bool>,
    /// When device history alone gets process rows.
    history_rows: HistoryRows,
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
            statics: Vec::new(),
            prev_scan: None,
            cur_scan: FdinfoScan::default(),
            pending_extra: VecDeque::new(),
            processes: Cell::new(false),
            history: Cell::new(false),
            history_rows: HistoryRows::default(),
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

        // One fdinfo walk serves every card and the process rows. Every card needs it: AMD for
        // compute, DMA and JPEG, Intel for every engine.
        let processes = self
            .history_rows
            .due(self.processes.get(), self.history.get());
        scan(&self.root, &mut self.cur_scan);
        let partial = self.cur_scan.partial;
        let drm_processes = processes.then(|| {
            process_vram(
                &self.cur_scan,
                self.prev_scan.as_ref(),
                &self.cards,
                &mut self.vram,
            );
            self.vram.clone()
        });

        let mut raw = SmallVec::<[(Arc<str>, f32); 8]>::new();
        let mut busy = SmallVec::<[(Arc<str>, f32); 8]>::new();
        let mut devices = Vec::with_capacity(self.cards.len());
        for ((card, state), info) in self.cards.iter().zip(&mut self.state).zip(&self.statics) {
            let pdev = &*card.pci_slot;
            let card_link = CardLink {
                link: read_pcie_link(&self.root.sys, card),
                info,
            };
            let t = match state {
                CardState::Amd { sample, seen } => {
                    let fdinfo = match &self.prev_scan {
                        Some(prev) => {
                            engine_busy(prev, &self.cur_scan, pdev, &mut busy);
                            *seen |= seen_mask(&busy);
                            Some(&busy[..])
                        }
                        None => None,
                    };
                    totals(sample, fdinfo, *seen, partial, card_link)
                }
                CardState::Intel {
                    drv,
                    sample,
                    discrete,
                    prev_energy,
                    seen,
                } => {
                    let fdinfo = match &self.prev_scan {
                        Some(prev) => {
                            intel_busy(*drv, prev, &self.cur_scan, pdev, &mut raw, &mut busy);
                            *seen |= seen_mask(&busy);
                            Some(&busy[..])
                        }
                        None => None,
                    };
                    let mem = fdinfo_memory(&self.cur_scan, pdev);
                    let discrete = *discrete || mem.1.is_some();
                    let fd = FdinfoEngines {
                        busy: fdinfo,
                        seen: *seen,
                        partial,
                    };
                    let t = intel(sample, *prev_energy, t_ns, fd, mem, discrete, card_link);
                    *prev_energy = sample.energy_uj.map(|e| (e, t_ns));
                    t
                }
            };
            devices.push((identity(card), t));
        }
        let cur = std::mem::take(&mut self.cur_scan);
        self.cur_scan = self.prev_scan.replace(cur).unwrap_or_default();

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
        self.statics.clear();

        let mut props = UdevProps::default();
        let mut entries = Vec::with_capacity(self.cards.len());
        let mut infos = Vec::with_capacity(self.cards.len());
        for (card, state) in self.cards.iter().zip(&mut self.state) {
            state.read(card, &self.root);
            let (vendor, mem_total, power_limit_uw) = match state {
                CardState::Amd { sample, .. } => {
                    ("AMD", sample.vram_total.unwrap_or(0), sample.power_cap_uw)
                }
                // No sysfs file gives the Intel VRAM total (R-164); the samples carry fdinfo's.
                CardState::Intel { sample, .. } => ("Intel", 0, sample.power_max_uw),
            };
            self.statics.push(card_static(
                card,
                state,
                read_pcie_link(&self.root.sys, card),
            ));
            let name = display_name(&self.root, card, vendor, &mut props);
            let key: Arc<str> = Arc::from(format!("gpu:{}", card.pci_slot));
            entries.push(DeviceDesc {
                identity: identity(card),
                display_name: Arc::clone(&name),
                parent: None,
                persistent_key: Arc::clone(&key),
                net_kind: None,
                hot_c: None,
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
                hot_c: None,
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
        self.history.set(d.device_history);
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

/// The static values of one card (PERF-066, PERF-067, PERF-070). sysfs gives no CUDA version,
/// architecture or temperature thresholds for AMD and Intel. Only amdgpu gives the minimum power
/// limit.
fn card_static(card: &DrmCard, state: &CardState, link: PcieLink) -> Arc<DeviceStatic> {
    let (clock_gfx_max_mhz, clock_mem_max_mhz, power_min_limit_w) = match state {
        CardState::Amd { sample, .. } => (
            sample.sclk_max_mhz,
            sample.mclk_max_mhz,
            amd_power_min_limit_w(card),
        ),
        CardState::Intel { sample, .. } => (sample.max_freq_mhz, None, None),
    };
    Arc::new(DeviceStatic {
        pcie_max_gen: link.max_gen,
        pcie_max_width: link.max_width,
        clock_gfx_max_mhz,
        clock_mem_max_mhz,
        power_min_limit_w,
        ..DeviceStatic::default()
    })
}

/// amdgpu hwmon `power1_cap_min`, microwatts, in W (PERF-067).
fn amd_power_min_limit_w(card: &DrmCard) -> Option<f32> {
    let path = card.hwmon.as_ref()?.join("power1_cap_min");
    let mut buf = Vec::new();
    read_u64(&path, &mut buf).ok().map(|uw| uw as f32 / 1e6)
}

/// What one card adds to each sample: its current PCIe link and its static values.
#[derive(Clone, Copy)]
struct CardLink<'a> {
    link: PcieLink,
    info: &'a Arc<DeviceStatic>,
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
/// the `gpu_metrics` blob, and so are compute and DMA. JPEG is listed when `busy` names it.
pub fn amd_totals(s: &AmdSample, busy: &[(Arc<str>, f32)]) -> GpuTotals {
    let info = Arc::default();
    let card = CardLink {
        link: PcieLink::default(),
        info: &info,
    };
    totals(
        s,
        (!busy.is_empty()).then_some(busy),
        seen_mask(busy),
        false,
        card,
    )
}

/// The AMD engines every amdgpu card has that only fdinfo reports.
const AMD_FDINFO_ENGINES: [GpuEngine; 2] = [GpuEngine::Compute, GpuEngine::Dma];
/// AMD engines listed once fdinfo has named them on the card: cards before Vega have no JPEG.
const AMD_OPTIONAL_ENGINES: [GpuEngine; 1] = [GpuEngine::Jpeg];
/// The Intel engines every i915 and xe card has.
const INTEL_ENGINES: [GpuEngine; 3] = [GpuEngine::Render, GpuEngine::Copy, GpuEngine::Video];
/// Intel engines listed once fdinfo has named them on the card (i915 and xe print every engine
/// class the card has).
const INTEL_OPTIONAL_ENGINES: [GpuEngine; 2] = [GpuEngine::Compute, GpuEngine::VideoEnhance];

/// `busy` is `None` before the second fdinfo scan. Once warm, an engine with no clients is 0.
/// `seen` is the mask of fdinfo engines named on the card so far; `partial` is
/// `FdinfoScan::partial`.
fn totals(
    s: &AmdSample,
    busy: Option<&[(Arc<str>, f32)]>,
    seen: u16,
    partial: bool,
    card: CardLink<'_>,
) -> GpuTotals {
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
    let sensor = |kind| {
        opt(s
            .temps
            .iter()
            .find(|&&(k, _)| k == kind)
            .map(|&(_, milli)| milli as f32 / 1000.0))
    };
    let util_pct = opt(s.gpu_busy_pct.map(f32::from));
    let mem_util_pct = opt(s.mem_busy_pct.map(f32::from));
    // Graphics, Compute, Memory controller, Video Encode, Video Decode, JPEG, DMA (PERF-063).
    let mut engines = GpuEngines::none();
    engines.set(GpuEngine::Gpu, util_pct);
    engines.set(GpuEngine::MemCtl, mem_util_pct);
    engines.set(GpuEngine::Enc, enc_pct);
    engines.set(GpuEngine::Dec, dec_pct);
    set_fdinfo_engines(
        &mut engines,
        busy,
        &AMD_FDINFO_ENGINES,
        &AMD_OPTIONAL_ENGINES,
        seen,
    );
    GpuTotals {
        groups: Default::default(),
        vram_groups: Default::default(),
        device: DeviceIdx(0),
        util_pct,
        mem_util_pct,
        mem_used: opt(s.vram_used),
        mem_total: opt(s.vram_total),
        shared_used: opt(s.gtt_used),
        shared_total: opt(s.gtt_total),
        // amdgpu has no hardware reserved figure (PERF-064).
        mem_reserved: Metric::with(Validity::Unsupported),
        enc_pct,
        dec_pct,
        enc_sessions: Metric::with(Validity::Unsupported),
        enc_dec_shared: vcn.is_some(),
        temp_c: opt(main_temp(s).map(|(_, milli)| milli as f32 / 1000.0)),
        temp_edge_c: sensor(AmdTemp::Edge),
        temp_junction_c: sensor(AmdTemp::Junction),
        temp_mem_c: sensor(AmdTemp::Mem),
        power_w: opt(s.power_uw.map(|uw| uw as f32 / 1e6)),
        power_limit_w: opt(s.power_cap_uw.map(|uw| uw as f32 / 1e6)),
        energy_since_boot: Metric::with(Validity::Unsupported),
        clock_gfx_mhz: opt(s.sclk_mhz),
        clock_mem_mhz: opt(s.mclk_mhz),
        clock_video_mhz: Metric::with(Validity::Unsupported),
        pstate: Metric::with(Validity::Unsupported),
        clock_event_reasons: Metric::with(Validity::Unsupported),
        throttling: Metric::with(Validity::Unsupported),
        fan_pct: opt(s.fan_pwm.map(|pwm| f32::from(pwm) / 255.0 * 100.0)),
        fan_rpm: opt(s.fan_rpm),
        pcie_rx_bps: Metric::with(Validity::Unsupported),
        pcie_tx_bps: Metric::with(Validity::Unsupported),
        pcie_gen: opt(card.link.cur_gen),
        pcie_width: opt(card.link.cur_width),
        engines,
        fdinfo_partial: partial,
        info: Arc::clone(card.info),
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
    let fd = FdinfoEngines {
        busy: (!busy.is_empty()).then_some(busy),
        seen: seen_mask(busy),
        partial: false,
    };
    let info = Arc::default();
    let card = CardLink {
        link: PcieLink::default(),
        info: &info,
    };
    intel(s, prev_energy, now, fd, mem, discrete, card)
}

/// The fdinfo engine data of one card for one sample.
#[derive(Clone, Copy)]
struct FdinfoEngines<'a> {
    /// `None` before the second fdinfo scan. Once warm, an engine with no clients is 0.
    busy: Option<&'a [(Arc<str>, f32)]>,
    /// The fdinfo engines named on the card so far, as a mask of `bit`.
    seen: u16,
    /// `FdinfoScan::partial`.
    partial: bool,
}

fn intel(
    s: &IntelSample,
    prev_energy: Option<(u64, TimeNs)>,
    now: TimeNs,
    fd: FdinfoEngines<'_>,
    (local_used, local_total, system_used): (Option<u64>, Option<u64>, Option<u64>),
    discrete: bool,
    card: CardLink<'_>,
) -> GpuTotals {
    let busy = fd.busy;
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
    // Render/3D, Compute, Copy, Video, Video Processing (PERF-063). The video engine is listed
    // once, as Video, not as Encode and Decode.
    let mut engines = GpuEngines::none();
    set_fdinfo_engines(
        &mut engines,
        busy,
        &INTEL_ENGINES,
        &INTEL_OPTIONAL_ENGINES,
        fd.seen,
    );
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
        groups: Default::default(),
        vram_groups: Default::default(),
        device: DeviceIdx(0),
        util_pct,
        mem_util_pct: Metric::with(Validity::Unsupported),
        mem_used,
        mem_total,
        shared_used: opt(system_used),
        shared_total: Metric::with(Validity::Unsupported),
        mem_reserved: Metric::with(Validity::Unsupported),
        // Intel video engines both encode and decode (PERF-065).
        enc_pct: video,
        dec_pct: video,
        enc_sessions: Metric::with(Validity::Unsupported),
        enc_dec_shared: true,
        temp_c,
        temp_edge_c: Metric::with(Validity::Unsupported),
        temp_junction_c: Metric::with(Validity::Unsupported),
        temp_mem_c: Metric::with(Validity::Unsupported),
        power_w,
        power_limit_w: opt(s.power_max_uw.map(|uw| uw as f32 / 1e6)),
        // The hwmon energy counter, µJ since the driver loaded.
        energy_since_boot: opt(s.energy_uj.map(|uj| uj as f64 / 1e6)),
        clock_gfx_mhz: opt(s.act_freq_mhz),
        clock_mem_mhz: Metric::with(Validity::Unsupported),
        clock_video_mhz: Metric::with(Validity::Unsupported),
        pstate: Metric::with(Validity::Unsupported),
        clock_event_reasons: Metric::with(Validity::Unsupported),
        throttling: Metric::with(Validity::Unsupported),
        fan_pct: Metric::with(Validity::Unsupported),
        fan_rpm: Metric::with(Validity::Unsupported),
        pcie_rx_bps: Metric::with(Validity::Unsupported),
        pcie_tx_bps: Metric::with(Validity::Unsupported),
        pcie_gen: opt(card.link.cur_gen),
        pcie_width: opt(card.link.cur_width),
        engines,
        fdinfo_partial: fd.partial,
        info: Arc::clone(card.info),
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

/// The engine of an fdinfo engine name: the amdgpu names and the i915 names (xe classes are
/// mapped to those first). Graphics, encode and decode do not come from here.
fn fdinfo_engine(name: &str) -> Option<GpuEngine> {
    match name {
        "compute" => Some(GpuEngine::Compute),
        "dma" => Some(GpuEngine::Dma),
        "jpeg" => Some(GpuEngine::Jpeg),
        "render" => Some(GpuEngine::Render),
        "copy" => Some(GpuEngine::Copy),
        "video" => Some(GpuEngine::Video),
        "video-enhance" => Some(GpuEngine::VideoEnhance),
        _ => None,
    }
}

/// The bit of `engine` in a `seen` mask.
const fn bit(engine: GpuEngine) -> u16 {
    1 << engine as u16
}

/// The engines `busy` names, as a mask of `bit`.
fn seen_mask(busy: &[(Arc<str>, f32)]) -> u16 {
    busy.iter()
        .filter_map(|(n, _)| fdinfo_engine(n))
        .fold(0, |m, e| m | bit(e))
}

/// Sets every engine of `always`, and every engine of `optional` that is in `seen`, from
/// `busy`: `WarmingUp` before the second scan, else the engine's busy percent, 0 without
/// clients. The other engines keep their value.
fn set_fdinfo_engines(
    out: &mut GpuEngines,
    busy: Option<&[(Arc<str>, f32)]>,
    always: &[GpuEngine],
    optional: &[GpuEngine],
    seen: u16,
) {
    let listed = always
        .iter()
        .chain(optional.iter().filter(|&&e| seen & bit(e) != 0));
    for &e in listed {
        let value = match busy {
            Some(busy) => Metric::valid(
                busy.iter()
                    .find(|(n, _)| fdinfo_engine(n) == Some(e))
                    .map_or(0.0, |&(_, p)| p),
            ),
            None => Metric::with(Validity::WarmingUp),
        };
        out.set(e, value);
    }
}

fn engine(busy: &[(Arc<str>, f32)], name: &str) -> Metric<f32> {
    let pct = busy
        .iter()
        .find(|(n, _)| &**n == name)
        .map_or(0.0, |&(_, p)| p);
    Metric::valid(pct)
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
        // Hidden with device history on: totals at 1 Hz (ADR-008, ADR-017). Process rows come
        // only every 10th sample (`HistoryRows`), not from `wants_processes`.
        let history = Demand {
            device_history: true,
            ..Demand::default()
        };
        assert_eq!(gpu_period(&history, &s), Some(Duration::from_secs(1)));
        assert!(!wants_processes(&history));
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
        process_vram(&fd_scan, None, cards, &mut out);
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
        // with `drm-memory-vram`, for the pid the scan meets first. Client 41 of pid 3000 has
        // only the common keys: resident wins over total. pid 3004 is on a card this worker does
        // not read.
        let one_pid = [("0000:03:00.0".to_string(), 3000, 80 * MIB)];
        let two_pids = [
            ("0000:03:00.0".to_string(), 3000, 16 * MIB),
            ("0000:03:00.0".to_string(), 3001, 64 * MIB),
        ];
        assert!(rows == one_pid || rows == two_pids, "{rows:?}");
    }

    #[test]
    fn intel_fdinfo_vram() {
        let rows = fixture_vram(&[card("0000:00:02.0", "i915"), card("0000:04:00.0", "xe")]);
        // i915 `local0`. xe `vram0`: pid 3003 has two clients, one with system memory only, and
        // gets one row.
        assert_eq!(
            rows,
            [
                ("0000:00:02.0".into(), 3002, 8 * MIB),
                ("0000:04:00.0".into(), 3003, 20 * MIB),
            ]
        );
    }

    /// The `drm-vram` fixture scanned once, at `t_ns`.
    fn fixture_scan(t_ns: TimeNs) -> FdinfoScan {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-vram");
        let mut fd_scan = FdinfoScan::default();
        scan(&SysRoot::at(&dir), &mut fd_scan);
        fd_scan.t_ns = t_ns;
        fd_scan
    }

    #[test]
    fn fdinfo_shared_bytes() {
        let cards = [
            card("0000:03:00.0", "amdgpu"),
            card("0000:00:02.0", "i915"),
            card("0000:04:00.0", "xe"),
        ];
        let mut cur = fixture_scan(2 * SEC);
        // amdgpu `drm-memory-gtt` wins over `drm-resident-gtt`, as for VRAM.
        let amd = cur
            .clients
            .iter_mut()
            .find(|c| c.client_id == 41)
            .expect("client 41");
        amd.memory_bytes.push((Arc::from("gtt"), 6 * MIB));
        let mut out = Vec::new();
        process_vram(&cur, Some(&fixture_scan(SEC)), &cards, &mut out);
        let row = |pid: u32| {
            out.iter()
                .find(|r| r.pid == pid)
                .map(|r| (r.pdev.to_string(), r.shared_bytes))
        };
        // amdgpu client 41 of pid 3000: `drm-memory-gtt` before `drm-resident-gtt` (4 MiB).
        // Client 40 has no shared memory, so pid 3000 has 6 MiB wherever client 40 counts.
        assert_eq!(row(3000), Some(("0000:03:00.0".into(), 6 * MIB)));
        // i915: `drm-resident-system0`.
        assert_eq!(row(3002), Some(("0000:00:02.0".into(), MIB)));
        // xe: `drm-resident-system` of client 10; client 9 has only `drm-total-system`, which is
        // not resident. Both are pid 3003.
        assert_eq!(row(3003), Some(("0000:04:00.0".into(), 4 * MIB)));
        let xe = out.iter().find(|r| r.pid == 3003).expect("xe row");
        assert_eq!(xe.vram_bytes, 20 * MIB);
        assert!(matches!(out.len(), 3 | 4), "{out:?}");
    }

    /// An i915 client of `pid` with the given engine ns.
    fn engine_client(pid: u32, id: u64, engines: &[(&str, u64)]) -> DrmClient {
        DrmClient {
            pid,
            driver: Arc::from("i915"),
            pdev: Arc::from("0000:00:02.0"),
            client_id: id,
            engine_ns: engines
                .iter()
                .map(|&(n, ns)| (Arc::<str>::from(n), ns))
                .collect(),
            ..DrmClient::default()
        }
    }

    /// The engines of the row of `pid`, as (name, percent).
    fn engines_of(out: &[DrmProcessVram], pid: u32) -> Vec<(String, f32)> {
        let rows: Vec<_> = out.iter().filter(|r| r.pid == pid).collect();
        assert_eq!(rows.len(), 1, "one row for pid {pid}: {out:?}");
        rows[0]
            .engines
            .iter()
            .map(|(n, p)| (n.to_string(), *p))
            .collect()
    }

    #[test]
    fn fdinfo_engine_percent_from_ns_step() {
        let client = engine_client;
        let prev = FdinfoScan {
            clients: vec![
                client(10, 1, &[("render", 1_000), ("video", 5 * SEC)]),
                // Client 2 moved from pid 11 to pid 12: no previous ns for pid 12.
                client(11, 2, &[("render", 0)]),
            ],
            partial: false,
            t_ns: SEC,
        };
        let cur = FdinfoScan {
            clients: vec![
                // 500 ms of render and 250 ms of video in 2 s; `copy` is new since `prev`.
                client(
                    10,
                    1,
                    &[
                        ("render", 1_000 + SEC / 2),
                        ("video", 5 * SEC + SEC / 4),
                        ("copy", 7),
                    ],
                ),
                client(12, 2, &[("render", SEC)]),
                // New since `prev`: adds nothing to pid 10.
                client(10, 3, &[("render", SEC)]),
            ],
            partial: false,
            t_ns: 3 * SEC,
        };
        let cards = [card("0000:00:02.0", "i915")];
        let mut out = Vec::new();

        // The first scan has nothing to compare with: rows, but no engines.
        process_vram(&prev, None, &cards, &mut out);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|r| r.engines.is_empty()), "{out:?}");
        assert!(
            out.iter()
                .all(|r| r.encode_pct.is_none() && r.decode_pct.is_none())
        );

        process_vram(&cur, Some(&prev), &cards, &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(
            engines_of(&out, 10),
            [
                ("render".to_string(), 25.0),
                ("video".to_string(), 12.5),
                ("copy".to_string(), 0.0),
            ]
        );
        assert!(engines_of(&out, 12).is_empty());
        // Intel `video` is both encode and decode (PROC-033).
        let pid10 = out.iter().find(|r| r.pid == 10).expect("pid 10");
        assert_eq!(
            (pid10.encode_pct, pid10.decode_pct),
            (Some(12.5), Some(12.5))
        );

        // A step of the same counter at the same time gives no engines, and a counter that went
        // backwards gives 0.
        process_vram(&cur, Some(&cur), &cards, &mut out);
        assert!(out.iter().all(|r| r.engines.is_empty()), "{out:?}");
        process_vram(
            &prev,
            Some(&FdinfoScan {
                t_ns: 0,
                ..cur.clone()
            }),
            &cards,
            &mut out,
        );
        assert!(
            out.iter().flat_map(|r| &r.engines).all(|(_, p)| *p == 0.0),
            "{out:?}"
        );
    }

    #[test]
    fn fdinfo_engine_two_clients_of_one_pid_sum() {
        let client = engine_client;
        let prev = FdinfoScan {
            clients: vec![
                client(20, 1, &[("render", 0), ("video", 0)]),
                client(20, 2, &[("render", 0), ("copy", 0)]),
                client(21, 3, &[("render", 0)]),
                client(21, 4, &[("render", 0)]),
            ],
            partial: false,
            t_ns: SEC,
        };
        let cur = FdinfoScan {
            clients: vec![
                // pid 20 in 1 s: render 250 ms + 500 ms, video 100 ms, copy 200 ms.
                client(20, 1, &[("render", SEC / 4), ("video", SEC / 10)]),
                client(20, 2, &[("render", SEC / 2), ("copy", SEC / 5)]),
                // pid 21: 600 ms + 700 ms of render is more than the second: clamped to 100.
                client(21, 3, &[("render", SEC * 6 / 10)]),
                client(21, 4, &[("render", SEC * 7 / 10)]),
            ],
            partial: false,
            t_ns: 2 * SEC,
        };
        let mut out = Vec::new();
        process_vram(&cur, Some(&prev), &[card("0000:00:02.0", "i915")], &mut out);
        assert_eq!(out.len(), 2, "one row per pid: {out:?}");
        assert_eq!(
            engines_of(&out, 20),
            [
                ("render".to_string(), 75.0),
                ("video".to_string(), 10.0),
                ("copy".to_string(), 20.0),
            ]
        );
        assert_eq!(engines_of(&out, 21), [("render".to_string(), 100.0)]);
    }

    #[test]
    fn fdinfo_amdgpu_enc_dec_map_to_encode_and_decode() {
        let amd = |pid: u32, id: u64, engines: &[(&str, u64)]| DrmClient {
            driver: Arc::from("amdgpu"),
            pdev: Arc::from("0000:03:00.0"),
            ..engine_client(pid, id, engines)
        };
        let prev = FdinfoScan {
            clients: vec![
                amd(30, 1, &[("gfx", 0), ("enc", 0), ("dec", 0)]),
                amd(30, 2, &[("enc_1", 0), ("dec", 0)]),
                amd(31, 3, &[("gfx", 0)]),
            ],
            partial: false,
            t_ns: SEC,
        };
        let cur = FdinfoScan {
            clients: vec![
                // pid 30 in 1 s: gfx 900 ms, enc 100 ms, enc_1 300 ms, dec 200 ms + 150 ms.
                amd(
                    30,
                    1,
                    &[("gfx", SEC * 9 / 10), ("enc", SEC / 10), ("dec", SEC / 5)],
                ),
                amd(30, 2, &[("enc_1", SEC * 3 / 10), ("dec", SEC * 15 / 100)]),
                amd(31, 3, &[("gfx", SEC / 2)]),
            ],
            partial: false,
            t_ns: 2 * SEC,
        };
        let mut out = Vec::new();
        process_vram(
            &cur,
            Some(&prev),
            &[card("0000:03:00.0", "amdgpu")],
            &mut out,
        );
        let pid = |p: u32| out.iter().find(|r| r.pid == p).expect("row");
        // Encode is the busier of `enc` and `enc_1`; decode is `dec` summed over both clients.
        // `gfx` is neither.
        assert_eq!(pid(30).encode_pct, Some(30.0));
        assert_eq!(pid(30).decode_pct, Some(35.0));
        assert_eq!((pid(31).encode_pct, pid(31).decode_pct), (None, None));
    }

    #[test]
    fn engine_role_names() {
        for (name, role) in [
            ("video", (true, true)),
            ("video-enhance", (true, true)),
            ("enc", (true, false)),
            ("enc_1", (true, false)),
            ("dec", (false, true)),
            ("dec_2", (false, true)),
            ("gfx", (false, false)),
            ("render", (false, false)),
            ("encoder", (false, false)),
            ("compute", (false, false)),
        ] {
            assert_eq!(engine_role(name), role, "{name}");
        }
    }

    #[test]
    fn fdinfo_engine_client_on_two_fds_counts_once() {
        // Client 40 is open on fds 3 and 4 of pid 3000 and fd 7 of pid 3001, each with
        // `drm-engine-gfx: 1000 ns`. 250 ms of gfx in 1 s is 25 %, not 50 % or 75 %.
        let prev = fixture_scan(SEC);
        let mut cur = fixture_scan(2 * SEC);
        for c in cur.clients.iter_mut().filter(|c| c.client_id == 40) {
            for (_, ns) in c.engine_ns.iter_mut() {
                *ns += SEC / 4;
            }
        }
        let mut out = Vec::new();
        process_vram(
            &cur,
            Some(&prev),
            &[card("0000:03:00.0", "amdgpu")],
            &mut out,
        );
        let gfx: Vec<_> = out
            .iter()
            .flat_map(|r| {
                r.engines
                    .iter()
                    .map(move |(n, p)| (r.pid, n.to_string(), *p))
            })
            .collect();
        assert_eq!(gfx.len(), 1, "{gfx:?}");
        assert!(matches!(gfx[0].0, 3000 | 3001), "{gfx:?}");
        assert_eq!((gfx[0].1.as_str(), gfx[0].2), ("gfx", 25.0));
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
            seen: 0,
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
        // pid 3003 has two xe clients: one row with both.
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            (rows[0].pid, rows[0].vram_bytes, rows[0].shared_bytes),
            (3003, 20 * MIB, 4 * MIB)
        );

        src.set_processes(false);
        let (mut src, up) = src.sample_at(clock(2 * SEC));
        let SourceUpdate::Gpu(g) = up else {
            panic!("{up:?}");
        };
        assert!(g.drm_processes.is_none());

        // Window hidden, device history on (tasklens-g00wx.1): rows with the first sample, then
        // with every 10th, so the GPU group series have a point in every 10 s history bucket.
        let history = Demand {
            device_history: true,
            ..Demand::default()
        };
        src.period(&history, &EngineSettings::default());
        let mut with_rows = Vec::new();
        for n in 1..=21u64 {
            let t = (2 + n) * SEC;
            // Keep the hand-made card list: no enumeration falls due.
            let _ = src.enum_clock.due(t);
            let (next, up) = src.sample_at(clock(t));
            src = next;
            let SourceUpdate::Gpu(g) = up else {
                panic!("{up:?}");
            };
            if let Some(rows) = g.drm_processes {
                assert_eq!(rows.len(), 1, "{rows:?}");
                with_rows.push(n);
            }
        }
        assert_eq!(with_rows, [1, 11, 21]);
    }

    #[test]
    fn history_rows_every_10th_sample() {
        assert_eq!(HISTORY_ROWS_EVERY, 10);
        let mut rows = HistoryRows::default();
        // Wanted by a view: every sample.
        assert!((0..3).all(|_| rows.due(true, true)));
        // Neither wanted nor history: none.
        assert!((0..30).all(|_| !rows.due(false, false)));
        // History alone: the first sample, then every 10th.
        let due: Vec<usize> = (0..25).filter(|_| rows.due(false, true)).collect();
        assert_eq!(due, [0, 10, 20]);
        // A view that asks again starts the count over: the next history-only sample has rows.
        assert!(rows.due(true, true));
        assert!(rows.due(false, true));
        assert!(!rows.due(false, true));
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
        assert!(t.enc_dec_shared);
        // Missing inputs are `Unsupported`, never 0.
        assert_eq!(t.util_pct.validity, Validity::Unsupported);
        assert_eq!(t.temp_c.validity, Validity::Unsupported);
        assert_eq!(t.temp_edge_c.validity, Validity::Unsupported);
        assert_eq!(t.temp_junction_c.validity, Validity::Unsupported);
        assert_eq!(t.temp_mem_c.validity, Validity::Unsupported);
        assert_eq!(t.mem_reserved.validity, Validity::Unsupported);
        assert_eq!(t.enc_sessions.validity, Validity::Unsupported);
    }

    #[test]
    fn amd_sensor_temperatures() {
        let s = AmdSample {
            temps: SmallVec::from_slice(&[(AmdTemp::Edge, 48_000), (AmdTemp::Junction, 52_500)]),
            ..AmdSample::default()
        };
        let t = amd_totals(&s, &[]);
        assert_eq!(t.temp_c, Metric::valid(52.5));
        assert_eq!(t.temp_edge_c, Metric::valid(48.0));
        assert_eq!(t.temp_junction_c, Metric::valid(52.5));
        assert_eq!(t.temp_mem_c.validity, Validity::Unsupported);
    }

    #[test]
    fn fdinfo_enc_dec_fallback() {
        let s = AmdSample::default();
        let busy = [(Arc::<str>::from("enc"), 5.0)];
        let t = amd_totals(&s, &busy);
        assert_eq!(t.enc_pct, Metric::valid(5.0));
        assert_eq!(t.dec_pct, Metric::valid(0.0));
        // Separate fdinfo engines: two rows.
        assert!(!t.enc_dec_shared);

        let t = amd_totals(&s, &[]);
        assert_eq!(t.enc_pct.validity, Validity::WarmingUp);
        assert_eq!(t.dec_pct.validity, Validity::WarmingUp);

        // Warm, but no client on the card: both 0.
        let t = totals(&s, Some(&[]), 0, false, no_link());
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
        assert!(t.enc_dec_shared);
        assert_eq!(t.mem_util_pct.validity, Validity::Unsupported);
        assert_eq!(t.fan_pct.validity, Validity::Unsupported);
        assert_eq!(t.clock_mem_mhz.validity, Validity::Unsupported);

        // No engine values yet.
        let t = intel_totals(&s, None, SEC, &[], (None, None, None), false);
        assert_eq!(t.util_pct.validity, Validity::WarmingUp);
        assert_eq!(t.enc_pct.validity, Validity::WarmingUp);

        // Warm, but no client on the card.
        let warm = FdinfoEngines {
            busy: Some(&[]),
            seen: 0,
            partial: false,
        };
        let t = intel(&s, None, SEC, warm, (None, None, None), false, no_link());
        assert_eq!(t.util_pct, Metric::valid(0.0));
        assert_eq!(t.dec_pct, Metric::valid(0.0));
    }

    /// No PCIe link files and no static values.
    fn no_link() -> CardLink<'static> {
        static NO_INFO: std::sync::LazyLock<Arc<DeviceStatic>> =
            std::sync::LazyLock::new(Arc::default);
        CardLink {
            link: PcieLink::default(),
            info: &NO_INFO,
        }
    }

    fn engine_list(t: &GpuTotals) -> Vec<(GpuEngine, Metric<f32>)> {
        t.engines.reported().collect()
    }

    #[test]
    fn amd_reports_every_engine() {
        let s = AmdSample {
            gpu_busy_pct: Some(30),
            mem_busy_pct: Some(10),
            ..AmdSample::default()
        };
        let b = busy(&[
            ("gfx", 30.0),
            ("compute", 12.5),
            ("dma", 2.0),
            ("jpeg", 4.0),
            ("enc", 1.0),
            ("dec", 3.0),
        ]);
        let t = amd_totals(&s, &b);
        assert_eq!(
            engine_list(&t),
            [
                (GpuEngine::Gpu, Metric::valid(30.0)),
                (GpuEngine::MemCtl, Metric::valid(10.0)),
                (GpuEngine::Enc, Metric::valid(1.0)),
                (GpuEngine::Dec, Metric::valid(3.0)),
                (GpuEngine::Jpeg, Metric::valid(4.0)),
                (GpuEngine::Compute, Metric::valid(12.5)),
                (GpuEngine::Dma, Metric::valid(2.0)),
            ]
        );
        assert!(!t.fdinfo_partial);
    }

    #[test]
    fn amd_engines_before_and_after_warm_up() {
        let s = AmdSample {
            gpu_busy_pct: Some(5),
            ..AmdSample::default()
        };
        // First scan: fdinfo engines are WarmingUp, and JPEG is not named yet.
        let t = totals(&s, None, 0, false, no_link());
        assert_eq!(
            engine_list(&t),
            [
                (GpuEngine::Gpu, Metric::valid(5.0)),
                (GpuEngine::Enc, Metric::with(Validity::WarmingUp)),
                (GpuEngine::Dec, Metric::with(Validity::WarmingUp)),
                (GpuEngine::Compute, Metric::with(Validity::WarmingUp)),
                (GpuEngine::Dma, Metric::with(Validity::WarmingUp)),
            ]
        );
        // JPEG seen in an earlier sample stays listed, at 0 without clients, and a denied fdinfo
        // read is passed on.
        let t = totals(&s, Some(&[]), bit(GpuEngine::Jpeg), true, no_link());
        assert_eq!(t.engines.get(GpuEngine::Jpeg), Metric::valid(0.0));
        assert_eq!(t.engines.get(GpuEngine::Compute), Metric::valid(0.0));
        assert!(t.fdinfo_partial);
        // The VCN engine still gives encode and decode, and fdinfo still gives compute.
        let vcn = AmdSample {
            metrics: Some(GpuMetrics {
                vcn_activity: SmallVec::from_slice(&[2500]),
                ..GpuMetrics::default()
            }),
            ..s
        };
        let t = totals(&vcn, Some(&busy(&[("compute", 7.0)])), 0, false, no_link());
        assert_eq!(t.engines.get(GpuEngine::Enc), Metric::valid(25.0));
        assert_eq!(t.engines.get(GpuEngine::Dec), Metric::valid(25.0));
        assert_eq!(t.engines.get(GpuEngine::Compute), Metric::valid(7.0));
    }

    #[test]
    fn intel_reports_its_engines() {
        let s = IntelSample::default();
        let b = busy(&[
            ("render", 40.0),
            ("copy", 1.0),
            ("video", 10.0),
            ("video-enhance", 2.0),
            ("compute", 6.0),
        ]);
        let t = intel_totals(&s, None, SEC, &b, (None, None, None), false);
        // The video engine is listed once, as Video; GPU_UTIL stays the busiest engine.
        assert_eq!(
            engine_list(&t),
            [
                (GpuEngine::Compute, Metric::valid(6.0)),
                (GpuEngine::Render, Metric::valid(40.0)),
                (GpuEngine::Copy, Metric::valid(1.0)),
                (GpuEngine::Video, Metric::valid(10.0)),
                (GpuEngine::VideoEnhance, Metric::valid(2.0)),
            ]
        );
        assert_eq!(t.util_pct, Metric::valid(40.0));
        assert_eq!(t.enc_pct, Metric::valid(10.0));

        // A card that never named compute or video-enhance does not list them.
        let fd = FdinfoEngines {
            busy: None,
            seen: 0,
            partial: true,
        };
        let t = intel(&s, None, SEC, fd, (None, None, None), false, no_link());
        assert_eq!(
            engine_list(&t),
            [
                (GpuEngine::Render, Metric::with(Validity::WarmingUp)),
                (GpuEngine::Copy, Metric::with(Validity::WarmingUp)),
                (GpuEngine::Video, Metric::with(Validity::WarmingUp)),
            ]
        );
        assert!(t.fdinfo_partial);
    }

    #[test]
    fn seen_mask_names() {
        let b = busy(&[
            ("gfx", 1.0),
            ("jpeg", 0.0),
            ("video-enhance", 0.0),
            ("xyz", 1.0),
        ]);
        assert_eq!(
            seen_mask(&b),
            bit(GpuEngine::Jpeg) | bit(GpuEngine::VideoEnhance)
        );
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
