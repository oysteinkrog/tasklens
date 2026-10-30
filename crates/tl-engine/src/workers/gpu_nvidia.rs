//! The `GpuNvidia` worker (ADR-007, ADR-008, R-155 items 3, 4, 7 and 8).
//!
//! It starts the `tl-gpu-probe` child, publishes `GpuInfo` once per generation, then samples
//! device totals at 1 Hz while GPUs are demanded. Per-process rows are asked for only while
//! `gpu_process` is demanded. Nothing is requested while neither is wanted.
//!
//! It runs its own loop instead of `source::run`, because it owns a child process and must
//! return `Err` so the supervisor restarts it (ADR-007 failure table). Expected conditions are
//! health states: a missing probe, `LibraryNotFound` and `DriverNotLoaded` give the terminal
//! `Unsupported`, and a protocol version mismatch gives `Failed`. In those cases the worker
//! parks and never retries. Deadlines, crashes, `NoPermission` and `Other` are `Err`.
//!
//! Once per process, the first generation that reaches `GpuInfo` runs `tl-gpu-probe
//! --api-versions` (E13-010) and sends `GpuApiVersions` before the first sample. A failure is
//! logged at info and not retried; the UI shows `Unknown`.
//!
//! Every probe call runs as a blocking job that checks for cancel at least every 20 ms. A
//! cancelled worker kills the probe at once instead of asking it to exit, so engine shutdown
//! never waits for NVML (tasklens-cr0.30).
//!
//! The reducer maps pids and assigns `DeviceIdx`. The probe is not restarted after resume, so
//! `reset_generation` is ignored. Device names are logged at debug only; pids never.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::{MissedTickBehavior, interval, sleep, sleep_until};
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::gpu_drm::{discover_cards, read_pcie_link, same_pci_slot};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::gpu_proto::{
    ApiVersions, GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, Reading,
};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::DeviceIdx;
use tl_model::system::GpuStaticInfo as DeviceStatic;
use tl_model::system::{ClockEventReasons, GpuEngine, GpuEngines, GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::gpu::client::{HelloInfo, HelloOutcome, ProbeClient, ProbeResponseSample};
use crate::gpu::{API_VERSIONS_DEADLINE, NOT_INSTALLED, ProbeError, VERSION_MISMATCH};
use crate::source::{WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{
    EngineMsg, GpuInfoUpdate, GpuSample, GpuStaticInfo, SourceUpdate, WorkerError,
};
use crate::workers::WorkerSpec;

/// Never faster; the Refresh setting does not apply (R-026).
pub const PERIOD: Duration = Duration::from_secs(1);

/// A parked worker wakes this often to check for cancellation. It does nothing else.
pub const PARK_RECHECK: Duration = Duration::from_secs(60);

/// E03 has no "not yet assigned" device index. The reducer overwrites `GpuTotals.device` from
/// the identity (R-155 item 8).
const DEVICE_PLACEHOLDER: DeviceIdx = DeviceIdx(u16::MAX);

/// Set by the first generation that runs `--api-versions`. `io` has no generation counter, and
/// the versions do not change while the app runs, so a restart does not run it again.
static ONCE_PER_PROCESS: AtomicBool = AtomicBool::new(false);

/// What the worker needs from a probe connection. [`ProbeClient`] is the real one; tests
/// inject a fake.
pub trait ProbeLink: Sized + Send + 'static {
    /// What `start` needs besides the probe path: `()` for the real client.
    type Starter: Send + 'static;

    /// Starts the probe and runs `Hello`. Runs on the blocking pool. Returns
    /// [`ProbeError::Cancelled`] soon after `cancel` turns true, with the child reaped.
    fn start(
        starter: Self::Starter,
        probe: &Path,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(Self, HelloOutcome), ProbeError>;

    /// One `Sample` request. Runs on the blocking pool. On `Err` the child is already gone.
    fn sample(
        &mut self,
        processes: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<ProbeResponseSample, ProbeError>;

    /// Runs `probe --api-versions` in its own process under [`API_VERSIONS_DEADLINE`]. Runs on
    /// the blocking pool.
    fn api_versions(
        &self,
        probe: &Path,
        cancel: &dyn Fn() -> bool,
    ) -> Result<ApiVersions, ProbeError>;

    /// Asks the child to exit and gives it 200 ms; a child that still runs then is killed and
    /// reaped on a background thread. The stop of a generation that was not cancelled.
    fn shutdown(self);

    /// Kills the child at once and reaps it on a background thread. The stop of a cancelled
    /// generation.
    fn kill(self);
}

impl ProbeLink for ProbeClient {
    /// Where sysfs lives, for the PCIe link maximum.
    type Starter = SysRoot;

    fn start(
        root: SysRoot,
        probe: &Path,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(Self, HelloOutcome), ProbeError> {
        let client = ProbeClient::start_or_cancel(probe, &[], cancel)?;
        let hello = match (&client.info, &client.init_failed) {
            (Some(info), _) => HelloOutcome::Ready(with_sysfs_pcie_max(info.clone(), &root.sys)),
            (None, Some((kind, message))) => HelloOutcome::InitFailed {
                kind: *kind,
                message: Arc::clone(message),
            },
            // `start` sets one of the two whenever it returns `Ok`.
            (None, None) => {
                client.shutdown();
                return Err(ProbeError::Protocol("hello left no state".into()));
            }
        };
        Ok((client, hello))
    }

    fn sample(
        &mut self,
        processes: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<ProbeResponseSample, ProbeError> {
        ProbeClient::sample_or_cancel(self, processes, cancel)
    }

    fn api_versions(
        &self,
        probe: &Path,
        cancel: &dyn Fn() -> bool,
    ) -> Result<ApiVersions, ProbeError> {
        crate::gpu::client::api_versions_or_cancel(probe, API_VERSIONS_DEADLINE, cancel)
    }

    fn shutdown(self) {
        ProbeClient::shutdown(self);
    }

    fn kill(self) {
        ProbeClient::kill(self);
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::GpuNvidia,
        name: "gpu-nvidia",
        modes: &[EngineMode::Full],
        factory,
    }
}

pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let cfg = Arc::clone(&cfg);
        async move { run(cx, io, cfg, crate::gpu::locate).await }
    })
}

/// The worker body with the real [`ProbeClient`].
pub async fn run(
    cx: Cx,
    io: WorkerIo,
    cfg: Arc<EngineConfig>,
    locate: fn() -> Option<PathBuf>,
) -> Outcome<(), WorkerError> {
    let root = cfg.sys_root.clone();
    run_with::<ProbeClient>(cx, io, locate, root, &ONCE_PER_PROCESS).await
}

/// The worker body over any [`ProbeLink`]. `locate` is called once. `--api-versions` runs only
/// in the generation that flips `api_once` from false to true.
pub async fn run_with<L: ProbeLink>(
    cx: Cx,
    io: WorkerIo,
    locate: impl FnOnce() -> Option<PathBuf>,
    starter: L::Starter,
    api_once: &'static AtomicBool,
) -> Outcome<(), WorkerError> {
    let Some(probe) = locate() else {
        return park(&cx, &io, SourceState::Unsupported, NOT_INSTALLED.into()).await;
    };

    let start_probe = probe.clone();
    let started = match blocking(&cx, "gpu_nvidia.start", move |cancel| {
        L::start(starter, &start_probe, cancel)
    })
    .await
    {
        Ok(started) => started,
        Err(stop) => return stop,
    };
    let (mut link, info) = match started {
        Ok((link, HelloOutcome::Ready(info))) => (link, info),
        Ok((link, HelloOutcome::InitFailed { kind, message })) => {
            tracing::debug!(?kind, "GPU probe init failed");
            if let Err(stop) = blocking(&cx, "gpu_nvidia.shutdown", move |cancel| {
                stop_link(link, cancel());
            })
            .await
            {
                return stop;
            }
            return match kind {
                InitFailure::LibraryNotFound | InitFailure::DriverNotLoaded => {
                    let text: Arc<str> = format!("{kind:?}").into();
                    park(&cx, &io, SourceState::Unsupported, text).await
                }
                InitFailure::NoPermission | InitFailure::Other => {
                    Outcome::Err(ProbeError::InitFailed { kind, message }.into())
                }
            };
        }
        // The client already killed and reaped the child. No restart until the app restarts.
        Err(ProbeError::VersionMismatch(proto)) => {
            tracing::debug!(proto, "GPU probe version mismatch");
            return park(&cx, &io, SourceState::Failed, VERSION_MISMATCH.into()).await;
        }
        Err(ProbeError::Cancelled) => return Outcome::Ok(()),
        Err(e) => return Outcome::Err(e.into()),
    };
    for device in info.devices.iter() {
        tracing::debug!(name = %device.name, "GPU probe device");
    }
    // One per Hello device, shared by every sample (PERF-066 to PERF-071).
    let statics: Vec<Arc<DeviceStatic>> = info.devices.iter().map(device_static).collect();

    // Stays true until a process sample shows that the driver has no per-process utilisation.
    let mut supports_process_util = true;
    let first = [
        SourceUpdate::GpuInfo(info_update(&info, supports_process_util)),
        health(SourceState::Running, None),
    ];
    for update in first {
        if let Some(stop) = send(&cx, &io, update).await {
            stop_link(link, cx.is_cancel_requested());
            return stop;
        }
    }

    if !api_once.swap(true, Ordering::SeqCst) {
        // On cancel the closure stops the child and returns; the link is then dropped, which
        // kills the probe.
        let (back, result) = match blocking(&cx, "gpu_nvidia.api_versions", move |cancel| {
            let result = link.api_versions(&probe, cancel);
            (link, result)
        })
        .await
        {
            Ok(done) => done,
            Err(stop) => return stop,
        };
        link = back;
        match result {
            Ok(versions) => {
                if let Some(stop) = send(&cx, &io, SourceUpdate::GpuApiVersions(versions)).await {
                    stop_link(link, cx.is_cancel_requested());
                    return stop;
                }
            }
            Err(e) => tracing::info!(target: "tasklens::probe", %e, "api versions unavailable"),
        }
    }

    let mut iv = interval(cx.now(), PERIOD);
    iv.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // R-276: resume after the last command a generation started. If it never finished, it
    // was a `RefreshNow` whose sample failed; it is skipped without a message.
    let progress = Arc::clone(&io.control.borrow_and_clone().progress);
    let mut last_seq = progress.started(SourceId::GpuNvidia);
    if last_seq > progress.done(SourceId::GpuNvidia) {
        progress.set_done(SourceId::GpuNvidia, last_seq);
    }

    loop {
        sleep_until(iv.deadline()).await;
        if cx.checkpoint().is_err() {
            link.kill();
            return Outcome::Ok(());
        }
        iv.tick(cx.now());

        let (totals, processes) = wanted(&io.demand.borrow_and_clone());
        let control = io.control.borrow_and_clone();
        // The newest `RefreshNow`, done once this turn's sample is sent. The worker has no
        // other command, so any other one is done at once.
        let mut pending_done = None;
        for queued in control.commands.iter().filter(|c| c.seq > last_seq) {
            if queued.target != SourceId::GpuNvidia {
                continue;
            }
            progress.set_started(SourceId::GpuNvidia, queued.seq);
            if queued.command == WorkerCommand::RefreshNow || pending_done.is_some() {
                pending_done = Some(queued.seq);
            } else {
                progress.set_done(SourceId::GpuNvidia, queued.seq);
            }
        }
        let forced = pending_done.is_some();
        if let Some(newest) = control.commands.last() {
            last_seq = last_seq.max(newest.seq);
        }
        drop(control);
        if !totals && !processes && !forced {
            continue;
        }

        // On cancel the sample kills the child and returns, and the link is dropped.
        let (back, result) = match blocking(&cx, "gpu_nvidia.sample", move |cancel| {
            let result = link.sample(processes, cancel);
            (link, result)
        })
        .await
        {
            Ok(done) => done,
            Err(stop) => return stop,
        };
        link = back;
        let sample = match result {
            Ok(sample) => sample,
            Err(ProbeError::Cancelled) => return Outcome::Ok(()),
            Err(e) => return Outcome::Err(e.into()),
        };
        let now = cx.now();
        if now > iv.deadline() {
            iv.tick(now);
        }

        let mut updates = Vec::with_capacity(2);
        if supports_process_util && no_process_util(sample.processes.as_deref()) {
            supports_process_util = false;
            updates.push(SourceUpdate::GpuInfo(info_update(&info, false)));
        }
        updates.push(SourceUpdate::Gpu(gpu_sample(
            sample,
            &info.devices,
            &statics,
        )));
        for update in updates {
            if let Some(stop) = send(&cx, &io, update).await {
                stop_link(link, cx.is_cancel_requested());
                return stop;
            }
        }
        if let Some(seq) = pending_done {
            progress.set_done(SourceId::GpuNvidia, seq);
        }
    }
}

/// `(totals, processes)`. Hidden surfaces stop sampling unless history records GPUs (ADR-008).
/// Device history keeps the totals only.
pub fn wanted(d: &Demand) -> (bool, bool) {
    let totals = (d.any_surface_visible && d.gpus) || d.recording || d.device_history;
    let processes = (d.any_surface_visible && d.gpu_process) || d.recording;
    (totals, processes)
}

pub fn identity(info: &GpuDeviceInfo) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid: info.uuid,
        pci_bus_id: info.pci_bus_id.as_str().into(),
    }
}

