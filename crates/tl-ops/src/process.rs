//! Verified process operations: signals, nice, scheduling, affinity and I/O priority.
//!
//! Every operation first proves the target is still the captured process (ADR-006). It opens a
//! pidfd, which pins the process, then re-reads the start time and compares it with the
//! `ProcessKey`. Signals then go through that pidfd, so they can never hit a reused PID. There is
//! no fallback to `kill(pid)`.

use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::process::{Pid, PidfdFlags};
use tl_model::actions::{IoPriority, SchedPolicy, Signal};
use tl_model::{ids::ProcessKey, system::CpuSet, time::BootId};

use crate::error::{OpsError, OpsResult};
use crate::sys;

/// Opens a pidfd for `key` and proves it is the same process: same boot, same pid and the same
/// start time read after the pidfd is open. Any mismatch, or a process that is gone, gives
/// `IdentityChanged`.
pub fn open_verified(key: ProcessKey, boot: BootId) -> OpsResult<OwnedFd> {
    let result = open_verified_inner(key, boot);
    tracing::debug!(pid = key.pid, ok = result.is_ok(), error = ?result.as_ref().err(), "open_verified");
    result
}

fn open_verified_inner(key: ProcessKey, boot: BootId) -> OpsResult<OwnedFd> {
    if key.boot_id != boot || key.pid == 0 {
        return Err(OpsError::IdentityChanged);
    }
    let Some(pid) = i32::try_from(key.pid).ok().and_then(Pid::from_raw) else {
        return Err(OpsError::IdentityChanged);
    };
    let pidfd = match rustix::process::pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(Errno::SRCH) => return Err(OpsError::IdentityChanged),
        Err(e) => return Err(e.into()),
    };
    match read_start_ticks(key.pid) {
        Ok(ticks) if ticks == key.start_ticks => Ok(pidfd),
        Ok(_) | Err(OpsError::Os(libc::ENOENT)) | Err(OpsError::NoSuchProcess) => {
            Err(OpsError::IdentityChanged)
        }
        Err(e) => Err(e),
    }
}

/// Reads the start time (clock ticks since boot) from `/proc/<pid>/stat`. A line that does not
/// parse gives `Os(EIO)`.
pub fn read_start_ticks(pid: u32) -> OpsResult<u64> {
    let path = format!("/proc/{pid}/stat");
    let fd = rustix::fs::open(
        path.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut buf = [0u8; 4096];
    let n = rustix::io::read(&fd, &mut buf)?;
    parse_start_ticks(&buf[..n]).ok_or(OpsError::Os(libc::EIO))
}

/// Field 22 (`starttime`) of a `/proc/<pid>/stat` line. The name in field 2 may hold spaces and
/// `)`, so the scan starts after the last `)`.
pub(crate) fn parse_start_ticks(stat: &[u8]) -> Option<u64> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    let rest = stat.get(close + 1..)?.strip_prefix(b" ")?;
    let token = rest.split(|&b| b == b' ').nth(19)?;
    let token = token.strip_suffix(b"\n").unwrap_or(token);
    if token.is_empty() || !token.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(token).ok()?.parse().ok()
}

/// True when the process behind `pidfd` has exited (zombie or reaped). Does not block.
pub(crate) fn has_exited(pidfd: &OwnedFd) -> OpsResult<bool> {
    let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
    let zero = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    rustix::event::poll(&mut fds, Some(&zero))?;
    Ok(fds[0].revents().contains(PollFlags::IN))
}

/// The x86_64 signal number. `Other(n)` must be 1..=64.
pub fn signal_number(sig: Signal) -> OpsResult<i32> {
    match sig {
        Signal::Other(n) if !(1..=64).contains(&n) => {
            Err(OpsError::InvalidInput("signal number out of range"))
        }
        sig => Ok(sig.number()),
    }
}

/// Sends `sig` to the verified process through its pidfd.
pub fn send_signal(key: ProcessKey, boot: BootId, sig: Signal) -> OpsResult {
    let number = signal_number(sig)?;
    let pidfd = open_verified(key, boot)?;
    let result = sys::pidfd_send_signal_raw(pidfd.as_fd(), number).map_err(OpsError::from);
    tracing::debug!(pid = key.pid, signal = number, ok = result.is_ok(), error = ?result.as_ref().err(), "send_signal");
    result
}

/// Stops the process with SIGSTOP.
pub fn suspend(key: ProcessKey, boot: BootId) -> OpsResult {
    send_signal(key, boot, Signal::Stop)
}

/// Continues the process with SIGCONT.
pub fn resume(key: ProcessKey, boot: BootId) -> OpsResult {
    send_signal(key, boot, Signal::Cont)
}

/// The most targets one tree operation takes (ADR-006). The engine rejects more first.
const MAX_TREE_TARGETS: usize = 256;

/// Ends every process in `keys` (R-037). It opens a verified pidfd for each target, sends SIGSTOP
/// to all of them, then SIGKILL to all of them, so no target can fork a new child between passes.
/// Returns one result per key, in input order.
pub fn kill_tree(keys: &[ProcessKey], boot: BootId) -> Vec<OpsResult> {
    kill_tree_with(keys, boot, &mut sys::pidfd_send_signal_raw)
}

