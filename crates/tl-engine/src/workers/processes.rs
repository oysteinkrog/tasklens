//! The `Processes` worker (ADR-004, ADR-008, ADR-016): wraps the tl-collect `ProcessSampler` and
//! turns each sweep into a `SourceUpdate::Processes(ProcessSweep)`.
//!
//! One sweep is one blocking job, at `processes.interval_ms` (250 to 10,000 ms) while the table,
//! a process detail, a process graph or recording needs it, and not at all otherwise
//! (PROC-002, PROC-004, PROC-093). Records hold command lines and paths: this module logs only
//! the record count (ADR-015).
//!
//! While the helper is `Unlocked`, `HelperReads` fills the Restricted cells of visible rows
//! through `ReadProcessFiles` at 1 Hz, after the sweep and outside it (PROC-076). The next sweep
//! applies the values. Exe and cwd paths from the helper are never logged.
//!
//! The sweep uses the session's shared package index (R-163 item 1). Every `PKG_CHECK` the
//! worker checks `PackageIndex::due` and runs `run_batch` as its own blocking job, beside the
//! sweep job and never inside it. The index keeps the 2 s debounce.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use asupersync::channel::watch;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep_until;
use asupersync::types::Time;
use asupersync::{Cx, Outcome};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::{SmallVec, smallvec};
use tl_collect::CollectError;
use tl_collect::apps::AppPaths;
use tl_collect::apps::classify::{ProtectInputs, wayland_compositor_pid};
use tl_collect::apps::naming::exe_basename;
use tl_collect::counters::{Clock, resumed_between};
use tl_collect::parse::filenr::parse_file_nr;
use tl_collect::parse::topology::parse_cpu_list;
use tl_collect::pkg::PackageIndex;
use tl_collect::proc::io::{PidIo, parse_io};
use tl_collect::proc::meta::{PasswdUserNames, UserNames};
use tl_collect::proc::sweep::{ProcessSampler, SweepCtx, SweepInputs, SweepOutput};
use tl_collect::sanitize::sanitize_link;
use tl_model::demand::{ColumnSet, Demand};
use tl_model::health::{HelperState, SourceId, SourceState};
use tl_model::helper_proto::{HelperError, HelperReply, HelperReplyBody, HelperRequest, ProcFiles};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::process::{ProcFlags, ProcessExtras, ProcessExtrasP2, ProcessMeta, ProcessRecord};
use tl_model::settings::EngineSettings;
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::{ProcessField, Validity};

use crate::config::{EngineConfig, EngineMode};
use crate::helper::{HelperCallError, HelperHandle, HelperReadError};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{ProcessSweep, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

/// `processes.interval_ms` is clamped to this range (ADR-008).
pub const MIN_INTERVAL_MS: u32 = 250;
pub const MAX_INTERVAL_MS: u32 = 10_000;

/// The process rate table (ADR-008 row "Process table"): on while any process consumer is
/// visible or recording is on. Graph targets keep it on with the table hidden (R-154 item 2),
/// and so does the Overview Top processes box (`process_top`, crw.45).
pub fn processes_period(d: &Demand, s: &EngineSettings) -> Option<Duration> {
    let wanted = d.process_table
        || d.process_top
        || !d.detail_keys.is_empty()
        || d.gpu_process
        || d.recording
        || !d.details.is_empty()
        || !d.affinity_targets.is_empty()
        || !d.graph_targets.is_empty();
    wanted.then(|| {
        Duration::from_millis(u64::from(
            s.interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS),
        ))
    })
}

/// The status-only columns (R-286): with one shown, `status` is read for every pid.
const STATUS_COLUMNS: [ColumnId; 4] = [
    ColumnId::Swap,
    ColumnId::WsPeak,
    ColumnId::VsizePeak,
    ColumnId::Affinity,
];

/// The optional per-process reads of a sweep, from the demand and settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepWants {
    /// The Handles column: one directory read per process.
    pub fd_count: bool,
    /// A status-only column or the affinity dialog: `status` for every pid (R-286).
    pub status_fields: bool,
    /// An I/O or disk column, or history recording: `io` for every pid (R-286).
    pub io: bool,
}

/// The reads `period` turns on for the next sweep.
pub fn sweep_wants(d: &Demand, s: &EngineSettings) -> SweepWants {
    let columns = &d.process_columns;
    SweepWants {
        fd_count: columns.contains(ColumnId::Handles),
        status_fields: STATUS_COLUMNS.iter().any(|&c| columns.contains(c))
            || !d.affinity_targets.is_empty(),
        io: columns
            .iter()
            .any(|c| (ColumnId::Disk..=ColumnId::IoWriteOps).contains(&c))
            || d.recording
            || s.history.enabled,
    }
}

pub struct ProcessesSource {
    sampler: ProcessSampler,
    /// Reused every sweep.
    out: SweepOutput,
    ctx: SweepCtx,
    protect: ProtectInputs,
    cfg: Arc<EngineConfig>,
    /// Set by `period`: the optional reads run only while something shows them.
    wants: Cell<SweepWants>,
    /// `Demand.detail_keys`, set by `period`: the visible rows whose P2 columns the sweep reads.
    detail_keys: RefCell<Arc<[ProcessKey]>>,
    /// The sampler's clock, also used for the helper's I/O rates.
    clock: fn() -> (TimeNs, u64),
    /// Set by `helper_reads`.
    helper: Option<HelperSide>,
}

