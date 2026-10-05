//! One sweep over all processes in /proc (PROC-002, PROC-029, PROC-030, PROC-031, ADR-008,
//! ADR-016).
//!
//! Each pid directory is opened once as an `O_DIRECTORY` fd, its files are read relative to it,
//! and it is closed again. Rates follow the ADR-008 counter rules: no rate on the first sample,
//! after a counter reset or across a suspend. Nothing here logs above `trace` per process.
//!
//! `ProcessSampler` (E05-A1) adds metadata, app groups, flags, the P1 extra columns and the app
//! unit samples on top of `SweepCore`. Command lines, paths and user names are never logged
//! (ADR-015).

use std::mem::MaybeUninit;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags, RawDir};
use smallvec::SmallVec;
use tl_model::actions::{CgroupRef, SchedPolicy};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{
    AppCgroupSample, Coverage, GroupKind, ProcFlags, ProcState, ProcessExtras, ProcessMeta,
    ProcessRecord,
};
use tl_model::system::CpuSet;
use tl_model::table::{Manager, UnitRef};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::{FieldValidity, ProcessField, Validity};

use crate::CollectError;
use crate::apps::classify::{ProcClass, ProtectInputs, flags_for, protected_set};
use crate::apps::naming::{
    NameInput, display_name, exe_basename, image_name, is_wine, search_lower,
};
use crate::apps::{AppInfo, AppPaths, AppResolver, ResolveInput, is_app_primary};
use crate::counters::{Clock, resumed_between};
use crate::parse::topology::parse_cpu_list;
use crate::proc::cgroup::{
    CgroupPath, CgroupRoot, CgroupStats, CgroupWant, UserPlacement, placement,
};
use crate::proc::fdcount::{FdCount, fd_count};
use crate::proc::io::{IoRead, PidIo, read_io};
use crate::proc::meta::{ExeLink, Interner, MetaCache, MetaChange, RawMeta, ReadBufs, UserNames};
use crate::proc::pid_stat::{PF_KTHREAD, PidStat, parse_pid_stat};
use crate::proc::smaps::{CoverageBuilder, PssReader, RollupRead, apply_rollups};
use crate::proc::statm::{PidStatm, parse_statm};
use crate::proc::status::{
    PidStatus, STATUS_READ_MAX, commit_bytes, parse_status, ws_shared_bytes,
};
use crate::sysroot::{SMALL_FILE_LIMIT, SysRoot, read_at};

/// getdents buffer for the `<proc>` directory listing.
const DIRENT_BUF: usize = 32 * 1024;

/// What the engine passes to every sweep.
#[derive(Clone, Debug)]
pub struct SweepCtx {
    pub boot_id: BootId,
    /// `rustix::process::getuid()` of tasklens.
    pub self_uid: u32,
    /// `rustix::param::clock_ticks_per_second()`.
    pub ticks_per_sec: u64,
    /// From the CPU topology (E04-A2).
    pub online_cpus: u32,
    /// Read the open fd count of every process.
    pub want_fd_count: bool,
}

/// The counters of one process at its last sample.
#[derive(Clone, Copy, Debug, Default)]
struct Prev {
    t_ns: TimeNs,
    mono_ns: u64,
    utime: u64,
    stime: u64,
    minflt: u64,
    majflt: u64,
    io: Option<PidIo>,
}

/// The `io` file of one process.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IoState {
    Valid(PidIo),
    /// EACCES or EPERM: another user's process. Shown as a lock, never as zero.
    Restricted,
    /// No counters: the file went away or could not be read or parsed.
    WarmingUp,
}

/// One process from one sweep, with rates against the previous sweep.
#[derive(Clone, Debug)]
pub struct SweptProc {
    pub key: ProcessKey,
    /// CLOCK_BOOTTIME right before the first read of this process.
    pub t_ns: TimeNs,
    pub stat: PidStat,
    pub status: PidStatus,
    pub statm: PidStatm,
    pub io: IoState,
    /// `None` when the sweep did not ask for fd counts.
    pub fd_count: Option<FdCount>,
    /// Share of all online CPUs, 0 to 100.
    pub cpu_pct: f32,
    pub cpu_kernel_pct: f32,
    pub cpu_user_pct: f32,
    /// One core = 100% ("CPU (one core = 100%)").
    pub cpu_relative_pct: f32,
    pub io_read_bps: f32,
    /// Written minus cancelled bytes per second (PROC-031).
    pub io_write_bps: f32,
    pub disk_read_bps: f32,
    /// Written bytes per second without the cancelled bytes (Disk write rate column).
    pub disk_write_raw_bps: f32,
    pub page_faults_delta: u64,
    pub validity: FieldValidity,
}

/// Per-process state kept between sweeps.
pub struct SweepCore {
    root: SysRoot,
    prev: FxHashMap<ProcessKey, Prev>,
    /// Filled during a sweep, then swapped with `prev`, so unseen keys drop out.
    next: FxHashMap<ProcessKey, Prev>,
    bufs: ReadBufs,
    dirents: Vec<MaybeUninit<u8>>,
    stat: PidStat,
    status: PidStatus,
    clock: fn() -> (TimeNs, u64),
}

