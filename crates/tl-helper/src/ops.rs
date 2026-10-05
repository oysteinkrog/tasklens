//! Process operations as root: signals, nice, scheduling policy, affinity and I/O priority on any
//! user's process or thread (ADR-006, R-004, R-097).
//!
//! These are the same `tl-ops` functions the engine uses. The helper never trusts the engine's
//! identity check: it runs `open_verified` itself on every target, and the `tl-ops` function
//! verifies again before its syscall. Every value is checked here before any syscall. The
//! `tl-ops` functions log the pid and the result at `debug`; this module logs nothing more.

use rustix::io::Errno;
use tl_model::actions::{IoPriority, SchedPolicy, Signal};
use tl_model::helper_proto::{HelperError, HelperReplyBody, HelperRequest, HelperTarget};
use tl_model::system::CpuSet;
use tl_model::time::BootId;
use tl_ops::OpsError;
use tl_ops::process;

/// Reads the boot id once at helper start. `None` when the file is missing or does not parse.
#[cfg_attr(not(test), expect(dead_code, reason = "dispatch arrives with E29-A1"))]
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
#[cfg_attr(not(test), expect(dead_code, reason = "dispatch arrives with E29-A1"))]
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

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, Stdio};
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
}
