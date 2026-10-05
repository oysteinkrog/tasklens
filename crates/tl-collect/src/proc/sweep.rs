//! One sweep over all processes in /proc (PROC-002, PROC-029, PROC-030, PROC-031, ADR-008,
//! ADR-016).
//!
//! Each pid directory is opened once as an `O_DIRECTORY` fd, its files are read relative to it,
//! and it is closed again. Rates follow the ADR-008 counter rules: no rate on the first sample,
//! after a counter reset or across a suspend. Nothing here logs above `trace` per process.

use std::mem::MaybeUninit;

use rustc_hash::FxHashMap;
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags, RawDir};
use tl_model::ids::ProcessKey;
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::{FieldValidity, ProcessField, Validity};

use crate::counters::{Clock, resumed_between};
use crate::proc::fdcount::{FdCount, fd_count};
use crate::proc::io::{IoRead, PidIo, read_io};
use crate::proc::meta::ReadBufs;
use crate::proc::pid_stat::{PidStat, parse_pid_stat};
use crate::proc::statm::{PidStatm, parse_statm};
use crate::proc::status::{PidStatus, STATUS_READ_MAX, parse_status};
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