impl SweepCore {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            prev: FxHashMap::default(),
            next: FxHashMap::default(),
            bufs: ReadBufs::default(),
            dirents: vec![MaybeUninit::uninit(); DIRENT_BUF],
            stat: PidStat::default(),
            status: PidStatus::default(),
            clock: real_clock,
        }
    }

    /// Test hook: `clock` returns (CLOCK_BOOTTIME ns, CLOCK_MONOTONIC ns) in place of the real
    /// clocks.
    pub fn with_clock(mut self, clock: fn() -> (TimeNs, u64)) -> Self {
        self.clock = clock;
        self
    }

    /// Forgets every baseline, so the next sweep gives no rates.
    pub fn reset_baselines(&mut self) {
        self.prev.clear();
    }

    /// Test hook: true when `key` has a baseline for the next sweep.
    #[doc(hidden)]
    pub fn has_baseline(&self, key: &ProcessKey) -> bool {
        self.prev.contains_key(key)
    }

    /// Reads every process into `out` (cleared first) and returns (observed_start,
    /// observed_end). A pid that goes away during the sweep is skipped. A missing `<proc>`
    /// directory gives an empty result.
    pub fn sweep_core(&mut self, ctx: &SweepCtx, out: &mut Vec<SweptProc>) -> (TimeNs, TimeNs) {
        self.sweep_core_with(ctx, out, &mut |_, _| {})
    }

    /// `sweep_core` that also calls `per_pid` for each process with its `/proc/<pid>` dirfd,
    /// before the dirfd is closed, so metadata is read from the same process.
    pub fn sweep_core_with(
        &mut self,
        ctx: &SweepCtx,
        out: &mut Vec<SweptProc>,
        per_pid: &mut dyn FnMut(&SweptProc, BorrowedFd<'_>),
    ) -> (TimeNs, TimeNs) {
        out.clear();
        self.next.clear();
        let observed_start = (self.clock)().0;

        if let Ok(proc_fd) = rustix::fs::open(
            &self.root.proc,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            let mut dir = RawDir::new(proc_fd.as_fd(), &mut self.dirents[..]);
            while let Some(entry) = dir.next() {
                let Ok(entry) = entry else { break };
                let name = entry.file_name();
                let Some(pid) = parse_pid(name.to_bytes()) else {
                    continue;
                };
                let Ok(pid_fd) = rustix::fs::openat(
                    proc_fd.as_fd(),
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                ) else {
                    tracing::trace!(pid, "pid directory not opened");
                    continue;
                };
                if let Some(p) = sweep_one(
                    ctx,
                    pid,
                    &pid_fd,
                    self.clock,
                    &self.prev,
                    &mut self.next,
                    &mut self.bufs,
                    &mut self.stat,
                    &mut self.status,
                ) {
                    per_pid(&p, pid_fd.as_fd());
                    out.push(p);
                }
            }
        }

        std::mem::swap(&mut self.prev, &mut self.next);
        self.next.clear();
        let observed_end = (self.clock)().0;
        (observed_start, observed_end)
    }
}

impl std::fmt::Debug for SweepCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SweepCore")
            .field("baselines", &self.prev.len())
            .finish_non_exhaustive()
    }
}

fn real_clock() -> (TimeNs, u64) {
    let c = Clock::now();
    (c.boot_ns, c.mono_ns)
}

/// A directory name made of ASCII digits only that fits a `u32`.
fn parse_pid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    name.iter().try_fold(0u32, |n, &d| {
        n.checked_mul(10)?.checked_add(u32::from(d - b'0'))
    })
}

