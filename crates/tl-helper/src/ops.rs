//! Process operations as root: signals, nice, scheduling policy, affinity and I/O priority on any
//! user's process or thread (ADR-006, R-004, R-097).
//!
//! These are the same `tl-ops` functions the engine uses. The helper never trusts the engine's
//! identity check: it runs `open_verified` itself on every target, and the `tl-ops` function
//! verifies again before its syscall. Every value is checked here before any syscall. The
//! `tl-ops` functions log the pid and the result at `debug`; this module logs nothing more.
//!
//! Cgroup operations as root: freeze, thaw and kill a scope or service of any uid
//! (`run_cgroup_op`). The engine's `tl_ops::cgroup::open_validated` accepts only the user's own
//! `app.slice`, so the helper has its own rule (R-168 E29-OI-8) and then writes with the same
//! `tl-ops` cgroup helpers through `CgroupDir::from_validated_fd` (R-281). The helper does not
//! log (R-281).
//!
//! `Dump` as root (`run_dump`): gdb writes a core of any user's process into an fd the engine
//! opened and passed with the frame (R-145 items 3 and 4), so the file belongs to the user. The
//! helper never opens a path for the core. It checks that the fd is an empty regular file opened
//! for writing, because gdb reopens it through `/proc/<helper pid>/fd/<n>` as root.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use tl_model::actions::{DumpMode, IoPriority, SchedPolicy, Signal};
use tl_model::helper_proto::{
    HelperCgroup, HelperError, HelperReplyBody, HelperRequest, HelperTarget,
};
use tl_model::ids::ProcessKey;
use tl_model::system::CpuSet;
use tl_model::time::BootId;
use tl_ops::OpsError;
use tl_ops::{cgroup, dump, process};

/// Reads the boot id once at helper start. `None` when the file is missing or does not parse.
pub fn read_boot_id() -> Option<BootId> {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    BootId::from_uuid_text(&text)
}

/// The one change a process request asks for.
#[derive(Clone, Copy, Debug)]
enum Op<'a> {
    Signal(Signal),
    Nice(i8),
    Sched(SchedPolicy, u8),
    Affinity(&'a CpuSet),
    IoPriority(IoPriority),
}

/// Answers `Signal`, `SetNice`, `SetSched`, `SetAffinity` and `SetIoPriority`. Any other request
/// gets `Err(NotAllowed)`.
pub fn run_process_op(req: &HelperRequest, boot: BootId) -> HelperReplyBody {
    let (target, op) = match req {
        HelperRequest::Signal { target, signal } => (*target, Op::Signal(*signal)),
        HelperRequest::SetNice { target, nice } => (*target, Op::Nice(*nice)),
        HelperRequest::SetSched {
            target,
            policy,
            rt_prio,
        } => (*target, Op::Sched(*policy, *rt_prio)),
        HelperRequest::SetAffinity { target, set } => (*target, Op::Affinity(set)),
        HelperRequest::SetIoPriority { key, prio } => {
            (HelperTarget::Process(*key), Op::IoPriority(*prio))
        }
        // Rule "not a process operation".
        _ => return HelperReplyBody::Err(HelperError::NotAllowed),
    };
    match run_op(target, op, boot) {
        Ok(()) => HelperReplyBody::Ok,
        Err(e) => HelperReplyBody::Err(e),
    }
}

/// Runs one operation on one target. The checks run in this order, and each one that fails
/// returns before any syscall: values, thread target, pid 1. Then `open_verified`, then the
/// `tl-ops` call.
fn run_op(target: HelperTarget, op: Op<'_>, boot: BootId) -> Result<(), HelperError> {
    check_values(op)?;
    check_target(target, op)?;
    match target {
        HelperTarget::Process(key) => {
            drop(process::open_verified(key, boot).map_err(map_error)?);
            match op {
                Op::Signal(sig) => process::send_signal(key, boot, sig),
                Op::Nice(nice) => process::set_nice(key, boot, nice),
                Op::Sched(policy, rt_prio) => process::set_sched(key, boot, policy, rt_prio),
                Op::Affinity(set) => process::set_affinity(key, boot, set),
                Op::IoPriority(prio) => process::set_io_priority(key, boot, prio),
            }
        }
        HelperTarget::Thread { process: key, tid } => {
            drop(process::open_verified(key, boot).map_err(map_error)?);
            match op {
                Op::Signal(sig) => process::thread_send_signal(key, boot, tid, sig),
                Op::Nice(nice) => process::thread_set_nice(key, boot, tid, nice),
                Op::Sched(policy, rt_prio) => {
                    process::thread_set_sched(key, boot, tid, policy, rt_prio)
                }
                Op::Affinity(set) => process::thread_set_affinity(key, boot, tid, set),
                // Refused by `check_target`.
                Op::IoPriority(_) => return Err(HelperError::NotAllowed),
            }
        }
    }
    .map_err(map_error)
}

/// The value rules. Each failure is `NotAllowed`, before any syscall.
fn check_values(op: Op<'_>) -> Result<(), HelperError> {
    let ok = match op {
        // Rule "nice out of range".
        Op::Nice(nice) => (-20..=19).contains(&nice),
        // Rule "rt_prio out of range".
        Op::Sched(SchedPolicy::Fifo | SchedPolicy::Rr, rt_prio) => (1..=99).contains(&rt_prio),
        Op::Sched(SchedPolicy::Other | SchedPolicy::Batch | SchedPolicy::Idle, rt_prio) => {
            rt_prio == 0
        }
        // Rule "policy not settable".
        Op::Sched(SchedPolicy::Deadline, _) => false,
        // Rule "ioprio level out of range".
        Op::IoPriority(IoPriority::Realtime(level) | IoPriority::BestEffort(level)) => level <= 7,
        Op::IoPriority(IoPriority::None | IoPriority::Idle) => true,
        // Rule "signal out of range".
        Op::Signal(Signal::Other(n)) => (1..=64).contains(&n),
        Op::Signal(_) => true,
        // Rule "empty cpu set".
        Op::Affinity(set) => set.0.iter().any(|&word| word != 0),
    };
    if ok {
        Ok(())
    } else {
        Err(HelperError::NotAllowed)
    }
}

