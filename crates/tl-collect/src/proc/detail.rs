//! Readers for the process detail window. One section per tab.

use std::fmt::Write as _;
use std::mem::MaybeUninit;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{AtFlags, Mode, OFlags, RawDir};
use tl_model::ids::ProcessKey;
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::CollectError;
use crate::counters::{Clock, resumed_between};
use crate::proc::meta::ReadBufs;
use crate::proc::pid_stat::{PidStat, parse_pid_stat, read_start_ticks};
use crate::proc::status::{PidStatus, STATUS_READ_MAX, parse_status};
use crate::proc::{is_denied, is_gone};
use crate::sanitize::{sanitize_comm, sanitize_text};
use crate::sysroot::{SMALL_FILE_LIMIT, SysRoot, read_at};

/// What a detail reader found at the process directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailRead {
    Ok,
    /// EACCES or EPERM on the process directory or its list.
    Restricted,
    /// The process exited, or its pid now belongs to another process.
    Gone,
}

/// Opens `/proc/<pid>` and checks that its start time is `key.start_ticks`. The directory fd
/// pins the process, so a pid reused after the check gives ENOENT or ESRCH, never another
/// process's files.
fn open_checked(
    root: &SysRoot,
    key: ProcessKey,
    buf: &mut Vec<u8>,
) -> Result<Result<OwnedFd, DetailRead>, CollectError> {
    let dir = match rustix::fs::open(
        root.proc.join(key.pid.to_string()),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(dir) => dir,
        Err(e) => return dir_error(e),
    };
    match read_start_ticks(dir.as_fd(), buf) {
        Ok(ticks) if ticks == key.start_ticks => Ok(Ok(dir)),
        Ok(_) | Err(CollectError::NotFound) => Ok(Err(DetailRead::Gone)),
        Err(CollectError::PermissionDenied) => Ok(Err(DetailRead::Restricted)),
        Err(e) => Err(e),
    }
}

/// Maps a directory error to a `DetailRead`, or to an error when it is neither gone nor denied.
fn dir_error<T>(e: rustix::io::Errno) -> Result<Result<T, DetailRead>, CollectError> {
    if is_gone(e) {
        Ok(Err(DetailRead::Gone))
    } else if is_denied(e) {
        Ok(Err(DetailRead::Restricted))
    } else {
        Err(e.into())
    }
}

// Threads tab (PROC-079).

/// getdents buffer for one `task/` listing.
const TASK_DIRENT_BUF: usize = 8 * 1024;
/// Read limit for `wchan`. Kernel symbol names are at most 512 bytes (KSYM_NAME_LEN).
const WCHAN_MAX: usize = 512;
/// Kept length of `Cpus_allowed_list`.
const CPUS_LIST_MAX: usize = 1024;

/// The `wchan` file of one thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WchanValue {
    /// The kernel function the thread waits in, sanitized.
    Name(Arc<str>),
    /// The file holds `0`: the thread is not waiting.
    Running,
    /// The file could not be read: another user's thread, or another read error.
    Restricted,
}

/// One row of the Threads tab. The engine (E10) maps it to the `tl-model` `ThreadRow`.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadInfo {
    pub tid: u32,
    /// Sanitized thread name.
    pub comm: Arc<str>,
    /// The raw stat state letter.
    pub state: u8,
    /// Share of all online CPUs, 0 to 100. 0 unless `cpu_validity` is `Valid`.
    pub cpu_pct: f32,
    pub utime_ms: u64,
    pub stime_ms: u64,
    pub policy: u32,
    pub rt_prio: u32,
    pub nice: i8,
    /// `Cpus_allowed_list`, for example `0-7,16-23`. Empty when `status` could not be read.
    pub cpus: Arc<str>,
    pub last_cpu: i32,
    pub vol_ctx: Option<u64>,
    pub invol_ctx: Option<u64>,
    pub wchan: WchanValue,
    pub cpu_validity: Validity,
}