/// Reads one process and computes its rates. `None` when the process is gone or its `stat`
/// cannot be read, since then it has no key.
#[allow(clippy::too_many_arguments, reason = "split borrows of SweepCore")]
fn sweep_one(
    ctx: &SweepCtx,
    pid: u32,
    dir: &OwnedFd,
    clock: fn() -> (TimeNs, u64),
    prev: &FxHashMap<ProcessKey, Prev>,
    next: &mut FxHashMap<ProcessKey, Prev>,
    bufs: &mut ReadBufs,
    stat: &mut PidStat,
    status: &mut PidStatus,
) -> Option<SweptProc> {
    let dir = dir.as_fd();
    let (t_ns, mono_ns) = clock();

    let stat_read = read_at(dir, "stat", &mut bufs.small, SMALL_FILE_LIMIT)
        .and_then(|_| parse_pid_stat(&bufs.small, stat));
    if let Err(e) = stat_read {
        tracing::trace!(pid, error = %e, "stat not read, process skipped");
        return None;
    }

    let mut validity = FieldValidity::default();

    let mem_validity = match read_at(dir, "status", &mut bufs.small, STATUS_READ_MAX) {
        Ok(_) => match parse_status(&bufs.small, status) {
            Ok(()) => None,
            Err(e) => Some(e.validity()),
        },
        Err(e) => Some(e.validity()),
    };
    if mem_validity.is_some() {
        *status = PidStatus::default();
    }
    let present = |v: Option<u64>| match (mem_validity, v) {
        (Some(failed), _) => failed,
        (None, Some(_)) => Validity::Valid,
        (None, None) => Validity::Unsupported,
    };
    validity.set(ProcessField::RssAnon, present(status.rss_anon));
    validity.set(ProcessField::Rss, present(status.vm_rss));
    validity.set(ProcessField::Swap, present(status.vm_swap));
    validity.set(ProcessField::Threads, Validity::Valid);

    let mut statm = PidStatm::default();
    if read_at(dir, "statm", &mut bufs.small, SMALL_FILE_LIMIT).is_err()
        || parse_statm(&bufs.small, &mut statm).is_err()
    {
        statm = PidStatm::default();
    }

    let (io, io_validity) = match read_io(dir, &mut bufs.small) {
        Ok(IoRead::Ok(io)) => (IoState::Valid(io), None),
        Ok(IoRead::Restricted) => (IoState::Restricted, Some(Validity::Restricted)),
        Ok(IoRead::Gone) => (IoState::WarmingUp, Some(Validity::Missing)),
        Err(e) => (IoState::WarmingUp, Some(e.validity())),
    };

    let fd_count = ctx.want_fd_count.then(|| fd_count(dir));

    let key = ProcessKey {
        start_ticks: stat.start_ticks,
        boot_id: ctx.boot_id,
        pid,
    };
    let cur = Prev {
        t_ns,
        mono_ns,
        utime: stat.utime,
        stime: stat.stime,
        minflt: stat.minflt,
        majflt: stat.majflt,
        io: match io {
            IoState::Valid(io) => Some(io),
            IoState::Restricted | IoState::WarmingUp => None,
        },
    };
    next.insert(key, cur);

    let mut p = SweptProc {
        key,
        t_ns,
        stat: stat.clone(),
        status: status.clone(),
        statm,
        io,
        fd_count,
        cpu_pct: 0.0,
        cpu_kernel_pct: 0.0,
        cpu_user_pct: 0.0,
        cpu_relative_pct: 0.0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        disk_read_bps: 0.0,
        disk_write_raw_bps: 0.0,
        page_faults_delta: 0,
        validity,
    };

    let io_rates = match prev.get(&key).and_then(|old| rate_base(old, &cur)) {
        Some((old, dt_s)) => {
            apply_rates(ctx, old, &cur, dt_s, &mut p);
            p.validity.set(ProcessField::Cpu, Validity::Valid);
            old.io.is_some() && cur.io.is_some()
        }
        None => false,
    };
    let io_v = io_validity.unwrap_or(if io_rates {
        Validity::Valid
    } else {
        Validity::WarmingUp
    });
    p.validity.set(ProcessField::IoRead, io_v);
    p.validity.set(ProcessField::IoWrite, io_v);
    Some(p)
}

/// The previous sample and the elapsed seconds, or `None` when ADR-008 gives no rate: time did
/// not move forward, the machine slept in between, or any counter went down.
fn rate_base<'a>(old: &'a Prev, cur: &Prev) -> Option<(&'a Prev, f64)> {
    if cur.t_ns <= old.t_ns {
        return None;
    }
    let was = Clock {
        boot_ns: old.t_ns,
        mono_ns: old.mono_ns,
    };
    let now = Clock {
        boot_ns: cur.t_ns,
        mono_ns: cur.mono_ns,
    };
    if resumed_between(was, now) {
        return None;
    }
    let mut reset = cur.utime < old.utime
        || cur.stime < old.stime
        || cur.minflt < old.minflt
        || cur.majflt < old.majflt;
    if let (Some(a), Some(b)) = (old.io, cur.io) {
        reset |= b.read_bytes < a.read_bytes
            || b.write_bytes < a.write_bytes
            || b.cancelled_write_bytes < a.cancelled_write_bytes;
    }
    if reset {
        return None;
    }
    Some((old, (cur.t_ns - old.t_ns) as f64 / 1e9))
}

/// Fills the rate fields of `p`. Counters did not go down (checked by `rate_base`).
fn apply_rates(ctx: &SweepCtx, old: &Prev, cur: &Prev, dt_s: f64, p: &mut SweptProc) {
    let ticks = ctx.ticks_per_sec.max(1) as f64;
    let cpus = f64::from(ctx.online_cpus.max(1));
    let d_user = (cur.utime - old.utime) as f64;
    let d_sys = (cur.stime - old.stime) as f64;
    let pct = |d: f64| 100.0 * d / ticks / dt_s / cpus;
    p.cpu_pct = pct(d_user + d_sys) as f32;
    p.cpu_user_pct = pct(d_user) as f32;
    p.cpu_kernel_pct = pct(d_sys) as f32;
    p.cpu_relative_pct = (pct(d_user + d_sys) * cpus) as f32;
    p.page_faults_delta = (cur.minflt - old.minflt) + (cur.majflt - old.majflt);

    if let (Some(a), Some(b)) = (old.io, cur.io) {
        let d_read = b.read_bytes - a.read_bytes;
        let d_write = b.write_bytes - a.write_bytes;
        let d_cancel = b.cancelled_write_bytes - a.cancelled_write_bytes;
        p.io_read_bps = (d_read as f64 / dt_s) as f32;
        p.io_write_bps = (d_write.saturating_sub(d_cancel) as f64 / dt_s) as f32;
        p.disk_read_bps = p.io_read_bps;
        p.disk_write_raw_bps = (d_write as f64 / dt_s) as f32;
    }
}