/// The target rules. Each failure is `NotAllowed`, before any syscall.
fn check_target(target: HelperTarget, op: Op<'_>) -> Result<(), HelperError> {
    let pid = match target {
        HelperTarget::Process(key) => key.pid,
        // Rule "thread target": I/O priority is set per process only.
        HelperTarget::Thread { .. } if matches!(op, Op::IoPriority(_)) => {
            return Err(HelperError::NotAllowed);
        }
        HelperTarget::Thread { process, .. } => process.pid,
    };
    // Rule "pid 1". The engine refuses this earlier; this is the helper's own guard.
    let ends_or_stops = matches!(
        op,
        Op::Signal(Signal::Kill | Signal::Term | Signal::Stop | Signal::Other(_))
    );
    if pid == 1 && ends_or_stops {
        return Err(HelperError::NotAllowed);
    }
    Ok(())
}

/// `OpsError` to the wire error. A process that exited during the change (`OutcomeUnknown`)
/// gives `Os(ESRCH)`, which the engine reports as "process exited during the change". The
/// rejections `tl-ops` makes on its own values give `NotAllowed`.
fn map_error(e: OpsError) -> HelperError {
    match e {
        OpsError::IdentityChanged => HelperError::IdentityChanged,
        OpsError::NoSuchProcess => HelperError::NoSuchProcess,
        OpsError::PermissionDenied => HelperError::PermissionDenied,
        OpsError::Partial { done, total } => HelperError::Partial { done, total },
        OpsError::Os(errno) => HelperError::Os(errno),
        OpsError::OutcomeUnknown(_) => HelperError::Os(Errno::SRCH.raw_os_error()),
        OpsError::InvalidInput(_)
        | OpsError::NotOwner
        | OpsError::Protected
        | OpsError::Unsupported(_) => HelperError::NotAllowed,
    }
}

/// Longest `HelperCgroup::rel_path` the helper accepts, in bytes.
const CGROUP_PATH_MAX: usize = 4096;

/// The one change a cgroup request asks for.
#[derive(Clone, Copy, Debug)]
enum GroupOp {
    Freeze,
    Thaw,
    Kill,
}

/// Answers `FreezeGroup`, `ThawGroup` and `KillGroup` for a cgroup of any uid (ADR-006,
/// R-168 E29-OI-8). Any other request gets `Err(NotAllowed)`.
///
/// `validate_cgroup` runs first and every write goes through the dirfd it returns. A freeze
/// waits for `frozen 1` like the engine does; when it does not arrive within `FREEZE_WAIT` the
/// reply is `Err(Os(EINPROGRESS))`, which the engine reports as "freeze still in progress".
pub fn run_cgroup_op(req: &HelperRequest) -> HelperReplyBody {
    let (cg, op) = match req {
        HelperRequest::FreezeGroup(cg) => (cg, GroupOp::Freeze),
        HelperRequest::ThawGroup(cg) => (cg, GroupOp::Thaw),
        HelperRequest::KillGroup(cg) => (cg, GroupOp::Kill),
        // Rule "not a cgroup operation".
        _ => return HelperReplyBody::Err(HelperError::NotAllowed),
    };
    let result = open_cgroup_root()
        .and_then(|root| validate_cgroup(cg, root.as_fd()))
        .and_then(|dir| write_group(dir, op));
    match result {
        Ok(()) => HelperReplyBody::Ok,
        Err(e) => HelperReplyBody::Err(e),
    }
}

fn open_cgroup_root() -> Result<OwnedFd, HelperError> {
    rustix::fs::open(
        cgroup::CGROUP_ROOT,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| HelperError::Os(e.raw_os_error()))
}

/// The one write for `op`, through the validated dirfd, with the `tl-ops` cgroup helpers.
fn write_group(dir: OwnedFd, op: GroupOp) -> Result<(), HelperError> {
    let dir = cgroup::CgroupDir::from_validated_fd(dir);
    match op {
        GroupOp::Freeze => cgroup::freeze(&dir)
            .and_then(|()| cgroup::wait_frozen(&dir, cgroup::FREEZE_WAIT, cgroup::FREEZE_POLL)),
        GroupOp::Thaw => cgroup::thaw(&dir),
        GroupOp::Kill => cgroup::kill(&dir),
    }
    .map_err(|e| match e {
        // Only `wait_frozen` gives `OutcomeUnknown` here.
        OpsError::OutcomeUnknown(_) => HelperError::Os(Errno::INPROGRESS.raw_os_error()),
        e => map_error(e),
    })
}

/// Checks `cg` and returns an `O_DIRECTORY` fd of it. The checks run in this order, and each one
/// that fails returns before any write: the path rules, the per-component open, the inode, then
/// the rule that tasklens and the helper are not inside the target.
fn validate_cgroup(cg: &HelperCgroup, root: BorrowedFd<'_>) -> Result<OwnedFd, HelperError> {
    check_cgroup_path(&cg.rel_path)?;
    let dir = open_components(root, &cg.rel_path)?;
    let st = rustix::fs::fstat(&dir).map_err(|e| HelperError::Os(e.raw_os_error()))?;
    #[allow(clippy::unnecessary_cast)] // `st_ino` is `c_ulong` on some targets.
    if st.st_ino as u64 != cg.cgroup_id {
        return Err(HelperError::IdentityChanged);
    }
    check_not_inside(&cg.rel_path, &protected_cgroups()?)?;
    Ok(dir)
}

