//! One sweep over all processes in /proc (PROC-002, PROC-029, PROC-030, PROC-031, ADR-008,
//! ADR-016).
//!
//! Per-process files are opened as `"<pid>/<file>"` relative to the `/proc` dirfd; the pid
//! directory itself is opened only for a metadata read or an fd count (R-286). `stat` and `statm`
//! are read every sweep, `status` for a tenth of the pids (or all when a status-only field is
//! demanded), `io` only when demanded. Rates follow the ADR-008 counter rules: no rate on the first sample,
//! after a counter reset or across a suspend. Nothing here logs above `trace` per process.
//!
//! In memory-only mode (`set_mem_only`, while only the memory group history needs the sweep:
//! D-036, UI-PRF-060) each process gets `stat` and `status` read every sweep and no `statm`.
//! `stat` gives the key (pid, start_ticks), so a reused pid is a new process at once. `status`
//! gives RssAnon, VmRSS, commit, VmSize and the uid, so a uid change shows at the next sweep.
//! The metadata (exe, cmdline, cgroup) is re-read for a new key, when `comm` changes and every
//! [`MEM_ONLY_META_REFRESH`] sweeps, so an exec or a cgroup move shows within that many sweeps.
//! No rates, `io`, fd counts or cgroup stats.
//!
//! `ProcessSampler` (E05-A1) adds metadata, app groups, flags, the P1 extra columns, the app
//! unit samples and the Frozen state of unit cgroups on top of `SweepCore`. Command lines, paths and user names are never logged
//! (ADR-015).

use std::cell::Cell;
use std::fmt::Write as _;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::buffer::spare_capacity;
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{Mode, OFlags, RawDir};
use smallvec::SmallVec;
use tl_model::actions::{CgroupRef, SchedPolicy};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{
    AppCgroupSample, Coverage, GroupKind, ProcFlags, ProcState, ProcessExtras, ProcessExtrasP2,
    ProcessMeta, ProcessRecord, WineProcess,
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
use crate::pkg::{PackageIndex, Pacman};
use crate::proc::cgroup::{
    CgroupPath, CgroupRoot, CgroupStats, CgroupWant, UserPlacement, placement,
};
use crate::proc::extras_p2::ExtrasP2Reader;
use crate::proc::fdcount::{FdCount, fd_count};
use crate::proc::io::{IoRead, PidIo, read_io_at};
use crate::proc::meta::{
    ExeLink, Interner, META_REFRESH_SWEEPS, MetaCache, MetaChange, RawMeta, ReadBufs, UserNames,
};
use crate::proc::pid_stat::{PF_KTHREAD, PidStat, parse_pid_stat};
use crate::proc::smaps::{CoverageBuilder, PssReader, RollupRead, apply_rollups};
use crate::proc::statm::{PidStatm, parse_statm};
use crate::proc::status::{PidStatus, STATUS_READ_MAX, parse_status};
use crate::sysroot::{SMALL_FILE_LIMIT, SysRoot};

/// getdents buffer for the `<proc>` directory listing.
const DIRENT_BUF: usize = 32 * 1024;

/// Without a demand, `status` is read for the pids with `pid % STATUS_STAGGER == sweep %
/// STATUS_STAGGER`, so its fields are at most this many sweeps old (R-286).
pub const STATUS_STAGGER: u64 = 10;

/// In memory-only mode the metadata of a process is re-read at least this often, in sweeps:
/// 60 s at the 10 s period (tasklens-8fx.45).
pub const MEM_ONLY_META_REFRESH: u32 = 6;

thread_local! {
    static STATUS_READS: Cell<u32> = const { Cell::new(0) };
    static IO_READS: Cell<u32> = const { Cell::new(0) };
}

/// Test hook: how many per-process `status` files the sweep opened on this thread.
#[doc(hidden)]
pub fn status_reads() -> u32 {
    STATUS_READS.with(Cell::get)
}

/// Test hook: how many per-process `io` files the sweep opened on this thread.
#[doc(hidden)]
pub fn io_reads() -> u32 {
    IO_READS.with(Cell::get)
}

fn count(c: &'static std::thread::LocalKey<Cell<u32>>) {
    c.with(|c| c.set(c.get() + 1));
}

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
    /// A status-only field is demanded (per-process Swap, peak RSS, peak VmSize or CPU
    /// affinity): read `status` for every pid, not only the staggered tenth (R-286).
    pub want_status_fields: bool,
    /// An I/O or disk column is demanded, or history recording is on: read `io` for every pid.
    /// Without it the I/O rates are WarmingUp, and stay so for one interval after it is set.
    pub want_io: bool,
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
    /// From this sweep or, when `status_fresh` is false, from an earlier one (at most
    /// `STATUS_STAGGER` sweeps old). Validity of its fields is from the same read.
    pub status: PidStatus,
    pub status_fresh: bool,
    /// All zero in memory-only mode: the byte fields below then come from `status`.
    pub statm: PidStatm,
    /// statm (resident − shared) × page size: the kernel's `RssAnon` (R-286).
    pub rss_anon_bytes: u64,
    /// statm resident × page size (`VmRSS`).
    pub rss_bytes: u64,
    /// statm shared × page size (`RssFile` + `RssShmem`).
    pub rss_shared_bytes: u64,
    /// statm data × page size (`VmData` + `VmStk`).
    pub commit_bytes: u64,
    /// statm size × page size (`VmSize`).
    pub vsize_bytes: u64,
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
    /// Cancelled write bytes per second (PROC-031).
    pub cancelled_write_bps: f32,
    pub page_faults_delta: u64,
    pub validity: FieldValidity,
}