// ---- Process sampler (E05-A1) ----

/// The tl-collect-only values of one record, at the same index as `SweepOutput.records`. The
/// engine's grouping reads them; they never go into tl-model.
#[derive(Clone, Copy, Debug)]
pub struct SweepSide {
    pub class: ProcClass,
    pub is_app_primary: bool,
}

/// One app unit from this sweep's app.slice scan, with its cgroup stats.
#[derive(Clone, Debug)]
pub struct AppUnitSample {
    pub group_key: GroupKey,
    /// The cgroup path relative to `/sys/fs/cgroup`.
    pub rel: Arc<str>,
    pub cgroup_id: u64,
    pub app_name: Option<Arc<str>>,
    /// The app icon at 24 px.
    pub icon_app: Option<Arc<str>>,
    pub qualifier: Option<&'static str>,
    pub stats: CgroupStats,
    /// Δ`rbytes` / Δt from `io.stat`. Used when a member's Disk is Restricted (R-103).
    pub io_read_bps: f32,
    /// Δ`wbytes` / Δt from `io.stat`.
    pub io_write_bps: f32,
    pub io_validity: Validity,
}

impl AppUnitSample {
    /// The tl-model value (R-141 item 2). A missing `anon` or swap value is 0 with its own
    /// validity, never a valid zero.
    pub fn to_model(&self) -> AppCgroupSample {
        let mut validity = FieldValidity::default();
        validity.set(ProcessField::IoRead, self.io_validity);
        validity.set(ProcessField::IoWrite, self.io_validity);
        let present = |v: Option<u64>| match v {
            Some(_) => Validity::Valid,
            None if self.stats.denied => Validity::Restricted,
            None => Validity::Unsupported,
        };
        validity.set(ProcessField::RssAnon, present(self.stats.anon));
        validity.set(ProcessField::Swap, present(self.stats.swap_current));
        AppCgroupSample {
            group: self.group_key,
            anon_bytes: self.stats.anon.unwrap_or(0),
            swap_bytes: self.stats.swap_current.unwrap_or(0),
            io_read_bps: self.io_read_bps,
            io_write_bps: self.io_write_bps,
            rios: self.stats.rios,
            wios: self.stats.wios,
            pids: self
                .stats
                .pids_current
                .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            cgroup: CgroupRef {
                rel_path: self.rel.clone(),
                cgroup_id: self.cgroup_id,
            },
            validity,
        }
    }
}

/// The result of one `ProcessSampler::sweep`. `records`, `extras` and `side` have the same
/// length and order (sorted by `ProcessKey`). Reuse one value across sweeps: the vectors keep
/// their capacity.
#[derive(Default)]
pub struct SweepOutput {
    pub observed_start: TimeNs,
    pub observed_end: TimeNs,
    pub records: Vec<ProcessRecord>,
    pub extras: Vec<ProcessExtras>,
    pub side: Vec<SweepSide>,
    pub apps: Vec<AppUnitSample>,
    pub coverage: Coverage,
}

/// What the engine passes to `ProcessSampler::sweep` besides `SweepCtx`.
pub struct SweepInputs<'a> {
    pub protect: &'a ProtectInputs,
    /// Keys whose PSS and USS are read when `want_pss` is set.
    pub detail_keys: &'a [ProcessKey],
    pub want_pss: bool,
    /// Pids that own a KWin window (P2). `None` in P1.
    pub window_pids: Option<&'a FxHashSet<u32>>,
    /// KWin `resourceClass` per pid (P2). `None` in P1.
    pub wm_class: Option<&'a FxHashMap<u32, Arc<str>>>,
}

/// The `ProcessMeta` of one key and what it was built from, so an unchanged process keeps the
/// same `Arc` (pointer equal) from sweep to sweep.
struct MetaSlot {
    meta: Arc<ProcessMeta>,
    app: Arc<AppInfo>,
    primary: bool,
    cgroup: Option<CgroupPath>,
    exe_name: Arc<str>,
    exe_path: Arc<str>,
    exe_deleted: bool,
    unit: Option<UnitRef>,
}

/// The `io.stat` counters of one app unit at its last sample.
#[derive(Clone, Copy)]
struct AppPrev {
    t_ns: TimeNs,
    mono_ns: u64,
    rbytes: u64,
    wbytes: u64,
}

