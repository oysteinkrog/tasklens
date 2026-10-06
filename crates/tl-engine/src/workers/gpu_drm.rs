//! The `GpuDrm` worker for AMD cards (ADR-004, ADR-008, ADR-016, PERF-063 to PERF-069).
//!
//! It reads the tl-collect DRM readers at 1 Hz while a GPU page is shown or history records,
//! and not at all otherwise. Cards are enumerated again every 5 s; only `amdgpu` cards are kept
//! (NVIDIA belongs to `GpuNvidia`, Intel to E12-027). When the card set changes, a `Devices`
//! list and a `GpuInfo` go out through `take_extra` before the sample.
//!
//! Encode and decode come from the `gpu_metrics` VCN activity when the blob has it, else from
//! two DRM fdinfo scans. AMD cards have no UUID, so their identity is `uuid: [0; 16]` plus the
//! PCI slot (R-166). Everything here is read-only.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use smallvec::SmallVec;
use tl_collect::SysRoot;
use tl_collect::counters::Clock;
use tl_collect::gpu_drm::{
    AmdSample, DrmCard, FdinfoScan, discover_cards, engine_busy, main_temp, read_amd, scan,
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
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{
    DeviceDesc, DeviceKinds, DeviceList, GpuInfoUpdate, GpuSample, GpuStaticInfo, SourceUpdate,
    WorkerError,
};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::EnumClock;

pub const PERIOD: Duration = Duration::from_secs(1);

const DRIVER: &str = "amdgpu";
const NAME_MAX: usize = 256;
const GPU_UDEV_KEYS: [&str; 1] = ["ID_MODEL_FROM_DATABASE"];

/// The GPU rate table (ADR-008 row "GPU"): 1 Hz while a GPU page is shown or recording is on.
pub fn gpu_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.gpus || d.recording {
        Some(PERIOD)
    } else {
        None
    }
}

pub struct GpuDrmSource {
    root: SysRoot,
    /// The `amdgpu` cards of the last enumeration, sorted by card number.
    cards: Vec<DrmCard>,
    /// When the cards are enumerated again.
    enum_clock: EnumClock,
    /// One per card, reused every sample.
    amd: Vec<AmdSample>,
    /// The last fdinfo scan; `None` until the first scan and after `reset_baselines`.
    prev_scan: Option<FdinfoScan>,
    /// Reused for the next fdinfo scan.
    cur_scan: FdinfoScan,
    /// `Devices` then `GpuInfo`, waiting for `take_extra`.
    pending_extra: VecDeque<SourceUpdate>,
}

impl GpuDrmSource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            cards: Vec::new(),
            enum_clock: EnumClock::default(),
            amd: Vec::new(),
            prev_scan: None,
            cur_scan: FdinfoScan::default(),
            pending_extra: VecDeque::new(),
        }
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        let t_ns = clock.boot_ns;
        if self.enum_clock.due(t_ns) {
            self.enumerate(t_ns);
        }
        if self.cards.is_empty() {
            // No AMD card: an empty `Gpu` sample would clear the NVIDIA rows in the reducer.
            let idle = SourceUpdate::Health {
                id: SourceId::GpuDrm,
                state: SourceState::Off,
                last_error: None,
                t_ns,
            };
            return (self, idle);
        }

        for (card, amd) in self.cards.iter().zip(&mut self.amd) {
            read_amd(card, &self.root.sys, amd);
        }

        // One fdinfo walk serves every card without VCN activity.
        let need_fdinfo = self.amd.iter().any(|s| !has_vcn(s));
        if need_fdinfo {
            scan(&self.root, &mut self.cur_scan);
        }

        let mut busy = SmallVec::<[(Arc<str>, f32); 8]>::new();
        let mut devices = Vec::with_capacity(self.cards.len());
        for (card, amd) in self.cards.iter().zip(&self.amd) {
            let fdinfo = match (&self.prev_scan, has_vcn(amd)) {
                (Some(prev), false) => {
                    engine_busy(prev, &self.cur_scan, &card.pci_slot, &mut busy);
                    Some(&busy[..])
                }
                _ => None,
            };
            devices.push((identity(card), totals(amd, fdinfo)));
        }
        if need_fdinfo {
            let cur = std::mem::take(&mut self.cur_scan);
            self.cur_scan = self.prev_scan.replace(cur).unwrap_or_default();
        }

        let sample = GpuSample {
            t_ns,
            devices,
            processes: None,
        };
        (self, SourceUpdate::Gpu(sample))
    }

    /// Re-reads the card list. When the `amdgpu` set changed, queues `Devices` and `GpuInfo`.
    fn enumerate(&mut self, t_ns: u64) {
        let cards: Vec<DrmCard> = discover_cards(&self.root.sys)
            .into_iter()
            .filter(|c| c.driver.as_deref() == Some(DRIVER))
            .collect();
        if cards == self.cards {
            return;
        }
        self.cards = cards;
        self.amd = vec![AmdSample::default(); self.cards.len()];

        let driver = driver_text(&self.root);
        let mut props = UdevProps::default();
        let mut entries = Vec::with_capacity(self.cards.len());
        let mut infos = Vec::with_capacity(self.cards.len());
        for (card, amd) in self.cards.iter().zip(&mut self.amd) {
            read_amd(card, &self.root.sys, amd);
            let name = display_name(&self.root, card, &mut props);
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
                mem_total: amd.vram_total.unwrap_or(0),
                power_limit_mw: amd
                    .power_cap_uw
                    .and_then(|uw| u32::try_from(uw / 1000).ok()),
                driver: Arc::clone(&driver),
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
        gpu_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    /// The next fdinfo-based encode and decode values are `WarmingUp`.
    fn reset_baselines(&mut self) {
        self.prev_scan = None;
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

/// AMD cards have no UUID (R-166): the PCI slot in sysfs form names the card.
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

/// `ID_MODEL_FROM_DATABASE` of the card's PCI udev entry, else "AMD GPU <device id>".
fn display_name(root: &SysRoot, card: &DrmCard, props: &mut UdevProps) -> Arc<str> {
    let path = root.run_path(&format!("udev/data/{}", pci_entry(&card.pci_slot)));
    let mut clean = String::new();
    if read_udev_props(&path, &GPU_UDEV_KEYS, props).is_ok()
        && let Some(model) = props.get("ID_MODEL_FROM_DATABASE")
    {
        sanitize_text(model.as_bytes(), NAME_MAX, &mut clean);
    }
    if clean.is_empty() {
        return Arc::from(format!("AMD GPU {:04x}", card.device_id));
    }
    Arc::from(clean)
}

/// "amdgpu <module version>", or "amdgpu" when the module has no version file.
fn driver_text(root: &SysRoot) -> Arc<str> {
    let mut buf = Vec::new();
    let file = root.sys.join(format!("module/{DRIVER}/version"));
    let mut clean = String::new();
    if let Ok(text) = read_trimmed(&file, &mut buf) {
        sanitize_text(text, NAME_MAX, &mut clean);
    }
    if clean.is_empty() {
        Arc::from(DRIVER)
    } else {
        Arc::from(format!("{DRIVER} {clean}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_collect::gpu_drm::GpuMetrics;

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
}