/// `device` is a placeholder the reducer overwrites (R-155 item 8). `statics` is the device's
/// [`device_static`].
pub fn to_totals(
    s: &GpuDeviceSample,
    info: &GpuDeviceInfo,
    statics: &Arc<DeviceStatic>,
) -> GpuTotals {
    let pct = |r: Reading<u32>| metric(r, |v| v as f32);
    let kbps = |r: Reading<u32>| metric(r, |v| v as f32 * 1024.0);
    // NVML engines (PERF-063). An engine whose call returns NotSupported is not listed.
    let mut engines = GpuEngines::none();
    engines.set(GpuEngine::Gpu, pct(s.util_pct));
    engines.set(GpuEngine::MemCtl, pct(s.mem_util_pct));
    engines.set(GpuEngine::Enc, pct(s.enc_pct));
    engines.set(GpuEngine::Dec, pct(s.dec_pct));
    engines.set(GpuEngine::Jpeg, pct(s.jpeg_pct));
    engines.set(GpuEngine::Ofa, pct(s.ofa_pct));
    GpuTotals {
        device: DEVICE_PLACEHOLDER,
        util_pct: pct(s.util_pct),
        mem_util_pct: pct(s.mem_util_pct),
        mem_used: metric(s.mem_used, |v| v),
        mem_total: Metric::valid(info.mem_total),
        // NVML has no GTT (PERF-064).
        shared_used: Metric::with(Validity::Unsupported),
        shared_total: Metric::with(Validity::Unsupported),
        mem_reserved: metric(s.mem_reserved, |v| v),
        enc_pct: pct(s.enc_pct),
        dec_pct: pct(s.dec_pct),
        enc_sessions: metric(s.enc_sessions, |v| v),
        // NVENC and NVDEC are separate engines.
        enc_dec_shared: false,
        temp_c: metric(s.temp_c, |v| v as f32),
        // NVML reads one GPU core sensor (PERF-069).
        temp_edge_c: Metric::with(Validity::Unsupported),
        temp_junction_c: Metric::with(Validity::Unsupported),
        temp_mem_c: Metric::with(Validity::Unsupported),
        power_w: metric(s.power_mw, milliwatts),
        power_limit_w: match info.power_limit_mw {
            Some(mw) => Metric::valid(milliwatts(mw)),
            None => Metric::with(Validity::Unsupported),
        },
        // NVML reports millijoules.
        energy_since_boot: metric(s.energy_mj, |mj| mj as f64 / 1000.0),
        clock_gfx_mhz: metric(s.clock_gfx_mhz, |v| v),
        clock_mem_mhz: metric(s.clock_mem_mhz, |v| v),
        clock_video_mhz: metric(s.clock_video_mhz, |v| v),
        pstate: metric(s.pstate, |v| v),
        clock_event_reasons: metric(s.clock_event_reasons, clock_event_reasons),
        throttling: metric(s.clock_event_reasons, |bits| {
            clock_event_reasons(bits)
                .intersects(ClockEventReasons::HW_SLOWDOWN | ClockEventReasons::SW_THERMAL)
        }),
        fan_pct: pct(s.fan_pct),
        fan_rpm: metric(s.fan_rpm, |v| v),
        // NVML reports KB/s.
        pcie_rx_bps: kbps(s.pcie_rx_kbps),
        pcie_tx_bps: kbps(s.pcie_tx_kbps),
        pcie_gen: small(s.pcie_gen),
        pcie_width: small(s.pcie_width),
        engines,
        // NVIDIA engines come from NVML, not fdinfo.
        fdinfo_partial: false,
        info: Arc::clone(statics),
    }
}