/// Buffers reused across sweeps.
#[derive(Default)]
struct Scratch {
    swept: Vec<SweptProc>,
    name: String,
    search: String,
    cpus: SmallVec<[u16; 64]>,
    app_pids: FxHashSet<u32>,
    pid_index: FxHashMap<u32, usize>,
    live: FxHashSet<ProcessKey>,
    sorted: Vec<(ProcessRecord, ProcessExtras, SweepSide)>,
    rollups: Vec<(ProcessKey, RollupRead)>,
    rollup_buf: Vec<u8>,
    coverage: CoverageBuilder,
    stat_buf: Vec<u8>,
}

/// Wanted cgroup files of an app unit. `memory.current` is never read here (R-137).
const APP_WANT: CgroupWant = CgroupWant {
    memory_stat: true,
    swap: true,
    memory_current: false,
    cpu_stat: true,
    io_stat: true,
    pids: true,
    events: true,
};

/// One pass over `/proc` into `ProcessRecord`s with metadata, groups, flags and the P1 extra
/// columns, plus the app unit samples (PROC-002, PROC-004 to PROC-008, ADR-003, ADR-016). The
/// engine's `Processes` worker runs it in one blocking job per interval.
pub struct ProcessSampler {
    core: SweepCore,
    meta: MetaCache,
    apps: AppResolver,
    cg: CgroupRoot,
    pss: PssReader,
    users: Box<dyn UserNames + Send>,
    interner: Interner,
    metas: FxHashMap<ProcessKey, MetaSlot>,
    app_prev: FxHashMap<u64, AppPrev>,
    /// The flags of each key and the sweep time they were first seen with.
    flag_since: FxHashMap<ProcessKey, (ProcFlags, TimeNs)>,
    bufs: ReadBufs,
    empty: Arc<str>,
    scratch: Scratch,
}

impl ProcessSampler {
    /// Fails only when `<sys>/fs/cgroup` cannot be opened (`ProcRead { file: "cgroup" }`).
    pub fn new(
        root: SysRoot,
        paths: &AppPaths,
        users: Box<dyn UserNames + Send>,
    ) -> Result<Self, CollectError> {
        let cg = CgroupRoot::open(&root.sys)?;
        let mut interner = Interner::new();
        let empty = interner.intern("");
        Ok(Self {
            core: SweepCore::new(root),
            meta: MetaCache::new(),
            apps: AppResolver::new(paths),
            cg,
            pss: PssReader::new(),
            users,
            interner,
            metas: FxHashMap::default(),
            app_prev: FxHashMap::default(),
            flag_since: FxHashMap::default(),
            bufs: ReadBufs::default(),
            empty,
            scratch: Scratch::default(),
        })
    }

    /// Test hook: `clock` returns (CLOCK_BOOTTIME ns, CLOCK_MONOTONIC ns) in place of the real
    /// clocks.
    pub fn with_clock(mut self, clock: fn() -> (TimeNs, u64)) -> Self {
        self.core.clock = clock;
        self
    }

    /// Forgets every process and app unit baseline, so the next sweep gives no rates.
    pub fn reset_baselines(&mut self) {
        self.core.reset_baselines();
        self.app_prev.clear();
    }

    /// Reads every process into `out`. Per-process problems become validity, never errors.
    pub fn sweep(&mut self, ctx: &SweepCtx, inp: &SweepInputs<'_>, out: &mut SweepOutput) {
        out.records.clear();
        out.extras.clear();
        out.side.clear();
        out.apps.clear();
        out.coverage = Coverage::default();

        let now = (self.core.clock)().0;
        self.apps.begin_sweep(&self.cg, ctx.self_uid, now);
        let Self {
            core,
            meta,
            apps,
            users,
            interner,
            metas,
            bufs,
            empty,
            scratch,
            ..
        } = self;
        let Scratch {
            swept,
            name,
            search,
            cpus,
            app_pids,
            ..
        } = scratch;
        app_pids.clear();
        for unit in apps.app_units() {
            app_pids.extend(unit.pids.iter().copied());
        }

        let mut build = Build {
            ctx,
            inp,
            meta,
            apps,
            users: &mut **users,
            interner,
            metas,
            bufs,
            empty,
            name,
            search,
            cpus,
            app_pids,
            out,
        };
        let (start, end) = core.sweep_core_with(ctx, swept, &mut |p, dir| build.push(p, dir));
        out.observed_start = start;
        out.observed_end = end;

        self.set_flags(ctx, inp, out);
        self.sample_apps(out);
        if inp.want_pss {
            let s = &mut self.scratch;
            self.pss.read(
                &self.core.root,
                inp.detail_keys,
                &mut s.rollup_buf,
                &mut s.rollups,
            );
            apply_rollups(&mut out.records, &s.rollups, &mut s.coverage);
            out.coverage = s.coverage.build();
        }
        self.sort(out);

        let live = &mut self.scratch.live;
        live.clear();
        live.extend(out.records.iter().map(|r| r.key));
        self.meta.retain_live(live);
        self.apps.end_sweep(live, out.observed_end);
        self.metas.retain(|k, _| live.contains(k));
        self.flag_since.retain(|k, _| live.contains(k));
    }