impl ProcessesSource {
    /// `cfg.app_paths`, or the paths from the environment when it is `None`, and the user names
    /// of `<root>/etc/passwd`, where `<root>` is the parent of `cfg.sys_root.proc`. Fails when
    /// the boot id cannot be read or `<sys>/fs/cgroup` cannot be opened.
    pub fn new(cfg: Arc<EngineConfig>) -> Result<Self, CollectError> {
        let base = cfg.sys_root.proc.parent().unwrap_or(Path::new("/"));
        let users = PasswdUserNames::load(&base.join("etc/passwd"));
        let paths = cfg.app_paths.clone().unwrap_or_else(host_app_paths);
        Self::with_parts(cfg, &paths, Box::new(users))
    }

    /// `new` with the app paths and user names given, for tests.
    pub fn with_parts(
        cfg: Arc<EngineConfig>,
        paths: &AppPaths,
        users: Box<dyn UserNames + Send>,
    ) -> Result<Self, CollectError> {
        let root = &cfg.sys_root;
        let boot_id = std::fs::read_to_string(root.proc.join("sys/kernel/random/boot_id"))
            .map_err(CollectError::from)
            .and_then(|text| {
                BootId::from_uuid_text(&text).ok_or(CollectError::Parse {
                    file: "boot_id",
                    what: "format",
                })
            })?;
        let sampler = ProcessSampler::new(root.clone(), paths, users)?;
        let ctx = SweepCtx {
            boot_id,
            self_uid: rustix::process::getuid().as_raw(),
            ticks_per_sec: rustix::param::clock_ticks_per_second(),
            online_cpus: 1,
            want_fd_count: false,
            want_status_fields: false,
            want_io: false,
        };
        let protect = ProtectInputs {
            self_pids: smallvec![
                rustix::process::getpid()
                    .as_raw_nonzero()
                    .get()
                    .unsigned_abs()
            ],
            compositor_pid: host_compositor_pid(),
            session_bus_pid: None,
        };
        Ok(Self {
            sampler,
            out: SweepOutput::default(),
            ctx,
            protect,
            cfg,
            wants: Cell::new(SweepWants::default()),
            detail_keys: RefCell::new(Arc::from([])),
            clock: real_clock,
            helper: None,
        })
    }

    /// The sweep reads packages from `index`, the session's shared index.
    pub fn with_package_index(mut self, index: PackageIndex) -> Self {
        *self.sampler.pkg_mut() = index;
        self
    }

    /// The sampler with a test clock (CLOCK_BOOTTIME ns, CLOCK_MONOTONIC ns).
    pub fn with_clock(mut self, clock: fn() -> (TimeNs, u64)) -> Self {
        self.sampler = self.sampler.with_clock(clock);
        self.clock = clock;
        self
    }

    /// Connects the sweep to `handle`. Run the returned `HelperReads` next to `source::run`, as
    /// `run` does.
    pub fn helper_reads(&mut self, handle: HelperHandle) -> HelperReads {
        let store = Arc::new(Mutex::new(HelperStore::default()));
        let (wanted_tx, wanted) = watch::channel(Wanted::from([]));
        self.helper = Some(HelperSide {
            handle: handle.clone(),
            wanted: wanted_tx,
            wanted_sent: false,
            store: Arc::clone(&store),
            metas: FxHashMap::default(),
        });
        HelperReads {
            handle,
            wanted,
            store,
            clock: self.clock,
            cursor: None,
        }
    }

    /// `<sys>/devices/system/cpu/online`, or the last value when it cannot be read.
    fn read_online_cpus(&self) -> u32 {
        let path = self.cfg.sys_root.sys.join("devices/system/cpu/online");
        let mut cpus: SmallVec<[u16; 64]> = SmallVec::new();
        match std::fs::read(path) {
            Ok(bytes) if parse_cpu_list(&bytes, &mut cpus).is_ok() && !cpus.is_empty() => {
                u32::try_from(cpus.len()).unwrap_or(u32::MAX)
            }
            _ => self.ctx.online_cpus,
        }
    }

    /// `/proc/sys/fs/file-nr`, field 1 (R-166 B3-7).
    fn read_file_nr(&self) -> Option<u64> {
        let path = self.cfg.sys_root.proc.join("sys/fs/file-nr");
        let bytes = std::fs::read(path).ok()?;
        parse_file_nr(&bytes).ok()
    }
}

impl Source for ProcessesSource {
    fn id(&self) -> SourceId {
        SourceId::Processes
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        self.wants.set(sweep_wants(d, s));
        self.detail_keys.replace(Arc::clone(&d.detail_keys));
        processes_period(d, s)
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        self.ctx.online_cpus = self.read_online_cpus();
        let wants = self.wants.get();
        self.ctx.want_fd_count = wants.fd_count;
        self.ctx.want_status_fields = wants.status_fields;
        self.ctx.want_io = wants.io;
        let detail_keys = Arc::clone(&self.detail_keys.borrow());
        let inputs = SweepInputs {
            protect: &self.protect,
            // P2 columns, the package among them, for the visible rows (R-163 item 2).
            detail_keys: &detail_keys,
            // PSS and USS for detail keys come from the ProcessDetail worker (E10-A2).
            want_pss: false,
            window_pids: None,
            wm_class: None,
        };
        self.sampler.sweep(&self.ctx, &inputs, &mut self.out);

        // pid 1 is always visible, so no record at all means `<proc>` itself is unreadable.
        if self.out.records.is_empty() && std::fs::read_dir(&self.cfg.sys_root.proc).is_err() {
            let health = SourceUpdate::Health {
                id: SourceId::Processes,
                state: SourceState::Failed,
                last_error: Some("cannot read /proc".into()),
                t_ns: now_boot_ns(),
            };
            return (self, health);
        }

        let file_nr = self.read_file_nr();
        // Index aligned with `records` when `detail_keys` is not empty, else empty.
        let mut extras_p2 = std::mem::take(&mut self.out.extras_p2);
        if let Some(helper) = &mut self.helper {
            helper.after_sweep(&mut self.out, &mut extras_p2);
        }
        let out = &mut self.out;
        let records = std::mem::take(&mut out.records);
        tracing::debug!(count = records.len(), "process sweep");
        let system_threads = records
            .iter()
            .fold(0u32, |sum, r| sum.saturating_add(r.threads));
        let sweep = ProcessSweep {
            observed_start: out.observed_start,
            observed_end: out.observed_end,
            records,
            extras: std::mem::take(&mut out.extras),
            // Filled for detail keys by the P2 readers (R-163 item 2), plus the helper's cwd
            // values.
            extras_p2,
            app_stats: out.apps.iter().map(|a| a.to_model()).collect(),
            // Unit resource columns are P2.
            unit_stats: Vec::new(),
            system_threads,
            file_nr,
            online_cpus: u16::try_from(self.ctx.online_cpus).unwrap_or(u16::MAX),
            low_memory: None,
        };
        (self, SourceUpdate::Processes(sweep))
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
    }
}