/// The static values of one `Hello` device (PERF-066 to PERF-071).
pub fn device_static(info: &GpuDeviceInfo) -> Arc<DeviceStatic> {
    let text = |v: &Option<String>| v.as_deref().map(Arc::from);
    let byte = |v: Option<u32>| v.and_then(|v| u8::try_from(v).ok());
    Arc::new(DeviceStatic {
        cuda_version: text(&info.cuda_version),
        vbios: text(&info.vbios),
        architecture: text(&info.architecture),
        pcie_max_gen: byte(info.pcie_max_gen),
        pcie_max_width: byte(info.pcie_max_width),
        clock_gfx_max_mhz: info.clock_gfx_max_mhz,
        clock_mem_max_mhz: info.clock_mem_max_mhz,
        power_max_limit_w: info.power_max_limit_mw.map(milliwatts),
        power_min_limit_w: info.power_min_limit_mw.map(milliwatts),
        slowdown_temp_c: info.slowdown_temp_c.map(|c| c as f32),
        shutdown_temp_c: info.shutdown_temp_c.map(|c| c as f32),
    })
}

/// Replaces each device's maximum PCIe generation and width with sysfs `max_link_speed` and
/// `max_link_width` of its card, where sysfs has them (PERF-070). NVML's
/// `nvmlDeviceGetMaxPcieLinkGeneration` gives the lower of the card's and the host's maximum,
/// while the row shows the card's own maximum. Runs on the blocking pool.
pub fn with_sysfs_pcie_max(mut info: HelloInfo, sys: &Path) -> HelloInfo {
    let cards = discover_cards(sys);
    let devices: Vec<GpuDeviceInfo> = info
        .devices
        .iter()
        .map(|d| {
            let mut d = d.clone();
            if let Some(card) = cards
                .iter()
                .find(|c| same_pci_slot(&c.pci_slot, &d.pci_bus_id))
            {
                let link = read_pcie_link(sys, card);
                if let Some(generation) = link.max_gen {
                    d.pcie_max_gen = Some(u32::from(generation));
                }
                if let Some(width) = link.max_width {
                    d.pcie_max_width = Some(u32::from(width));
                }
            }
            d
        })
        .collect();
    info.devices = devices.into();
    info
}

/// `persistent_key` is `gpu:<NVML UUID>` (PERF-005, R-155 items 7 and 8).
pub fn to_static(
    info: &GpuDeviceInfo,
    hello: &HelloInfo,
    supports_process_util: bool,
) -> GpuStaticInfo {
    GpuStaticInfo {
        identity: identity(info),
        name: info.name.as_str().into(),
        mem_total: info.mem_total,
        power_limit_mw: info.power_limit_mw,
        driver: Arc::clone(&hello.driver),
        nvml: Arc::clone(&hello.nvml),
        supports_process_util,
        persistent_key: format!("gpu:{}", uuid_text(&info.uuid)).into(),
        hot_c: info.slowdown_temp_c.map(|c| c as f32),
    }
}