    /// Step 5: the protected set from this sweep's processes, then the flags of each record.
    fn set_flags(&mut self, ctx: &SweepCtx, inp: &SweepInputs<'_>, out: &mut SweepOutput) {
        let set = {
            let rows: Vec<(u32, u32, &str, Option<&CgroupPath>, u32)> = out
                .records
                .iter()
                .zip(&out.extras)
                .map(|(r, x)| {
                    let name = if x.exe_name.is_empty() {
                        &*r.meta.comm
                    } else {
                        &*x.exe_name
                    };
                    let cgroup = self.metas.get(&r.key).and_then(|s| s.cgroup.as_ref());
                    (r.key.pid, r.ppid, name, cgroup, r.uid)
                })
                .collect();
            protected_set(&rows, ctx.self_uid, inp.protect)
        };
        let now = out.observed_start;
        for (r, x) in out.records.iter_mut().zip(&mut out.extras) {
            let kernel_thread = r.flags.contains(ProcFlags::KERNEL_THREAD);
            r.flags = flags_for(
                r.key.pid,
                kernel_thread,
                r.uid,
                ctx.self_uid,
                &set,
                inp.protect,
            );
            let since = self.flag_since.entry(r.key).or_insert((r.flags, now));
            if since.0 != r.flags {
                *since = (r.flags, now);
            }
            x.flag_since_ns = since.1;
        }
    }

    /// Step 7: the cgroup stats and io rates of each app unit, and the Frozen state of its
    /// members.
    fn sample_apps(&mut self, out: &mut SweepOutput) {
        let pid_index = &mut self.scratch.pid_index;
        pid_index.clear();
        pid_index.extend(out.records.iter().enumerate().map(|(i, r)| (r.key.pid, i)));

        for unit in self.apps.app_units() {
            let mut stats = CgroupStats::default();
            let (t_ns, mono_ns) = (self.core.clock)();
            if self
                .cg
                .read_stats(&unit.rel, APP_WANT, &mut self.scratch.stat_buf, &mut stats)
                .is_err()
            {
                continue;
            }

            // The group, name and icon of the first member that resolved to an app.
            let member = unit
                .pids
                .iter()
                .filter_map(|pid| pid_index.get(pid))
                .filter_map(|&i| self.metas.get(&out.records[i].key))
                .find(|s| s.app.class == ProcClass::App);
            let (group_key, app_name, icon_app, qualifier) = match member {
                Some(s) => (
                    s.app.group_key,
                    s.app.app_name.clone(),
                    s.app.icon_app.clone(),
                    s.app.qualifier,
                ),
                None => (
                    GroupKind::App {
                        app_id: unit.id.app_id.clone(),
                    }
                    .key(),
                    None,
                    None,
                    None,
                ),
            };

            let (io_read_bps, io_write_bps, io_validity) = if stats.io_valid {
                let cur = AppPrev {
                    t_ns,
                    mono_ns,
                    rbytes: stats.rbytes,
                    wbytes: stats.wbytes,
                };
                let rates = self
                    .app_prev
                    .insert(unit.cgroup_id, cur)
                    .and_then(|old| app_io_rates(&old, &cur));
                match rates {
                    Some((r, w)) => (r, w, Validity::Valid),
                    None => (0.0, 0.0, Validity::WarmingUp),
                }
            } else {
                self.app_prev.remove(&unit.cgroup_id);
                let v = if stats.denied {
                    Validity::Restricted
                } else {
                    Validity::Unsupported
                };
                (0.0, 0.0, v)
            };

            if stats.frozen == Some(true) {
                for pid in &unit.pids {
                    if let Some(&i) = pid_index.get(pid) {
                        out.extras[i].frozen = true;
                    }
                }
            }

            out.apps.push(AppUnitSample {
                group_key,
                rel: unit.rel.clone(),
                cgroup_id: unit.cgroup_id,
                app_name,
                icon_app,
                qualifier,
                stats,
                io_read_bps,
                io_write_bps,
                io_validity,
            });
        }
        let units = self.apps.app_units();
        self.app_prev
            .retain(|id, _| units.iter().any(|u| u.cgroup_id == *id));
    }

    /// Step 10: sorts `records`, `extras` and `side` together by `ProcessKey`.
    fn sort(&mut self, out: &mut SweepOutput) {
        let sorted = &mut self.scratch.sorted;
        sorted.clear();
        sorted.extend(
            out.records
                .drain(..)
                .zip(out.extras.drain(..))
                .zip(out.side.drain(..))
                .map(|((r, x), s)| (r, x, s)),
        );
        sorted.sort_unstable_by_key(|(r, _, _)| r.key);
        for (r, x, s) in sorted.drain(..) {
            out.records.push(r);
            out.extras.push(x);
            out.side.push(s);
        }
    }
}