/// In both modes: E32 swaps the low-memory sweep in through `factory`.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Processes,
        name: "processes",
        modes: &[EngineMode::Full, EngineMode::LowMemory],
        factory,
    }
}

/// A new `ProcessesSource` per generation, built on the blocking pool (it reads the desktop
/// files and icon themes), then `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    match cfg.mode {
        EngineMode::Full => full_factory(cfg, io),
        // The stripped sweep (E32): no desktop files or icons, so it is built in place.
        EngineMode::LowMemory => Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
            let io = io.clone();
            let root = cfg.sys_root.clone();
            async move { source::run(cx, io, crate::workers::lowmem::LowMemSource::new(root)).await }
        }),
    }
}

fn full_factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let cfg = cfg.clone();
        let pkg = io.shared.pkg.clone();
        async move {
            let make = move |_| {
                let src = ProcessesSource::new(cfg)?;
                Ok::<_, CollectError>(match pkg {
                    Some(index) => src.with_package_index(index),
                    None => src,
                })
            };
            let mut job = match cx.spawn_blocking(make) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(Ok(src)) => src,
                Ok(Err(e)) => return Outcome::Err(WorkerError::Collect(e)),
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
            run(cx, io, src).await
        }
    })
}

/// `source::run` for the sweep, with `HelperReads` beside it when the session has a helper and
/// the package batches beside it when the session has a package index.
pub async fn run(cx: Cx, io: WorkerIo, mut src: ProcessesSource) -> Outcome<(), WorkerError> {
    let reads = io.shared.helper.clone().map(|h| src.helper_reads(h));
    let pkg = io.shared.pkg.clone();
    if reads.is_none() && pkg.is_none() {
        return source::run(cx, io, src).await;
    }
    let demand = io.demand.clone();
    let side_cx = cx.clone();
    let sweeps = source::run(cx, io, src);
    let side = async move {
        let helper = async {
            if let Some(reads) = reads {
                reads.run(&side_cx, demand).await;
            }
        };
        let batches = async {
            if let Some(index) = pkg {
                package_batches(&side_cx, index).await;
            }
        };
        futures_lite::future::zip(helper, batches).await;
        // Only a cancel ends them; the sweep loop returns for it.
        std::future::pending().await
    };
    futures_lite::future::or(sweeps, side).await
}

// ---- Package owner batches (R-163 item 1) ----

/// How often the worker checks whether a package batch is due.
pub const PKG_CHECK: Duration = Duration::from_millis(500);

/// Runs `index.run_batch()` as a blocking job every `PKG_CHECK` while `index.due`, until `cx`
/// is cancelled. Batch errors are logged at debug and the paths are requested again.
pub async fn package_batches(cx: &Cx, index: PackageIndex) {
    loop {
        sleep_until(cx.now() + PKG_CHECK).await;
        if cx.checkpoint().is_err() {
            return;
        }
        if !index.due(Instant::now()) {
            continue;
        }
        let batch = index.clone();
        let mut job = match cx.spawn_blocking(move |_| batch.run_batch()) {
            Ok(job) => job,
            Err(e) => {
                tracing::debug!(error = %e, "pkg: batch job not started");
                continue;
            }
        };
        match job.join(cx).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::debug!(error = %e, "pkg: owner batch failed"),
            Err(JoinError::Cancelled(_)) => return,
            Err(JoinError::Panicked(_)) => tracing::warn!("pkg: owner batch panicked"),
            Err(JoinError::PolledAfterCompletion) => {}
        }
    }
}

/// The app lookup paths of the user tasklens runs as. The icon cache is
/// `$XDG_CACHE_HOME/tasklens/icons.toml`, as in `tl-app` `paths.rs`.
pub fn host_app_paths() -> AppPaths {
    let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
    let cache = absolute(std::env::var_os("XDG_CACHE_HOME")).unwrap_or_else(|| home.join(".cache"));
    AppPaths {
        xdg_data_home: std::env::var_os("XDG_DATA_HOME"),
        xdg_data_dirs: std::env::var_os("XDG_DATA_DIRS"),
        xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
        icon_cache: cache.join("tasklens").join("icons.toml"),
        home,
    }
}

fn real_clock() -> (TimeNs, u64) {
    let c = Clock::now();
    (c.boot_ns, c.mono_ns)
}

/// The value as a path when it is set, not empty and absolute.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_absolute().then_some(path)
}

