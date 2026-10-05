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
//! The reducer maps pids and assigns `DeviceIdx`. The probe is not restarted after resume, so
//! `reset_generation` is ignored. Device names are logged at debug only; pids never.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::{MissedTickBehavior, interval, sleep, sleep_until};
use asupersync::{Cx, Outcome};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::gpu_proto::{GpuDeviceInfo, GpuDeviceSample, GpuProcessSample, InitFailure, Reading};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::DeviceIdx;
use tl_model::system::{GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::gpu::client::{HelloInfo, HelloOutcome, ProbeClient, ProbeResponseSample};
use crate::gpu::{NOT_INSTALLED, ProbeError, VERSION_MISMATCH};
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

/// What the worker needs from a probe connection. [`ProbeClient`] is the real one; tests
/// inject a fake.
pub trait ProbeLink: Sized + Send + 'static {
    /// What `start` needs besides the probe path: `()` for the real client.
    type Starter: Send + 'static;

    /// Starts the probe and runs `Hello`. Runs on the blocking pool.
    fn start(starter: Self::Starter, probe: &Path) -> Result<(Self, HelloOutcome), ProbeError>;

    /// One `Sample` request. Runs on the blocking pool. On `Err` the child is already gone.
    fn sample(&mut self, processes: bool) -> Result<ProbeResponseSample, ProbeError>;

    /// Asks the child to exit, then kills and reaps it. Blocks for at most about 200 ms.
    fn shutdown(self);
}

impl ProbeLink for ProbeClient {
    type Starter = ();