/// Per-process state kept between sweeps.
pub struct SweepCore {
    root: SysRoot,
    prev: FxHashMap<ProcessKey, Prev>,
    /// Filled during a sweep, then swapped with `prev`, so unseen keys drop out.
    next: FxHashMap<ProcessKey, Prev>,
    /// The last `status` read of each key and the validity it gave (`None` when it parsed).
    status: FxHashMap<ProcessKey, (PidStatus, Option<Validity>)>,
    sweeps: u64,
    page_size: u64,
    bufs: ReadBufs,
    path: String,
    dirents: Vec<MaybeUninit<u8>>,
    stat: PidStat,
    clock: fn() -> (TimeNs, u64),
    /// Memory-only mode: see the module doc.
    mem_only: bool,
}

impl SweepCore {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            prev: FxHashMap::default(),
            next: FxHashMap::default(),
            status: FxHashMap::default(),
            sweeps: 0,
            page_size: rustix::param::page_size() as u64,
            bufs: ReadBufs::default(),
            path: String::new(),
            dirents: vec![MaybeUninit::uninit(); DIRENT_BUF],
            stat: PidStat::default(),
            clock: real_clock,
            mem_only: false,
        }
    }

    /// Turns memory-only mode on or off (see the module doc). A change forgets every baseline.
    pub fn set_mem_only(&mut self, on: bool) {
        if self.mem_only != on {
            self.mem_only = on;
            self.reset_baselines();
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

    /// `sweep_core` that also calls `per_pid` for each process with the `/proc` dirfd, right
    /// after its files are read, so metadata is read from the same process.
    pub fn sweep_core_with(
        &mut self,
        ctx: &SweepCtx,
        out: &mut Vec<SweptProc>,
        per_pid: &mut dyn FnMut(&SweptProc, PidDir<'_>),
    ) -> (TimeNs, TimeNs) {
        out.clear();
        self.next.clear();
        self.sweeps += 1;
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
                let dir = PidDir {
                    proc_fd: proc_fd.as_fd(),
                    pid,
                };
                let mut core = SweepIo {
                    sweep: self.sweeps,
                    page_size: self.page_size,
                    clock: self.clock,
                    prev: &self.prev,
                    next: &mut self.next,
                    status: &mut self.status,
                    bufs: &mut self.bufs,
                    path: &mut self.path,
                    stat: &mut self.stat,
                    mem_only: self.mem_only,
                };
                if let Some(p) = sweep_one(ctx, dir, &mut core) {
                    per_pid(&p, dir);
                    out.push(p);
                }
            }
        }

        std::mem::swap(&mut self.prev, &mut self.next);
        self.next.clear();
        let live = &self.prev;
        self.status.retain(|k, _| live.contains_key(k));
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

/// One process's place in `/proc`: its files are opened as `"<pid>/<file>"` relative to
/// `proc_fd`, and the `/proc/<pid>` directory only when a reader needs a dirfd (R-286).
#[derive(Clone, Copy, Debug)]
pub struct PidDir<'a> {
    pub proc_fd: BorrowedFd<'a>,
    pub pid: u32,
}

impl PidDir<'_> {
    /// Opens `/proc/<pid>` as an `O_DIRECTORY` fd. `None` when the process is gone.
    pub fn open(&self) -> Option<OwnedFd> {
        let name = self.pid.to_string();
        rustix::fs::openat(
            self.proc_fd,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .inspect_err(|_| tracing::trace!(pid = self.pid, "pid directory not opened"))
        .ok()
    }
}

/// Split borrows of `SweepCore` for `sweep_one`.
struct SweepIo<'a> {
    sweep: u64,
    page_size: u64,
    clock: fn() -> (TimeNs, u64),
    prev: &'a FxHashMap<ProcessKey, Prev>,
    next: &'a mut FxHashMap<ProcessKey, Prev>,
    status: &'a mut FxHashMap<ProcessKey, (PidStatus, Option<Validity>)>,
    bufs: &'a mut ReadBufs,
    path: &'a mut String,
    stat: &'a mut PidStat,
    mem_only: bool,
}