/// The Wayland compositor of this session, for the protected set.
fn host_compositor_pid() -> Option<u32> {
    let runtime = absolute(std::env::var_os("XDG_RUNTIME_DIR"))?;
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    wayland_compositor_pid(&runtime, &display)
}

// ---- Helper reads (E29-017, PROC-076) ----

/// The helper reads run at most this often (PROC-076).
const HELPER_PERIOD: Duration = Duration::from_secs(1);
/// The deadline of one `ReadProcessFiles` call.
const HELPER_DEADLINE: Duration = Duration::from_secs(2);
/// The most `ReadProcessFiles` calls per period.
pub const HELPER_READS_MAX: usize = 64;
/// A helper value older than this is not applied: the stale threshold at 1 Hz (ADR-008).
const HELPER_MAX_AGE_NS: u64 = 3_000_000_000;

/// The rows of the last sweep with a Restricted cell the helper can fill, with the files that
/// fill them, sorted by key.
type Wanted = Arc<[(ProcessKey, ProcFiles)]>;

/// The helper files the shown columns need: I/O and Disk columns need `IO`, Handles `FD_LIST`,
/// Image name and Image path `EXE`, Working directory `CWD`.
pub fn helper_files(columns: &ColumnSet) -> ProcFiles {
    let mut files = ProcFiles::empty();
    for c in columns.iter() {
        files |= match c {
            c if (ColumnId::Disk..=ColumnId::IoWriteOps).contains(&c) => ProcFiles::IO,
            ColumnId::Handles => ProcFiles::FD_LIST,
            ColumnId::ImageName | ColumnId::ImagePath => ProcFiles::EXE,
            ColumnId::Cwd => ProcFiles::CWD,
            _ => ProcFiles::empty(),
        };
    }
    files
}

/// The files that would fill the Restricted cells of one swept row.
fn restricted_files(r: &ProcessRecord, x: &ProcessExtras) -> ProcFiles {
    let mut files = ProcFiles::empty();
    if r.validity.get(ProcessField::IoRead) == Validity::Restricted {
        files |= ProcFiles::IO;
    }
    // `fd/`, `exe` and `cwd` are readable only for processes the user may ptrace.
    if r.flags.contains(ProcFlags::OTHER_USER) {
        if x.handles.is_none() {
            files |= ProcFiles::FD_LIST;
        }
        if !r.flags.contains(ProcFlags::KERNEL_THREAD) {
            if r.meta.exe.is_none() {
                files |= ProcFiles::EXE;
            }
            files |= ProcFiles::CWD;
        }
    }
    files
}

/// I/O rates from two helper reads.
#[derive(Clone, Copy, Debug, Default)]
struct HelperIo {
    read_bps: f32,
    write_bps: f32,
    write_raw_bps: f32,
    /// `Valid`, or `WarmingUp` on the first read and after a reset or a suspend.
    validity: Validity,
}

/// The values of one helper reply. A file that failed is `None`.
#[derive(Clone, Debug, Default)]
struct HelperValues {
    /// Sampler clock when the reply arrived.
    t_ns: TimeNs,
    io: Option<HelperIo>,
    handles: Option<u32>,
    exe: Option<Arc<str>>,
    cwd: Option<Arc<str>>,
}

/// Written by `HelperReads`, read by the sweep.
#[derive(Default)]
struct HelperStore {
    values: FxHashMap<ProcessKey, HelperValues>,
    /// The last helper `io` counters per key, for the rates.
    io_prev: FxHashMap<ProcessKey, (Clock, PidIo)>,
}

fn lock(store: &Mutex<HelperStore>) -> std::sync::MutexGuard<'_, HelperStore> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The sweep's side of the helper reads.
struct HelperSide {
    handle: HelperHandle,
    wanted: watch::Sender<Wanted>,
    /// The last value sent on `wanted` was not empty.
    wanted_sent: bool,
    store: Arc<Mutex<HelperStore>>,
    /// Per key, the swept meta and the copy with the helper's exe, so an unchanged row keeps
    /// one `Arc`.
    metas: FxHashMap<ProcessKey, (Arc<ProcessMeta>, Arc<ProcessMeta>)>,
}

impl HelperSide {
    /// Sends the Restricted rows of this sweep to `HelperReads`, then applies the helper values
    /// to `out`. `extras_p2` gets an entry per record when a cwd value is applied. While the
    /// helper is not `Unlocked`, nothing is sent or applied and the values are dropped.
    fn after_sweep(&mut self, out: &mut SweepOutput, extras_p2: &mut Vec<ProcessExtrasP2>) {
        if self.handle.state() != HelperState::Unlocked {
            if self.wanted_sent {
                let _ = self.wanted.send(Wanted::from([]));
                self.wanted_sent = false;
                let mut store = lock(&self.store);
                store.values.clear();
                store.io_prev.clear();
                self.metas.clear();
            }
            return;
        }
        let wanted: Wanted = out
            .records
            .iter()
            .zip(&out.extras)
            .filter_map(|(r, x)| {
                let files = restricted_files(r, x);
                (!files.is_empty()).then_some((r.key, files))
            })
            .collect();
        self.wanted_sent = !wanted.is_empty();
        // `send` fails only when `HelperReads` is gone.
        let _ = self.wanted.send(wanted);

        let mut store = lock(&self.store);
        let records = &out.records;
        let live = |k: &ProcessKey| records.binary_search_by_key(k, |r| r.key).is_ok();
        store.values.retain(|k, _| live(k));
        store.io_prev.retain(|k, _| live(k));
        self.metas.retain(|k, _| live(k));
        if store.values.is_empty() {
            return;
        }
        let (now, count) = (out.observed_end, out.records.len());
        for (i, (r, x)) in out.records.iter_mut().zip(&mut out.extras).enumerate() {
            let Some(v) = store.values.get(&r.key) else {
                continue;
            };
            if now.saturating_sub(v.t_ns) > HELPER_MAX_AGE_NS {
                continue;
            }
            if let Some(io) = v.io
                && r.validity.get(ProcessField::IoRead) == Validity::Restricted
            {
                r.io_read_bps = io.read_bps;
                r.io_write_bps = io.write_bps;
                x.disk_read_bps = io.read_bps;
                x.disk_write_raw_bps = io.write_raw_bps;
                r.validity.set(ProcessField::IoRead, io.validity);
                r.validity.set(ProcessField::IoWrite, io.validity);
            }
            if x.handles.is_none() {
                x.handles = v.handles;
            }
            if let Some(exe) = &v.exe
                && r.meta.exe.is_none()
            {
                r.meta = patched_meta(&mut self.metas, r, exe);
                let path = exe.strip_suffix(" (deleted)");
                x.exe_deleted = path.is_some();
                x.exe_name = Arc::from(exe_basename(path.unwrap_or(exe)));
                x.exe_path = Arc::clone(exe);
            }
            if let Some(cwd) = &v.cwd {
                if extras_p2.is_empty() {
                    extras_p2.resize(count, ProcessExtrasP2::default());
                }
                let p2 = &mut extras_p2[i];
                p2.key = Some(r.key);
                p2.cwd = Some(Arc::clone(cwd));
            }
        }
    }
}