/// The NVML text form, `GPU-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, in lowercase hex.
fn uuid_text(uuid: &[u8; 16]) -> String {
    let mut out = String::with_capacity(40);
    out.push_str("GPU");
    for (i, byte) in uuid.iter().enumerate() {
        if matches!(i, 0 | 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn metric<T, U: Copy + Default>(r: Reading<T>, f: impl FnOnce(T) -> U) -> Metric<U> {
    let validity = r.validity();
    match r {
        Reading::Ok(v) => Metric::valid(f(v)),
        Reading::Unsupported | Reading::Failed => Metric::with(validity),
    }
}

/// NVML `nvmlClocksEventReason*` bits (nvml.h, stable across drivers) and the model flag for
/// each. Bits NVML adds later are dropped.
const NVML_CLOCK_EVENT_REASONS: [(u64, ClockEventReasons); 9] = [
    (0x1, ClockEventReasons::IDLE),
    (0x2, ClockEventReasons::APP_CLOCKS),
    (0x4, ClockEventReasons::POWER_CAP),
    (0x8, ClockEventReasons::HW_SLOWDOWN),
    (0x10, ClockEventReasons::SYNC_BOOST),
    (0x20, ClockEventReasons::SW_THERMAL),
    (0x40, ClockEventReasons::HW_THERMAL),
    (0x80, ClockEventReasons::HW_POWER_BRAKE),
    (0x100, ClockEventReasons::DISPLAY_CLOCKS),
];

/// The model set for an NVML clock event reason mask (PERF-066).
pub fn clock_event_reasons(bits: u64) -> ClockEventReasons {
    NVML_CLOCK_EVENT_REASONS
        .iter()
        .filter(|&&(bit, _)| bits & bit != 0)
        .fold(ClockEventReasons::empty(), |acc, &(_, flag)| acc | flag)
}

fn milliwatts(mw: u32) -> f32 {
    mw as f32 / 1000.0
}

/// A PCIe generation or lane count. A value above 255 is not a real link, so it is `Failed`.
fn small(r: Reading<u32>) -> Metric<u8> {
    match r {
        Reading::Ok(v) => u8::try_from(v).map_or(Metric::with(Validity::Failed), Metric::valid),
        Reading::Unsupported | Reading::Failed => Metric::with(r.validity()),
    }
}

fn info_update(hello: &HelloInfo, supports_process_util: bool) -> GpuInfoUpdate {
    GpuInfoUpdate {
        devices: hello
            .devices
            .iter()
            .map(|d| to_static(d, hello, supports_process_util))
            .collect(),
    }
}

/// Device rows keyed by identity. A device that was not in the `Hello` list is dropped.
/// `statics` holds one entry per `devices` entry, in the same order.
fn gpu_sample(
    s: ProbeResponseSample,
    devices: &[GpuDeviceInfo],
    statics: &[Arc<DeviceStatic>],
) -> GpuSample {
    let rows = s
        .devices
        .iter()
        .filter_map(|d| {
            let i = devices.iter().position(|i| i.uuid == d.uuid)?;
            let info = &devices[i];
            Some((identity(info), to_totals(d, info, statics.get(i)?)))
        })
        .collect();
    GpuSample {
        t_ns: s.t_ns,
        devices: rows,
        processes: s.processes,
        drm_processes: None,
    }
}

/// True when the probe returned process rows and none of them has a utilisation reading.
fn no_process_util(rows: Option<&[GpuProcessSample]>) -> bool {
    rows.is_some_and(|rows| {
        !rows.is_empty() && rows.iter().all(|r| r.sm_pct == Reading::Unsupported)
    })
}

/// Sends `state` with `last_error`, then waits for cancellation, waking every
/// [`PARK_RECHECK`]. No retry: the controller never restarts a parked worker.
async fn park(
    cx: &Cx,
    io: &WorkerIo,
    state: SourceState,
    last_error: Arc<str>,
) -> Outcome<(), WorkerError> {
    if let Some(stop) = send(cx, io, health(state, Some(last_error))).await {
        return stop;
    }
    loop {
        sleep(cx.now(), PARK_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
    }
}

/// Kills the probe of a cancelled generation at once; any other stop asks it to exit first.
fn stop_link<L: ProbeLink>(link: L, cancelled: bool) {
    if cancelled {
        link.kill();
    } else {
        link.shutdown();
    }
}

/// Runs `f` on the blocking pool as the job `name`. `f` gets the job's cancel check, which turns
/// true when the worker is cancelled. `Err` holds the worker's result when it must stop.
async fn blocking<R: Send + 'static>(
    cx: &Cx,
    name: &'static str,
    f: impl FnOnce(&dyn Fn() -> bool) -> R + Send + 'static,
) -> Result<R, Outcome<(), WorkerError>> {
    let mut job = match cx.spawn_blocking(move |job_cx| {
        let _running = crate::bridge::job_running(name);
        f(&|| job_cx.is_cancel_requested())
    }) {
        Ok(job) => job,
        Err(e) => {
            return Err(Outcome::Err(WorkerError::Other(
                format!("spawn: {e}").into(),
            )));
        }
    };
    match job.join(cx).await {
        Ok(done) => Ok(done),
        Err(JoinError::Cancelled(reason)) => {
            if cx.checkpoint().is_err() {
                return Err(Outcome::Ok(()));
            }
            Err(Outcome::Cancelled(reason))
        }
        Err(JoinError::Panicked(payload)) => Err(Outcome::Panicked(payload)),
        Err(JoinError::PolledAfterCompletion) => Err(Outcome::Err(WorkerError::Other(
            "job polled after completion".into(),
        ))),
    }
}

fn health(state: SourceState, last_error: Option<Arc<str>>) -> SourceUpdate {
    SourceUpdate::Health {
        id: SourceId::GpuNvidia,
        state,
        last_error,
        t_ns: now_boot_ns(),
    }
}

/// Sends one update. Returns the worker's result when it must stop: `Ok` on cancel,
/// `Err(Closed)` when the reducer is gone.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;

    use asupersync::channel::watch;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::gpu_proto::GpuApiVersions;
    use tl_model::settings::EngineSettings;

    use crate::source::{QueuedCommand, WorkerControl};

    const UUID: [u8; 16] = [
        0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x60, 0x71, 0x82, 0x93, 0xa4, 0xb5, 0xc6, 0xd7, 0xe8,
        0xf9,
    ];

    fn device_info() -> GpuDeviceInfo {
        GpuDeviceInfo {
            uuid: UUID,
            name: "NVIDIA GeForce RTX 4070".into(),
            pci_bus_id: "00000000:01:00.0".into(),
            mem_total: 12 << 30,
            power_limit_mw: Some(200_000),
            cuda_version: None,
            vbios: None,
            architecture: None,
            clock_gfx_max_mhz: Some(3_105),
            clock_mem_max_mhz: Some(14_001),
            power_max_limit_mw: Some(350_000),
            power_min_limit_mw: Some(100_000),
            slowdown_temp_c: Some(88),
            shutdown_temp_c: Some(95),
            pcie_max_gen: Some(5),
            pcie_max_width: Some(16),
        }
    }

    fn hello_info() -> HelloInfo {
        HelloInfo {
            proto: 1,
            driver: "560.35.03".into(),
            nvml: "12.560.35.03".into(),
            devices: Arc::from([device_info()]),
        }
    }

    fn device_sample() -> GpuDeviceSample {
        GpuDeviceSample {
            uuid: UUID,
            util_pct: Reading::Ok(37),
            mem_util_pct: Reading::Ok(12),
            mem_used: Reading::Ok(3 << 30),
            mem_reserved: Reading::Ok(380 << 20),
            enc_pct: Reading::Ok(0),
            dec_pct: Reading::Unsupported,
            enc_sessions: Reading::Ok(2),
            jpeg_pct: Reading::Unsupported,
            ofa_pct: Reading::Unsupported,
            temp_c: Reading::Failed,
            power_mw: Reading::Ok(31_900),
            clock_gfx_mhz: Reading::Ok(2_100),
            clock_mem_mhz: Reading::Ok(10_501),
            clock_video_mhz: Reading::Ok(1_620),
            pstate: Reading::Ok(3),
            // SwPowerCap and SwThermalSlowdown.
            clock_event_reasons: Reading::Ok(0x4 | 0x20),
            fan_pct: Reading::Unsupported,
            fan_rpm: Reading::Ok(0),
            pcie_rx_kbps: Reading::Ok(1_000),
            pcie_tx_kbps: Reading::Failed,
            pcie_gen: Reading::Ok(4),
            pcie_width: Reading::Ok(16),
            energy_mj: Reading::Ok(4_464_000_000),
        }
    }

    fn demand(visible: bool, gpus: bool, gpu_process: bool, recording: bool) -> Demand {
        Demand {
            any_surface_visible: visible,
            gpus,
            gpu_process,
            recording,
            ..Demand::default()
        }
    }

    #[test]
    fn wanted_table() {
        assert_eq!(wanted(&demand(true, true, false, false)), (true, false));
        assert_eq!(wanted(&demand(true, false, false, false)), (false, false));
        assert_eq!(wanted(&demand(false, false, false, true)), (true, true));
        assert_eq!(wanted(&demand(false, true, false, false)), (false, false));
        assert_eq!(wanted(&demand(true, true, true, false)), (true, true));
        assert_eq!(wanted(&demand(false, true, true, false)), (false, false));
        // Hidden with device history on: totals, no processes (ADR-008, ADR-017).
        let history = Demand {
            device_history: true,
            gpu_process: true,
            ..Demand::default()
        };
        assert_eq!(wanted(&history), (true, false));
    }

    #[test]
    fn to_totals_mapping() {
        let info = device_info();
        let t = to_totals(&device_sample(), &info, &device_static(&info));
        assert_eq!(t.device, DEVICE_PLACEHOLDER);
        assert_eq!(t.util_pct, Metric::valid(37.0));
        assert_eq!(t.mem_util_pct, Metric::valid(12.0));
        assert_eq!(t.mem_used, Metric::valid(3 << 30));
        assert_eq!(t.mem_total, Metric::valid(info.mem_total));
        assert_eq!(t.shared_used.validity, Validity::Unsupported);
        assert_eq!(t.shared_total.validity, Validity::Unsupported);
        assert_eq!(t.enc_pct, Metric::valid(0.0));
        assert_eq!(t.dec_pct.validity, Validity::Unsupported);
        assert_eq!(t.mem_reserved, Metric::valid(380 << 20));
        assert_eq!(t.enc_sessions, Metric::valid(2));
        assert!(!t.enc_dec_shared);
        assert_eq!(t.temp_c.validity, Validity::Failed);
        assert_eq!(t.temp_junction_c.validity, Validity::Unsupported);
        assert_eq!(t.power_w, Metric::valid(31.9));
        assert_eq!(t.power_limit_w, Metric::valid(200.0));
        assert_eq!(t.clock_gfx_mhz, Metric::valid(2_100));
        assert_eq!(t.clock_mem_mhz, Metric::valid(10_501));
        assert_eq!(t.fan_pct.validity, Validity::Unsupported);
        assert_eq!(t.pcie_rx_bps, Metric::valid(1_024_000.0));
        assert_eq!(t.pcie_tx_bps.validity, Validity::Failed);

        let no_limit = GpuDeviceInfo {
            power_limit_mw: None,
            ..info
        };
        let t = to_totals(&device_sample(), &no_limit, &device_static(&no_limit));
        assert_eq!(t.power_limit_w.validity, Validity::Unsupported);

        let unsupported = GpuDeviceSample {
            mem_reserved: Reading::Unsupported,
            enc_sessions: Reading::Unsupported,
            ..device_sample()
        };
        let t = to_totals(&unsupported, &device_info(), &device_static(&device_info()));
        assert_eq!(t.mem_reserved.validity, Validity::Unsupported);
        assert_eq!(t.enc_sessions.validity, Validity::Unsupported);
    }

    #[test]
    fn to_totals_details_fields() {
        let info = device_info();
        let statics = device_static(&info);
        let t = to_totals(&device_sample(), &info, &statics);
        assert_eq!(t.fan_rpm, Metric::valid(0));
        assert_eq!(t.clock_video_mhz, Metric::valid(1_620));
        assert_eq!(t.pstate, Metric::valid(3));
        assert_eq!(t.pcie_gen, Metric::valid(4));
        assert_eq!(t.pcie_width, Metric::valid(16));
        // 4,464,000,000 mJ is 1.24 kWh.
        assert_eq!(t.energy_since_boot, Metric::valid(4_464_000.0));
        assert!(Arc::ptr_eq(&t.info, &statics));

        let unsupported = GpuDeviceSample {
            fan_rpm: Reading::Unsupported,
            clock_video_mhz: Reading::Unsupported,
            pstate: Reading::Unsupported,
            pcie_gen: Reading::Failed,
            pcie_width: Reading::Ok(300),
            energy_mj: Reading::Unsupported,
            ..device_sample()
        };
        let t = to_totals(&unsupported, &info, &statics);
        assert_eq!(t.fan_rpm.validity, Validity::Unsupported);
        assert_eq!(t.clock_video_mhz.validity, Validity::Unsupported);
        assert_eq!(t.pstate.validity, Validity::Unsupported);
        assert_eq!(t.pcie_gen.validity, Validity::Failed);
        // No link has 300 lanes.
        assert_eq!(t.pcie_width.validity, Validity::Failed);
        assert_eq!(t.energy_since_boot.validity, Validity::Unsupported);
    }

    #[test]
    fn to_totals_clock_event_reasons() {
        let info = device_info();
        let statics = device_static(&info);
        let t = to_totals(&device_sample(), &info, &statics);
        assert_eq!(
            t.clock_event_reasons,
            Metric::valid(ClockEventReasons::POWER_CAP | ClockEventReasons::SW_THERMAL)
        );

        // No reason: the clocks are not held down.
        let none = GpuDeviceSample {
            clock_event_reasons: Reading::Ok(0),
            ..device_sample()
        };
        let t = to_totals(&none, &info, &statics);
        assert_eq!(
            t.clock_event_reasons,
            Metric::valid(ClockEventReasons::empty())
        );

        for (r, validity) in [
            (Reading::Unsupported, Validity::Unsupported),
            (Reading::Failed, Validity::Failed),
        ] {
            let without = GpuDeviceSample {
                clock_event_reasons: r,
                ..device_sample()
            };
            let t = to_totals(&without, &info, &statics);
            assert_eq!(t.clock_event_reasons.validity, validity);
        }
    }

    #[test]
    fn throttling_follows_thermal_reasons() {
        let info = device_info();
        let statics = device_static(&info);
        let with = |bits: u64| {
            let s = GpuDeviceSample {
                clock_event_reasons: Reading::Ok(bits),
                ..device_sample()
            };
            to_totals(&s, &info, &statics).throttling
        };
        // POWER_CAP (0x4) and IDLE (0x1) are not thermal.
        assert_eq!(with(0x4 | 0x1), Metric::valid(false));
        assert_eq!(with(0x8), Metric::valid(true));
        assert_eq!(with(0x20), Metric::valid(true));
        // HW_THERMAL alone is not one of the two flags the spec names.
        assert_eq!(with(0x40), Metric::valid(false));
        let unsupported = GpuDeviceSample {
            clock_event_reasons: Reading::Unsupported,
            ..device_sample()
        };
        let t = to_totals(&unsupported, &info, &statics);
        assert_eq!(t.throttling.validity, Validity::Unsupported);
    }

    #[test]
    fn to_static_carries_slowdown_threshold_as_hot_c() {
        let hello = hello_info();
        let s = to_static(&device_info(), &hello, true);
        assert_eq!(s.hot_c, Some(88.0));
        let none = GpuDeviceInfo {
            slowdown_temp_c: None,
            ..device_info()
        };
        assert_eq!(to_static(&none, &hello, true).hot_c, None);
    }

    #[test]
    fn clock_event_reason_bits() {
        assert_eq!(clock_event_reasons(0), ClockEventReasons::empty());
        assert_eq!(clock_event_reasons(0x1), ClockEventReasons::IDLE);
        assert_eq!(clock_event_reasons(0x2), ClockEventReasons::APP_CLOCKS);
        assert_eq!(clock_event_reasons(0x8), ClockEventReasons::HW_SLOWDOWN);
        assert_eq!(clock_event_reasons(0x10), ClockEventReasons::SYNC_BOOST);
        assert_eq!(clock_event_reasons(0x40), ClockEventReasons::HW_THERMAL);
        assert_eq!(clock_event_reasons(0x80), ClockEventReasons::HW_POWER_BRAKE);
        assert_eq!(
            clock_event_reasons(0x100),
            ClockEventReasons::DISPLAY_CLOCKS
        );
        // Every known bit, plus bits NVML may add later, which are dropped.
        assert_eq!(clock_event_reasons(u64::MAX), ClockEventReasons::all());
    }

    #[test]
    fn device_static_mapping() {
        let s = device_static(&GpuDeviceInfo {
            cuda_version: Some("13.2".into()),
            vbios: Some("98.06.1f.00.dc".into()),
            architecture: Some("Ada Lovelace".into()),
            ..device_info()
        });
        assert_eq!(s.cuda_version.as_deref(), Some("13.2"));
        assert_eq!(s.vbios.as_deref(), Some("98.06.1f.00.dc"));
        assert_eq!(s.architecture.as_deref(), Some("Ada Lovelace"));
        assert_eq!(s.clock_gfx_max_mhz, Some(3_105));
        assert_eq!(s.clock_mem_max_mhz, Some(14_001));
        assert_eq!(s.power_max_limit_w, Some(350.0));
        assert_eq!(s.power_min_limit_w, Some(100.0));
        assert_eq!(s.slowdown_temp_c, Some(88.0));
        assert_eq!(s.shutdown_temp_c, Some(95.0));
        assert_eq!(s.pcie_max_gen, Some(5));
        assert_eq!(s.pcie_max_width, Some(16));

        let none = device_static(&GpuDeviceInfo {
            clock_gfx_max_mhz: None,
            clock_mem_max_mhz: None,
            power_max_limit_mw: None,
            power_min_limit_mw: None,
            slowdown_temp_c: None,
            shutdown_temp_c: None,
            pcie_max_gen: None,
            pcie_max_width: Some(300),
            ..device_info()
        });
        assert_eq!(*none, DeviceStatic::default());
    }

    #[test]
    fn sysfs_pcie_max_wins_over_nvml() {
        let sys = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tl-collect/tests/fixtures/drm-basic/sys");
        // card1 in the fixture is the NVIDIA card at 0000:2d:00.0: 32 GT/s x16 maximum.
        let on_card = GpuDeviceInfo {
            pci_bus_id: "00000000:2D:00.0".into(),
            pcie_max_gen: Some(4),
            pcie_max_width: Some(8),
            ..device_info()
        };
        // No card in the fixture has this slot, so NVML's values stay.
        let elsewhere = GpuDeviceInfo {
            uuid: [9; 16],
            pci_bus_id: "00000000:41:00.0".into(),
            pcie_max_gen: Some(4),
            pcie_max_width: Some(8),
            ..device_info()
        };
        let hello = HelloInfo {
            devices: Arc::from([on_card, elsewhere]),
            ..hello_info()
        };
        let out = with_sysfs_pcie_max(hello, &sys);
        assert_eq!(out.devices[0].pcie_max_gen, Some(5));
        assert_eq!(out.devices[0].pcie_max_width, Some(16));
        assert_eq!(out.devices[1].pcie_max_gen, Some(4));
        assert_eq!(out.devices[1].pcie_max_width, Some(8));

        // No sysfs at all keeps NVML's values.
        let out = with_sysfs_pcie_max(hello_info(), Path::new("/nonexistent-tasklens-sys"));
        assert_eq!(out.devices[0].pcie_max_gen, Some(5));
    }

    #[test]
    fn to_totals_engines() {
        let t = to_totals(
            &device_sample(),
            &device_info(),
            &device_static(&device_info()),
        );
        let engines: Vec<(GpuEngine, Metric<f32>)> = t.engines.reported().collect();
        // Decode, JPEG and OFA are NotSupported in the sample, so they are not listed.
        assert_eq!(
            engines,
            [
                (GpuEngine::Gpu, Metric::valid(37.0)),
                (GpuEngine::MemCtl, Metric::valid(12.0)),
                (GpuEngine::Enc, Metric::valid(0.0)),
            ]
        );
        assert!(!t.fdinfo_partial);

        let failed = GpuDeviceSample {
            util_pct: Reading::Failed,
            ..device_sample()
        };
        let t = to_totals(&failed, &device_info(), &device_static(&device_info()));
        // A failed call keeps the engine listed.
        assert_eq!(t.engines.get(GpuEngine::Gpu).validity, Validity::Failed);
    }

    #[test]
    fn to_totals_jpeg_and_ofa() {
        let info = device_info();
        let statics = device_static(&info);
        let with = GpuDeviceSample {
            jpeg_pct: Reading::Ok(17),
            ofa_pct: Reading::Ok(0),
            ..device_sample()
        };
        let t = to_totals(&with, &info, &statics);
        let engines: Vec<GpuEngine> = t.engines.reported().map(|(e, _)| e).collect();
        assert_eq!(
            engines,
            [
                GpuEngine::Gpu,
                GpuEngine::MemCtl,
                GpuEngine::Enc,
                GpuEngine::Jpeg,
                GpuEngine::Ofa,
            ]
        );
        assert_eq!(t.engines.get(GpuEngine::Jpeg), Metric::valid(17.0));
        assert_eq!(t.engines.get(GpuEngine::Ofa), Metric::valid(0.0));

        // Without the readings neither engine is listed.
        let t = to_totals(&device_sample(), &info, &statics);
        assert!(!t.engines.is_reported(GpuEngine::Jpeg));
        assert!(!t.engines.is_reported(GpuEngine::Ofa));

        // A failed call keeps the engine listed as failed.
        let failed = GpuDeviceSample {
            jpeg_pct: Reading::Failed,
            ..device_sample()
        };
        let t = to_totals(&failed, &info, &statics);
        assert_eq!(t.engines.get(GpuEngine::Jpeg).validity, Validity::Failed);
        assert!(!t.engines.is_reported(GpuEngine::Ofa));
    }

    #[test]
    fn static_info_and_key() {
        let info = device_info();
        let s = to_static(&info, &hello_info(), true);
        assert_eq!(
            &*s.persistent_key,
            "gpu:GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"
        );
        assert!(matches!(
            &s.identity,
            DeviceIdentity::Gpu { uuid, pci_bus_id }
                if *uuid == UUID && &**pci_bus_id == "00000000:01:00.0"
        ));
        assert_eq!(&*s.name, "NVIDIA GeForce RTX 4070");
        assert_eq!(&*s.driver, "560.35.03");
        assert_eq!(&*s.nvml, "12.560.35.03");
        assert_eq!(s.mem_total, 12 << 30);
        assert_eq!(s.power_limit_mw, Some(200_000));
        assert!(s.supports_process_util);
    }

    #[test]
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::GpuNvidia);
        assert_eq!(s.name, "gpu-nvidia");
        assert_eq!(s.modes, &[EngineMode::Full]);
    }

    /// Calls the fake saw, shared between the test and every fake link.
    #[derive(Default)]
    struct Calls {
        starts: AtomicU32,
        shutdowns: AtomicU32,
        kills: AtomicU32,
        api_versions: AtomicU32,
        /// The `processes` flag of each sample request.
        samples: Mutex<Vec<bool>>,
    }

    impl Calls {
        fn samples(&self) -> Vec<bool> {
            self.samples.lock().expect("samples lock").clone()
        }
    }

    #[derive(Clone, Copy)]
    enum Hello {
        Ready,
        InitFailed(InitFailure),
    }

    #[derive(Clone, Copy)]
    enum Rows {
        /// One row with utilisation.
        Supported,
        /// One row without utilisation.
        Unsupported,
    }

    struct Fake {
        calls: Arc<Calls>,
        hello: Hello,
        rows: Rows,
        sample_err: bool,
        /// `None` makes `api_versions` fail.
        api: Option<ApiVersions>,
    }

    struct FakeLink {
        calls: Arc<Calls>,
        rows: Rows,
        sample_err: bool,
        api: Option<ApiVersions>,
    }

    impl ProbeLink for FakeLink {
        type Starter = Fake;

        fn start(
            fake: Fake,
            _probe: &Path,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<(Self, HelloOutcome), ProbeError> {
            fake.calls.starts.fetch_add(1, Ordering::SeqCst);
            let hello = match fake.hello {
                Hello::Ready => HelloOutcome::Ready(hello_info()),
                Hello::InitFailed(kind) => HelloOutcome::InitFailed {
                    kind,
                    message: "fake".into(),
                },
            };
            let link = FakeLink {
                calls: fake.calls,
                rows: fake.rows,
                sample_err: fake.sample_err,
                api: fake.api,
            };
            Ok((link, hello))
        }

        fn sample(
            &mut self,
            processes: bool,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<ProbeResponseSample, ProbeError> {
            let mut samples = self.calls.samples.lock().expect("samples lock");
            samples.push(processes);
            if self.sample_err {
                return Err(ProbeError::Deadline(crate::gpu::SAMPLE_DEADLINE));
            }
            let util = match self.rows {
                Rows::Supported => Reading::Ok(5),
                Rows::Unsupported => Reading::Unsupported,
            };
            let row = GpuProcessSample {
                uuid: UUID,
                pid: 1234,
                sm_pct: util,
                mem_pct: util,
                enc_pct: util,
                dec_pct: util,
                mem_used: Reading::Ok(1 << 20),
            };
            Ok(ProbeResponseSample {
                seq: u32::try_from(samples.len()).expect("fits"),
                t_ns: 1,
                devices: vec![device_sample()],
                processes: processes.then(|| vec![row]),
            })
        }

        fn api_versions(
            &self,
            _probe: &Path,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<ApiVersions, ProbeError> {
            self.calls.api_versions.fetch_add(1, Ordering::SeqCst);
            self.api
                .clone()
                .ok_or(ProbeError::Exited("signal 11".into()))
        }

        fn shutdown(self) {
            self.calls.shutdowns.fetch_add(1, Ordering::SeqCst);
        }

        fn kill(self) {
            self.calls.kills.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A probe whose `Hello` never comes: `start` waits until cancelled, at most 5 s, and
    /// records when it saw the cancel.
    struct SlowLink;

    #[derive(Default)]
    struct SlowStart {
        running: AtomicBool,
        saw_cancel: Mutex<Option<std::time::Instant>>,
    }

    impl ProbeLink for SlowLink {
        type Starter = Arc<SlowStart>;

        fn start(
            slow: Arc<SlowStart>,
            _probe: &Path,
            cancel: &dyn Fn() -> bool,
        ) -> Result<(Self, HelloOutcome), ProbeError> {
            slow.running.store(true, Ordering::SeqCst);
            let begin = std::time::Instant::now();
            while begin.elapsed() < Duration::from_secs(5) {
                if cancel() {
                    *slow.saw_cancel.lock().expect("lock") = Some(std::time::Instant::now());
                    return Err(ProbeError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(ProbeError::Deadline(crate::gpu::HELLO_DEADLINE))
        }

        fn sample(
            &mut self,
            _processes: bool,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<ProbeResponseSample, ProbeError> {
            unreachable!("start never succeeds")
        }

        fn api_versions(
            &self,
            _probe: &Path,
            _cancel: &dyn Fn() -> bool,
        ) -> Result<ApiVersions, ProbeError> {
            unreachable!("start never succeeds")
        }

        fn shutdown(self) {}

        fn kill(self) {}
    }

    /// On the real runtime, cancelling the worker while `Hello` is pending stops the start job
    /// within 100 ms, so the region drains and the runtime tears down (tasklens-cr0.30).
    #[test]
    fn worker_cancel_during_start_returns_fast() {
        use asupersync::runtime::RootDrainOutcome;
        use std::time::Instant;

        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let (updates, _rx) = mpsc::channel::<EngineMsg>(256);
        let (_demand, demand_rx) = watch::channel(visible_gpus());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control, control_rx) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand_rx,
            settings,
            control_rx,
            crate::dbus::SharedHandles::default(),
        );
        let slow = Arc::new(SlowStart::default());
        let starter = Arc::clone(&slow);
        let out = Ended::default();
        let sink = Arc::clone(&out);
        rt.handle()
            .try_spawn_with_cx(move |cx| async move {
                let locate = || Some(PathBuf::from("/fake/tl-gpu-probe"));
                let outcome = run_with::<SlowLink>(cx, io, locate, starter, fresh_once()).await;
                *sink.lock().expect("slot lock") = Some(outcome);
            })
            .expect("worker spawns");
        let waited = Instant::now();
        while !slow.running.load(Ordering::SeqCst) {
            assert!(waited.elapsed() < Duration::from_secs(5), "start never ran");
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(50));

        let cancel_at = Instant::now();
        let drained = rt.drain_root_region(crate::bridge::DRAIN_BUDGET);
        let took = cancel_at.elapsed();
        assert_eq!(drained, RootDrainOutcome::Quiescent);
        assert!(took < Duration::from_millis(100), "drain took {took:?}");
        let saw = slow
            .saw_cancel
            .lock()
            .expect("lock")
            .expect("start saw the cancel");
        assert!(saw.duration_since(cancel_at) < Duration::from_millis(100));
        let outcome = out.lock().expect("slot lock").take();
        assert!(matches!(outcome, Some(Outcome::Ok(()))), "{outcome:?}");
        assert!(rt.shutdown_timeout(crate::bridge::TEARDOWN_BUDGET));
    }

    type Ended = Arc<Mutex<Option<Outcome<(), WorkerError>>>>;

    struct Rig {
        lab: LabRuntime,
        rx: mpsc::Receiver<EngineMsg>,
        demand: watch::Sender<Demand>,
        control: watch::Sender<WorkerControl>,
        calls: Arc<Calls>,
        locates: Arc<AtomicU32>,
        out: Ended,
        handle: asupersync::runtime::TaskHandle<()>,
    }

    impl Rig {
        fn start(installed: bool, hello: Hello, rows: Rows, sample_err: bool, d: Demand) -> Rig {
            Self::start_with(
                installed,
                hello,
                rows,
                sample_err,
                d,
                None,
                fresh_once(),
                WorkerControl::default(),
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn start_with(
            installed: bool,
            hello: Hello,
            rows: Rows,
            sample_err: bool,
            d: Demand,
            api: Option<ApiVersions>,
            api_once: &'static AtomicBool,
            initial: WorkerControl,
        ) -> Rig {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(10_000_000));
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
            let (demand, demand_rx) = watch::channel(d);
            let (_settings_tx, settings) = watch::channel(EngineSettings::default());
            let (control, control_rx) = watch::channel(initial);
            let io = WorkerIo::new(
                updates,
                demand_rx,
                settings,
                control_rx,
                crate::dbus::SharedHandles::default(),
            );
            let calls = Arc::new(Calls::default());
            let fake = Fake {
                calls: Arc::clone(&calls),
                hello,
                rows,
                sample_err,
                api,
            };
            let locates = Arc::new(AtomicU32::new(0));
            let counter = Arc::clone(&locates);
            let locate = move || {
                counter.fetch_add(1, Ordering::SeqCst);
                installed.then(|| PathBuf::from("/fake/tl-gpu-probe"))
            };
            let out = Ended::default();
            let sink = Arc::clone(&out);
            let root = lab.state.create_root_region(Budget::INFINITE);
            let (task, handle) = lab
                .state
                .create_task(root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let outcome = run_with::<FakeLink>(cx, io, locate, fake, api_once).await;
                    *sink.lock().expect("slot lock") = Some(outcome);
                })
                .expect("task is created");
            lab.scheduler.lock().schedule(task, 0);
            lab.run_until_idle();
            Rig {
                lab,
                rx,
                demand,
                control,
                calls,
                locates,
                out,
                handle,
            }
        }

        fn advance(&mut self, d: Duration) {
            self.lab
                .advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        fn drain(&mut self) -> Vec<SourceUpdate> {
            let mut got = Vec::new();
            while let Ok(EngineMsg::Source(update)) = self.rx.try_recv() {
                got.push(update);
            }
            got
        }

        fn ended(&self) -> bool {
            self.out.lock().expect("slot lock").is_some()
        }

        fn take_outcome(&self) -> Option<Outcome<(), WorkerError>> {
            self.out.lock().expect("slot lock").take()
        }
    }

    /// A flag of its own per test, so parallel tests do not share the process-wide one.
    fn fresh_once() -> &'static AtomicBool {
        Box::leak(Box::new(AtomicBool::new(false)))
    }

    fn api_fixture() -> ApiVersions {
        ApiVersions {
            devices: vec![GpuApiVersions {
                pci_bus_id: "00000000:01:00.0".into(),
                vulkan_api: Some("1.4.312".into()),
                vulkan_driver: Some("NVIDIA 615.71.09".into()),
                opengl: Some("4.6.0 NVIDIA 615.71.09".into()),
            }],
        }
    }

    fn is_health(u: &SourceUpdate, want: SourceState, text: Option<&str>) -> bool {
        matches!(
            u,
            SourceUpdate::Health { id: SourceId::GpuNvidia, state, last_error, .. }
                if *state == want && last_error.as_deref() == text
        )
    }

    fn gpu_samples(updates: &[SourceUpdate]) -> Vec<&GpuSample> {
        updates
            .iter()
            .filter_map(|u| match u {
                SourceUpdate::Gpu(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn visible_gpus() -> Demand {
        demand(true, true, false, false)
    }

    #[test]
    fn worker_not_installed_parks() {
        let mut rig = Rig::start(false, Hello::Ready, Rows::Supported, false, visible_gpus());
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            is_health(&got[0], SourceState::Unsupported, Some(NOT_INSTALLED)),
            "{got:?}"
        );
        for _ in 0..10 {
            rig.advance(PARK_RECHECK);
        }
        assert!(!rig.ended(), "still parked after 10 minutes");
        assert_eq!(rig.locates.load(Ordering::SeqCst), 1);
        assert_eq!(rig.calls.starts.load(Ordering::SeqCst), 0);
        assert!(rig.drain().is_empty());
    }

    #[test]
    fn worker_init_failed_unsupported_no_retry() {
        for kind in [InitFailure::DriverNotLoaded, InitFailure::LibraryNotFound] {
            let mut rig = Rig::start(
                true,
                Hello::InitFailed(kind),
                Rows::Supported,
                false,
                visible_gpus(),
            );
            let text = format!("{kind:?}");
            let got = rig.drain();
            assert_eq!(got.len(), 1, "{got:?}");
            assert!(
                is_health(&got[0], SourceState::Unsupported, Some(&text)),
                "{got:?}"
            );
            assert_eq!(rig.calls.shutdowns.load(Ordering::SeqCst), 1);
            for _ in 0..30 {
                rig.advance(PARK_RECHECK);
            }
            assert!(!rig.ended(), "{kind:?}: still parked after 30 minutes");
            assert_eq!(rig.calls.starts.load(Ordering::SeqCst), 1, "{kind:?}");
            assert!(rig.calls.samples().is_empty());
            assert!(rig.drain().is_empty());
        }
    }

    #[test]
    fn worker_init_failed_no_permission_is_err() {
        let mut rig = Rig::start(
            true,
            Hello::InitFailed(InitFailure::NoPermission),
            Rows::Supported,
            false,
            visible_gpus(),
        );
        let outcome = rig.take_outcome().expect("returned at once");
        assert!(
            matches!(outcome, Outcome::Err(WorkerError::Probe(_))),
            "{outcome:?}"
        );
        assert_eq!(rig.calls.shutdowns.load(Ordering::SeqCst), 1);
        assert!(rig.drain().is_empty());
    }

    #[test]
    fn worker_samples_at_1hz_and_process_flag() {
        let mut rig = Rig::start(true, Hello::Ready, Rows::Supported, false, visible_gpus());
        let got = rig.drain();
        assert!(
            matches!(&got[0], SourceUpdate::GpuInfo(info)
                if info.devices.len() == 1 && info.devices[0].supports_process_util),
            "{got:?}"
        );
        assert!(is_health(&got[1], SourceState::Running, None), "{got:?}");
        let mut samples = gpu_samples(&got).len();
        assert_eq!(samples, 1, "first sample at 0 s");
        for second in 1..10 {
            rig.advance(PERIOD);
            let got = rig.drain();
            let gpu = gpu_samples(&got);
            assert_eq!(gpu.len(), 1, "one sample at {second} s: {got:?}");
            assert!(gpu[0].processes.is_none());
            assert_eq!(gpu[0].devices.len(), 1);
            samples += 1;
        }
        assert_eq!(samples, 10);
        assert_eq!(rig.calls.samples(), vec![false; 10]);

        rig.demand
            .send(demand(true, true, true, false))
            .expect("demand sends");
        rig.advance(PERIOD);
        let got = rig.drain();
        let gpu = gpu_samples(&got);
        assert_eq!(gpu.len(), 1, "{got:?}");
        assert_eq!(gpu[0].processes.as_ref().map(Vec::len), Some(1));
        assert_eq!(rig.calls.samples().last(), Some(&true));
        // Rows with utilisation keep `supports_process_util` as it was.
        assert!(!got.iter().any(|u| matches!(u, SourceUpdate::GpuInfo(_))));
        assert!(!rig.ended());
    }

    #[test]
    fn worker_resends_info_without_process_util() {
        let d = demand(true, true, true, false);
        let mut rig = Rig::start(true, Hello::Ready, Rows::Unsupported, false, d);
        let got = rig.drain();
        let infos: Vec<bool> = got
            .iter()
            .filter_map(|u| match u {
                SourceUpdate::GpuInfo(info) => Some(info.devices[0].supports_process_util),
                _ => None,
            })
            .collect();
        assert_eq!(infos, vec![true, false], "{got:?}");
        rig.advance(PERIOD);
        let got = rig.drain();
        assert!(
            !got.iter().any(|u| matches!(u, SourceUpdate::GpuInfo(_))),
            "sent once"
        );
    }

    #[test]
    fn worker_idle_sends_nothing() {
        let mut rig = Rig::start(
            true,
            Hello::Ready,
            Rows::Supported,
            false,
            Demand::default(),
        );
        let got = rig.drain();
        assert_eq!(got.len(), 2, "GpuInfo and Running only: {got:?}");
        for _ in 0..60 {
            rig.advance(PERIOD);
        }
        assert!(rig.calls.samples().is_empty());
        assert!(rig.drain().is_empty());
        assert!(!rig.ended());
    }

    #[test]
    fn worker_refresh_now_forces_one_sample() {
        let mut rig = Rig::start(
            true,
            Hello::Ready,
            Rows::Supported,
            false,
            Demand::default(),
        );
        rig.drain();
        rig.control
            .send(WorkerControl {
                commands: Arc::from([QueuedCommand {
                    seq: 1,
                    target: SourceId::GpuNvidia,
                    command: WorkerCommand::RefreshNow,
                }]),
                ..WorkerControl::default()
            })
            .expect("control sends");
        for _ in 0..5 {
            rig.advance(PERIOD);
        }
        assert_eq!(rig.calls.samples(), vec![false]);
        assert_eq!(gpu_samples(&rig.drain()).len(), 1);
    }

    fn refresh(seq: u64) -> QueuedCommand {
        QueuedCommand {
            seq,
            target: SourceId::GpuNvidia,
            command: WorkerCommand::RefreshNow,
        }
    }

    /// R-276: a generation resumes after `started`. A `RefreshNow` that was running when the
    /// last generation died is skipped without a message; one queued during the restart runs.
    #[test]
    fn worker_refresh_now_progress_after_restart() {
        let progress = Arc::new(crate::source::CommandProgress::default());
        progress.set_started(SourceId::GpuNvidia, 3);
        progress.set_done(SourceId::GpuNvidia, 2);
        let initial = WorkerControl {
            commands: Arc::from([refresh(2), refresh(3), refresh(4)]),
            progress: Arc::clone(&progress),
            ..WorkerControl::default()
        };
        let mut rig = Rig::start_with(
            true,
            Hello::Ready,
            Rows::Supported,
            false,
            Demand::default(),
            None,
            fresh_once(),
            initial,
        );
        // The first turn at 0 s skips 3 and runs 4.
        let got = rig.drain();
        assert!(
            !got.iter().any(|u| matches!(
                u,
                SourceUpdate::Health {
                    state: SourceState::Failed,
                    ..
                }
            )),
            "no message for a skipped RefreshNow: {got:?}"
        );
        assert_eq!(gpu_samples(&got).len(), 1, "{got:?}");
        for _ in 0..3 {
            rig.advance(PERIOD);
        }
        assert_eq!(rig.calls.samples(), vec![false], "4 runs once");
        assert_eq!(progress.started(SourceId::GpuNvidia), 4);
        assert_eq!(progress.done(SourceId::GpuNvidia), 4);
        assert!(rig.drain().is_empty());
    }

    #[test]
    fn worker_sample_error_restarts() {
        let mut rig = Rig::start(true, Hello::Ready, Rows::Supported, true, visible_gpus());
        let outcome = rig.take_outcome().expect("returned after the first sample");
        assert!(
            matches!(&outcome, Outcome::Err(WorkerError::Probe(text)) if text.contains("deadline")),
            "{outcome:?}"
        );
        let got = rig.drain();
        assert!(gpu_samples(&got).is_empty(), "{got:?}");
    }

    #[test]
    fn worker_cancel_kills_once() {
        let mut rig = Rig::start(true, Hello::Ready, Rows::Supported, false, visible_gpus());
        for _ in 0..3 {
            rig.advance(PERIOD);
        }
        rig.handle.abort();
        rig.lab.run_until_idle();
        for _ in 0..5 {
            if rig.ended() {
                break;
            }
            rig.advance(PERIOD);
        }
        let outcome = rig.take_outcome().expect("returned after the cancel");
        assert!(matches!(outcome, Outcome::Ok(())), "{outcome:?}");
        // A cancelled worker kills the probe without the Shutdown request and its grace wait.
        assert_eq!(rig.calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(rig.calls.shutdowns.load(Ordering::SeqCst), 0);
        let report = rig.lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn worker_api_versions_once() {
        let once = fresh_once();
        let mut rig = Rig::start_with(
            true,
            Hello::Ready,
            Rows::Supported,
            false,
            visible_gpus(),
            Some(api_fixture()),
            once,
            WorkerControl::default(),
        );
        let got = rig.drain();
        let api: Vec<usize> = got
            .iter()
            .enumerate()
            .filter(|(_, u)| matches!(u, SourceUpdate::GpuApiVersions(v) if *v == api_fixture()))
            .map(|(i, _)| i)
            .collect();
        let first_gpu = got
            .iter()
            .position(|u| matches!(u, SourceUpdate::Gpu(_)))
            .expect("a first sample");
        assert_eq!(api.len(), 1, "{got:?}");
        assert!(api[0] < first_gpu, "{got:?}");
        assert!(
            matches!(&got[0], SourceUpdate::GpuInfo(_)),
            "after GpuInfo: {got:?}"
        );
        assert_eq!(rig.calls.api_versions.load(Ordering::SeqCst), 1);

        // A second generation in the same process does not run it again.
        let mut again = Rig::start_with(
            true,
            Hello::Ready,
            Rows::Supported,
            false,
            visible_gpus(),
            Some(api_fixture()),
            once,
            WorkerControl::default(),
        );
        again.advance(PERIOD);
        let got = again.drain();
        assert!(
            !got.iter()
                .any(|u| matches!(u, SourceUpdate::GpuApiVersions(_))),
            "{got:?}"
        );
        assert_eq!(gpu_samples(&got).len(), 2, "{got:?}");
        assert_eq!(again.calls.api_versions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn worker_api_versions_error_is_quiet() {
        let mut rig = Rig::start(true, Hello::Ready, Rows::Supported, false, visible_gpus());
        let got = rig.drain();
        assert_eq!(rig.calls.api_versions.load(Ordering::SeqCst), 1);
        assert_eq!(
            got.len(),
            3,
            "GpuInfo, Running and the first sample: {got:?}"
        );
        assert!(matches!(&got[0], SourceUpdate::GpuInfo(_)), "{got:?}");
        assert!(is_health(&got[1], SourceState::Running, None), "{got:?}");
        assert!(matches!(&got[2], SourceUpdate::Gpu(_)), "{got:?}");
        rig.advance(PERIOD);
        assert_eq!(gpu_samples(&rig.drain()).len(), 1);
        assert!(!rig.ended());
    }
}