/// Reads `name` relative to `dir` into `buf` with one `read` (R-286). The per-process files are
/// seq_files that hand over their whole text in one call, so a read shorter than the buffer is
/// the end of the file and the EOF read `sysroot::read_at` makes is skipped. Text past `limit` is cut.
fn read_once(
    dir: BorrowedFd<'_>,
    name: &str,
    buf: &mut Vec<u8>,
    limit: usize,
) -> Result<(), CollectError> {
    buf.clear();
    buf.reserve(limit + 1);
    let fd = rustix::fs::openat(dir, name, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    rustix::io::read(&fd, spare_capacity(buf))?;
    buf.truncate(limit);
    Ok(())
}

/// Sets `path` to `"<pid>/<file>"`.
fn pid_path<'p>(path: &'p mut String, pid: u32, file: &str) -> &'p str {
    path.clear();
    let _ = write!(path, "{pid}/{file}");
    path
}

/// Reads one process and computes its rates. `None` when the process is gone or its `stat`
/// cannot be read, since then it has no key.
fn sweep_one(ctx: &SweepCtx, dir: PidDir<'_>, core: &mut SweepIo<'_>) -> Option<SweptProc> {
    let SweepIo {
        sweep,
        page_size,
        clock,
        prev,
        next,
        status: status_cache,
        bufs,
        path,
        stat,
        mem_only,
    } = core;
    let mem_only = *mem_only;
    let (proc_fd, pid) = (dir.proc_fd, dir.pid);
    let (t_ns, mono_ns) = clock();

    let stat_read = read_once(
        proc_fd,
        pid_path(path, pid, "stat"),
        &mut bufs.small,
        SMALL_FILE_LIMIT,
    )
    .and_then(|()| parse_pid_stat(&bufs.small, stat));
    if let Err(e) = stat_read {
        tracing::trace!(pid, error = %e, "stat not read, process skipped");
        return None;
    }
    let key = ProcessKey {
        start_ticks: stat.start_ticks,
        boot_id: ctx.boot_id,
        pid,
    };
    let kernel_thread = stat.flags & PF_KTHREAD != 0;

    let mut validity = FieldValidity::default();
    validity.set(ProcessField::Threads, Validity::Valid);

    // RssAnon, VmRSS, commit and VmSize come from statm every sweep (R-286). Memory-only mode
    // takes them from status, which it reads every sweep anyway. A kernel thread has no mm: its
    // status has no memory lines, so the fields stay Unsupported.
    let mut statm = PidStatm::default();
    if !mem_only {
        let statm_validity = match read_once(
            proc_fd,
            pid_path(path, pid, "statm"),
            &mut bufs.small,
            SMALL_FILE_LIMIT,
        )
        .and_then(|()| parse_statm(&bufs.small, &mut statm))
        {
            Ok(()) if kernel_thread => Validity::Unsupported,
            Ok(()) => Validity::Valid,
            Err(e) => {
                statm = PidStatm::default();
                e.validity()
            }
        };
        validity.set(ProcessField::RssAnon, statm_validity);
        validity.set(ProcessField::Rss, statm_validity);
    }

    // status: on first sight, when a status-only field is demanded, or for this sweep's tenth.
    // Otherwise the last read's values and validity stand (R-286). Memory-only mode reads it
    // every sweep: the memory groups need its RssAnon and uid.
    let staggered = u64::from(pid) % STATUS_STAGGER == *sweep % STATUS_STAGGER;
    let due = mem_only || ctx.want_status_fields || staggered || !status_cache.contains_key(&key);
    if due {
        count(&STATUS_READS);
        let slot = status_cache.entry(key).or_default();
        slot.1 = match read_once(
            proc_fd,
            pid_path(path, pid, "status"),
            &mut bufs.small,
            STATUS_READ_MAX,
        ) {
            Ok(()) => parse_status(&bufs.small, &mut slot.0)
                .err()
                .map(|e| e.validity()),
            Err(e) => Some(e.validity()),
        };
        if slot.1.is_some() {
            slot.0 = PidStatus::default();
        }
    }
    let (status, mem_validity) = status_cache.get(&key)?;
    let present = |v: Option<u64>| match (*mem_validity, v) {
        (Some(failed), _) => failed,
        (None, Some(_)) => Validity::Valid,
        (None, None) => Validity::Unsupported,
    };
    validity.set(ProcessField::Swap, present(status.vm_swap));
    if mem_only {
        validity.set(ProcessField::RssAnon, present(status.rss_anon));
        validity.set(ProcessField::Rss, present(status.vm_rss));
    }

    let (io, io_validity) = if ctx.want_io && !mem_only {
        count(&IO_READS);
        match read_io_at(proc_fd, pid_path(path, pid, "io"), &mut bufs.small) {
            Ok(IoRead::Ok(io)) => (IoState::Valid(io), None),
            Ok(IoRead::Restricted) => (IoState::Restricted, Some(Validity::Restricted)),
            Ok(IoRead::Gone) => (IoState::WarmingUp, Some(Validity::Missing)),
            Err(e) => (IoState::WarmingUp, Some(e.validity())),
        }
    } else {
        // Not read: no counters, so the rates are WarmingUp now and for one interval after
        // `want_io` is set.
        (IoState::WarmingUp, None)
    };

    let fd_count = if ctx.want_fd_count && !mem_only {
        Some(dir.open().map_or(FdCount::Gone, |d| fd_count(d.as_fd())))
    } else {
        None
    };
    let page = *page_size;
    let (rss_anon_bytes, rss_bytes, rss_shared_bytes, commit_bytes, vsize_bytes) = if mem_only {
        let or0 = |v: Option<u64>| v.unwrap_or(0);
        (
            or0(status.rss_anon),
            or0(status.vm_rss),
            or0(status.rss_file).saturating_add(or0(status.rss_shmem)),
            or0(status.vm_data).saturating_add(or0(status.vm_stk)),
            or0(status.vm_size),
        )
    } else {
        (
            statm
                .resident
                .saturating_sub(statm.shared)
                .saturating_mul(page),
            statm.resident.saturating_mul(page),
            statm.shared.saturating_mul(page),
            statm.data.saturating_mul(page),
            statm.size.saturating_mul(page),
        )
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
        status_fresh: due,
        statm,
        rss_anon_bytes,
        rss_bytes,
        rss_shared_bytes,
        commit_bytes,
        vsize_bytes,
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
        cancelled_write_bps: 0.0,
        page_faults_delta: 0,
        validity,
    };

    // Memory-only mode gives no rates: the series it feeds need none, and `io` is not read.
    let base = if mem_only { None } else { prev.get(&key) };
    let io_rates = match base.and_then(|old| rate_base(old, &cur)) {
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
        p.cancelled_write_bps = (d_cancel as f64 / dt_s) as f32;
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
            frozen: self.stats.frozen == Some(true),
            cgroup: CgroupRef {
                rel_path: self.rel.clone(),
                cgroup_id: self.cgroup_id,
            },
            validity,
        }
    }
}