/// `r.meta` with `exe` set, reusing the last copy while the swept meta is the same `Arc`.
fn patched_meta(
    metas: &mut FxHashMap<ProcessKey, (Arc<ProcessMeta>, Arc<ProcessMeta>)>,
    r: &ProcessRecord,
    exe: &Arc<str>,
) -> Arc<ProcessMeta> {
    if let Some((swept, patched)) = metas.get(&r.key)
        && Arc::ptr_eq(swept, &r.meta)
        && patched.exe.as_ref() == Some(exe)
    {
        return Arc::clone(patched);
    }
    let patched = Arc::new(ProcessMeta {
        exe: Some(Arc::clone(exe)),
        ..ProcessMeta::clone(&r.meta)
    });
    metas.insert(r.key, (Arc::clone(&r.meta), Arc::clone(&patched)));
    patched
}

/// The helper side of the reads: at most once per second, after a sweep, one
/// `ReadProcessFiles` per visible Restricted row, at most `HELPER_READS_MAX` (PROC-076).
pub struct HelperReads {
    handle: HelperHandle,
    wanted: watch::Receiver<Wanted>,
    store: Arc<Mutex<HelperStore>>,
    clock: fn() -> (TimeNs, u64),
    /// The last key read: the next period starts after it, so rows past the cap wait their
    /// turn in key order.
    cursor: Option<ProcessKey>,
}

impl HelperReads {
    /// Runs until `cx` is cancelled or the sweep side is dropped.
    pub async fn run(mut self, cx: &Cx, demand: watch::Receiver<Demand>) {
        let mut last: Option<Time> = None;
        while self.wanted.changed(cx).await.is_ok() {
            if let Some(at) = last {
                let next = at + HELPER_PERIOD;
                if cx.now() < next {
                    sleep_until(next).await;
                }
            }
            if cx.checkpoint().is_err() {
                return;
            }
            if self.handle.state() != HelperState::Unlocked {
                continue;
            }
            let batch = self.pick(&demand.borrow_and_clone());
            if batch.is_empty() {
                continue;
            }
            last = Some(cx.now());
            for (key, files) in batch {
                let req = HelperRequest::ReadProcessFiles { key, files };
                // `read` never starts a session: reads run only after the user unlocked, and
                // a helper that exits meanwhile gives `NotRunning`, never a polkit prompt.
                match self.handle.read(cx, req, HELPER_DEADLINE).await {
                    Ok(replies) => self.store(key, files, &replies),
                    Err(HelperReadError::NotRunning) => break,
                    Err(HelperReadError::Call(HelperCallError::Cancelled)) => return,
                    // The cell stays Restricted.
                    Err(HelperReadError::Call(_)) => {}
                }
            }
        }
    }

    /// The visible rows of the last sweep with a Restricted shown cell, starting after
    /// `cursor` in key order, at most `HELPER_READS_MAX`.
    fn pick(&mut self, demand: &Demand) -> Vec<(ProcessKey, ProcFiles)> {
        let mask = helper_files(&demand.process_columns);
        if mask.is_empty() || demand.detail_keys.is_empty() {
            return Vec::new();
        }
        let visible: FxHashSet<ProcessKey> = demand.detail_keys.iter().copied().collect();
        let wanted = self.wanted.borrow_and_clone();
        let rows = wanted.iter().filter_map(|&(key, files)| {
            let files = files & mask;
            (visible.contains(&key) && !files.is_empty()).then_some((key, files))
        });
        let cursor = self.cursor;
        let after = |(key, _): &(ProcessKey, ProcFiles)| cursor.is_none_or(|c| *key > c);
        let mut batch: Vec<_> = rows.clone().filter(after).collect();
        batch.extend(rows.filter(|row| !after(row)));
        batch.truncate(HELPER_READS_MAX);
        self.cursor = batch.last().map(|(key, _)| *key);
        batch
    }