/// `kill_tree` with the signal call passed in, so tests can record the order of signals.
pub(crate) fn kill_tree_with(
    keys: &[ProcessKey],
    boot: BootId,
    send: &mut dyn FnMut(BorrowedFd<'_>, i32) -> Result<(), Errno>,
) -> Vec<OpsResult> {
    tracing::debug!(count = keys.len(), "kill_tree");
    if keys.len() > MAX_TREE_TARGETS {
        return vec![Err(OpsError::InvalidInput("too many targets")); keys.len()];
    }
    // Pass 0: a key that does not verify keeps its open error and gets no signal.
    let pidfds: Vec<OpsResult<OwnedFd>> =
        keys.iter().map(|&key| open_verified(key, boot)).collect();
    let mut results: Vec<OpsResult> = pidfds
        .iter()
        .map(|pidfd| pidfd.as_ref().map(|_| ()).map_err(Clone::clone))
        .collect();
    // Pass 1 stops every target, pass 2 kills every target, even one whose SIGSTOP failed. Each
    // target keeps its first error.
    for sig in [libc::SIGSTOP, libc::SIGKILL] {
        for (pidfd, result) in pidfds.iter().zip(results.iter_mut()) {
            let Ok(pidfd) = pidfd else { continue };
            if let Err(errno) = send(pidfd.as_fd(), sig)
                && result.is_ok()
            {
                *result = Err(errno.into());
            }
        }
    }
    drop(pidfds);
    for (key, result) in keys.iter().zip(&results) {
        tracing::debug!(pid = key.pid, ok = result.is_ok(), error = ?result.as_ref().err(), "kill_tree target");
    }
    results
}

/// The thread ids of `pid` from `/proc/<pid>/task`, sorted ascending. Names that are not a
/// number are skipped. A process that is gone gives `NoSuchProcess`.
pub(crate) fn list_tids(pid: u32) -> OpsResult<Vec<u32>> {
    let path = format!("/proc/{pid}/task");
    let dir = match rustix::fs::open(
        path.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Err(OpsError::NoSuchProcess),
        Err(e) => return Err(e.into()),
    };
    let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
    let mut entries = rustix::fs::RawDir::new(&dir, &mut buf);
    let mut tids = Vec::new();
    while let Some(entry) = entries.next() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(Errno::NOENT) => return Err(OpsError::NoSuchProcess),
            Err(e) => return Err(e.into()),
        };
        if let Some(tid) = entry
            .file_name()
            .to_str()
            .ok()
            .and_then(|name| name.parse().ok())
        {
            tids.push(tid);
        }
    }
    tids.sort_unstable();
    Ok(tids)
}

/// Runs `op` on every thread of the verified process (ADR-006). Nice and affinity take a PID, not
/// a pidfd, so the pidfd is held open for the whole loop and polled after it: a live process
/// cannot have its PID reused, so a pidfd that is still not readable proves every call hit the
/// target. A thread that exits mid-loop (`ESRCH`) is skipped and not counted.
pub(crate) fn for_each_tid(
    key: ProcessKey,
    boot: BootId,
    op: &mut dyn FnMut(u32) -> Result<(), Errno>,
) -> OpsResult {
    let pidfd = open_verified(key, boot)?;
    let tids = list_tids(key.pid)?;
    let (mut done, mut total) = (0usize, 0usize);
    let mut first_err = None;
    for &tid in &tids {
        match op(tid) {
            Err(Errno::SRCH) => continue,
            Ok(()) => done += 1,
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
        total += 1;
    }
    let result = if has_exited(&pidfd)? || total == 0 {
        Err(OpsError::OutcomeUnknown("process exited during the change"))
    } else if done == total {
        Ok(())
    } else if done == 0 {
        Err(first_err.map_or(OpsError::Os(libc::EIO), OpsError::from))
    } else {
        Err(OpsError::Partial {
            done: u16::try_from(done).unwrap_or(u16::MAX),
            total: u16::try_from(total).unwrap_or(u16::MAX),
        })
    };
    tracing::debug!(pid = key.pid, tids = tids.len(), ok = result.is_ok(), error = ?result.as_ref().err(), "for_each_tid");
    result
}

/// A thread id as a rustix `Pid`. A tid that does not fit gives `ESRCH`, so the loop skips it;
/// it must never become `None`, which means the calling process.
fn tid_pid(tid: u32) -> Result<Pid, Errno> {
    i32::try_from(tid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(Errno::SRCH)
}

/// Sets the nice value of every thread of the verified process. `nice` must be -20..=19.
pub fn set_nice(key: ProcessKey, boot: BootId, nice: i8) -> OpsResult {
    if !(-20..=19).contains(&nice) {
        return Err(OpsError::InvalidInput("nice out of range"));
    }
    for_each_tid(key, boot, &mut |tid| {
        rustix::process::setpriority_process(Some(tid_pid(tid)?), i32::from(nice))
    })
}

/// Sets the CPU affinity of every thread of the verified process. A mask outside the CPUs the
/// cgroup allows gives `Os(EINVAL)`.
pub fn set_affinity(key: ProcessKey, boot: BootId, set: &CpuSet) -> OpsResult {
    let set = to_rustix_cpuset(set)?;
    for_each_tid(key, boot, &mut |tid| {
        rustix::thread::sched_setaffinity(Some(tid_pid(tid)?), &set)
    })
}

/// Bit `i` of word `w` is CPU `w * 64 + i`. An empty set gives `InvalidInput`.
pub(crate) fn to_rustix_cpuset(set: &CpuSet) -> OpsResult<rustix::thread::CpuSet> {
    let mut out = rustix::thread::CpuSet::new();
    let mut any = false;
    for (w, &word) in set.0.iter().enumerate() {
        for i in 0..64 {
            if word & (1 << i) != 0 {
                let cpu = w * 64 + i;
                if cpu >= rustix::thread::CpuSet::MAX_CPU {
                    return Err(OpsError::InvalidInput("CPU number out of range"));
                }
                out.set(cpu);
                any = true;
            }
        }
    }
    if !any {
        return Err(OpsError::InvalidInput("empty CPU set"));
    }
    Ok(out)
}

/// Sets the scheduling policy of every thread of the verified process. `rt_prio` is 1..=99 for
/// `Fifo` and `Rr`, else 0. Each thread keeps its own nice value.
pub fn set_sched(key: ProcessKey, boot: BootId, policy: SchedPolicy, rt_prio: u8) -> OpsResult {
    let base = sched_attr_for(policy, rt_prio, 0)?;
    let result = for_each_tid(key, boot, &mut |tid| {
        let nice = rustix::process::getpriority_process(Some(tid_pid(tid)?))?;
        // The values were checked above, so this only fills in the nice value.
        let attr = sched_attr_for(policy, rt_prio, nice).unwrap_or(base);
        sys::sched_setattr_tid(tid, &attr)
    });
    tracing::debug!(pid = key.pid, policy = ?policy, rt_prio, ok = result.is_ok(), error = ?result.as_ref().err(), "set_sched");
    result
}

/// The kernel `sched_attr` for a policy. Every `Fifo` and `Rr` request sets
/// `SCHED_FLAG_RESET_ON_FORK` (R-165), so a child of a real-time process starts as `SCHED_OTHER`.
/// The other policies carry `current_nice`, because `sched_setattr` would otherwise reset it.
pub(crate) fn sched_attr_for(
    policy: SchedPolicy,
    rt_prio: u8,
    current_nice: i32,
) -> OpsResult<sys::SchedAttr> {
    let base = sys::SchedAttr {
        size: sys::SCHED_ATTR_SIZE_VER0,
        ..sys::SchedAttr::default()
    };
    match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            if !(1..=99).contains(&rt_prio) {
                return Err(OpsError::InvalidInput("rt priority out of range"));
            }
            let sched_policy = if policy == SchedPolicy::Fifo {
                sys::SCHED_FIFO
            } else {
                sys::SCHED_RR
            };
            Ok(sys::SchedAttr {
                sched_policy,
                sched_flags: sys::SCHED_FLAG_RESET_ON_FORK,
                sched_priority: u32::from(rt_prio),
                ..base
            })
        }
        SchedPolicy::Other | SchedPolicy::Batch | SchedPolicy::Idle => {
            if rt_prio != 0 {
                return Err(OpsError::InvalidInput("rt priority must be 0"));
            }
            let sched_policy = match policy {
                SchedPolicy::Other => sys::SCHED_OTHER,
                SchedPolicy::Batch => sys::SCHED_BATCH,
                _ => sys::SCHED_IDLE,
            };
            Ok(sys::SchedAttr {
                sched_policy,
                sched_nice: current_nice,
                ..base
            })
        }
    }
}