/// The path rules. Pure, no I/O. Each failure is `NotAllowed`.
fn check_cgroup_path(rel: &str) -> Result<(), HelperError> {
    // Rule "bad cgroup path". Splitting on `/` gives an empty component for a leading or
    // trailing `/` and for `//`.
    if rel.is_empty()
        || rel.len() > CGROUP_PATH_MAX
        || rel.contains('\0')
        || rel
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(HelperError::NotAllowed);
    }
    // Rule "cgroup outside user.slice and system.slice".
    let first = rel.split('/').next().unwrap_or("");
    if first != "user.slice" && first != "system.slice" {
        return Err(HelperError::NotAllowed);
    }
    let last = rel.rsplit('/').next().unwrap_or("");
    // Rule "not a scope or service".
    if !last.ends_with(".scope") && !last.ends_with(".service") {
        return Err(HelperError::NotAllowed);
    }
    // Rule "init.scope": the system manager and every user manager live in one.
    if last == "init.scope" {
        return Err(HelperError::NotAllowed);
    }
    Ok(())
}

/// Opens `rel` one component at a time from `root`, with `O_NOFOLLOW`, never by a joined path.
/// A missing component gives `IdentityChanged` (the cgroup is gone). A symlink or a file gives
/// `NotAllowed` (rule "bad cgroup path").
fn open_components(root: BorrowedFd<'_>, rel: &str) -> Result<OwnedFd, HelperError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir: Option<OwnedFd> = None;
    for name in rel.split('/') {
        let at = dir.as_ref().map_or(root, |fd| fd.as_fd());
        let next = match rustix::fs::openat(at, name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Err(HelperError::IdentityChanged),
            Err(Errno::LOOP | Errno::NOTDIR) => return Err(HelperError::NotAllowed),
            Err(e) => return Err(HelperError::Os(e.raw_os_error())),
        };
        dir = Some(next);
    }
    dir.ok_or(HelperError::NotAllowed)
}

/// The cgroups of the helper and of its parent (tasklens), relative to the cgroup root. A cgroup
/// that cannot be read gives `NotAllowed`, so the rule fails closed.
fn protected_cgroups() -> Result<[String; 2], HelperError> {
    let parent = rustix::process::getppid().ok_or(HelperError::NotAllowed)?;
    Ok([
        read_cgroup("/proc/self/cgroup")?,
        read_cgroup(&format!("/proc/{}/cgroup", parent.as_raw_nonzero()))?,
    ])
}

/// The cgroup v2 path in a `/proc/<pid>/cgroup` file, without the leading `/`.
fn read_cgroup(path: &str) -> Result<String, HelperError> {
    let text = std::fs::read_to_string(path).map_err(|_| HelperError::NotAllowed)?;
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or(HelperError::NotAllowed)?;
    Ok(line.strip_prefix('/').unwrap_or(line).to_owned())
}