    /// Keeps the values of one reply for the next sweep.
    fn store(&self, key: ProcessKey, files: ProcFiles, replies: &[HelperReply]) {
        let (boot_ns, mono_ns) = (self.clock)();
        let now = Clock { boot_ns, mono_ns };
        let mut values = HelperValues {
            t_ns: boot_ns,
            ..HelperValues::default()
        };
        let mut store = lock(&self.store);
        for (file, data) in reply_files(files, replies) {
            match file {
                ProcFiles::IO => {
                    let mut io = PidIo::default();
                    if parse_io(&data, &mut io).is_ok() {
                        let prev = store.io_prev.insert(key, (now, io));
                        values.io = Some(io_rates(prev, now, io));
                    }
                }
                ProcFiles::FD_LIST => values.handles = fd_list_count(&data),
                ProcFiles::EXE => values.exe = link_text(&data),
                ProcFiles::CWD => values.cwd = link_text(&data),
                _ => {}
            }
        }
        store.values.insert(key, values);
    }
}

/// The data of each file of a `ReadProcessFiles` reply that was read. The helper sends each
/// requested file in ascending bit order as one or more `File` bodies or one `Err(Os(_))` body
/// (E29-004); any other `Err` fails the whole reply.
fn reply_files(files: ProcFiles, replies: &[HelperReply]) -> Vec<(ProcFiles, Vec<u8>)> {
    let order: SmallVec<[ProcFiles; 4]> = files.iter().collect();
    let mut next = 0;
    let mut out: Vec<(ProcFiles, Vec<u8>)> = Vec::new();
    // The file of the last `File` body, which a following chunk continues.
    let mut current = None;
    for reply in replies {
        match &reply.body {
            HelperReplyBody::File { file, data } if current == Some(*file) => {
                if let Some((_, buf)) = out.last_mut() {
                    buf.extend_from_slice(data);
                }
            }
            HelperReplyBody::File { file, data } => {
                let rest = order.get(next..).unwrap_or_default();
                let Some(at) = rest.iter().position(|f| f == file) else {
                    return Vec::new();
                };
                next += at + 1;
                current = Some(*file);
                out.push((*file, data.clone()));
            }
            HelperReplyBody::Err(HelperError::Os(_)) => {
                next += 1;
                current = None;
            }
            HelperReplyBody::Err(_) => return Vec::new(),
            _ => {}
        }
    }
    out
}

/// The number of entries of an `FD_LIST` body: postcard `Vec<(u32, Vec<u8>)>` (E29-004).
fn fd_list_count(data: &[u8]) -> Option<u32> {
    let fds = postcard::from_bytes::<Vec<(u32, &[u8])>>(data).ok()?;
    Some(u32::try_from(fds.len()).unwrap_or(u32::MAX))
}

/// Sanitized link text, `None` when empty.
fn link_text(data: &[u8]) -> Option<Arc<str>> {
    let mut text = String::new();
    sanitize_link(data, &mut text);
    (!text.is_empty()).then(|| Arc::from(text))
}

/// The ADR-008 counter rules for helper `io` reads: no rate on the first read, when time did
/// not move forward, across a suspend or after a counter went down.
fn io_rates(prev: Option<(Clock, PidIo)>, now: Clock, cur: PidIo) -> HelperIo {
    let Some((then, old)) = prev else {
        return HelperIo::default();
    };
    let reset = cur.read_bytes < old.read_bytes
        || cur.write_bytes < old.write_bytes
        || cur.cancelled_write_bytes < old.cancelled_write_bytes;
    if now.boot_ns <= then.boot_ns || resumed_between(then, now) || reset {
        return HelperIo::default();
    }
    let dt_s = (now.boot_ns - then.boot_ns) as f64 / 1e9;
    let d_read = cur.read_bytes - old.read_bytes;
    let d_write = cur.write_bytes - old.write_bytes;
    let d_cancel = cur.cancelled_write_bytes - old.cancelled_write_bytes;
    HelperIo {
        read_bps: (d_read as f64 / dt_s) as f32,
        write_bps: (d_write.saturating_sub(d_cancel) as f64 / dt_s) as f32,
        write_raw_bps: (d_write as f64 / dt_s) as f32,
        validity: Validity::Valid,
    }
}

#[cfg(test)]
mod tests {
    use tl_model::ids::{GraphTarget, ProcessKey};

    use super::*;

    fn settings(interval_ms: u32) -> EngineSettings {
        EngineSettings {
            interval_ms,
            ..EngineSettings::default()
        }
    }

    #[test]
    fn period_table() {
        let s = settings(2000);
        assert_eq!(processes_period(&Demand::default(), &s), None);

        let table = Demand {
            process_table: true,
            ..Demand::default()
        };
        assert_eq!(processes_period(&table, &s), Some(Duration::from_secs(2)));
        assert_eq!(
            processes_period(&table, &settings(100)),
            Some(Duration::from_millis(250))
        );

        let details = Demand {
            detail_keys: Arc::from(vec![ProcessKey {
                boot_id: BootId(1),
                pid: 1,
                start_ticks: 1,
            }]),
            ..Demand::default()
        };
        assert_eq!(processes_period(&details, &s), Some(Duration::from_secs(2)));
    }

    #[test]
    fn wants_follow_columns_and_recording() {
        let s = settings(1000);
        assert_eq!(sweep_wants(&Demand::default(), &s), SweepWants::default());

        let mut d = Demand::default();
        d.process_columns.insert(ColumnId::Handles);
        d.process_columns.insert(ColumnId::WsPeak);
        d.process_columns.insert(ColumnId::DiskReadRate);
        let w = sweep_wants(&d, &s);
        assert!(w.fd_count && w.status_fields && w.io, "{w:?}");

        let mut d = Demand::default();
        d.process_columns.insert(ColumnId::Memory);
        d.process_columns.insert(ColumnId::Cpu);
        assert_eq!(sweep_wants(&d, &s), SweepWants::default());

        let affinity = Demand {
            affinity_targets: Arc::from(vec![ProcessKey {
                boot_id: BootId(1),
                pid: 1,
                start_ticks: 1,
            }]),
            ..Demand::default()
        };
        assert!(sweep_wants(&affinity, &s).status_fields);

        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        assert!(sweep_wants(&recording, &s).io);
        let mut history = settings(1000);
        history.history.enabled = true;
        assert!(sweep_wants(&Demand::default(), &history).io);
    }