/// Sets the I/O priority of every thread of the verified process. `Realtime` needs
/// `CAP_SYS_ADMIN` and gives `PermissionDenied` without it.
pub fn set_io_priority(key: ProcessKey, boot: BootId, prio: IoPriority) -> OpsResult {
    let value = ioprio_for(prio)?;
    let result = for_each_tid(key, boot, &mut |tid| sys::ioprio_set_tid(tid, value));
    tracing::debug!(pid = key.pid, ioprio = value, ok = result.is_ok(), error = ?result.as_ref().err(), "set_io_priority");
    result
}

/// The kernel ioprio value. Levels are 0..=7.
pub(crate) fn ioprio_for(prio: IoPriority) -> OpsResult<u16> {
    let (class, level) = match prio {
        IoPriority::None => (sys::IOPRIO_CLASS_NONE, 0),
        IoPriority::Realtime(level) => (sys::IOPRIO_CLASS_RT, level),
        IoPriority::BestEffort(level) => (sys::IOPRIO_CLASS_BE, level),
        IoPriority::Idle => (sys::IOPRIO_CLASS_IDLE, 0),
    };
    if level > 7 {
        return Err(OpsError::InvalidInput("I/O priority level out of range"));
    }
    Ok(sys::ioprio_value(class, u16::from(level)))
}

/// Current I/O priority of the main thread, after the identity check (R-144 item 1). A thread
/// with no I/O class set (`IOPRIO_CLASS_NONE`) reads as the kernel's default, best effort at
/// level `(nice + 20) / 5`. The pidfd is polled after the reads, as in `for_each_tid`, so a
/// process that exited in between gives `NoSuchProcess`, never another process's value.
pub fn io_priority_of(key: ProcessKey, boot: BootId) -> OpsResult<IoPriority> {
    let result = io_priority_of_inner(key, boot);
    tracing::debug!(pid = key.pid, ok = result.is_ok(), error = ?result.as_ref().err(), "io_priority_of");
    result
}