impl std::fmt::Debug for ProcessSampler {
    // Counts only: metadata holds command lines and paths (ADR-015).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessSampler")
            .field("core", &self.core)
            .field("metas", &self.metas.len())
            .field("app_units", &self.app_prev.len())
            .finish_non_exhaustive()
    }
}

/// Read and write bytes per second of an app unit, or `None` when ADR-008 gives no rate: time
/// did not move forward, the machine slept in between, or a counter went down.
fn app_io_rates(old: &AppPrev, cur: &AppPrev) -> Option<(f32, f32)> {
    if cur.t_ns <= old.t_ns || cur.rbytes < old.rbytes || cur.wbytes < old.wbytes {
        return None;
    }
    let was = Clock {
        boot_ns: old.t_ns,
        mono_ns: old.mono_ns,
    };
    let now = Clock {
        boot_ns: cur.t_ns,
        mono_ns: cur.mono_ns,
    };
    if resumed_between(was, now) {
        return None;
    }
    let dt_s = (cur.t_ns - old.t_ns) as f64 / 1e9;
    Some((
        ((cur.rbytes - old.rbytes) as f64 / dt_s) as f32,
        ((cur.wbytes - old.wbytes) as f64 / dt_s) as f32,
    ))
}

/// The per-process part of `ProcessSampler::sweep`, run inside the sweep while the process's
/// dirfd is open: steps 2 to 4, 6 and 8.
struct Build<'a, 'b> {
    ctx: &'a SweepCtx,
    inp: &'a SweepInputs<'b>,
    meta: &'a mut MetaCache,
    apps: &'a mut AppResolver,
    users: &'a mut (dyn UserNames + Send),
    interner: &'a mut Interner,
    metas: &'a mut FxHashMap<ProcessKey, MetaSlot>,
    bufs: &'a mut ReadBufs,
    empty: &'a Arc<str>,
    name: &'a mut String,
    search: &'a mut String,
    cpus: &'a mut SmallVec<[u16; 64]>,
    app_pids: &'a FxHashSet<u32>,
    out: &'a mut SweepOutput,
}

impl Build<'_, '_> {
    fn push(&mut self, p: &SweptProc, dir: BorrowedFd<'_>) {
        let key = p.key;
        let pid = key.pid;
        let ruid = p.status.uid[0];
        let kernel_thread = p.stat.flags & PF_KTHREAD != 0;
        let (raw, change) = self
            .meta
            .get_or_refresh(key, dir, &p.stat.comm_raw, self.bufs);

        let owns_window = self.inp.window_pids.is_some_and(|s| s.contains(&pid));
        let wm_class = self.inp.wm_class.and_then(|m| m.get(&pid)).map(|c| &**c);
        let app = self.apps.resolve(
            key,
            &ResolveInput {
                pid,
                ruid,
                kernel_thread,
                meta: raw,
                owns_window,
                wm_class,
            },
            self.ctx.self_uid,
        );
        let primary = is_app_primary(pid, p.stat.ppid, self.app_pids);
        let user = self.users.name(ruid);

        let reuse = change == MetaChange::Unchanged
            && self.metas.get(&key).is_some_and(|s| {
                Arc::ptr_eq(&s.app, &app)
                    && s.primary == primary
                    && Arc::ptr_eq(&s.meta.user, &user)
            });
        if !reuse {
            let slot = build_meta(
                raw,
                app.clone(),
                primary,
                user,
                kernel_thread,
                self.interner,
                self.empty,
                self.name,
                self.search,
            );
            self.metas.insert(key, slot);
        }
        let Some(slot) = self.metas.get(&key) else {
            return;
        };

        let status = &p.status;
        let threads = if status.threads > 0 {
            status.threads
        } else {
            p.stat.num_threads
        };
        // Only KERNEL_THREAD is known here; `set_flags` computes the full set.
        let mut flags = ProcFlags::empty();
        flags.set(ProcFlags::KERNEL_THREAD, kernel_thread);
        self.out.records.push(ProcessRecord {
            key,
            ppid: p.stat.ppid,
            uid: ruid,
            meta: slot.meta.clone(),
            group: app.group_key,
            state: ProcState::from_stat_char(p.stat.state),
            flags,
            nice: p.stat.nice,
            threads,
            cpu_pct: p.cpu_pct,
            rss_anon_bytes: status.rss_anon.unwrap_or(0),
            rss_bytes: status.vm_rss.unwrap_or(0),
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: status.vm_swap.unwrap_or(0),
            io_read_bps: p.io_read_bps,
            io_write_bps: p.io_write_bps,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            validity: p.validity,
        });

        let mut affinity = CpuSet::empty();
        if parse_cpu_list(&status.cpus_allowed_list, self.cpus).is_ok() {
            for &cpu in self.cpus.iter() {
                affinity.insert(cpu);
            }
        }
        let ticks = u128::from(self.ctx.ticks_per_sec.max(1));
        let start_boot_ns = u128::from(p.stat.start_ticks) * 1_000_000_000 / ticks;
        self.out.extras.push(ProcessExtras {
            cpu_kernel_pct: p.cpu_kernel_pct,
            cpu_user_pct: p.cpu_user_pct,
            cpu_relative_pct: p.cpu_relative_pct,
            utime_ticks: p.stat.utime,
            stime_ticks: p.stat.stime,
            start_boot_ns: u64::try_from(start_boot_ns).unwrap_or(u64::MAX),
            rss_peak_bytes: status.vm_hwm.unwrap_or(0),
            rss_shared_bytes: ws_shared_bytes(status).unwrap_or(0),
            commit_bytes: commit_bytes(status).unwrap_or(0),
            vsize_bytes: status.vm_size.unwrap_or(0),
            vsize_peak_bytes: status.vm_peak.unwrap_or(0),
            page_faults: p.stat.minflt.saturating_add(p.stat.majflt),
            page_faults_delta: p.page_faults_delta,
            major_faults: p.stat.majflt,
            policy: sched_policy(p.stat.policy),
            rt_prio: u8::try_from(p.stat.rt_priority).unwrap_or(u8::MAX),
            affinity,
            handles: match p.fd_count {
                Some(FdCount::Count(n)) => Some(n),
                Some(FdCount::Restricted | FdCount::Gone) | None => None,
            },
            exe_name: slot.exe_name.clone(),
            exe_path: slot.exe_path.clone(),
            exe_deleted: slot.exe_deleted,
            unit: slot.unit.clone(),
            state_letter: p.stat.state,
            frozen: false,
            disk_read_bps: p.disk_read_bps,
            disk_write_raw_bps: p.disk_write_raw_bps,
            flag_since_ns: 0,
        });
        self.out.side.push(SweepSide {
            class: app.class,
            is_app_primary: primary,
        });
    }
}