    #[test]
    fn period_graph_targets_without_table() {
        let d = Demand {
            graph_targets: Arc::from(vec![(GraphTarget::Name("firefox".into()), ColumnId::Cpu)]),
            ..Demand::default()
        };
        assert!(!d.process_table);
        assert_eq!(
            processes_period(&d, &settings(1000)),
            Some(Duration::from_secs(1))
        );
    }

    fn reply(body: HelperReplyBody) -> HelperReply {
        HelperReply {
            seq: 1,
            last: false,
            body,
        }
    }

    fn file(file: ProcFiles, data: &[u8]) -> HelperReply {
        reply(HelperReplyBody::File {
            file,
            data: data.to_vec(),
        })
    }

    #[test]
    fn reply_files_follow_bit_order() {
        let files = ProcFiles::IO | ProcFiles::FD_LIST | ProcFiles::EXE;
        let replies = [
            file(ProcFiles::IO, b"ab"),
            file(ProcFiles::IO, b"cd"),
            reply(HelperReplyBody::Err(HelperError::Os(13))),
            file(ProcFiles::EXE, b"/usr/bin/x"),
        ];
        assert_eq!(
            reply_files(files, &replies),
            [
                (ProcFiles::IO, b"abcd".to_vec()),
                (ProcFiles::EXE, b"/usr/bin/x".to_vec())
            ]
        );
        // An `Err` before any `File` is the first file's.
        let replies = [
            reply(HelperReplyBody::Err(HelperError::Os(13))),
            file(ProcFiles::FD_LIST, b"z"),
        ];
        let files = ProcFiles::IO | ProcFiles::FD_LIST;
        assert_eq!(
            reply_files(files, &replies),
            [(ProcFiles::FD_LIST, b"z".to_vec())]
        );
        // The process exited during the read: nothing is used.
        let gone = [reply(HelperReplyBody::Err(HelperError::NoSuchProcess))];
        assert!(reply_files(ProcFiles::IO, &gone).is_empty());
    }

    #[test]
    fn fd_list_counts_entries() {
        let list: Vec<(u32, Vec<u8>)> = vec![(0, b"/dev/null".to_vec()), (3, b"pipe:[7]".to_vec())];
        let data = postcard::to_allocvec(&list).expect("encode");
        assert_eq!(fd_list_count(&data), Some(2));
        assert_eq!(fd_list_count(&[0xff]), None);
    }

    #[test]
    fn helper_io_rates_follow_counter_rules() {
        let at = |s: u64| Clock {
            boot_ns: s * 1_000_000_000,
            mono_ns: s * 1_000_000_000,
        };
        let io = |read: u64| PidIo {
            read_bytes: read,
            write_bytes: read,
            ..PidIo::default()
        };
        assert_eq!(io_rates(None, at(1), io(10)).validity, Validity::WarmingUp);
        let rate = io_rates(Some((at(1), io(10))), at(3), io(4_010));
        assert_eq!(rate.validity, Validity::Valid);
        assert_eq!(rate.read_bps, 2_000.0);
        // A counter went down: no rate.
        let reset = io_rates(Some((at(1), io(10))), at(2), io(5));
        assert_eq!(reset.validity, Validity::WarmingUp);
        // The machine slept in between.
        let slept = Clock {
            boot_ns: 60_000_000_000,
            mono_ns: 2_000_000_000,
        };
        let suspend = io_rates(Some((at(1), io(10))), slept, io(20));
        assert_eq!(suspend.validity, Validity::WarmingUp);
    }

    #[test]
    fn helper_files_from_columns() {
        let mut columns = ColumnSet::EMPTY;
        assert!(helper_files(&columns).is_empty());
        columns.insert(ColumnId::Disk);
        columns.insert(ColumnId::Handles);
        columns.insert(ColumnId::ImagePath);
        columns.insert(ColumnId::Cwd);
        columns.insert(ColumnId::Cpu);
        let want = ProcFiles::IO | ProcFiles::FD_LIST | ProcFiles::EXE | ProcFiles::CWD;
        assert_eq!(helper_files(&columns), want);
    }

    // ---- Package owner batches ----

    use std::sync::atomic::{AtomicUsize, Ordering};

    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_collect::SysRoot;
    use tl_collect::pkg::{PathOwners, PkgBackend, VerifyOutcome};
    use tl_model::process::PackageInfo;

    /// Owns `owned` as package `firefox 131.0-1`; counts `owners` calls.
    struct FakePkg {
        owned: PathBuf,
        calls: Arc<AtomicUsize>,
    }

    impl PkgBackend for FakePkg {
        fn owners(&mut self, paths: &[&Path]) -> Result<PathOwners, CollectError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(paths
                .iter()
                .map(|p| {
                    (
                        p.to_path_buf(),
                        (*p == self.owned).then(|| Arc::from("firefox")),
                    )
                })
                .collect())
        }

        fn info(
            &mut self,
            name: &str,
        ) -> Result<Option<tl_model::process::PackageInfo>, CollectError> {
            Ok(Some(PackageInfo {
                name: Arc::from(name),
                version: Arc::from("131.0-1"),
                description: None,
                packager: Some(Arc::from("Jan Doe")),
                build_date_s: None,
                url: None,
            }))
        }