/// Reads the threads of one process and keeps the CPU counters of each thread between reads.
pub struct ThreadReader {
    /// (CLOCK_BOOTTIME ns, utime + stime ticks) of each thread at its last read.
    prev: FxHashMap<(ProcessKey, u32), (TimeNs, u64)>,
    /// Both clocks at the last read of each process, for the suspend check (ADR-008).
    prev_clock: FxHashMap<ProcessKey, Clock>,
    dirents: Vec<MaybeUninit<u8>>,
    tids: Vec<u32>,
    seen: FxHashSet<u32>,
    name: String,
    stat: PidStat,
    status: PidStatus,
    clock: fn() -> (TimeNs, u64),
}

impl Default for ThreadReader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ThreadReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadReader")
            .field("baselines", &self.prev.len())
            .finish_non_exhaustive()
    }
}

impl ThreadReader {
    pub fn new() -> Self {
        Self {
            prev: FxHashMap::default(),
            prev_clock: FxHashMap::default(),
            dirents: vec![MaybeUninit::uninit(); TASK_DIRENT_BUF],
            tids: Vec::new(),
            seen: FxHashSet::default(),
            name: String::new(),
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

    /// Reads every thread of `key` into `out` (cleared first). A thread that exits during the
    /// read is skipped. `Gone` and `Restricted` leave `out` empty and drop the baselines of `key`.
    pub fn read(
        &mut self,
        root: &SysRoot,
        key: ProcessKey,
        ticks_per_sec: u64,
        online_cpus: u32,
        bufs: &mut ReadBufs,
        out: &mut Vec<ThreadInfo>,
    ) -> Result<DetailRead, CollectError> {
        out.clear();
        let listed = open_checked(root, key, &mut bufs.small).and_then(|dir| match dir {
            Ok(dir) => self.list_tids(dir.as_fd()).map(|r| r.map(|()| dir)),
            Err(r) => Ok(Err(r)),
        });
        let pid_dir = match listed {
            Ok(Ok(dir)) => dir,
            Ok(Err(r)) => {
                self.forget(key);
                return Ok(r);
            }
            Err(e) => {
                self.forget(key);
                return Err(e);
            }
        };

        let (t_ns, mono_ns) = (self.clock)();
        let now = Clock {
            boot_ns: t_ns,
            mono_ns,
        };
        let resumed = self
            .prev_clock
            .get(&key)
            .is_some_and(|&was| resumed_between(was, now));
        let ticks = ticks_per_sec.max(1);
        let cpus = f64::from(online_cpus.max(1));

        self.seen.clear();
        let tids = std::mem::take(&mut self.tids);
        for &tid in &tids {
            let Some(mut row) = self.read_one(pid_dir.as_fd(), tid, ticks, bufs) else {
                continue;
            };
            self.seen.insert(tid);
            let total = self.stat.utime.saturating_add(self.stat.stime);
            if let Some(&(old_t, old_total)) = self.prev.get(&(key, tid))
                && !resumed
                && t_ns > old_t
                && total >= old_total
            {
                let dt_s = (t_ns - old_t) as f64 / 1e9;
                row.cpu_pct =
                    (100.0 * (total - old_total) as f64 / ticks as f64 / dt_s / cpus) as f32;
                row.cpu_validity = Validity::Valid;
            }
            self.prev.insert((key, tid), (t_ns, total));
            out.push(row);
        }
        self.tids = tids;

        let seen = &self.seen;
        self.prev
            .retain(|(k, tid), _| *k != key || seen.contains(tid));
        self.prev_clock.insert(key, now);
        Ok(DetailRead::Ok)
    }

    /// Lists the numeric entries of `task/` into `self.tids`.
    fn list_tids(
        &mut self,
        pid_dir: BorrowedFd<'_>,
    ) -> Result<Result<(), DetailRead>, CollectError> {
        self.tids.clear();
        let task = match rustix::fs::openat(
            pid_dir,
            "task",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(e) => return dir_error(e),
        };
        let mut dir = RawDir::new(task.as_fd(), &mut self.dirents[..]);
        while let Some(entry) = dir.next() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => return dir_error(e),
            };
            if let Some(tid) = parse_tid(entry.file_name().to_bytes()) {
                self.tids.push(tid);
            }
        }
        self.tids.sort_unstable();
        Ok(Ok(()))
    }