fn io_priority_of_inner(key: ProcessKey, boot: BootId) -> OpsResult<IoPriority> {
    let pidfd = open_verified(key, boot)?;
    let value = sys::ioprio_get_tid(key.pid)?;
    let class = value >> sys::IOPRIO_CLASS_SHIFT;
    // The level is in bits 0 to 2, so it always fits in a u8.
    let level = (value & 0x7) as u8;
    let prio = match class {
        sys::IOPRIO_CLASS_NONE => {
            let nice = rustix::process::getpriority_process(Some(tid_pid(key.pid)?))?;
            let level = u8::try_from((nice.clamp(-20, 19) + 20) / 5).unwrap_or(4);
            IoPriority::BestEffort(level)
        }
        sys::IOPRIO_CLASS_RT => IoPriority::Realtime(level),
        sys::IOPRIO_CLASS_BE => IoPriority::BestEffort(level),
        sys::IOPRIO_CLASS_IDLE => IoPriority::Idle,
        _ => return Err(OpsError::Os(libc::EIO)),
    };
    if has_exited(&pidfd)? {
        return Err(OpsError::NoSuchProcess);
    }
    Ok(prio)
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, ExitStatus};
    use std::sync::MutexGuard;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::sys::tests::lock_children;

    const BOOT: BootId = BootId(0x1234_5678_9abc_def0);

    /// A `sleep 60` child that is killed and reaped on drop, even when an assertion fails.
    struct ChildGuard {
        child: Child,
        _lock: MutexGuard<'static, ()>,
    }

    impl ChildGuard {
        fn sleep() -> Self {
            let lock = lock_children();
            let child = Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn sleep");
            ChildGuard { child, _lock: lock }
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn key(&self) -> ProcessKey {
            ProcessKey {
                start_ticks: read_start_ticks(self.pid()).expect("read start ticks"),
                boot_id: BOOT,
                pid: self.pid(),
            }
        }

        fn wait(&mut self) -> ExitStatus {
            self.child.wait().expect("wait child")
        }

        fn state(&self) -> u8 {
            let stat = std::fs::read(format!("/proc/{}/stat", self.pid())).expect("read stat");
            let close = stat.iter().rposition(|&b| b == b')').expect("no )");
            stat[close + 2]
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Polls `cond` every 5 ms for up to 5 s.
    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn parse_start_ticks_name_with_spaces_and_parens() {
        let line =
            b"1234 (a b) c) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0 98765 1000 100";
        assert_eq!(parse_start_ticks(line), Some(98765));
        let with_newline = b"1 (x) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 42\n";
        assert_eq!(parse_start_ticks(with_newline), Some(42));
    }

    #[test]
    fn parse_start_ticks_truncated_is_none() {
        // Cut after field 20 (`num_threads`).
        let line = b"1234 (a b) c) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1";
        assert_eq!(parse_start_ticks(line), None);
        assert_eq!(parse_start_ticks(b"1234 (abc"), None);
        assert_eq!(parse_start_ticks(b""), None);
    }

    #[test]
    fn parse_start_ticks_non_numeric_is_none() {
        let line = b"1234 (a) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0 98x65 1000";
        assert_eq!(parse_start_ticks(line), None);
        let negative = b"1234 (a) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0 -5 1000";
        assert_eq!(parse_start_ticks(negative), None);
        let double_space = b"1234 (a) S 1 1234 1234 0 -1 4194304 10 0 0 0 5 6 0 0 20 0 1 0  98765";
        assert_eq!(parse_start_ticks(double_space), None);
    }

    #[test]
    fn boot_mismatch_is_identity_changed() {
        // Our own process: if the boot check ran after pidfd_open, this key would verify.
        let pid = std::process::id();
        let key = ProcessKey {
            start_ticks: read_start_ticks(pid).expect("read own start ticks"),
            boot_id: BootId(1),
            pid,
        };
        assert_eq!(
            open_verified(key, BootId(2)).err(),
            Some(OpsError::IdentityChanged)
        );
        assert_eq!(
            open_verified(ProcessKey { pid: 0, ..key }, BootId(1)).err(),
            Some(OpsError::IdentityChanged)
        );
    }

    #[test]
    fn open_verified_true_key_opens() {
        let child = ChildGuard::sleep();
        let pidfd = open_verified(child.key(), BOOT).expect("open_verified");
        assert_eq!(has_exited(&pidfd), Ok(false));
    }

    /// Regression test for PID reuse protection: a key whose start time does not match the live
    /// process must never verify.
    #[test]
    fn open_verified_shifted_start_ticks_is_identity_changed() {
        let child = ChildGuard::sleep();
        let mut key = child.key();
        key.start_ticks += 1;
        assert_eq!(
            open_verified(key, BOOT).err(),
            Some(OpsError::IdentityChanged)
        );
        assert_eq!(
            send_signal(key, BOOT, Signal::Kill),
            Err(OpsError::IdentityChanged)
        );
        assert_ne!(child.state(), b'Z', "the signal reached the child");
    }

    #[test]
    fn open_verified_gone_pid_is_identity_changed() {
        let mut child = ChildGuard::sleep();
        let key = child.key();
        child.child.kill().expect("kill child");
        child.wait();
        assert_eq!(
            open_verified(key, BOOT).err(),
            Some(OpsError::IdentityChanged)
        );
    }

    #[test]
    fn signal_numbers_table() {
        let table = [
            (Signal::Term, 15),
            (Signal::Kill, 9),
            (Signal::Hup, 1),
            (Signal::Int, 2),
            (Signal::Quit, 3),
            (Signal::Stop, 19),
            (Signal::Cont, 18),
            (Signal::Usr1, 10),
            (Signal::Usr2, 12),
            (Signal::Other(1), 1),
            (Signal::Other(34), 34),
            (Signal::Other(64), 64),
        ];
        for (sig, number) in table {
            assert_eq!(signal_number(sig), Ok(number), "{sig:?}");
        }
        for n in [0, 65, 255] {
            assert_eq!(
                signal_number(Signal::Other(n)),
                Err(OpsError::InvalidInput("signal number out of range"))
            );
        }
    }

    #[test]
    fn send_term_ends_child() {
        let mut child = ChildGuard::sleep();
        assert_eq!(send_signal(child.key(), BOOT, Signal::Term), Ok(()));
        assert_eq!(child.wait().signal(), Some(15));
    }

    #[test]
    fn suspend_resume_changes_state() {
        let child = ChildGuard::sleep();
        let key = child.key();
        assert_eq!(suspend(key, BOOT), Ok(()));
        assert!(wait_for(|| child.state() == b'T'), "child did not stop");
        assert_eq!(resume(key, BOOT), Ok(()));
        assert!(wait_for(|| child.state() != b'T'), "child did not resume");
    }

    #[test]
    fn realtime_signal_34_delivered() {
        let mut child = ChildGuard::sleep();
        assert_eq!(send_signal(child.key(), BOOT, Signal::Other(34)), Ok(()));
        assert_eq!(child.wait().signal(), Some(34));
    }

    #[test]
    fn has_exited_tracks_child() {
        let child = ChildGuard::sleep();
        let pidfd = open_verified(child.key(), BOOT).expect("open_verified");
        assert_eq!(has_exited(&pidfd), Ok(false));
        sys::pidfd_send_signal_raw(pidfd.as_fd(), libc::SIGKILL).expect("kill child");
        // The child is a zombie until the guard reaps it on drop.
        assert!(
            wait_for(|| has_exited(&pidfd) == Ok(true)),
            "pidfd never became readable"
        );
        assert_eq!(child.state(), b'Z');
    }

    /// Several children under one hold of the crate-wide child lock. Every child is killed and
    /// reaped on drop.
    struct Children {
        children: Vec<Child>,
        _lock: MutexGuard<'static, ()>,
    }

    impl Children {
        fn spawn(commands: &mut [&mut Command]) -> Self {
            let lock = lock_children();
            let children = commands
                .iter_mut()
                .map(|command| command.spawn().expect("spawn child"))
                .collect();
            Children {
                children,
                _lock: lock,
            }
        }

        fn sleeps(n: usize) -> Self {
            let mut commands: Vec<Command> = (0..n)
                .map(|_| {
                    let mut command = Command::new("sleep");
                    command.arg("60");
                    command
                })
                .collect();
            Self::spawn(&mut commands.iter_mut().collect::<Vec<_>>())
        }

        fn pid(&self, i: usize) -> u32 {
            self.children[i].id()
        }

        fn key(&self, i: usize) -> ProcessKey {
            key_of(self.pid(i))
        }
    }

    impl Drop for Children {
        fn drop(&mut self) {
            for child in &mut self.children {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn key_of(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: read_start_ticks(pid).expect("read start ticks"),
            boot_id: BOOT,
            pid,
        }
    }

    /// The fields after the name in `/proc/<pid>/stat`, or `None` when the process is gone.
    fn stat_fields(pid: u32) -> Option<Vec<String>> {
        let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.iter().rposition(|&b| b == b')')?;
        let rest = String::from_utf8_lossy(&stat[close + 1..]).into_owned();
        Some(rest.split_whitespace().map(str::to_owned).collect())
    }

    /// The pid behind a pidfd, from the `Pid:` line of `/proc/self/fdinfo/<fd>`.
    fn pidfd_pid(pidfd: BorrowedFd<'_>) -> u32 {
        use std::os::fd::AsRawFd;
        let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd()))
            .expect("read fdinfo");
        info.lines()
            .find_map(|line| line.strip_prefix("Pid:"))
            .expect("no Pid line")
            .trim()
            .parse()
            .expect("parse Pid")
    }

    /// Regression test for R-037: every SIGSTOP goes out before any SIGKILL.
    #[test]
    fn kill_tree_stops_all_before_killing_any() {
        let children = Children::sleeps(3);
        let keys: Vec<ProcessKey> = (0..3).map(|i| children.key(i)).collect();
        let mut log = Vec::new();
        let mut send = |pidfd: BorrowedFd<'_>, sig: i32| {
            log.push((pidfd_pid(pidfd), sig));
            sys::pidfd_send_signal_raw(pidfd, sig)
        };
        let results = kill_tree_with(&keys, BOOT, &mut send);
        assert_eq!(results, vec![Ok(()), Ok(()), Ok(())]);
        let (a, b, c) = (children.pid(0), children.pid(1), children.pid(2));
        assert_eq!(log, vec![(a, 19), (b, 19), (c, 19), (a, 9), (b, 9), (c, 9)]);
    }

    #[test]
    fn kill_tree_ends_parent_and_children() {
        let mut parent = Command::new("sh");
        parent.args(["-c", "sleep 60 & sleep 60 & wait"]);
        let mut children = Children::spawn(&mut [&mut parent]);
        let parent_pid = children.pid(0);
        let mut sleeps = Vec::new();
        let found = wait_for(|| {
            sleeps = std::fs::read_dir("/proc")
                .expect("read /proc")
                .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
                .filter(|&pid| {
                    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"));
                    stat_fields(pid).is_some_and(|f| f.get(1) == Some(&parent_pid.to_string()))
                        && comm.is_ok_and(|c| c.trim() == "sleep")
                })
                .collect();
            sleeps.len() == 2
        });
        assert!(found, "sh did not start two sleep children");
        let mut keys = vec![key_of(parent_pid)];
        keys.extend(sleeps.iter().map(|&pid| key_of(pid)));

        assert_eq!(kill_tree(&keys, BOOT), vec![Ok(()), Ok(()), Ok(())]);
        let status = children.children[0].wait().expect("wait parent");
        assert_eq!(status.signal(), Some(9));
        // The sleeps are reparented to init or a subreaper, which reaps them.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2)
            && sleeps.iter().any(|&pid| stat_fields(pid).is_some())
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        for pid in sleeps {
            assert!(stat_fields(pid).is_none(), "sleep {pid} still in /proc");
        }
    }

    #[test]
    fn kill_tree_skips_changed_identity() {
        let children = Children::sleeps(3);
        let mut shifted = children.key(1);
        shifted.start_ticks += 1;
        let keys = [children.key(0), shifted, children.key(2)];
        assert_eq!(
            kill_tree(&keys, BOOT),
            vec![Ok(()), Err(OpsError::IdentityChanged), Ok(())]
        );
        let state = stat_fields(children.pid(1)).expect("X is gone")[0].clone();
        assert!(
            state != "Z" && state != "T",
            "X got a signal: state {state}"
        );
    }

    #[test]
    fn kill_tree_over_256_is_invalid() {
        let key = key_of(std::process::id());
        let keys = vec![key; 257];
        let mut calls = 0;
        let mut send = |_: BorrowedFd<'_>, _: i32| {
            calls += 1;
            Ok(())
        };
        let results = kill_tree_with(&keys, BOOT, &mut send);
        assert_eq!(results.len(), 257);
        assert!(
            results
                .iter()
                .all(|r| *r == Err(OpsError::InvalidInput("too many targets")))
        );
        assert_eq!(calls, 0);
    }

    #[test]
    fn kill_tree_empty() {
        assert_eq!(kill_tree(&[], BOOT), Vec::<OpsResult>::new());
    }

    /// The child half of the threaded test child. It returns at once unless
    /// `TL_OPS_THREADED_CHILD=1`; with it set, it starts 2 extra threads, prints `ready` to stderr
    /// and sleeps
    /// 60 s.
    #[test]
    fn threaded_child_entry() {
        if std::env::var_os("TL_OPS_THREADED_CHILD").is_none_or(|v| v != "1") {
            return;
        }
        for _ in 0..2 {
            std::thread::spawn(|| std::thread::sleep(Duration::from_secs(60)));
        }
        eprintln!("ready");
        std::thread::sleep(Duration::from_secs(60));
    }

    /// Starts this test binary as a child with at least 3 threads and waits until they all run.
    fn threaded_child() -> Children {
        use std::io::BufRead;
        let mut command = Command::new(std::env::current_exe().expect("current_exe"));
        command
            .args([
                "--exact",
                "process::tests::threaded_child_entry",
                "--nocapture",
            ])
            .env("TL_OPS_THREADED_CHILD", "1")
            .stderr(std::process::Stdio::piped());
        let mut children = Children::spawn(&mut [&mut command]);
        let stderr = children.children[0].stderr.take().expect("child stderr");
        let ready = std::io::BufReader::new(stderr)
            .lines()
            .any(|line| line.is_ok_and(|l| l.trim() == "ready"));
        assert!(ready, "threaded child never printed ready");
        children
    }

    /// Field 19 (`nice`) of `/proc/<pid>/task/<tid>/stat`.
    fn tid_nice(pid: u32, tid: u32) -> i32 {
        let stat = std::fs::read(format!("/proc/{pid}/task/{tid}/stat")).expect("read task stat");
        let close = stat.iter().rposition(|&b| b == b')').expect("no )");
        let rest = String::from_utf8_lossy(&stat[close + 1..]).into_owned();
        // Field 3 (state) is the first field after the name.
        rest.split_whitespace()
            .nth(19 - 3)
            .expect("no nice field")
            .parse()
            .expect("parse nice")
    }

    fn all_nice(pid: u32) -> Vec<i32> {
        let tids = list_tids(pid).expect("list_tids");
        tids.iter().map(|&tid| tid_nice(pid, tid)).collect()
    }

    /// 5, or one above the highest nice of `pid` when that is already 5 or more, capped at 19.
    /// Raising nice needs no privilege, so this works under any RLIMIT_NICE.
    fn raised_nice(pid: u32) -> i8 {
        let max = all_nice(pid).into_iter().max().expect("no threads");
        i8::try_from((max + 1).clamp(5, 19)).expect("nice fits i8")
    }

    #[test]
    fn set_nice_applies_to_every_tid() {
        let children = threaded_child();
        let pid = children.pid(0);
        assert!(list_tids(pid).expect("list_tids").len() >= 3);
        let target = raised_nice(pid);
        assert_eq!(set_nice(children.key(0), BOOT, target), Ok(()));
        let nice = all_nice(pid);
        assert!(
            nice.iter().all(|&n| n == i32::from(target)),
            "nice values {nice:?}"
        );
    }

    #[test]
    fn lowering_nice_without_cap_is_permission_denied() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root, which may lower nice");
            return;
        }
        let children = threaded_child();
        let pid = children.pid(0);
        // RLIMIT_NICE above 20 lets an unprivileged process lower nice; take that away.
        let zero = rustix::process::Rlimit {
            current: Some(0),
            maximum: Some(0),
        };
        rustix::process::prlimit(
            Pid::from_raw(pid as i32),
            rustix::process::Resource::Nice,
            zero,
        )
        .expect("prlimit");
        // The child inherits our nice, which a daemon such as ananicy may have raised above 5.
        let high = raised_nice(pid);
        assert_eq!(set_nice(children.key(0), BOOT, high), Ok(()));
        assert_eq!(
            set_nice(children.key(0), BOOT, high - 5),
            Err(OpsError::PermissionDenied)
        );
        let nice = all_nice(pid);
        assert!(
            nice.iter().all(|&n| n == i32::from(high)),
            "nice values {nice:?}"
        );
    }

    #[test]
    fn nice_out_of_range_is_invalid() {
        // A key that does not verify: if the range check ran later, the error would differ.
        let key = ProcessKey {
            start_ticks: 0,
            boot_id: BOOT,
            pid: 0,
        };
        for nice in [20, -21, i8::MAX, i8::MIN] {
            assert_eq!(
                set_nice(key, BOOT, nice),
                Err(OpsError::InvalidInput("nice out of range"))
            );
        }
    }

    #[test]
    fn set_affinity_cpu0_every_tid() {
        let children = threaded_child();
        let pid = children.pid(0);
        let mut set = CpuSet::empty();
        set.insert(0);
        assert_eq!(set_affinity(children.key(0), BOOT, &set), Ok(()));
        for tid in list_tids(pid).expect("list_tids") {
            let got = rustix::thread::sched_getaffinity(Pid::from_raw(tid as i32))
                .expect("sched_getaffinity");
            assert_eq!(got.count(), 1, "tid {tid}");
            assert!(got.is_set(0), "tid {tid}");
        }
    }

    #[test]
    fn empty_cpuset_is_invalid() {
        let child = ChildGuard::sleep();
        assert_eq!(
            set_affinity(child.key(), BOOT, &CpuSet::empty()),
            Err(OpsError::InvalidInput("empty CPU set"))
        );
    }

    #[test]
    fn to_rustix_cpuset_maps_bits() {
        let mut set = CpuSet::empty();
        for cpu in [0, 63, 64, 130, 1023] {
            set.insert(cpu);
        }
        let out = to_rustix_cpuset(&set).expect("to_rustix_cpuset");
        assert_eq!(out.count(), 5);
        for cpu in [0, 63, 64, 130, 1023] {
            assert!(out.is_set(cpu), "cpu {cpu}");
        }
    }

    #[test]
    fn for_each_tid_partial() {
        let children = threaded_child();
        let mut calls = 0;
        // Ok for the first TID, EPERM for the second; the rest act as vanished threads.
        let mut op = |_: u32| {
            calls += 1;
            match calls {
                1 => Ok(()),
                2 => Err(Errno::PERM),
                _ => Err(Errno::SRCH),
            }
        };
        assert_eq!(
            for_each_tid(children.key(0), BOOT, &mut op),
            Err(OpsError::Partial { done: 1, total: 2 })
        );
    }

    #[test]
    fn for_each_tid_all_failed_is_first_error() {
        let children = threaded_child();
        let mut op = |_: u32| Err(Errno::PERM);
        assert_eq!(
            for_each_tid(children.key(0), BOOT, &mut op),
            Err(OpsError::PermissionDenied)
        );
    }

    #[test]
    fn for_each_tid_skips_vanished_tid() {
        let children = threaded_child();
        let mut calls = 0;
        let mut op = |_: u32| {
            calls += 1;
            if calls == 2 { Err(Errno::SRCH) } else { Ok(()) }
        };
        assert_eq!(for_each_tid(children.key(0), BOOT, &mut op), Ok(()));
        assert!(calls >= 3);
    }

    /// Regression test for ADR-006: a target that exits during a PID-based change must never
    /// report success.
    #[test]
    fn for_each_tid_exit_during_change_is_outcome_unknown() {
        let children = threaded_child();
        let key = children.key(0);
        let pidfd = open_verified(key, BOOT).expect("open_verified");
        let mut killed = false;
        let mut op = |_: u32| {
            if !killed {
                sys::pidfd_send_signal_raw(pidfd.as_fd(), libc::SIGKILL).expect("kill child");
                assert!(
                    wait_for(|| has_exited(&pidfd) == Ok(true)),
                    "pidfd never became readable"
                );
                killed = true;
            }
            Ok(())
        };
        assert_eq!(
            for_each_tid(key, BOOT, &mut op),
            Err(OpsError::OutcomeUnknown("process exited during the change"))
        );
        assert!(killed);
    }

    #[test]
    fn set_nice_shifted_key_is_identity_changed() {
        let child = ChildGuard::sleep();
        let before = all_nice(child.pid());
        let mut key = child.key();
        key.start_ticks += 1;
        let target = if before[0] == 5 { 6 } else { 5 };
        assert_eq!(set_nice(key, BOOT, target), Err(OpsError::IdentityChanged));
        assert_eq!(all_nice(child.pid()), before);
    }

    #[test]
    fn list_tids_gone_pid_is_no_such_process() {
        let mut child = ChildGuard::sleep();
        let pid = child.pid();
        child.child.kill().expect("kill child");
        child.wait();
        assert_eq!(list_tids(pid), Err(OpsError::NoSuchProcess));
    }

    #[test]
    fn sched_attr_rt_has_reset_on_fork() {
        let attr = sched_attr_for(SchedPolicy::Rr, 1, 7).expect("sched_attr_for");
        assert_eq!(attr.sched_policy, 2);
        assert_eq!(attr.sched_priority, 1);
        assert_eq!(attr.sched_flags, 0x01);
        assert_eq!(attr.size, 48);
        assert_eq!(attr.sched_nice, 0);
        assert_eq!(
            (attr.sched_runtime, attr.sched_deadline, attr.sched_period),
            (0, 0, 0)
        );
        let fifo = sched_attr_for(SchedPolicy::Fifo, 99, 0).expect("sched_attr_for");
        assert_eq!((fifo.sched_policy, fifo.sched_priority), (1, 99));
        assert_eq!(fifo.sched_flags, 0x01);
    }

    #[test]
    fn sched_attr_rejects_bad_rt_prio() {
        let out_of_range = Err(OpsError::InvalidInput("rt priority out of range"));
        assert_eq!(sched_attr_for(SchedPolicy::Fifo, 0, 0), out_of_range);
        assert_eq!(sched_attr_for(SchedPolicy::Rr, 100, 0), out_of_range);
        let must_be_zero = Err(OpsError::InvalidInput("rt priority must be 0"));
        assert_eq!(sched_attr_for(SchedPolicy::Batch, 5, 0), must_be_zero);
        // A key that does not verify: if the check ran after open_verified, the error would differ.
        let key = ProcessKey {
            start_ticks: 0,
            boot_id: BOOT,
            pid: 0,
        };
        assert_eq!(set_sched(key, BOOT, SchedPolicy::Rr, 100), out_of_range);
        assert_eq!(set_sched(key, BOOT, SchedPolicy::Batch, 5), must_be_zero);
    }

    #[test]
    fn sched_attr_non_rt_keeps_nice() {
        let attr = sched_attr_for(SchedPolicy::Idle, 0, 7).expect("sched_attr_for");
        assert_eq!(attr.sched_policy, 5);
        assert_eq!(attr.sched_priority, 0);
        assert_eq!(attr.sched_flags, 0);
        assert_eq!(attr.sched_nice, 7);
        assert_eq!(attr.size, 48);
        for (policy, number) in [(SchedPolicy::Other, 0), (SchedPolicy::Batch, 3)] {
            let attr = sched_attr_for(policy, 0, -3).expect("sched_attr_for");
            assert_eq!((attr.sched_policy, attr.sched_nice), (number, -3));
        }
    }

    /// Field 41 (`policy`) of `/proc/<pid>/task/<tid>/stat` for every thread.
    fn all_policies(pid: u32) -> Vec<u32> {
        let tids = list_tids(pid).expect("list_tids");
        tids.iter()
            .map(|&tid| {
                let stat =
                    std::fs::read(format!("/proc/{pid}/task/{tid}/stat")).expect("read task stat");
                let close = stat.iter().rposition(|&b| b == b')').expect("no )");
                let rest = String::from_utf8_lossy(&stat[close + 1..]).into_owned();
                rest.split_whitespace()
                    .nth(41 - 3)
                    .expect("no policy field")
                    .parse()
                    .expect("parse policy")
            })
            .collect()
    }

    fn all_ioprio(pid: u32) -> Vec<u16> {
        let tids = list_tids(pid).expect("list_tids");
        tids.iter()
            .map(|&tid| sys::ioprio_get_tid(tid).expect("ioprio_get"))
            .collect()
    }

    #[test]
    fn set_sched_batch_keeps_nice_every_tid() {
        let children = threaded_child();
        let pid = children.pid(0);
        // 5, or higher when the child starts above 5 (a daemon such as ananicy may raise it).
        let target = raised_nice(pid);
        assert_eq!(set_nice(children.key(0), BOOT, target), Ok(()));
        assert_eq!(
            set_sched(children.key(0), BOOT, SchedPolicy::Batch, 0),
            Ok(())
        );
        let policies = all_policies(pid);
        assert!(policies.iter().all(|&p| p == 3), "policies {policies:?}");
        let nice = all_nice(pid);
        assert!(
            nice.iter().all(|&n| n == i32::from(target)),
            "nice values {nice:?}"
        );
    }

    #[test]
    fn set_sched_rr_without_cap_is_permission_denied() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root, which may set a real-time policy");
            return;
        }
        let children = threaded_child();
        let pid = children.pid(0);
        // RLIMIT_RTPRIO above 0 lets an unprivileged process pick a real-time policy; take that
        // away.
        let zero = rustix::process::Rlimit {
            current: Some(0),
            maximum: Some(0),
        };
        rustix::process::prlimit(
            Pid::from_raw(pid as i32),
            rustix::process::Resource::Rtprio,
            zero,
        )
        .expect("prlimit");
        let before = all_policies(pid);
        assert_eq!(
            set_sched(children.key(0), BOOT, SchedPolicy::Rr, 1),
            Err(OpsError::PermissionDenied)
        );
        assert_eq!(all_policies(pid), before);
    }

    #[test]
    fn ioprio_for_table() {
        let table = [
            (IoPriority::None, 0),
            (IoPriority::BestEffort(0), 0x4000),
            (IoPriority::BestEffort(4), 0x4004),
            (IoPriority::BestEffort(7), 0x4007),
            (IoPriority::Realtime(0), 0x2000),
            (IoPriority::Realtime(7), 0x2007),
            (IoPriority::Idle, 0x6000),
        ];
        for (prio, value) in table {
            assert_eq!(ioprio_for(prio), Ok(value), "{prio:?}");
        }
    }

    #[test]
    fn ioprio_level_out_of_range_is_invalid() {
        let invalid = Err(OpsError::InvalidInput("I/O priority level out of range"));
        assert_eq!(ioprio_for(IoPriority::BestEffort(8)), invalid);
        assert_eq!(ioprio_for(IoPriority::Realtime(u8::MAX)), invalid);
        let key = ProcessKey {
            start_ticks: 0,
            boot_id: BOOT,
            pid: 0,
        };
        assert_eq!(
            set_io_priority(key, BOOT, IoPriority::BestEffort(8)),
            invalid
        );
    }

    #[test]
    fn set_io_priority_best_effort_7_every_tid() {
        let children = threaded_child();
        let pid = children.pid(0);
        assert_eq!(
            set_io_priority(children.key(0), BOOT, IoPriority::BestEffort(7)),
            Ok(())
        );
        let values = all_ioprio(pid);
        assert!(values.iter().all(|&v| v == 0x4007), "ioprio {values:x?}");
    }

    #[test]
    fn set_io_priority_realtime_needs_admin() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: running as root, which may set real-time I/O");
            return;
        }
        let children = threaded_child();
        let pid = children.pid(0);
        let before = all_ioprio(pid);
        assert_eq!(
            set_io_priority(children.key(0), BOOT, IoPriority::Realtime(0)),
            Err(OpsError::PermissionDenied)
        );
        assert_eq!(all_ioprio(pid), before);
    }

    #[test]
    fn io_priority_of_reads_back() {
        let children = threaded_child();
        let key = children.key(0);
        let pid = children.pid(0);
        // The child inherits our I/O class, which a daemon such as ananicy may have set. Clear it
        // so the child reads as one that never set one.
        if sys::ioprio_get_tid(pid).expect("ioprio_get") >> sys::IOPRIO_CLASS_SHIFT != 0 {
            assert_eq!(set_io_priority(key, BOOT, IoPriority::None), Ok(()));
        }
        // The kernel default follows nice: level 4 at nice 0. The child may not start at 0.
        let nice =
            rustix::process::getpriority_process(Pid::from_raw(pid as i32)).expect("getpriority");
        let level = u8::try_from((nice + 20) / 5).expect("level fits u8");
        assert_eq!(io_priority_of(key, BOOT), Ok(IoPriority::BestEffort(level)));
        if nice == 0 {
            assert_eq!(level, 4);
        }

        assert_eq!(
            set_io_priority(key, BOOT, IoPriority::BestEffort(7)),
            Ok(())
        );
        assert_eq!(io_priority_of(key, BOOT), Ok(IoPriority::BestEffort(7)));
        assert_eq!(set_io_priority(key, BOOT, IoPriority::Idle), Ok(()));
        assert_eq!(io_priority_of(key, BOOT), Ok(IoPriority::Idle));
    }

    #[test]
    fn sched_and_ioprio_shifted_key_is_identity_changed() {
        let child = ChildGuard::sleep();
        let pid = child.pid();
        let policies = all_policies(pid);
        let ioprio = all_ioprio(pid);
        let mut key = child.key();
        key.start_ticks += 1;
        assert_eq!(
            set_sched(key, BOOT, SchedPolicy::Batch, 0),
            Err(OpsError::IdentityChanged)
        );
        assert_eq!(
            set_io_priority(key, BOOT, IoPriority::BestEffort(7)),
            Err(OpsError::IdentityChanged)
        );
        assert_eq!(io_priority_of(key, BOOT), Err(OpsError::IdentityChanged));
        assert_eq!(all_policies(pid), policies);
        assert_eq!(all_ioprio(pid), ioprio);
    }
}
