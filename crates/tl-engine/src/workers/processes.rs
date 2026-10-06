//! The `Processes` worker (ADR-004, ADR-008, ADR-016): wraps the tl-collect `ProcessSampler` and
//! turns each sweep into a `SourceUpdate::Processes(ProcessSweep)`.
//!
//! One sweep is one blocking job, at `processes.interval_ms` (250 to 10,000 ms) while the table,
//! a process detail, a process graph or recording needs it, and not at all otherwise
//! (PROC-002, PROC-004, PROC-093). Records hold command lines and paths: this module logs only
//! the record count (ADR-015).

use std::cell::Cell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use smallvec::{SmallVec, smallvec};
use tl_collect::CollectError;
use tl_collect::apps::AppPaths;
use tl_collect::apps::classify::{ProtectInputs, wayland_compositor_pid};
use tl_collect::parse::filenr::parse_file_nr;
use tl_collect::parse::topology::parse_cpu_list;
use tl_collect::proc::meta::{PasswdUserNames, UserNames};
use tl_collect::proc::sweep::{ProcessSampler, SweepCtx, SweepInputs, SweepOutput};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::ColumnId;
use tl_model::settings::EngineSettings;
use tl_model::time::{BootId, now_boot_ns};

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{ProcessSweep, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

/// `processes.interval_ms` is clamped to this range (ADR-008).
pub const MIN_INTERVAL_MS: u32 = 250;
pub const MAX_INTERVAL_MS: u32 = 10_000;

/// The process rate table (ADR-008 row "Process table"): on while any process consumer is
/// visible or recording is on. Graph targets keep it on with the table hidden (R-154 item 2).
pub fn processes_period(d: &Demand, s: &EngineSettings) -> Option<Duration> {
    let wanted = d.process_table
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
        })
    }

    /// The sampler with a test clock (CLOCK_BOOTTIME ns, CLOCK_MONOTONIC ns).
    pub fn with_clock(mut self, clock: fn() -> (tl_model::time::TimeNs, u64)) -> Self {
        self.sampler = self.sampler.with_clock(clock);
        self
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
        processes_period(d, s)
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        self.ctx.online_cpus = self.read_online_cpus();
        let wants = self.wants.get();
        self.ctx.want_fd_count = wants.fd_count;
        self.ctx.want_status_fields = wants.status_fields;
        self.ctx.want_io = wants.io;
        let inputs = SweepInputs {
            protect: &self.protect,
            // PSS and USS for detail keys come from the ProcessDetail worker (E10-A2).
            detail_keys: &[],
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
            // Filled for detail keys by the P2 readers (R-163 item 2).
            extras_p2: Vec::new(),
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
        async move {
            let mut job = match cx.spawn_blocking(move |_| ProcessesSource::new(cfg)) {
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
            source::run(cx, io, src).await
        }
    })
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
}