/// Rule "would affect tasklens": no protected cgroup is equal to `rel` or below it.
fn check_not_inside(rel: &str, protected: &[String]) -> Result<(), HelperError> {
    let inside = |path: &str| {
        path.strip_prefix(rel)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    if protected.iter().any(|path| inside(path)) {
        Err(HelperError::NotAllowed)
    } else {
        Ok(())
    }
}

/// Answers `Dump { key, mode }` with the fd that came with the frame. Blocks until gdb exits.
/// `out_fd` is closed when this returns.
///
/// The checks run in this order, and each one that fails returns before gdb runs:
/// `open_verified`, the fd, gdb installed. A gdb failure gives `Err(Os(EIO))`.
pub fn run_dump(key: ProcessKey, mode: DumpMode, out_fd: OwnedFd, boot: BootId) -> HelperReplyBody {
    match dump_into(key, mode, out_fd, boot) {
        Ok(()) => HelperReplyBody::Ok,
        Err(e) => HelperReplyBody::Err(e),
    }
}

fn dump_into(
    key: ProcessKey,
    mode: DumpMode,
    out_fd: OwnedFd,
    boot: BootId,
) -> Result<(), HelperError> {
    drop(process::open_verified(key, boot).map_err(map_error)?);
    check_dump_fd(out_fd.as_fd())?;
    // Rule "gdb not installed".
    if !dump::dump_available() {
        return Err(HelperError::NotAllowed);
    }
    dump::dump_to_fd(key, boot, mode, out_fd).map_err(map_error)
}

/// Rule "bad dump fd": the fd must be a regular file of size 0 opened for writing. Each failure,
/// and an fd that cannot be inspected, is `NotAllowed`.
fn check_dump_fd(fd: BorrowedFd<'_>) -> Result<(), HelperError> {
    let stat = rustix::fs::fstat(fd).map_err(|_| HelperError::NotAllowed)?;
    let regular =
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::RegularFile;
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|_| HelperError::NotAllowed)?;
    let access = flags & OFlags::RWMODE;
    let writable = access == OFlags::WRONLY || access == OFlags::RDWR;
    if regular && stat.st_size == 0 && writable {
        Ok(())
    } else {
        Err(HelperError::NotAllowed)
    }
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use tl_model::ids::ProcessKey;

    use super::*;

    const BOOT: BootId = BootId(0x5eed_0000_0000_0005);

    /// A child that is killed and reaped on drop, even when an assertion fails.
    struct ChildGuard(Child);

    impl ChildGuard {
        fn sleep() -> Self {
            ChildGuard(
                Command::new("sleep")
                    .arg("60")
                    .spawn()
                    .expect("spawn sleep"),
            )
        }

        /// This test binary with `TL_HELPER_TEST_THREADS=3`, after it printed `ready`.
        fn threaded() -> Self {
            let mut child = Command::new(std::env::current_exe().expect("current_exe"))
                .args(["--exact", "ops::tests::threaded_child_entry", "--nocapture"])
                .env("TL_HELPER_TEST_THREADS", "3")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn threaded child");
            let stderr = child.stderr.take().expect("child stderr");
            let guard = ChildGuard(child);
            let ready = std::io::BufReader::new(stderr)
                .lines()
                .any(|line| line.is_ok_and(|l| l.trim() == "ready"));
            assert!(ready, "threaded child never printed ready");
            guard
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }

        fn key(&self) -> ProcessKey {
            ProcessKey {
                start_ticks: process::read_start_ticks(self.pid()).expect("read start ticks"),
                boot_id: BOOT,
                pid: self.pid(),
            }
        }

        /// True while the child has not exited (a zombie counts as exited).
        fn alive(&mut self) -> bool {
            self.0.try_wait().expect("try_wait").is_none() && self.state() != b'Z'
        }

        fn state(&self) -> u8 {
            let stat = std::fs::read(format!("/proc/{}/stat", self.pid())).expect("read stat");
            let close = stat.iter().rposition(|&b| b == b')').expect("no )");
            stat[close + 2]
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// The child half of `set_nice_every_tid`. It returns at once unless `TL_HELPER_TEST_THREADS`
    /// is set; with it set, it starts that many threads, prints `ready` to stderr and sleeps 60 s.
    #[test]
    fn threaded_child_entry() {
        let Some(count) = std::env::var("TL_HELPER_TEST_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        else {
            return;
        };
        for _ in 0..count {
            std::thread::spawn(|| std::thread::sleep(Duration::from_secs(60)));
        }
        eprintln!("ready");
        std::thread::sleep(Duration::from_secs(60));
    }

    fn tids(pid: u32) -> Vec<u32> {
        std::fs::read_dir(format!("/proc/{pid}/task"))
            .expect("read task dir")
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect()
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

    /// 5, or one above the highest nice of `pid` when that is already 5 or more, capped at 19.
    /// Raising nice needs no privilege, so this works when a daemon already raised ours.
    fn raised_nice(pid: u32) -> i8 {
        let max = tids(pid)
            .into_iter()
            .map(|tid| tid_nice(pid, tid))
            .max()
            .expect("no threads");
        i8::try_from((max + 1).clamp(5, 19)).expect("nice fits i8")
    }

    /// One request of each process kind for `target`, with valid values. I/O priority takes the
    /// process key of `target`.
    fn every_kind(target: HelperTarget) -> Vec<HelperRequest> {
        let key = match target {
            HelperTarget::Process(key) | HelperTarget::Thread { process: key, .. } => key,
        };
        vec![
            HelperRequest::Signal {
                target,
                signal: Signal::Kill,
            },
            HelperRequest::SetNice { target, nice: 10 },
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Batch,
                rt_prio: 0,
            },
            HelperRequest::SetAffinity {
                target,
                set: CpuSet([1; 16]),
            },
            HelperRequest::SetIoPriority {
                key,
                prio: IoPriority::Idle,
            },
        ]
    }

    #[test]
    fn signal_verified_child() {
        let mut child = ChildGuard::sleep();
        let req = HelperRequest::Signal {
            target: HelperTarget::Process(child.key()),
            signal: Signal::Term,
        };
        assert_eq!(run_process_op(&req, BOOT), HelperReplyBody::Ok);
        let status = child.0.wait().expect("wait child");
        assert_eq!(status.signal(), Some(15));
    }

    /// Security regression: the helper runs its own `open_verified`, so a key whose start time
    /// does not match the live process is refused and the process is not touched.
    #[test]
    fn stale_start_ticks_rejected() {
        let mut child = ChildGuard::sleep();
        let mut key = child.key();
        key.start_ticks += 1;
        let targets = [
            HelperTarget::Process(key),
            HelperTarget::Thread {
                process: key,
                tid: key.pid,
            },
        ];
        for target in targets {
            for req in every_kind(target) {
                assert_eq!(
                    run_process_op(&req, BOOT),
                    HelperReplyBody::Err(HelperError::IdentityChanged),
                    "{req:?}"
                );
            }
        }
        assert!(child.alive(), "a stale key reached the child");
        assert_eq!(tid_nice(child.pid(), child.pid()), {
            let own = std::process::id();
            tid_nice(own, own)
        });
    }

    /// Security regression: a key from another boot never verifies, even with the right pid and
    /// start time.
    #[test]
    fn boot_id_mismatch_rejected() {
        let mut child = ChildGuard::sleep();
        let key = ProcessKey {
            boot_id: BootId(BOOT.0 + 1),
            ..child.key()
        };
        for req in every_kind(HelperTarget::Process(key)) {
            assert_eq!(
                run_process_op(&req, BOOT),
                HelperReplyBody::Err(HelperError::IdentityChanged),
                "{req:?}"
            );
        }
        assert!(child.alive(), "a key from another boot reached the child");
    }

    /// Every bad value is refused before `open_verified`. The key is stale, so a check that ran
    /// after `open_verified` would give `IdentityChanged` instead of `NotAllowed`.
    #[test]
    fn value_ranges_rejected_before_syscall() {
        let mut child = ChildGuard::sleep();
        let mut key = child.key();
        key.start_ticks += 1;
        let target = HelperTarget::Process(key);
        let bad = [
            HelperRequest::SetNice { target, nice: 25 },
            HelperRequest::SetNice { target, nice: -21 },
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Rr,
                rt_prio: 0,
            },
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Fifo,
                rt_prio: 100,
            },
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Other,
                rt_prio: 5,
            },
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Deadline,
                rt_prio: 0,
            },
            HelperRequest::SetIoPriority {
                key,
                prio: IoPriority::BestEffort(8),
            },
            HelperRequest::SetIoPriority {
                key,
                prio: IoPriority::Realtime(8),
            },
            HelperRequest::Signal {
                target,
                signal: Signal::Other(0),
            },
            HelperRequest::Signal {
                target,
                signal: Signal::Other(65),
            },
            HelperRequest::SetAffinity {
                target,
                set: CpuSet::empty(),
            },
        ];
        for req in bad {
            assert_eq!(
                run_process_op(&req, BOOT),
                HelperReplyBody::Err(HelperError::NotAllowed),
                "{req:?}"
            );
        }
        // The edges of each range pass the value check and reach `open_verified`.
        let edges = [
            HelperRequest::SetNice { target, nice: 19 },
            HelperRequest::SetNice { target, nice: -20 },
            HelperRequest::SetSched {
                target,
                policy: SchedPolicy::Fifo,
                rt_prio: 99,
            },
            HelperRequest::SetIoPriority {
                key,
                prio: IoPriority::BestEffort(7),
            },
            HelperRequest::Signal {
                target,
                signal: Signal::Other(64),
            },
        ];
        for req in edges {
            assert_eq!(
                run_process_op(&req, BOOT),
                HelperReplyBody::Err(HelperError::IdentityChanged),
                "{req:?}"
            );
        }
        assert!(child.alive());
    }

    /// Security regression: the helper never ends or stops PID 1, whatever the engine sends. The
    /// key is from another boot, so if the guard were missing `open_verified` would refuse it
    /// (`IdentityChanged`) before any syscall reached PID 1.
    #[test]
    fn pid1_guard() {
        let init = ProcessKey {
            start_ticks: 0,
            boot_id: BootId(BOOT.0 + 1),
            pid: 1,
        };
        let targets = [
            HelperTarget::Process(init),
            HelperTarget::Thread {
                process: init,
                tid: 1,
            },
        ];
        let signals = [
            Signal::Kill,
            Signal::Term,
            Signal::Stop,
            Signal::Other(9),
            Signal::Other(34),
        ];
        for target in targets {
            for signal in signals {
                let req = HelperRequest::Signal { target, signal };
                assert_eq!(
                    run_process_op(&req, BOOT),
                    HelperReplyBody::Err(HelperError::NotAllowed),
                    "{req:?}"
                );
            }
        }
        // Other signals to PID 1 are not this rule's business; they reach `open_verified`.
        let req = HelperRequest::Signal {
            target: HelperTarget::Process(init),
            signal: Signal::Cont,
        };
        assert_eq!(
            run_process_op(&req, BOOT),
            HelperReplyBody::Err(HelperError::IdentityChanged)
        );
    }

    #[test]
    fn not_a_process_operation() {
        let req = HelperRequest::FreezeGroup(tl_model::helper_proto::HelperCgroup {
            rel_path: "user.slice".into(),
            cgroup_id: 1,
        });
        assert_eq!(
            run_process_op(&req, BOOT),
            HelperReplyBody::Err(HelperError::NotAllowed)
        );
        let req = HelperRequest::Hello { proto: 1 };
        assert_eq!(
            run_process_op(&req, BOOT),
            HelperReplyBody::Err(HelperError::NotAllowed)
        );
    }

    /// I/O priority on a thread is refused (rule "thread target"). The wire request carries only
    /// a process key, so the rule is checked through `run_op`. Nice on a thread of the child
    /// works.
    #[test]
    fn thread_target_kinds() {
        let child = ChildGuard::sleep();
        let key = child.key();
        let thread = HelperTarget::Thread {
            process: key,
            tid: child.pid(),
        };
        assert_eq!(
            run_op(thread, Op::IoPriority(IoPriority::Idle), BOOT),
            Err(HelperError::NotAllowed)
        );
        let nice = raised_nice(child.pid());
        let req = HelperRequest::SetNice {
            target: thread,
            nice,
        };
        assert_eq!(run_process_op(&req, BOOT), HelperReplyBody::Ok);
        assert_eq!(tid_nice(child.pid(), child.pid()), i32::from(nice));
    }

    /// Nice applies to every TID: the main thread, the test harness thread and the 3 extra
    /// threads of the re-executed test binary.
    #[test]
    fn set_nice_every_tid() {
        let child = ChildGuard::threaded();
        let pid = child.pid();
        let before = tids(pid);
        assert!(before.len() >= 4, "only {} threads", before.len());
        let nice = raised_nice(pid);
        let req = HelperRequest::SetNice {
            target: HelperTarget::Process(child.key()),
            nice,
        };
        assert_eq!(run_process_op(&req, BOOT), HelperReplyBody::Ok);
        let values: Vec<i32> = before.iter().map(|&tid| tid_nice(pid, tid)).collect();
        assert!(
            values.iter().all(|&n| n == i32::from(nice)),
            "nice values {values:?}"
        );
    }

    #[test]
    fn error_mapping() {
        let table = [
            (OpsError::IdentityChanged, HelperError::IdentityChanged),
            (OpsError::NoSuchProcess, HelperError::NoSuchProcess),
            (OpsError::PermissionDenied, HelperError::PermissionDenied),
            (
                OpsError::Partial { done: 2, total: 3 },
                HelperError::Partial { done: 2, total: 3 },
            ),
            (OpsError::Os(22), HelperError::Os(22)),
            (OpsError::OutcomeUnknown("x"), HelperError::Os(3)),
            (OpsError::InvalidInput("x"), HelperError::NotAllowed),
        ];
        for (ops, helper) in table {
            assert_eq!(map_error(ops.clone()), helper, "{ops:?}");
        }
    }

    #[test]
    fn boot_id_reads() {
        assert!(read_boot_id().is_some());
    }

    fn cg(rel_path: &str, cgroup_id: u64) -> HelperCgroup {
        HelperCgroup {
            rel_path: rel_path.to_owned(),
            cgroup_id,
        }
    }

    /// A `HelperCgroup` for `rel_path` with the directory's current inode.
    fn live_cgroup(rel_path: &str) -> HelperCgroup {
        let meta = std::fs::metadata(format!("{}/{rel_path}", cgroup::CGROUP_ROOT))
            .expect("stat cgroup dir");
        cg(rel_path, meta.ino())
    }

    fn cgroup_of(pid: u32) -> Option<String> {
        read_cgroup(&format!("/proc/{pid}/cgroup")).ok()
    }

    fn cgroup_file(rel_path: &str, name: &str) -> Option<String> {
        std::fs::read_to_string(format!("{}/{rel_path}/{name}", cgroup::CGROUP_ROOT)).ok()
    }

    /// The `frozen` value in the cgroup's `cgroup.events`.
    fn events_frozen(rel_path: &str) -> Option<bool> {
        match cgroup_file(rel_path, "cgroup.events")?
            .lines()
            .find_map(|l| l.strip_prefix("frozen "))?
        {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }
    }

    fn wait_for(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < limit {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cond()
    }

    static SCOPE_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn scope_unit() -> String {
        let n = SCOPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("tasklens-helper-test-{}-{n}", std::process::id())
    }

    /// `systemd-run --user --scope` running `cmd` in the new unit `unit`. `systemd-run` moves
    /// itself into the scope and then execs `cmd`, so the child pid is the command's pid.
    fn spawn_in_scope(unit: &str, cmd: &[&str], env: &[(&str, &str)]) -> ChildGuard {
        let unit_arg = format!("--unit={unit}");
        let mut command = Command::new("systemd-run");
        command
            .args(["--user", "--scope", "--quiet", "--collect", &unit_arg])
            .args(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        ChildGuard(command.spawn().expect("spawn systemd-run"))
    }

    fn stop_unit(unit: &str) {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("{unit}.scope")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// A transient user scope running `sleep 60`, created by this test. On drop it kills the
    /// scope through `tl_ops::cgroup::kill_group`, reaps the child and stops the unit.
    struct ScopeGuard {
        child: ChildGuard,
        unit: String,
        cg: HelperCgroup,
    }

    impl ScopeGuard {
        fn start() -> Self {
            let unit = scope_unit();
            let child = spawn_in_scope(&unit, &["sleep", "60"], &[]);
            let suffix = format!("/{unit}.scope");
            let mut path = None;
            let found = wait_for(Duration::from_secs(2), || {
                path = cgroup_of(child.pid());
                path.as_deref().is_some_and(|p| p.ends_with(&suffix))
            });
            let mut guard = ScopeGuard {
                child,
                unit,
                cg: cg("", 0),
            };
            assert!(found, "the child did not move into {suffix}: {path:?}");
            guard.cg = live_cgroup(&path.expect("cgroup path"));
            guard
        }

        fn rel_path(&self) -> &str {
            &self.cg.rel_path
        }
    }

    impl Drop for ScopeGuard {
        fn drop(&mut self) {
            if self.cg.cgroup_id != 0 {
                let own = tl_model::actions::CgroupRef {
                    rel_path: self.cg.rel_path.as_str().into(),
                    cgroup_id: self.cg.cgroup_id,
                };
                let _ = cgroup::kill_group(&own, rustix::process::getuid().as_raw());
            }
            let _ = self.child.0.kill();
            let _ = self.child.0.wait();
            stop_unit(&self.unit);
        }
    }

    /// Security regression: the pure path rules (1 to 3 and `init.scope`) on hostile strings.
    /// None of these reaches a syscall.
    #[test]
    fn cgroup_path_rules() {
        let long = format!("user.slice/{}.scope", "a".repeat(CGROUP_PATH_MAX));
        let refused = [
            // Rule "bad cgroup path".
            "",
            "user.slice/../system.slice/x.service",
            "/user.slice/a.scope",
            "user.slice/a.scope/",
            "user.slice//a.scope",
            "user.slice/./a.scope",
            "user.slice/a\0b.scope",
            long.as_str(),
            // Rule "cgroup outside user.slice and system.slice".
            "machine.slice/x.scope",
            "init.slice/x.service",
            // Rule "not a scope or service".
            "user.slice/user-1000.slice",
            "system.slice",
            "system.slice/x.scope.d",
            // Rule "init.scope".
            "user.slice/user-1000.slice/user@1000.service/init.scope",
        ];
        for rel in refused {
            assert_eq!(
                check_cgroup_path(rel),
                Err(HelperError::NotAllowed),
                "{rel:?}"
            );
        }
        let allowed = [
            "system.slice/sshd.service",
            "user.slice/user-1001.slice/user@1001.service/app.slice/app-x@1.scope",
            "user.slice/user-1001.slice/session-3.scope",
        ];
        for rel in allowed {
            assert_eq!(check_cgroup_path(rel), Ok(()), "{rel:?}");
        }
        // The path rules run before any open: a bad path gives `NotAllowed` even with a
        // `cgroup_id` that could never match.
        let req = HelperRequest::KillGroup(cg("user.slice/../system.slice/x.service", 0));
        assert_eq!(
            run_cgroup_op(&req),
            HelperReplyBody::Err(HelperError::NotAllowed)
        );
    }

    /// A temp directory tree, removed on drop.
    struct TempTree(std::path::PathBuf);

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Security regression: the per-component open loop never follows a symlink, in the middle
    /// or at the leaf, and checks the inode of the final directory.
    #[test]
    fn cgroup_open_loop_no_follow() {
        let root = std::env::temp_dir().join(format!(
            "tl-helper-qfx6-{}-{}",
            std::process::id(),
            SCOPE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let tree = TempTree(root.clone());
        std::fs::create_dir_all(root.join("user.slice/real.slice/a.scope")).expect("mkdir");
        std::fs::write(root.join("user.slice/file.scope"), b"").expect("write file");
        std::os::unix::fs::symlink("real.slice", root.join("user.slice/link.slice"))
            .expect("symlink middle");
        std::os::unix::fs::symlink("real.slice/a.scope", root.join("user.slice/link.scope"))
            .expect("symlink leaf");
        let root_fd = std::fs::File::open(&tree.0).expect("open temp root");
        let ino = |rel: &str| std::fs::metadata(root.join(rel)).expect("stat").ino();

        let real = "user.slice/real.slice/a.scope";
        let fd = open_components(root_fd.as_fd(), real).expect("open real path");
        assert_eq!(
            rustix::fs::fstat(&fd).expect("fstat").st_ino as u64,
            ino(real)
        );

        // Rule "bad cgroup path": a symlink in the middle, a symlink leaf, a file.
        for rel in [
            "user.slice/link.slice/a.scope",
            "user.slice/link.scope",
            "user.slice/file.scope",
        ] {
            assert_eq!(
                open_components(root_fd.as_fd(), rel).map(drop),
                Err(HelperError::NotAllowed),
                "{rel:?}"
            );
            assert_eq!(
                validate_cgroup(&cg(rel, ino(real)), root_fd.as_fd()).map(drop),
                Err(HelperError::NotAllowed),
                "{rel:?}"
            );
        }
        // A missing component means the cgroup is gone.
        assert_eq!(
            open_components(root_fd.as_fd(), "user.slice/gone.slice/a.scope").map(drop),
            Err(HelperError::IdentityChanged)
        );
        // The inode check, before the tasklens rule.
        assert_eq!(
            validate_cgroup(&cg(real, ino(real) + 1), root_fd.as_fd()).map(drop),
            Err(HelperError::IdentityChanged)
        );
        assert_eq!(
            validate_cgroup(&cg(real, ino(real)), root_fd.as_fd()).map(drop),
            Ok(())
        );
    }

    /// Rule "would affect tasklens" compares whole components: equal or below is refused, a
    /// sibling with a common prefix is not. The second protected path stands for the parent.
    #[test]
    fn cgroup_not_inside_rule() {
        let protected = [
            "user.slice/user-1000.slice/user@1000.service/app.slice/helper.scope".to_owned(),
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-tasklens@1.service"
                .to_owned(),
        ];
        let refused = [
            "user.slice/user-1000.slice/user@1000.service/app.slice/helper.scope",
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-tasklens@1.service",
            "user.slice/user-1000.slice/user@1000.service",
        ];
        for rel in refused {
            assert_eq!(
                check_not_inside(rel, &protected),
                Err(HelperError::NotAllowed),
                "{rel:?}"
            );
        }
        let allowed = [
            "user.slice/user-1000.slice/user@1000.service/app.slice/helper.scope2",
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-tasklens@1.service.d",
            "user.slice/user-1000.slice/session-2.scope",
        ];
        for rel in allowed {
            assert_eq!(check_not_inside(rel, &protected), Ok(()), "{rel:?}");
        }
        assert_eq!(
            check_not_inside("user.slice/a.scope", &[String::new()]),
            Ok(())
        );
    }

    #[test]
    fn not_a_cgroup_operation() {
        let req = HelperRequest::Hello { proto: 1 };
        assert_eq!(
            run_cgroup_op(&req),
            HelperReplyBody::Err(HelperError::NotAllowed)
        );
    }

    #[test]
    #[ignore = "real system: run with --ignored on the owner machine"]
    fn cgroup_freeze_thaw_kill() {
        let mut scope = ScopeGuard::start();
        let freeze = HelperRequest::FreezeGroup(scope.cg.clone());
        assert_eq!(run_cgroup_op(&freeze), HelperReplyBody::Ok);
        assert_eq!(events_frozen(scope.rel_path()), Some(true));

        let thaw = HelperRequest::ThawGroup(scope.cg.clone());
        assert_eq!(run_cgroup_op(&thaw), HelperReplyBody::Ok);
        assert!(
            wait_for(Duration::from_secs(2), || events_frozen(scope.rel_path())
                == Some(false)),
            "frozen after thaw"
        );

        let kill = HelperRequest::KillGroup(scope.cg.clone());
        assert_eq!(run_cgroup_op(&kill), HelperReplyBody::Ok);
        let status = scope.child.0.wait().expect("wait child");
        assert_eq!(status.signal(), Some(9), "wait status {status:?}");
    }

    /// Security regression: a valid path with another `cgroup_id` is refused and nothing is
    /// written. The cgroup is a scope this test created.
    #[test]
    #[ignore = "real system: run with --ignored on the owner machine"]
    fn cgroup_inode_mismatch() {
        let mut scope = ScopeGuard::start();
        let stale = cg(scope.rel_path(), scope.cg.cgroup_id + 1);
        for req in [
            HelperRequest::FreezeGroup(stale.clone()),
            HelperRequest::ThawGroup(stale.clone()),
            HelperRequest::KillGroup(stale.clone()),
        ] {
            assert_eq!(
                run_cgroup_op(&req),
                HelperReplyBody::Err(HelperError::IdentityChanged),
                "{req:?}"
            );
        }
        assert_eq!(
            cgroup_file(scope.rel_path(), "cgroup.freeze").as_deref(),
            Some("0\n")
        );
        assert_eq!(events_frozen(scope.rel_path()), Some(false));
        assert!(scope.child.alive(), "a stale cgroup_id reached the scope");
    }

    /// The child half of `cgroup_self_refused`. It returns at once unless
    /// `TL_HELPER_TEST_SELF_SCOPE` is set. With it set, it checks that it runs in that scope,
    /// sends `KillGroup` for its own cgroup and exits 0 when the reply is `Err(NotAllowed)`, 1 for
    /// any other reply and 2 when it is not in the scope (then it sends nothing).
    #[test]
    fn self_scope_child_entry() {
        let Ok(unit) = std::env::var("TL_HELPER_TEST_SELF_SCOPE") else {
            return;
        };
        let own = cgroup_of(std::process::id()).unwrap_or_default();
        if !own.ends_with(&format!("/{unit}.scope")) {
            std::process::exit(2);
        }
        let req = HelperRequest::KillGroup(live_cgroup(&own));
        let code = if run_cgroup_op(&req) == HelperReplyBody::Err(HelperError::NotAllowed) {
            0
        } else {
            1
        };
        std::process::exit(code);
    }

    /// Security regression (rule "would affect tasklens"): `KillGroup` on the cgroup that holds
    /// the calling process is refused. The caller is this test binary re-executed inside a scope
    /// this test created, so a regression kills only that scope.
    #[test]
    #[ignore = "real system: run with --ignored on the owner machine"]
    fn cgroup_self_refused() {
        let unit = scope_unit();
        let exe = std::env::current_exe().expect("current_exe");
        let exe = exe.to_str().expect("exe path is UTF-8");
        let mut child = spawn_in_scope(
            &unit,
            &[
                exe,
                "--exact",
                "ops::tests::self_scope_child_entry",
                "--nocapture",
            ],
            &[("TL_HELPER_TEST_SELF_SCOPE", &unit)],
        );
        let status = child.0.wait().expect("wait child");
        stop_unit(&unit);
        assert_eq!(status.signal(), None, "the child was killed: {status:?}");
        assert_eq!(status.code(), Some(0), "child status {status:?}");
    }

    /// A file under the system temp dir, removed on drop.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(contents: &[u8]) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("tl-helper-dump-test-{}-{n}", std::process::id()));
            std::fs::write(&path, contents).expect("write temp file");
            TempFile(path)
        }

        fn open(&self, access: OFlags) -> OwnedFd {
            rustix::fs::open(&self.0, access | OFlags::CLOEXEC, Mode::empty()).expect("open temp")
        }

        fn read(&self) -> Vec<u8> {
            std::fs::read(&self.0).expect("read temp file")
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// True when gdb run by this test user may attach to a child of the test, as `tl-ops` decides
    /// it: root, or Yama `ptrace_scope` 0 or absent.
    fn ptrace_allows_child() -> bool {
        let scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
            .map(|text| text.trim().to_owned())
            .unwrap_or_else(|_| "0".to_owned());
        rustix::process::geteuid().is_root() || scope == "0"
    }

    fn coredump_filter(pid: u32) -> String {
        std::fs::read_to_string(format!("/proc/{pid}/coredump_filter")).expect("read filter")
    }

    #[test]
    fn dump_child_writes_elf() {
        if !dump::dump_available() || !ptrace_allows_child() {
            eprintln!("skipped: needs /usr/bin/gdb, /usr/bin/gcore and ptrace_scope 0");
            return;
        }
        let child = ChildGuard::sleep();
        let file = TempFile::new(b"");
        let reply = run_dump(child.key(), DumpMode::Mini, file.open(OFlags::WRONLY), BOOT);
        assert_eq!(reply, HelperReplyBody::Ok);
        assert!(file.read().starts_with(b"\x7fELF"), "not an ELF core");
    }

    #[test]
    fn dump_restores_coredump_filter() {
        let child = ChildGuard::sleep();
        let before = coredump_filter(child.pid());
        assert_ne!(
            before.trim(),
            "000001ff",
            "the test needs a filter other than 0x1ff"
        );
        let file = TempFile::new(b"");
        let reply = run_dump(child.key(), DumpMode::Full, file.open(OFlags::WRONLY), BOOT);
        if dump::dump_available() && ptrace_allows_child() {
            assert_eq!(reply, HelperReplyBody::Ok);
        } else {
            eprintln!("gdb did not run ({reply:?}); only the unchanged filter is checked");
        }
        assert_eq!(coredump_filter(child.pid()), before);
    }

    #[test]
    fn dump_rejects_bad_fd() {
        let child = ChildGuard::sleep();
        let not_allowed = HelperReplyBody::Err(HelperError::NotAllowed);

        // Rule "bad dump fd": read-only.
        let empty = TempFile::new(b"");
        let reply = run_dump(
            child.key(),
            DumpMode::Mini,
            empty.open(OFlags::RDONLY),
            BOOT,
        );
        assert_eq!(reply, not_allowed);
        assert!(empty.read().is_empty(), "gdb wrote into a read-only fd");

        // Rule "bad dump fd": not empty.
        let full = TempFile::new(b"keep me");
        let reply = run_dump(child.key(), DumpMode::Mini, full.open(OFlags::WRONLY), BOOT);
        assert_eq!(reply, not_allowed);
        assert_eq!(full.read(), b"keep me");

        // Rule "bad dump fd": a directory.
        let dir = rustix::fs::open(
            std::env::temp_dir(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open temp dir");
        assert_eq!(
            run_dump(child.key(), DumpMode::Mini, dir, BOOT),
            not_allowed
        );

        // Rule "bad dump fd": a pipe.
        let (_read, write) = rustix::pipe::pipe().expect("pipe");
        assert_eq!(
            run_dump(child.key(), DumpMode::Mini, write, BOOT),
            not_allowed
        );

        // Rule "bad dump fd": an `O_PATH` fd.
        let path_fd = empty.open(OFlags::PATH);
        assert_eq!(
            run_dump(child.key(), DumpMode::Mini, path_fd, BOOT),
            not_allowed
        );
    }

    #[test]
    fn dump_stale_key() {
        let child = ChildGuard::sleep();
        let mut key = child.key();
        key.start_ticks += 1;
        let file = TempFile::new(b"");
        let reply = run_dump(key, DumpMode::Full, file.open(OFlags::WRONLY), BOOT);
        assert_eq!(reply, HelperReplyBody::Err(HelperError::IdentityChanged));
        assert!(file.read().is_empty(), "gdb ran for a stale key");

        let mut key = child.key();
        key.boot_id = BootId(BOOT.0 + 1);
        let reply = run_dump(key, DumpMode::Mini, file.open(OFlags::WRONLY), BOOT);
        assert_eq!(reply, HelperReplyBody::Err(HelperError::IdentityChanged));
    }
}