/// The result of one `ProcessSampler::sweep`. `records`, `extras` and `side` have the same
/// length and order (sorted by `ProcessKey`); so does `extras_p2` when `detail_keys` is not
/// empty, and it is empty otherwise. Reuse one value across sweeps: the vectors keep their
/// capacity.
#[derive(Default)]
pub struct SweepOutput {
    pub observed_start: TimeNs,
    pub observed_end: TimeNs,
    pub records: Vec<ProcessRecord>,
    pub extras: Vec<ProcessExtras>,
    pub side: Vec<SweepSide>,
    /// P2 columns (E05-035): filled for `detail_keys`, `ProcessExtrasP2::default()` for every
    /// other record.
    pub extras_p2: Vec<ProcessExtrasP2>,
    pub apps: Vec<AppUnitSample>,
    pub coverage: Coverage,
}

/// What the engine passes to `ProcessSampler::sweep` besides `SweepCtx`.
pub struct SweepInputs<'a> {
    pub protect: &'a ProtectInputs,
    /// Keys whose PSS and USS are read when `want_pss` is set, and whose P2 columns are read.
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
    wine: Option<Arc<WineProcess>>,
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
    sorted_p2: Vec<(ProcessKey, ProcessExtrasP2)>,
    detail: FxHashSet<ProcessKey>,
    rollups: Vec<(ProcessKey, RollupRead)>,
    rollup_buf: Vec<u8>,
    coverage: CoverageBuilder,
    stat_buf: Vec<u8>,
    /// (unit, cgroup path, length of its unit prefix, record index) of each unit member.
    unit_members: Vec<(UnitRef, Arc<str>, usize, usize)>,
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