/// Steps 3 and 4: the display name, search text and `ProcessMeta` of one process.
#[allow(clippy::too_many_arguments, reason = "split borrows of ProcessSampler")]
fn build_meta(
    raw: &RawMeta,
    app: Arc<AppInfo>,
    primary: bool,
    user: Arc<str>,
    kernel_thread: bool,
    interner: &mut Interner,
    empty: &Arc<str>,
    name: &mut String,
    search: &mut String,
) -> MetaSlot {
    let (exe, exe_deleted) = match &raw.exe {
        ExeLink::Path { path, deleted } => (Some(path), *deleted),
        ExeLink::Restricted | ExeLink::None => (None, false),
    };
    let exe_str = exe.map(|e| &**e);
    display_name(
        &NameInput {
            app_name: app.app_name.as_deref(),
            is_app_primary: primary,
            exe: exe_str,
            argv: &raw.argv,
            comm: &raw.comm,
            kernel_thread,
        },
        name,
    );
    let wine = is_wine(exe_str.map(exe_basename), &raw.comm);
    let image = image_name(exe_str, &raw.argv, &raw.comm, wine).unwrap_or("");
    search_lower(
        &[
            name.as_str(),
            image,
            raw.cmdline.as_deref().unwrap_or(""),
            &user,
            app.description.as_deref().unwrap_or(""),
            app.app_name.as_deref().unwrap_or(""),
            app.unit.as_deref().unwrap_or(""),
        ],
        search,
    );
    let cgroup = raw.cgroup.clone();
    let unit = app.unit.clone().map(|name| UnitRef {
        manager: match cgroup.as_ref().map(placement) {
            Some(UserPlacement::InUserManager { .. }) => Manager::User,
            _ => Manager::System,
        },
        name,
    });
    let meta = Arc::new(ProcessMeta {
        comm: raw.comm.clone(),
        name: Arc::from(name.as_str()),
        exe: exe.cloned(),
        cmdline: raw.cmdline.clone(),
        user,
        cgroup: cgroup.as_ref().map(|c| c.rel.clone()),
        app_id: app.app_id.clone(),
        icon: app.icon_process.clone(),
        description: app.description.clone(),
        search_lower: Arc::from(search.as_str()),
    });
    MetaSlot {
        exe_name: exe_str.map_or_else(|| empty.clone(), |e| interner.intern(exe_basename(e))),
        exe_path: exe.cloned().unwrap_or_else(|| empty.clone()),
        exe_deleted,
        unit,
        cgroup,
        meta,
        app,
        primary,
    }
}

/// `SchedPolicy` from stat field 41. `SCHED_DEADLINE` (6) and unknown values show as Other.
fn sched_policy(policy: u32) -> SchedPolicy {
    match policy {
        1 => SchedPolicy::Fifo,
        2 => SchedPolicy::Rr,
        3 => SchedPolicy::Batch,
        5 => SchedPolicy::Idle,
        _ => SchedPolicy::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_names() {
        assert_eq!(parse_pid(b"4201"), Some(4201));
        assert_eq!(parse_pid(b"0"), Some(0));
        assert_eq!(parse_pid(b"self"), None);
        assert_eq!(parse_pid(b""), None);
        assert_eq!(parse_pid(b"12a"), None);
        assert_eq!(parse_pid(b"99999999999"), None);
    }
}