    /// Reads one thread. `None` when it is gone or its `stat` cannot be read or parsed. The CPU
    /// fields are left for the caller.
    fn read_one(
        &mut self,
        pid_dir: BorrowedFd<'_>,
        tid: u32,
        ticks: u64,
        bufs: &mut ReadBufs,
    ) -> Option<ThreadInfo> {
        self.name.clear();
        let _ = write!(self.name, "task/{tid}");
        let dir = rustix::fs::openat(
            pid_dir,
            self.name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        let dir = dir.as_fd();

        let stat_read = read_at(dir, "stat", &mut bufs.small, SMALL_FILE_LIMIT)
            .and_then(|_| parse_pid_stat(&bufs.small, &mut self.stat));
        if let Err(e) = stat_read {
            tracing::trace!(tid, error = %e, "thread stat not read, thread skipped");
            return None;
        }
        sanitize_comm(&self.stat.comm_raw, &mut bufs.text);
        let comm = Arc::from(bufs.text.as_str());

        let status_ok = read_at(dir, "status", &mut bufs.small, STATUS_READ_MAX)
            .and_then(|_| parse_status(&bufs.small, &mut self.status))
            .is_ok();
        let (cpus, vol_ctx, invol_ctx) = if status_ok {
            sanitize_text(
                &self.status.cpus_allowed_list,
                CPUS_LIST_MAX,
                &mut bufs.text,
            );
            (
                Arc::from(bufs.text.as_str()),
                self.status.voluntary_ctxt_switches,
                self.status.nonvoluntary_ctxt_switches,
            )
        } else {
            (Arc::from(""), None, None)
        };

        let wchan = match read_at(dir, "wchan", &mut bufs.small, WCHAN_MAX) {
            Ok(_) => match bufs.small.trim_ascii() {
                b"0" => WchanValue::Running,
                raw => {
                    sanitize_text(raw, WCHAN_MAX, &mut bufs.text);
                    WchanValue::Name(Arc::from(bufs.text.as_str()))
                }
            },
            Err(CollectError::NotFound) => return None,
            Err(_) => WchanValue::Restricted,
        };

        Some(ThreadInfo {
            tid,
            comm,
            state: self.stat.state,
            cpu_pct: 0.0,
            utime_ms: ticks_to_ms(self.stat.utime, ticks),
            stime_ms: ticks_to_ms(self.stat.stime, ticks),
            policy: self.stat.policy,
            rt_prio: self.stat.rt_priority,
            nice: self.stat.nice,
            cpus,
            last_cpu: self.stat.processor,
            vol_ctx,
            invol_ctx,
            wchan,
            cpu_validity: Validity::WarmingUp,
        })
    }

    /// Drops every baseline of `key`.
    fn forget(&mut self, key: ProcessKey) {
        self.prev.retain(|(k, _), _| *k != key);
        self.prev_clock.remove(&key);
    }
}

/// True when `task/<tid>` exists under the open `/proc/<pid>` directory (R-097 thread target
/// check).
pub fn thread_exists(pid_dir: BorrowedFd<'_>, tid: u32) -> bool {
    rustix::fs::statat(pid_dir, format!("task/{tid}"), AtFlags::empty()).is_ok()
}

fn real_clock() -> (TimeNs, u64) {
    let c = Clock::now();
    (c.boot_ns, c.mono_ns)
}

/// `ticks × 1000 / ticks_per_sec`, without overflow.
fn ticks_to_ms(ticks: u64, ticks_per_sec: u64) -> u64 {
    u64::try_from(u128::from(ticks) * 1000 / u128::from(ticks_per_sec)).unwrap_or(u64::MAX)
}

/// A directory name made of ASCII digits only that fits a `u32`.
fn parse_tid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    name.iter().try_fold(0u32, |n, &d| {
        n.checked_mul(10)?.checked_add(u32::from(d - b'0'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tid_names() {
        assert_eq!(parse_tid(b"4211"), Some(4211));
        assert_eq!(parse_tid(b"."), None);
        assert_eq!(parse_tid(b".."), None);
        assert_eq!(parse_tid(b""), None);
        assert_eq!(parse_tid(b"99999999999"), None);
    }

    #[test]
    fn ticks_to_ms_rounds_down() {
        assert_eq!(ticks_to_ms(300, 100), 3000);
        assert_eq!(ticks_to_ms(1, 3), 333);
        assert_eq!(ticks_to_ms(u64::MAX, 1), u64::MAX);
    }
}