/// Wanted cgroup files of a unit cgroup in the sweep: `cgroup.events` only (tasklens-8q0.42).
const UNIT_WANT: CgroupWant = CgroupWant {
    memory_stat: false,
    swap: false,
    memory_current: false,
    cpu_stat: false,
    io_stat: false,
    pids: false,
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
    p2: ExtrasP2Reader,
    pkg: PackageIndex,
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
            p2: ExtrasP2Reader::new(),
            pkg: PackageIndex::new(Box::new(Pacman::default())),
        })
    }

    /// The package index the P2 reads use. The engine puts its shared index here and runs
    /// `run_batch` in its own job.
    pub fn pkg_mut(&mut self) -> &mut PackageIndex {
        &mut self.pkg
    }

    /// A handle to the package index the P2 reads use.
    pub fn package_index(&self) -> PackageIndex {
        self.pkg.clone()
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

    /// Memory-only mode (see the module doc): for the memory group series alone. The metadata
    /// is re-read every `MEM_ONLY_META_REFRESH` sweeps and no app unit is sampled. A change
    /// forgets every baseline.
    pub fn set_mem_only(&mut self, on: bool) {
        if self.core.mem_only != on {
            self.core.set_mem_only(on);
            self.app_prev.clear();
        }
    }

    /// Reads every process into `out`. Per-process problems become validity, never errors.
    pub fn sweep(&mut self, ctx: &SweepCtx, inp: &SweepInputs<'_>, out: &mut SweepOutput) {
        out.records.clear();
        out.extras.clear();
        out.side.clear();
        out.extras_p2.clear();
        out.apps.clear();
        out.coverage = Coverage::default();
        // P2 columns only for the visible rows; nothing per record when there are none.
        let p2_root = (!inp.detail_keys.is_empty()).then(|| self.core.root.clone());
        self.scratch.detail.clear();
        self.scratch.detail.extend(inp.detail_keys.iter().copied());

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
            p2,
            pkg,
            ..
        } = self;
        let Scratch {
            swept,
            name,
            search,
            cpus,
            app_pids,
            detail,
            ..
        } = scratch;
        app_pids.clear();
        for unit in apps.app_units() {
            app_pids.extend(unit.pids.iter().copied());
        }

        let mut build = Build {
            ctx,
            inp,
            mem_only: core.mem_only,
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
            p2: p2_root.as_ref().map(|root| P2Build {
                root,
                reader: p2,
                pkg,
                detail,
                now: Instant::now(),
            }),
            out,
        };
        let (start, end) = core.sweep_core_with(ctx, swept, &mut |p, dir| build.push(p, dir));
        out.observed_start = start;
        out.observed_end = end;

        self.set_flags(ctx, inp, out);
        if !self.core.mem_only {
            self.sample_apps(out);
            self.sample_units(out);
        }
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
        self.p2.retain_live(live);
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

    /// After step 7: the Frozen state of each unit cgroup that holds a unit row, from its
    /// `cgroup.events`, set on its members (tasklens-8q0.42). The reducer folds two or more
    /// processes outside app units with the same `UnitRef` into one unit row, and that row can
    /// span several cgroups: `pipewire.service` of two users is one row. So every unit cgroup of
    /// a `UnitRef` with two or more such processes is read, and a `UnitRef` with one is not.
    /// App members are left out: `sample_apps` already read their cgroup.
    fn sample_units(&mut self, out: &mut SweepOutput) {
        let Scratch {
            app_pids,
            unit_members: members,
            stat_buf,
            ..
        } = &mut self.scratch;
        members.clear();
        for (i, ((r, side), x)) in out
            .records
            .iter()
            .zip(&out.side)
            .zip(&out.extras)
            .enumerate()
        {
            if side.class == ProcClass::App || app_pids.contains(&r.key.pid) {
                continue;
            }
            let Some(unit) = &x.unit else {
                continue;
            };
            let Some(cg) = self.metas.get(&r.key).and_then(|s| s.cgroup.as_ref()) else {
                continue;
            };
            if let Some(len) = unit_rel_len(cg) {
                members.push((unit.clone(), cg.rel.clone(), len, i));
            }
        }
        members.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1[..a.2].cmp(&b.1[..b.2])));
        let mut stats = CgroupStats::default();
        for row in members.chunk_by(|a, b| a.0 == b.0) {
            if row.len() < 2 {
                continue;
            }
            for run in row.chunk_by(|a, b| a.1[..a.2] == b.1[..b.2]) {
                let (_, rel, len, _) = &run[0];
                if self
                    .cg
                    .read_stats(&rel[..*len], UNIT_WANT, stat_buf, &mut stats)
                    .is_err()
                    || stats.frozen != Some(true)
                {
                    continue;
                }
                for &(_, _, _, i) in run {
                    out.extras[i].frozen = true;
                }
            }
        }
    }

    /// Step 10: sorts `records`, `extras`, `side` and a non-empty `extras_p2` together by
    /// `ProcessKey`.
    fn sort(&mut self, out: &mut SweepOutput) {
        if !out.extras_p2.is_empty() {
            // Keys are unique, so sorting by key gives the same order as `records` below.
            let sorted_p2 = &mut self.scratch.sorted_p2;
            sorted_p2.clear();
            sorted_p2.extend(
                out.records
                    .iter()
                    .map(|r| r.key)
                    .zip(out.extras_p2.drain(..)),
            );
            sorted_p2.sort_unstable_by_key(|(k, _)| *k);
            out.extras_p2.extend(sorted_p2.drain(..).map(|(_, p)| p));
        }
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

/// The length of the unit part of `cg.rel`: the path up to and including its innermost
/// `.service` or `.scope` component. `None` when the path has no unit.
fn unit_rel_len(cg: &CgroupPath) -> Option<usize> {
    let unit = cg.unit.as_deref()?;
    let mut end = None;
    let mut start = 0;
    for comp in cg.rel.split('/') {
        if comp == unit {
            end = Some(start + comp.len());
        }
        start += comp.len() + 1;
    }
    end
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
    /// Memory-only mode: the metadata is re-read every `MEM_ONLY_META_REFRESH` sweeps.
    mem_only: bool,
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
    /// `Some` when `detail_keys` is not empty.
    p2: Option<P2Build<'a>>,
    out: &'a mut SweepOutput,
}

/// What `Build` needs to read the P2 columns of the detail keys (E05-035).
struct P2Build<'a> {
    root: &'a SysRoot,
    reader: &'a mut ExtrasP2Reader,
    pkg: &'a mut PackageIndex,
    detail: &'a FxHashSet<ProcessKey>,
    now: Instant,
}

