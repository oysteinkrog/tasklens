//! Process actions through the verified pidfd (ADR-006, E11-003). Every signal first opens a
//! pidfd and proves it is the captured process, then signals through that pidfd, so it cannot
//! hit a reused PID.

use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use tl_model::actions::{
    ActionKind, ActionState, ActionTarget, IoPriority, RejectReason, SchedPolicy, Signal,
};
use tl_model::ids::ProcessKey;
use tl_model::system::CpuSet;
use tl_model::time::BootId;
use tl_ops::OpsError;

use super::{
    ActionCtx, ActionOps, TIMED_OUT, deadline, internal_error, map_ops_error, with_deadline,
};

/// Runs one `Process` or `Thread` target. A kind this module does not handle yet is
/// `Rejected(Unsupported)`. App rows arrive here already expanded to their primary processes.
///
/// This path is unprivileged: `run_body` refuses privileged requests until E29-014 routes them
/// through the helper. So `SetIoPriority(Realtime(_))`, which needs root, is refused here as well
/// as in the UI.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
) -> ActionState {
    let key = match target {
        ActionTarget::Process(key) => *key,
        ActionTarget::Thread { process, tid } => {
            return run_thread(cx, actx, kind, *process, *tid).await;
        }
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    let op = match kind {
        ActionKind::Signal(sig) => return signal(cx, actx, kind, target, key, *sig).await,
        ActionKind::Suspend => return signal(cx, actx, kind, target, key, Signal::Stop).await,
        ActionKind::Resume => return signal(cx, actx, kind, target, key, Signal::Cont).await,
        ActionKind::SetNice(n) => PidOp::Nice(*n),
        ActionKind::SetAffinity(set) => PidOp::Affinity(*set),
        ActionKind::SetSched { policy, rt_prio } => PidOp::Sched {
            policy: *policy,
            rt_prio: *rt_prio,
        },
        ActionKind::SetIoPriority(prio) => PidOp::IoPriority(*prio),
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    pid_based(cx, actx, kind, key, op).await
}

/// A change that takes a PID, not a pidfd, so it runs under the ADR-006 TID loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PidOp {
    Nice(i8),
    Affinity(CpuSet),
    Sched { policy: SchedPolicy, rt_prio: u8 },
    IoPriority(IoPriority),
}

/// The checks before any syscall: nice -20..=19, a non-empty CPU set, no `Deadline`, `rt_prio`
/// 1..=99 for `Fifo` and `Rr` and 0 for the other policies, I/O levels 0..=7, and no
/// `IoPriority::Realtime` on this unprivileged path.
fn valid(op: PidOp) -> bool {
    match op {
        PidOp::Nice(n) => (-20..=19).contains(&n),
        PidOp::Affinity(set) => set != CpuSet::empty(),
        PidOp::Sched {
            policy: SchedPolicy::Fifo | SchedPolicy::Rr,
            rt_prio,
        } => (1..=99).contains(&rt_prio),
        PidOp::Sched {
            policy: SchedPolicy::Deadline,
            ..
        } => false,
        PidOp::Sched { rt_prio, .. } => rt_prio == 0,
        PidOp::IoPriority(IoPriority::Realtime(_)) => false,
        PidOp::IoPriority(IoPriority::BestEffort(level)) => level <= 7,
        PidOp::IoPriority(IoPriority::None | IoPriority::Idle) => true,
    }
}

/// Runs `op` on every thread of `key` in one blocking job under the 2 s deadline. tl-ops opens
/// and verifies the pidfd, changes each TID and polls the pidfd after the loop, so its errors
/// carry the outcome: `Partial` is `Failed(Partial)`, an exit during the loop is
/// `OutcomeUnknown("process exited during the change")`, `IdentityChanged` is
/// `Rejected(IdentityChanged)` and EPERM is `Failed(PermissionDenied)`. An `op` that fails
/// `valid` is `Rejected(Unsupported)` before any syscall.
async fn pid_based(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    key: ProcessKey,
    op: PidOp,
) -> ActionState {
    if !valid(op) {
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    let ops = Arc::clone(&actx.ops);
    let boot = actx.boot_id;
    let target = ActionTarget::Process(key);
    let state = blocking(cx, kind, &target, move || ops.apply_pid_op(key, boot, &op)).await;
    tracing::debug!(target: "tasklens::action", pid = key.pid, op = ?op, result = ?state);
    state
}

/// What one `Thread` target does: a signal through `tgkill`, or a PID-based change on that TID.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadOp {
    Signal(Signal),
    Pid(PidOp),
}

/// Runs `kind` on thread `tid` of `process` (ADR-006, R-097) in one blocking job under the 2 s
/// deadline. tl-ops opens and verifies the process pidfd, checks `/proc/<pid>/task/<tid>`
/// exists (else `IdentityChanged`), acts on that TID only and polls the pidfd after the call.
/// Allowed kinds are `Signal`, `SetNice`, `SetSched` and `SetAffinity`. The others, and an op
/// that fails `valid`, are `Rejected(Unsupported)` before any syscall.
async fn run_thread(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    process: ProcessKey,
    tid: u32,
) -> ActionState {
    let op = match kind {
        ActionKind::Signal(sig) => ThreadOp::Signal(*sig),
        ActionKind::SetNice(n) => ThreadOp::Pid(PidOp::Nice(*n)),
        ActionKind::SetSched { policy, rt_prio } => ThreadOp::Pid(PidOp::Sched {
            policy: *policy,
            rt_prio: *rt_prio,
        }),
        ActionKind::SetAffinity(set) => ThreadOp::Pid(PidOp::Affinity(*set)),
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    if let ThreadOp::Pid(pid_op) = op
        && !valid(pid_op)
    {
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    let ops = Arc::clone(&actx.ops);
    let boot = actx.boot_id;
    let target = ActionTarget::Thread { process, tid };
    let state = blocking(cx, kind, &target, move || {
        ops.apply_thread_op(process, boot, tid, &op)
    })
    .await;
    tracing::debug!(target: "tasklens::action", pid = process.pid, tid, op = ?op, result = ?state);
    state
}

/// Opens the verified pidfd for `key` and sends `sig` through it, in one blocking job under the
/// 2 s deadline. `Signal::Other(n)` is checked (1..=64) by tl-ops.
async fn signal(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
    key: ProcessKey,
    sig: Signal,
) -> ActionState {
    let ops = Arc::clone(&actx.ops);
    let boot = actx.boot_id;
    let state = blocking(cx, kind, target, move || {
        let pidfd = ops.open_verified(key, boot)?;
        ops.send_signal(&pidfd, sig)
    })
    .await;
    tracing::debug!(target: "tasklens::action", pid = key.pid, signal = sig.number(), result = ?state);
    state
}

/// Runs `job` on the blocking pool under the deadline for `kind`. Past the deadline is
/// `OutcomeUnknown("timed out")`, a panic is `Failed(Internal)`, and an error goes through
/// `map_ops_error`, so `IdentityChanged` becomes `Rejected(IdentityChanged)`.
async fn blocking<F>(cx: &Cx, kind: &ActionKind, target: &ActionTarget, job: F) -> ActionState
where
    F: FnOnce() -> Result<(), OpsError> + Send + 'static,
{
    let Ok(mut job) = cx.spawn_blocking(move |_| job()) else {
        return internal_error();
    };
    match with_deadline(cx, deadline(kind, Some(target), false), job.join(cx)).await {
        Err(_) => ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
        Ok(Err(_)) => internal_error(),
        Ok(Ok(Ok(()))) => ActionState::Succeeded,
        Ok(Ok(Err(e))) => map_ops_error(&e),
    }
}

/// The most time End Process Tree waits for its one blocking job, whatever the target count.
const TREE_DEADLINE_CAP: Duration = Duration::from_secs(30);

/// End Process Tree (ADR-006, PROC-062, E11-004). One blocking job opens a verified pidfd for
/// every target, sends SIGSTOP through all of them, then SIGKILL through all of them, then closes
/// them. Stopping first means no process in the tree can fork between the passes. Returns one
/// state per target, in order. The deadline is 2 s per target, capped at 30 s.
pub async fn kill_tree(cx: &Cx, actx: &ActionCtx, targets: &[ActionTarget]) -> Vec<ActionState> {
    let n = targets.len();
    if n == 0 {
        return Vec::new();
    }
    let keys: Vec<Option<ProcessKey>> = targets
        .iter()
        .map(|target| match target {
            ActionTarget::Process(key) => Some(*key),
            _ => None,
        })
        .collect();
    let ops = Arc::clone(&actx.ops);
    let boot = actx.boot_id;
    let Ok(mut job) = cx.spawn_blocking(move |_| tree_passes(ops.as_ref(), &keys, boot)) else {
        return vec![internal_error(); n];
    };
    let per_target = deadline(&ActionKind::KillTree, None, false);
    let d = per_target.map(|d| {
        d.saturating_mul(u32::try_from(n).unwrap_or(u32::MAX))
            .min(TREE_DEADLINE_CAP)
    });
    match with_deadline(cx, d, job.join(cx)).await {
        Err(_) => vec![ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)); n],
        Ok(Err(_)) => vec![internal_error(); n],
        Ok(Ok(states)) => states,
    }
}

/// The three passes of `kill_tree`. A `None` key is a non-process target: `Rejected(Unsupported)`.
/// A failed open skips the target; a failed stop or kill marks it failed and the passes go on.
fn tree_passes(ops: &dyn ActionOps, keys: &[Option<ProcessKey>], boot: BootId) -> Vec<ActionState> {
    let mut states = Vec::with_capacity(keys.len());
    let mut fds = Vec::with_capacity(keys.len());
    for key in keys {
        let (state, fd) = match key {
            None => (ActionState::Rejected(RejectReason::Unsupported), None),
            Some(key) => match ops.open_verified(*key, boot) {
                Ok(fd) => (ActionState::Succeeded, Some(fd)),
                // `IdentityChanged` becomes `Rejected(IdentityChanged)`.
                Err(e) => (map_ops_error(&e), None),
            },
        };
        states.push(state);
        fds.push(fd);
    }
    for sig in [Signal::Stop, Signal::Kill] {
        for (state, fd) in states.iter_mut().zip(&fds) {
            let Some(fd) = fd else { continue };
            if let Err(e) = ops.send_signal(fd, sig)
                && *state == ActionState::Succeeded
            {
                *state = map_ops_error(&e);
            }
        }
    }
    for fd in fds.into_iter().flatten() {
        ops.close_pidfd(fd);
    }
    for (key, state) in keys.iter().zip(&states) {
        tracing::debug!(target: "tasklens::action", pid = key.map(|k| k.pid), kind = "KillTree", result = ?state);
    }
    states
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::os::fd::{AsRawFd, OwnedFd, RawFd};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use asupersync::channel::mpsc;
    use asupersync::time::{TimeSource, TimerDriverHandle, VirtualClock};
    use asupersync::types::{Budget, Time};
    use asupersync::{LabConfig, LabRuntime};
    use rustc_hash::{FxHashMap, FxHashSet};
    use rustix::io::Errno;
    use tl_model::actions::{ActionError, ActionId, ActionRequest};
    use tl_model::time::BootId;

    use super::*;
    use crate::actions::{ActionOps, Admission, RealOps, admit, summarize};
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;
    use crate::reducer::AdmissionView;

    const BOOT: BootId = BootId(1);

    #[derive(Clone)]
    enum Step {
        Ok,
        Err(OpsError),
        /// Advances the virtual clock by this much inside the blocking job.
        Block(Duration),
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Call {
        Open(u32),
        Send(Signal),
        Close(u32),
        Pid(u32, PidOp),
        Thread(u32, u32, ThreadOp),
    }

    struct FakeOps {
        open: Step,
        send: Step,
        /// What `apply_pid_op` returns.
        pid_op: Result<(), OpsError>,
        /// TIDs `apply_thread_op` treats as gone (`/proc/<pid>/task/<tid>` missing).
        missing_tids: Vec<u32>,
        /// Per-PID errors: `(pid, None)` fails the open, `(pid, Some(sig))` fails that signal.
        fail: Vec<(u32, Option<Signal>, OpsError)>,
        clock: Arc<VirtualClock>,
        log: Mutex<Vec<Call>>,
        /// The PID behind each open fake pidfd.
        fds: Mutex<FxHashMap<RawFd, u32>>,
    }

    impl FakeOps {
        fn new(open: Step, send: Step, clock: &Arc<VirtualClock>) -> Self {
            Self {
                open,
                send,
                pid_op: Ok(()),
                missing_tids: Vec::new(),
                fail: Vec::new(),
                clock: Arc::clone(clock),
                log: Mutex::new(Vec::new()),
                fds: Mutex::new(FxHashMap::default()),
            }
        }

        fn step(&self, step: Step) -> Result<(), OpsError> {
            match step {
                Step::Ok => Ok(()),
                Step::Err(e) => Err(e),
                Step::Block(d) => {
                    self.clock.advance(d.as_nanos() as u64);
                    Ok(())
                }
            }
        }

        fn failure(&self, pid: u32, sig: Option<Signal>) -> Option<OpsError> {
            self.fail
                .iter()
                .find(|(p, s, _)| *p == pid && *s == sig)
                .map(|(_, _, e)| e.clone())
        }
    }

    impl ActionOps for FakeOps {
        fn open_verified(&self, key: ProcessKey, _boot: BootId) -> Result<OwnedFd, OpsError> {
            self.log.lock().unwrap().push(Call::Open(key.pid));
            if let Some(e) = self.failure(key.pid, None) {
                return Err(e);
            }
            self.step(self.open.clone())?;
            let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
            self.fds.lock().unwrap().insert(fd.as_raw_fd(), key.pid);
            Ok(fd)
        }

        fn send_signal(&self, pidfd: &OwnedFd, sig: Signal) -> Result<(), OpsError> {
            self.log.lock().unwrap().push(Call::Send(sig));
            let pid = self.fds.lock().unwrap()[&pidfd.as_raw_fd()];
            if let Some(e) = self.failure(pid, Some(sig)) {
                return Err(e);
            }
            self.step(self.send.clone())
        }

        fn close_pidfd(&self, pidfd: OwnedFd) {
            let pid = self.fds.lock().unwrap().remove(&pidfd.as_raw_fd());
            self.log
                .lock()
                .unwrap()
                .push(Call::Close(pid.expect("fd is open")));
        }

        fn apply_pid_op(&self, key: ProcessKey, _boot: BootId, op: &PidOp) -> Result<(), OpsError> {
            self.log.lock().unwrap().push(Call::Pid(key.pid, *op));
            self.pid_op.clone()
        }

        fn apply_thread_op(
            &self,
            process: ProcessKey,
            _boot: BootId,
            tid: u32,
            op: &ThreadOp,
        ) -> Result<(), OpsError> {
            self.log
                .lock()
                .unwrap()
                .push(Call::Thread(process.pid, tid, *op));
            if self.missing_tids.contains(&tid) {
                return Err(OpsError::IdentityChanged);
            }
            Ok(())
        }
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 100 + u64::from(pid),
            boot_id: BOOT,
            pid,
        }
    }

    /// Runs `body` as one lab task with `ops` and `clock`, and returns what it returned.
    fn in_lab<T, F, Fut>(ops: Arc<dyn ActionOps>, clock: Arc<VirtualClock>, body: F) -> T
    where
        T: Send + 'static,
        F: FnOnce(Cx, Arc<ActionCtx>) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send,
    {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        lab.state
            .set_timer_driver(TimerDriverHandle::with_virtual_clock(clock));
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = mpsc::channel(16);
        let actx = Arc::new(ActionCtx {
            events,
            msgs,
            ops,
            boot_id: BOOT,
            uid: 1000,
            config: Arc::new(EngineConfig::new(
                EngineMode::Full,
                PathBuf::from("/nonexistent"),
            )),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: None,
            destroy_support_sent: std::sync::atomic::AtomicBool::new(false),
            unit_bus_override: None,
            startup_paths: crate::actions::startup::StartupPaths::from_env(),
        });
        let out = Arc::new(Mutex::new(None));
        let out_task = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let value = body(cx, actx).await;
                *out_task.lock().unwrap() = Some(value);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        out.lock().unwrap().take().expect("task finished")
    }

    /// Runs `kind` on one process target and returns its state, the fake's call log and the
    /// virtual time in ms when the state was ready.
    fn run(kind: ActionKind, open: Step, send: Step) -> (ActionState, Vec<Call>, u64) {
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        let ops = Arc::new(FakeOps::new(open, send, &clock));
        let task_clock = Arc::clone(&clock);
        let (state, ms) = in_lab(ops.clone(), clock, move |cx, actx| async move {
            let state = run_target(&cx, &actx, &kind, &ActionTarget::Process(key(7))).await;
            (state, task_clock.now().as_nanos() / 1_000_000)
        });
        let log = ops.log.lock().unwrap().clone();
        (state, log, ms)
    }

    /// Runs `kind` on process 7 with a fake whose `apply_pid_op` returns `result`.
    fn run_pid(kind: ActionKind, result: Result<(), OpsError>) -> (ActionState, Vec<Call>) {
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        let mut fake = FakeOps::new(Step::Ok, Step::Ok, &clock);
        fake.pid_op = result;
        let ops = Arc::new(fake);
        let state = in_lab(ops.clone(), clock, move |cx, actx| async move {
            run_target(&cx, &actx, &kind, &ActionTarget::Process(key(7))).await
        });
        let log = ops.log.lock().unwrap().clone();
        (state, log)
    }

    /// Runs `kind` on thread `tid` of process 7 with a fake that treats `missing_tids` as gone.
    fn run_thread_target(
        kind: ActionKind,
        tid: u32,
        missing_tids: Vec<u32>,
    ) -> (ActionState, Vec<Call>) {
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        let mut fake = FakeOps::new(Step::Ok, Step::Ok, &clock);
        fake.missing_tids = missing_tids;
        let ops = Arc::new(fake);
        let state = in_lab(ops.clone(), clock, move |cx, actx| async move {
            let target = ActionTarget::Thread {
                process: key(7),
                tid,
            };
            run_target(&cx, &actx, &kind, &target).await
        });
        let log = ops.log.lock().unwrap().clone();
        (state, log)
    }

    /// Runs `kill_tree` on PIDs 1, 2, 3 with the per-PID `fail` list.
    fn tree(fail: Vec<(u32, Option<Signal>, OpsError)>) -> (Vec<ActionState>, Vec<Call>) {
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        let mut fake = FakeOps::new(Step::Ok, Step::Ok, &clock);
        fake.fail = fail;
        let ops = Arc::new(fake);
        let states = in_lab(ops.clone(), clock, |cx, actx| async move {
            let targets: Vec<_> = [1, 2, 3].map(|p| ActionTarget::Process(key(p))).into();
            kill_tree(&cx, &actx, &targets).await
        });
        let log = ops.log.lock().unwrap().clone();
        (states, log)
    }

    fn os(e: Errno) -> Step {
        Step::Err(OpsError::Os(e.raw_os_error()))
    }

    #[test]
    fn signal_succeeds() {
        let (state, log, _) = run(ActionKind::Signal(Signal::Term), Step::Ok, Step::Ok);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Open(7), Call::Send(Signal::Term)]);
    }

    #[test]
    fn signal_identity_changed() {
        let (state, log, _) = run(
            ActionKind::Signal(Signal::Kill),
            Step::Err(OpsError::IdentityChanged),
            Step::Ok,
        );
        assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
        // No signal goes out when the identity check fails.
        assert_eq!(log, [Call::Open(7)]);
    }

    #[test]
    fn signal_eperm() {
        let (state, _, _) = run(ActionKind::Signal(Signal::Term), Step::Ok, os(Errno::PERM));
        assert_eq!(state, ActionState::Failed(ActionError::PermissionDenied));
    }

    #[test]
    fn signal_esrch() {
        let (state, _, _) = run(ActionKind::Signal(Signal::Term), Step::Ok, os(Errno::SRCH));
        assert_eq!(state, ActionState::Failed(ActionError::NoSuchProcess));
    }

    #[test]
    fn suspend_resume_signals() {
        let (state, log, _) = run(ActionKind::Suspend, Step::Ok, Step::Ok);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Open(7), Call::Send(Signal::Stop)]);
        let (state, log, _) = run(ActionKind::Resume, Step::Ok, Step::Ok);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Open(7), Call::Send(Signal::Cont)]);
    }

    #[test]
    fn signal_other_passes_through() {
        let (_, log, _) = run(ActionKind::Signal(Signal::Other(40)), Step::Ok, Step::Ok);
        assert_eq!(log, [Call::Open(7), Call::Send(Signal::Other(40))]);
    }

    #[test]
    fn signal_timeout() {
        let (state, _, ms) = run(
            ActionKind::Signal(Signal::Term),
            Step::Block(Duration::from_secs(3)),
            Step::Ok,
        );
        assert_eq!(state, ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)));
        // The fake's job moved the clock 3 s, past the 2 s deadline.
        assert!(ms >= 2_000, "{ms} ms");
    }

    #[test]
    fn other_kinds_unsupported() {
        let (state, log, _) = run(ActionKind::FreezeGroup, Step::Ok, Step::Ok);
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(log.is_empty());
    }

    #[test]
    fn nice_done() {
        let (state, log) = run_pid(ActionKind::SetNice(5), Ok(()));
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Pid(7, PidOp::Nice(5))]);
    }

    #[test]
    fn nice_partial() {
        let (state, _) = run_pid(
            ActionKind::SetNice(5),
            Err(OpsError::Partial { done: 3, total: 4 }),
        );
        assert_eq!(
            state,
            ActionState::Failed(ActionError::Partial { done: 3, total: 4 })
        );
    }

    #[test]
    fn nice_exited_during() {
        let (state, _) = run_pid(
            ActionKind::SetNice(5),
            Err(OpsError::OutcomeUnknown("process exited during the change")),
        );
        assert_eq!(
            state,
            ActionState::OutcomeUnknown(Arc::from("process exited during the change"))
        );
    }

    #[test]
    fn nice_eperm() {
        // tl-ops maps EPERM to `PermissionDenied`; a raw errno maps the same way.
        for err in [
            OpsError::PermissionDenied,
            OpsError::Os(Errno::PERM.raw_os_error()),
        ] {
            let (state, _) = run_pid(ActionKind::SetNice(-10), Err(err));
            assert_eq!(state, ActionState::Failed(ActionError::PermissionDenied));
        }
    }

    #[test]
    fn nice_identity_changed() {
        let (state, _) = run_pid(ActionKind::SetNice(5), Err(OpsError::IdentityChanged));
        assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
    }

    #[test]
    fn nice_out_of_range_rejected() {
        for n in [-21, 20, i8::MIN, i8::MAX] {
            let (state, log) = run_pid(ActionKind::SetNice(n), Ok(()));
            assert_eq!(
                state,
                ActionState::Rejected(RejectReason::Unsupported),
                "{n}"
            );
            assert!(log.is_empty());
        }
    }

    #[test]
    fn affinity_empty_rejected() {
        let (state, log) = run_pid(ActionKind::SetAffinity(CpuSet::empty()), Ok(()));
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(log.is_empty(), "the fake saw a call: {log:?}");
    }

    #[test]
    fn affinity_done() {
        let mut set = CpuSet::empty();
        set.insert(0);
        let (state, log) = run_pid(ActionKind::SetAffinity(set), Ok(()));
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Pid(7, PidOp::Affinity(set))]);
    }

    #[test]
    fn sched_validation_and_call() {
        let rr = PidOp::Sched {
            policy: SchedPolicy::Rr,
            rt_prio: 1,
        };
        let (state, log) = run_pid(
            ActionKind::SetSched {
                policy: SchedPolicy::Rr,
                rt_prio: 1,
            },
            Ok(()),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Pid(7, rr)]);
        let idle = PidOp::Sched {
            policy: SchedPolicy::Idle,
            rt_prio: 0,
        };
        let (state, log) = run_pid(
            ActionKind::SetSched {
                policy: SchedPolicy::Idle,
                rt_prio: 0,
            },
            Ok(()),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Pid(7, idle)]);
        for (policy, rt_prio) in [
            (SchedPolicy::Other, 5),
            (SchedPolicy::Batch, 1),
            (SchedPolicy::Idle, 99),
            (SchedPolicy::Fifo, 100),
            (SchedPolicy::Fifo, 0),
            (SchedPolicy::Rr, 0),
            (SchedPolicy::Rr, u8::MAX),
            (SchedPolicy::Deadline, 0),
        ] {
            let (state, log) = run_pid(ActionKind::SetSched { policy, rt_prio }, Ok(()));
            assert_eq!(
                state,
                ActionState::Rejected(RejectReason::Unsupported),
                "{policy:?} {rt_prio}"
            );
            assert!(log.is_empty(), "the fake saw a call: {log:?}");
        }
    }

    #[test]
    fn ioprio_classes_and_realtime_guard() {
        for prio in [
            IoPriority::BestEffort(4),
            IoPriority::BestEffort(0),
            IoPriority::BestEffort(7),
            IoPriority::Idle,
            // "Default (follows nice)": the fake sees class none.
            IoPriority::None,
        ] {
            let (state, log) = run_pid(ActionKind::SetIoPriority(prio), Ok(()));
            assert_eq!(state, ActionState::Succeeded, "{prio:?}");
            assert_eq!(log, [Call::Pid(7, PidOp::IoPriority(prio))]);
        }
        // Realtime needs root, and this path is never privileged. Levels stop at 7.
        for prio in [
            IoPriority::Realtime(0),
            IoPriority::Realtime(7),
            IoPriority::BestEffort(8),
        ] {
            let (state, log) = run_pid(ActionKind::SetIoPriority(prio), Ok(()));
            assert_eq!(
                state,
                ActionState::Rejected(RejectReason::Unsupported),
                "{prio:?}"
            );
            assert!(log.is_empty(), "the fake saw a call: {log:?}");
        }
    }

    #[test]
    fn signal_other_number() {
        let (state, log, _) = run(ActionKind::Signal(Signal::Other(34)), Step::Ok, Step::Ok);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Open(7), Call::Send(Signal::Other(34))]);
        assert_eq!(Signal::Other(34).number(), 34);
    }

    #[test]
    fn thread_target_tgkill_and_missing_tid() {
        let (state, log) = run_thread_target(ActionKind::Signal(Signal::Term), 9, Vec::new());
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(log, [Call::Thread(7, 9, ThreadOp::Signal(Signal::Term))]);
        assert_eq!(Signal::Term.number(), 15);
        let (state, _) = run_thread_target(ActionKind::Signal(Signal::Term), 9, vec![9]);
        assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
    }

    #[test]
    fn thread_target_pid_ops() {
        let mut set = CpuSet::empty();
        set.insert(1);
        let cases = [
            (ActionKind::SetNice(3), PidOp::Nice(3)),
            (ActionKind::SetAffinity(set), PidOp::Affinity(set)),
            (
                ActionKind::SetSched {
                    policy: SchedPolicy::Batch,
                    rt_prio: 0,
                },
                PidOp::Sched {
                    policy: SchedPolicy::Batch,
                    rt_prio: 0,
                },
            ),
        ];
        for (kind, op) in cases {
            let (state, log) = run_thread_target(kind, 9, Vec::new());
            assert_eq!(state, ActionState::Succeeded, "{op:?}");
            assert_eq!(log, [Call::Thread(7, 9, ThreadOp::Pid(op))]);
        }
        // The same input checks as a process target.
        for kind in [
            ActionKind::SetNice(20),
            ActionKind::SetAffinity(CpuSet::empty()),
            ActionKind::SetSched {
                policy: SchedPolicy::Fifo,
                rt_prio: 0,
            },
        ] {
            let (state, log) = run_thread_target(kind.clone(), 9, Vec::new());
            assert_eq!(
                state,
                ActionState::Rejected(RejectReason::Unsupported),
                "{kind:?}"
            );
            assert!(log.is_empty(), "the fake saw a call: {log:?}");
        }
    }

    #[test]
    fn thread_target_unsupported_kinds() {
        for kind in [
            ActionKind::Suspend,
            ActionKind::Resume,
            ActionKind::KillTree,
            ActionKind::SetIoPriority(IoPriority::BestEffort(4)),
        ] {
            let (state, log) = run_thread_target(kind.clone(), 9, Vec::new());
            assert_eq!(
                state,
                ActionState::Rejected(RejectReason::Unsupported),
                "{kind:?}"
            );
            assert!(log.is_empty(), "the fake saw a call: {log:?}");
        }
    }

    #[test]
    fn kill_tree_stop_all_then_kill_all() {
        let (states, log) = tree(Vec::new());
        assert_eq!(states, vec![ActionState::Succeeded; 3]);
        let stop = Call::Send(Signal::Stop);
        let kill = Call::Send(Signal::Kill);
        assert_eq!(
            log,
            [
                Call::Open(1),
                Call::Open(2),
                Call::Open(3),
                stop,
                stop,
                stop,
                kill,
                kill,
                kill,
                Call::Close(1),
                Call::Close(2),
                Call::Close(3),
            ]
        );
    }

    #[test]
    fn kill_tree_skips_identity_changed() {
        let (states, log) = tree(vec![(2, None, OpsError::IdentityChanged)]);
        assert_eq!(
            states,
            [
                ActionState::Succeeded,
                ActionState::Rejected(RejectReason::IdentityChanged),
                ActionState::Succeeded,
            ]
        );
        let stop = Call::Send(Signal::Stop);
        let kill = Call::Send(Signal::Kill);
        assert_eq!(
            log,
            [
                Call::Open(1),
                Call::Open(2),
                Call::Open(3),
                stop,
                stop,
                kill,
                kill,
                Call::Close(1),
                Call::Close(3),
            ]
        );
    }

    #[test]
    fn kill_tree_partial_eperm() {
        let eperm = OpsError::Os(Errno::PERM.raw_os_error());
        let (states, log) = tree(vec![(3, Some(Signal::Kill), eperm)]);
        assert_eq!(
            states,
            [
                ActionState::Succeeded,
                ActionState::Succeeded,
                ActionState::Failed(ActionError::PermissionDenied),
            ]
        );
        assert_eq!(
            summarize(&states),
            ActionState::Failed(ActionError::Partial { done: 2, total: 3 })
        );
        // Every fd is still closed.
        assert_eq!(log[log.len() - 3..], [1, 2, 3].map(Call::Close));
    }

    #[test]
    fn kill_tree_stop_failure_still_kills() {
        let esrch = OpsError::Os(Errno::SRCH.raw_os_error());
        let (states, log) = tree(vec![(1, Some(Signal::Stop), esrch)]);
        assert_eq!(states[0], ActionState::Failed(ActionError::NoSuchProcess));
        assert_eq!(
            states[1..],
            [ActionState::Succeeded, ActionState::Succeeded]
        );
        let kills = log
            .iter()
            .filter(|c| **c == Call::Send(Signal::Kill))
            .count();
        assert_eq!(kills, 3);
    }

    #[test]
    fn kill_tree_too_many_targets_is_rejected() {
        let live = FxHashMap::default();
        let exited = FxHashSet::default();
        let view = AdmissionView {
            boot_id: BOOT,
            live: &live,
            exited_within_linger: &exited,
            remote: false,
            remote_admin: false,
        };
        let req = ActionRequest {
            id: ActionId(1),
            kind: ActionKind::KillTree,
            targets: (0..300)
                .map(|p| ActionTarget::Process(key(1000 + p)))
                .collect(),
            privileged: false,
        };
        // A rejected request never starts a task, so `kill_tree` is never called.
        assert_eq!(
            admit(&req, &view, 0, &FxHashSet::default()),
            Admission::Reject(RejectReason::TooManyTargets)
        );
    }

    /// `RealOps`, plus a 50 ms pause before the first SIGKILL so the watcher thread can see the
    /// stopped state.
    struct PauseBeforeKill {
        paused: AtomicBool,
    }

    impl ActionOps for PauseBeforeKill {
        fn open_verified(&self, key: ProcessKey, boot: BootId) -> Result<OwnedFd, OpsError> {
            RealOps.open_verified(key, boot)
        }

        fn send_signal(&self, pidfd: &OwnedFd, sig: Signal) -> Result<(), OpsError> {
            if sig == Signal::Kill && !self.paused.swap(true, Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
            }
            RealOps.send_signal(pidfd, sig)
        }
    }

    /// The state letter from `/proc/<pid>/stat`, or `None` when the process is gone.
    fn proc_state(pid: u32) -> Option<u8> {
        let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.iter().rposition(|b| *b == b')')?;
        stat.get(close + 2).copied()
    }

    /// Kills the test's own `sh` if an assertion fails before `kill_tree` ran.
    struct ChildGuard(std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn kill_tree_real_parent_two_children() {
        // Only processes this test spawns are signalled: the `sh` and its two `sleep` children.
        let parent = std::process::Command::new("sh")
            .args(["-c", "sleep 60 & sleep 60 & wait"])
            .spawn()
            .expect("sh starts");
        let mut parent = ChildGuard(parent);
        let ppid = parent.0.id();
        let start = Instant::now();
        let children: Vec<u32> = loop {
            let list = std::fs::read_to_string(format!("/proc/{ppid}/task/{ppid}/children"))
                .unwrap_or_default();
            let pids: Vec<u32> = list
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect();
            let execed = pids.iter().all(|p| {
                std::fs::read_to_string(format!("/proc/{p}/comm"))
                    .is_ok_and(|c| c.trim() == "sleep")
            });
            if pids.len() == 2 && execed {
                break pids;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "children did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let pids = [ppid, children[0], children[1]];
        let targets: Vec<ActionTarget> = pids
            .iter()
            .map(|&pid| {
                ActionTarget::Process(ProcessKey {
                    start_ticks: tl_ops::process::read_start_ticks(pid).expect("start ticks"),
                    boot_id: BOOT,
                    pid,
                })
            })
            .collect();

        let stop_watch = Arc::new(AtomicBool::new(false));
        let watch_flag = Arc::clone(&stop_watch);
        let watcher = std::thread::spawn(move || {
            let mut seen_stopped = [false; 3];
            while !watch_flag.load(Ordering::SeqCst) {
                for (seen, pid) in seen_stopped.iter_mut().zip(pids) {
                    if proc_state(pid) == Some(b'T') {
                        *seen = true;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            seen_stopped
        });

        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        let ops = Arc::new(PauseBeforeKill {
            paused: AtomicBool::new(false),
        });
        let states = in_lab(ops, clock, move |cx, actx| async move {
            kill_tree(&cx, &actx, &targets).await
        });
        assert_eq!(states, vec![ActionState::Succeeded; 3]);

        let status = parent.0.wait().expect("sh is reaped");
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(9)
        );
        // The children are reparented and reaped by the subreaper; a zombie is dead too.
        let start = Instant::now();
        while children
            .iter()
            .any(|&c| proc_state(c).is_some_and(|s| s != b'Z'))
        {
            assert!(start.elapsed() < Duration::from_secs(5), "a child survived");
            std::thread::sleep(Duration::from_millis(5));
        }
        stop_watch.store(true, Ordering::SeqCst);
        let seen_stopped = watcher.join().expect("watcher thread");
        assert_eq!(
            seen_stopped, [true; 3],
            "parent, child, child seen in state T"
        );
    }

    /// Env var that turns `threads_child_helper` into the multi-threaded child.
    const THREADS_CHILD: &str = "TL_I3O7_THREADS_CHILD";

    /// Not a test: `set_nice_real_child_all_tids` and `thread_nice_only_that_tid` run this test
    /// binary again with `THREADS_CHILD` set, so this starts three extra threads and sleeps until
    /// killed.
    #[test]
    #[ignore = "child process for the real nice tests"]
    fn threads_child_helper() {
        if std::env::var_os(THREADS_CHILD).is_none() {
            return;
        }
        for _ in 0..3 {
            std::thread::spawn(|| std::thread::sleep(Duration::from_secs(60)));
        }
        std::thread::sleep(Duration::from_secs(60));
    }

    /// The key of a process this test spawned.
    fn real_key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: tl_ops::process::read_start_ticks(pid).expect("start ticks"),
            boot_id: BOOT,
            pid,
        }
    }

    /// Runs `kind` on `key` through `RealOps`.
    fn run_real(kind: ActionKind, key: ProcessKey) -> ActionState {
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        in_lab(Arc::new(RealOps), clock, move |cx, actx| async move {
            run_target(&cx, &actx, &kind, &ActionTarget::Process(key)).await
        })
    }

    /// Field 19 (`nice`) of every `/proc/<pid>/task/*/stat`.
    fn task_nices(pid: u32) -> Vec<i64> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(format!("/proc/{pid}/task")).expect("task dir") {
            let stat = std::fs::read_to_string(entry.expect("task entry").path().join("stat"))
                .expect("task stat");
            let rest = &stat[stat.rfind(')').expect("comm end") + 2..];
            // `rest` starts at field 3, so field 19 is index 16.
            let nice = rest.split(' ').nth(16).expect("field 19");
            out.push(nice.parse().expect("nice parses"));
        }
        out
    }

    #[test]
    fn set_nice_real_child_all_tids() {
        // Only the child this test spawns has its nice value changed.
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "actions::process::tests::threads_child_helper",
                "--ignored",
                "--test-threads=1",
            ])
            .env(THREADS_CHILD, "1")
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("child starts");
        let child = ChildGuard(child);
        let pid = child.0.id();
        let start = Instant::now();
        while std::fs::read_dir(format!("/proc/{pid}/task")).map_or(0, |d| d.count()) < 4 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "threads did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Lowering priority needs no privilege, but never ask for a value below the child's
        // current nice, which it inherits from this test (builds may run niced).
        let current = *task_nices(pid).iter().max().expect("a task");
        let target: i8 = if current <= 5 { 5 } else { 19 };
        let state = run_real(ActionKind::SetNice(target), real_key(pid));
        assert_eq!(state, ActionState::Succeeded);
        let nices = task_nices(pid);
        assert!(nices.len() >= 4, "{nices:?}");
        assert!(nices.iter().all(|&n| n == i64::from(target)), "{nices:?}");
    }

    #[test]
    fn thread_nice_only_that_tid() {
        // Only one thread of the child this test spawns has its nice value changed.
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "actions::process::tests::threads_child_helper",
                "--ignored",
                "--test-threads=1",
            ])
            .env(THREADS_CHILD, "1")
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("child starts");
        let child = ChildGuard(child);
        let pid = child.0.id();
        let start = Instant::now();
        while std::fs::read_dir(format!("/proc/{pid}/task")).map_or(0, |d| d.count()) < 4 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "threads did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let before = task_nice_map(pid);
        let tid = *before
            .keys()
            .filter(|&&t| t != pid)
            .max()
            .expect("an extra thread");
        // Raise the nice value above every thread's current one, so no privilege is needed.
        let current = *before.values().max().expect("a task");
        let target: i8 = if current < 3 { 3 } else { 19 };
        assert!(i64::from(target) > current || target == 19, "{before:?}");
        let state = run_real_target(
            ActionKind::SetNice(target),
            ActionTarget::Thread {
                process: real_key(pid),
                tid,
            },
        );
        assert_eq!(state, ActionState::Succeeded);
        let after = task_nice_map(pid);
        for (t, nice) in &after {
            if *t == tid {
                assert_eq!(*nice, i64::from(target), "{after:?}");
            } else {
                assert_eq!(Some(nice), before.get(t), "{before:?} {after:?}");
            }
        }
        // A TID that is not in this process is never touched.
        let state = run_real_target(
            ActionKind::SetNice(19),
            ActionTarget::Thread {
                process: real_key(pid),
                tid: std::process::id(),
            },
        );
        assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
    }

    /// Runs `kind` on `target` through `RealOps`.
    fn run_real_target(kind: ActionKind, target: ActionTarget) -> ActionState {
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        in_lab(Arc::new(RealOps), clock, move |cx, actx| async move {
            run_target(&cx, &actx, &kind, &target).await
        })
    }

    /// TID to field 19 (`nice`) of `/proc/<pid>/task/<tid>/stat`.
    fn task_nice_map(pid: u32) -> FxHashMap<u32, i64> {
        let mut out = FxHashMap::default();
        for entry in std::fs::read_dir(format!("/proc/{pid}/task")).expect("task dir") {
            let entry = entry.expect("task entry");
            let tid = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse().ok())
                .expect("tid");
            let stat = std::fs::read_to_string(entry.path().join("stat")).expect("task stat");
            let rest = &stat[stat.rfind(')').expect("comm end") + 2..];
            // `rest` starts at field 3, so field 19 is index 16.
            let nice = rest.split(' ').nth(16).expect("field 19");
            out.insert(tid, nice.parse().expect("nice parses"));
        }
        out
    }

    #[test]
    fn exiting_child_never_succeeds() {
        // Only processes this test spawns are targeted.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("true starts");
        let pid = child.id();
        let key = real_key(pid);
        let start = Instant::now();
        while proc_state(pid) != Some(b'Z') {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "true did not exit"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // A zombie: the pidfd opens but is readable after the loop.
        let state = run_real(ActionKind::SetNice(19), key);
        assert!(
            matches!(
                state,
                ActionState::Rejected(RejectReason::IdentityChanged)
                    | ActionState::OutcomeUnknown(_)
            ),
            "{state:?}"
        );
        child.wait().expect("true is reaped");
        // Reaped: the PID is gone or reused, so the identity check fails.
        let state = run_real(ActionKind::SetNice(19), key);
        assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
    }
}