    fn start((): (), probe: &Path) -> Result<(Self, HelloOutcome), ProbeError> {
        let client = ProbeClient::start(probe)?;
        let hello = match (&client.info, &client.init_failed) {
            (Some(info), _) => HelloOutcome::Ready(info.clone()),
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

    fn sample(&mut self, processes: bool) -> Result<ProbeResponseSample, ProbeError> {
        ProbeClient::sample(self, processes)
    }

    fn shutdown(self) {
        ProbeClient::shutdown(self);
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
    _cfg: Arc<EngineConfig>,
    locate: fn() -> Option<PathBuf>,
) -> Outcome<(), WorkerError> {
    run_with::<ProbeClient>(cx, io, locate, ()).await
}

/// The worker body over any [`ProbeLink`]. `locate` is called once.
pub async fn run_with<L: ProbeLink>(
    cx: Cx,
    io: WorkerIo,
    locate: impl FnOnce() -> Option<PathBuf>,
    starter: L::Starter,
) -> Outcome<(), WorkerError> {
    let Some(probe) = locate() else {
        return park(&cx, &io, SourceState::Unsupported, NOT_INSTALLED.into()).await;
    };

    let started = match blocking(&cx, move || L::start(starter, &probe)).await {
        Ok(started) => started,
        Err(stop) => return stop,
    };
    let (mut link, info) = match started {
        Ok((link, HelloOutcome::Ready(info))) => (link, info),
        Ok((link, HelloOutcome::InitFailed { kind, message })) => {
            tracing::debug!(?kind, "GPU probe init failed");
            if let Err(stop) = blocking(&cx, move || link.shutdown()).await {
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
        Err(e) => return Outcome::Err(e.into()),
    };
    for device in info.devices.iter() {
        tracing::debug!(name = %device.name, "GPU probe device");
    }

    // Stays true until a process sample shows that the driver has no per-process utilisation.
    let mut supports_process_util = true;
    let first = [
        SourceUpdate::GpuInfo(info_update(&info, supports_process_util)),
        health(SourceState::Running, None),
    ];
    for update in first {
        if let Some(stop) = send(&cx, &io, update).await {
            link.shutdown();
            return stop;
        }
    }

    let mut iv = interval(cx.now(), PERIOD);
    iv.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Commands queued before this generation started are not run.
    let mut last_seq = io
        .control
        .borrow_and_clone()
        .commands
        .last()
        .map_or(0, |c| c.seq);

    loop {
        sleep_until(iv.deadline()).await;
        if cx.checkpoint().is_err() {
            link.shutdown();
            return Outcome::Ok(());
        }
        iv.tick(cx.now());

        let (totals, processes) = wanted(&io.demand.borrow_and_clone());
        let control = io.control.borrow_and_clone();
        let forced = control.commands.iter().any(|c| {
            c.seq > last_seq
                && c.target == SourceId::GpuNvidia
                && c.command == WorkerCommand::RefreshNow
        });
        if let Some(newest) = control.commands.last() {
            last_seq = last_seq.max(newest.seq);
        }
        drop(control);
        if !totals && !processes && !forced {
            continue;
        }

        // On cancel the closure, and with it the link, is dropped, which kills the child.
        let (back, result) = match blocking(&cx, move || {
            let result = link.sample(processes);
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
        updates.push(SourceUpdate::Gpu(gpu_sample(sample, &info.devices)));
        for update in updates {
            if let Some(stop) = send(&cx, &io, update).await {
                link.shutdown();
                return stop;
            }
        }
    }
}

/// `(totals, processes)`. Hidden surfaces stop sampling unless history records GPUs (ADR-008).
pub fn wanted(d: &Demand) -> (bool, bool) {
    let totals = (d.any_surface_visible && d.gpus) || d.recording;
    let processes = (d.any_surface_visible && d.gpu_process) || d.recording;
    (totals, processes)
}

pub fn identity(info: &GpuDeviceInfo) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid: info.uuid,
        pci_bus_id: info.pci_bus_id.as_str().into(),
    }
}

/// `device` is a placeholder the reducer overwrites (R-155 item 8).
pub fn to_totals(s: &GpuDeviceSample, info: &GpuDeviceInfo) -> GpuTotals {
    let pct = |r: Reading<u32>| metric(r, |v| v as f32);
    let kbps = |r: Reading<u32>| metric(r, |v| v as f32 * 1024.0);
    GpuTotals {
        device: DEVICE_PLACEHOLDER,
        util_pct: pct(s.util_pct),
        mem_util_pct: pct(s.mem_util_pct),
        mem_used: metric(s.mem_used, |v| v),
        mem_total: Metric::valid(info.mem_total),
        // NVML has no GTT (PERF-064).
        shared_used: Metric::with(Validity::Unsupported),
        shared_total: Metric::with(Validity::Unsupported),
        enc_pct: pct(s.enc_pct),
        dec_pct: pct(s.dec_pct),
        temp_c: metric(s.temp_c, |v| v as f32),
        power_w: metric(s.power_mw, milliwatts),
        power_limit_w: match info.power_limit_mw {
            Some(mw) => Metric::valid(milliwatts(mw)),
            None => Metric::with(Validity::Unsupported),
        },
        clock_gfx_mhz: metric(s.clock_gfx_mhz, |v| v),
        clock_mem_mhz: metric(s.clock_mem_mhz, |v| v),
        fan_pct: pct(s.fan_pct),
        // NVML reports KB/s.
        pcie_rx_bps: kbps(s.pcie_rx_kbps),
        pcie_tx_bps: kbps(s.pcie_tx_kbps),
    }
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

fn milliwatts(mw: u32) -> f32 {
    mw as f32 / 1000.0
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
fn gpu_sample(s: ProbeResponseSample, devices: &[GpuDeviceInfo]) -> GpuSample {
    let rows = s
        .devices
        .iter()
        .filter_map(|d| {
            let info = devices.iter().find(|i| i.uuid == d.uuid)?;
            Some((identity(info), to_totals(d, info)))
        })
        .collect();
    GpuSample {
        t_ns: s.t_ns,
        devices: rows,
        processes: s.processes,
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

/// Runs `f` on the blocking pool. `Err` holds the worker's result when it must stop.
async fn blocking<R: Send + 'static>(
    cx: &Cx,
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, Outcome<(), WorkerError>> {
    let mut job = match cx.spawn_blocking(move |_| f()) {
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
    use std::sync::atomic::{AtomicU32, Ordering};

    use asupersync::channel::watch;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
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
            enc_pct: Reading::Ok(0),
            dec_pct: Reading::Unsupported,
            temp_c: Reading::Failed,
            power_mw: Reading::Ok(31_900),
            clock_gfx_mhz: Reading::Ok(2_100),
            clock_mem_mhz: Reading::Ok(10_501),
            fan_pct: Reading::Unsupported,
            pcie_rx_kbps: Reading::Ok(1_000),
            pcie_tx_kbps: Reading::Failed,
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
    }

    #[test]
    fn to_totals_mapping() {
        let info = device_info();
        let t = to_totals(&device_sample(), &info);
        assert_eq!(t.device, DEVICE_PLACEHOLDER);
        assert_eq!(t.util_pct, Metric::valid(37.0));
        assert_eq!(t.mem_util_pct, Metric::valid(12.0));
        assert_eq!(t.mem_used, Metric::valid(3 << 30));
        assert_eq!(t.mem_total, Metric::valid(info.mem_total));
        assert_eq!(t.shared_used.validity, Validity::Unsupported);
        assert_eq!(t.shared_total.validity, Validity::Unsupported);
        assert_eq!(t.enc_pct, Metric::valid(0.0));
        assert_eq!(t.dec_pct.validity, Validity::Unsupported);
        assert_eq!(t.temp_c.validity, Validity::Failed);
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
        let t = to_totals(&device_sample(), &no_limit);
        assert_eq!(t.power_limit_w.validity, Validity::Unsupported);
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
    }

    struct FakeLink {
        calls: Arc<Calls>,
        rows: Rows,
        sample_err: bool,
    }

    impl ProbeLink for FakeLink {
        type Starter = Fake;

        fn start(fake: Fake, _probe: &Path) -> Result<(Self, HelloOutcome), ProbeError> {
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
            };
            Ok((link, hello))
        }

        fn sample(&mut self, processes: bool) -> Result<ProbeResponseSample, ProbeError> {
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

        fn shutdown(self) {
            self.calls.shutdowns.fetch_add(1, Ordering::SeqCst);
        }
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
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(10_000_000));
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
            let (demand, demand_rx) = watch::channel(d);
            let (_settings_tx, settings) = watch::channel(EngineSettings::default());
            let (control, control_rx) = watch::channel(WorkerControl::default());
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
                    let outcome = run_with::<FakeLink>(cx, io, locate, fake).await;
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
    fn worker_cancel_shuts_down_once() {
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
        assert_eq!(rig.calls.shutdowns.load(Ordering::SeqCst), 1);
        let report = rig.lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
}