impl Build<'_, '_> {
    fn push(&mut self, p: &SweptProc, dir: PidDir<'_>) {
        let key = p.key;
        let pid = key.pid;
        // The real uid of the last status read, at most STATUS_STAGGER sweeps old (R-286), and
        // from this sweep in memory-only mode.
        let ruid = p.status.uid[0];
        let kernel_thread = p.stat.flags & PF_KTHREAD != 0;
        // The pid directory is opened only when the metadata is read. A process gone by then
        // is left out of this sweep.
        let every = if self.mem_only {
            MEM_ONLY_META_REFRESH
        } else {
            META_REFRESH_SWEEPS
        };
        let Some((raw, change)) =
            self.meta
                .get_or_refresh_every(key, &p.stat.comm_raw, self.bufs, every, || dir.open())
        else {
            return;
        };

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
        let threads = p.stat.num_threads;
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
            rss_anon_bytes: p.rss_anon_bytes,
            rss_bytes: p.rss_bytes,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: status.vm_swap.unwrap_or(0),
            io_read_bps: p.io_read_bps,
            io_write_bps: p.io_write_bps,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: Some(status.uid[1]).filter(|&euid| euid != ruid),
            net_shared: false,
            cancelled_write_bps: p.cancelled_write_bps,
            validity: p.validity,
        });

        // Only the table shows the affinity: memory-only mode leaves it empty.
        let mut affinity = CpuSet::empty();
        if !self.mem_only && parse_cpu_list(&status.cpus_allowed_list, self.cpus).is_ok() {
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
            rss_shared_bytes: p.rss_shared_bytes,
            commit_bytes: p.commit_bytes,
            vsize_bytes: p.vsize_bytes,
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
            pgrp: u32::try_from(p.stat.pgrp).unwrap_or(0),
            sid: u32::try_from(p.stat.session).unwrap_or(0),
            last_cpu: u32::try_from(p.stat.processor).unwrap_or(0),
            wine: slot.wine.clone(),
        });
        self.out.side.push(SweepSide {
            class: app.class,
            is_app_primary: primary,
        });
        if let Some(b) = self.p2.as_mut() {
            let entry = if b.detail.contains(&key) {
                // A failed status read shows as the Swap validity; then the status fields are
                // zeros, not values.
                let status_ok = matches!(
                    p.validity.get(ProcessField::Swap),
                    Validity::Valid | Validity::Unsupported
                );
                b.reader.read(
                    b.root,
                    dir,
                    key,
                    &p.stat,
                    status_ok.then_some(&p.status),
                    raw,
                    b.pkg,
                    b.now,
                    self.bufs,
                )
            } else {
                ProcessExtrasP2::default()
            };
            self.out.extras_p2.push(entry);
        }
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
        desktop_path: app.desktop_path.clone(),
        search_lower: Arc::from(search.as_str()),
    });
    MetaSlot {
        exe_name: exe_str.map_or_else(|| empty.clone(), |e| interner.intern(exe_basename(e))),
        exe_path: exe.cloned().unwrap_or_else(|| empty.clone()),
        exe_deleted,
        unit,
        wine: raw.wine.clone(),
        cgroup,
        meta,
        app,
        primary,
    }
}

/// `SchedPolicy` from stat field 41. Unknown values show as Other.
fn sched_policy(policy: u32) -> SchedPolicy {
    match policy {
        1 => SchedPolicy::Fifo,
        2 => SchedPolicy::Rr,
        3 => SchedPolicy::Batch,
        5 => SchedPolicy::Idle,
        6 => SchedPolicy::Deadline,
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

    #[test]
    fn unit_rel_len_cuts_at_innermost_unit() {
        let unit_rel = |path: &str| {
            let cg = crate::proc::cgroup::parse_cgroup_file(path.as_bytes()).expect("parse");
            unit_rel_len(&cg).map(|len| cg.rel[..len].to_string())
        };
        assert_eq!(
            unit_rel("0::/system.slice/docker.service/payload\n").as_deref(),
            Some("system.slice/docker.service")
        );
        assert_eq!(
            unit_rel(
                "0::/user.slice/user-1000.slice/user@1000.service/session.slice/pipewire.service\n"
            )
            .as_deref(),
            Some("user.slice/user-1000.slice/user@1000.service/session.slice/pipewire.service")
        );
        assert_eq!(unit_rel("0::/user.slice\n"), None);
        assert_eq!(unit_rel("0::/\n"), None);
    }
}