        fn verify(&mut self, _name: &str) -> Result<VerifyOutcome, CollectError> {
            Ok(VerifyOutcome::Unmodified)
        }

        fn db_stamp(&mut self) -> Option<i64> {
            Some(1)
        }
    }

    fn fake_index(owned: &Path) -> (PackageIndex, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = FakePkg {
            owned: owned.to_path_buf(),
            calls: Arc::clone(&calls),
        };
        (PackageIndex::new(Box::new(backend)), calls)
    }

    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        lab.step_for_test();
        lab.run_until_idle();
    }

    /// The batch loop runs a due batch as its own job at the next check, and nothing while no
    /// path waits.
    #[test]
    fn batch_loop_runs_due_batches() {
        let (index, calls) = fake_index(Path::new("/usr/bin/firefox"));
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let looped = index.clone();
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                package_batches(&cx, looped).await;
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        advance(&mut lab, PKG_CHECK);
        assert_eq!(calls.load(Ordering::Relaxed), 0, "nothing pending");

        // A request older than the 2 s debounce is due at the next check.
        let past = Instant::now()
            .checked_sub(Duration::from_secs(3))
            .expect("monotonic clock is past 3 s");
        index.request("/usr/bin/firefox", 100, past);
        index.request("/home/u/a.out", 5, past);
        advance(&mut lab, PKG_CHECK);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let info = index.get("/usr/bin/firefox", 100).expect("looked up");
        assert_eq!(info.as_deref().map(|i| &*i.name), Some("firefox"));
        assert_eq!(index.get("/home/u/a.out", 5), Some(None));

        // A new request waits out the debounce.
        index.request("/usr/bin/other", 1, Instant::now());
        advance(&mut lab, PKG_CHECK);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    /// Copies `src` into `dst`, symlinks included.
    fn copy_tree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).expect("create dir");
        for entry in std::fs::read_dir(src).expect("read dir") {
            let entry = entry.expect("dir entry");
            let to = dst.join(entry.file_name());
            let kind = entry.file_type().expect("file type");
            if kind.is_symlink() {
                let target = std::fs::read_link(entry.path()).expect("read link");
                std::os::unix::fs::symlink(target, &to).expect("create link");
            } else if kind.is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), &to).expect("copy file");
            }
        }
    }

    /// `sample` reads the shared index for the demand's detail keys: the first sweep queues the
    /// exe, and after the batch `ProcessSweep.extras_p2` holds the owner.
    #[test]
    fn sweep_extras_package_from_shared_index() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/process-edge");
        copy_tree(&fixture, base);
        std::fs::create_dir_all(base.join("proc/sys/kernel/random")).expect("create random dir");
        std::fs::write(
            base.join("proc/sys/kernel/random/boot_id"),
            "8a8a3a3e-7d55-4b6e-9a43-1f0b0c3a2d11\n",
        )
        .expect("write boot_id");
        let exe = base.join("firefox");
        std::fs::write(&exe, b"#!/bin/sh\n").expect("write exe");
        let link = base.join("proc/4201/exe");
        std::fs::remove_file(&link).expect("remove exe link");
        std::os::unix::fs::symlink(&exe, &link).expect("create exe link");

        let cfg = Arc::new(
            EngineConfig::new(EngineMode::Full, base.join("state"))
                .with_sys_root(SysRoot::at(base)),
        );
        let paths = AppPaths {
            home: base.join("home"),
            xdg_data_home: None,
            xdg_data_dirs: None,
            xdg_config_home: None,
            icon_cache: base.join("cache/icons.toml"),
        };
        let users = PasswdUserNames::load(&base.join("etc/passwd"));
        let (index, calls) = fake_index(&exe);
        let src = ProcessesSource::with_parts(cfg, &paths, Box::new(users))
            .expect("fixture source opens")
            .with_package_index(index.clone());
        assert!(src.sampler.package_index().same_index(&index));

        // The production path: `period` takes the visible rows from the demand, `sample`
        // sweeps and builds the `ProcessSweep`.
        let sample = |src: ProcessesSource, keys: &[ProcessKey]| {
            let demand = Demand {
                process_table: true,
                detail_keys: Arc::from(keys),
                ..Demand::default()
            };
            assert!(src.period(&demand, &settings(1000)).is_some());
            match src.sample() {
                (src, SourceUpdate::Processes(sweep)) => (src, sweep),
                _ => panic!("sample gives a sweep"),
            }
        };
        let (src, sweep) = sample(src, &[]);
        assert!(
            sweep.extras_p2.is_empty(),
            "no P2 reads without detail keys"
        );
        let key = sweep
            .records
            .iter()
            .find(|r| r.key.pid == 4201)
            .expect("4201 swept")
            .key;

        let (src, sweep) = sample(src, &[key]);
        assert_eq!(sweep.extras_p2.len(), sweep.records.len());
        let at = sweep
            .records
            .iter()
            .position(|r| r.key == key)
            .expect("4201");
        assert_eq!(sweep.extras_p2[at].key, Some(key));
        assert_eq!(sweep.extras_p2[at].package, None, "not looked up yet");
        assert!(index.due(Instant::now() + Duration::from_secs(3)));

        assert_eq!(index.run_batch().expect("batch"), 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let (_src, sweep) = sample(src, &[key]);
        let at = sweep
            .records
            .iter()
            .position(|r| r.key == key)
            .expect("4201");
        let p2 = &sweep.extras_p2[at];
        assert_eq!(p2.package.as_deref().map(|i| &*i.name), Some("firefox"));
        assert_eq!(
            p2.package_status,
            Some(tl_model::process::PackageStatus::Packaged)
        );
    }
}
