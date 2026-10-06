//! Process actions through the verified pidfd (ADR-006, E11-003). Every signal first opens a
//! pidfd and proves it is the captured process, then signals through that pidfd, so it cannot
//! hit a reused PID.

use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use tl_model::actions::{ActionKind, ActionState, ActionTarget, RejectReason, Signal};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

use super::{
    ActionCtx, ActionOps, TIMED_OUT, deadline, internal_error, map_ops_error, with_deadline,
};

/// Runs one `Process` or `Thread` target. A kind this module does not handle yet is
/// `Rejected(Unsupported)`. App rows arrive here already expanded to their primary processes.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
) -> ActionState {
    let ActionTarget::Process(key) = target else {
        // Thread targets are E11-006.
        return ActionState::Rejected(RejectReason::Unsupported);
    };
    let sig = match kind {
        ActionKind::Signal(sig) => *sig,
        ActionKind::Suspend => Signal::Stop,
        ActionKind::Resume => Signal::Cont,
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    signal(cx, actx, kind, target, *key, sig).await
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
    let Ok(mut job) = cx.spawn_blocking(move |_| {
        let pidfd = ops.open_verified(key, boot)?;
        ops.send_signal(&pidfd, sig)
    }) else {
        return internal_error();
    };
    let state = match with_deadline(cx, deadline(kind, Some(target), false), job.join(cx)).await {
        Err(_) => ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
        Ok(Err(_)) => internal_error(),
        Ok(Ok(Ok(()))) => ActionState::Succeeded,
        // `IdentityChanged` becomes `Rejected(IdentityChanged)`.
        Ok(Ok(Err(e))) => map_ops_error(&e),
    };
    tracing::debug!(target: "tasklens::action", pid = key.pid, signal = sig.number(), result = ?state);
    state
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
    use std::os::fd::OwnedFd;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use asupersync::channel::mpsc;
    use asupersync::time::{TimeSource, TimerDriverHandle, VirtualClock};
    use asupersync::types::{Budget, Time};
    use asupersync::{LabConfig, LabRuntime};
    use rustix::io::Errno;
    use tl_model::actions::ActionError;
    use tl_model::time::BootId;
    use tl_ops::OpsError;

    use super::*;
    use crate::actions::ActionOps;
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;

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
    }

    struct FakeOps {
        open: Step,
        send: Step,
        clock: Arc<VirtualClock>,
        log: Mutex<Vec<Call>>,
    }

    impl FakeOps {
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
    }

    impl ActionOps for FakeOps {
        fn open_verified(&self, key: ProcessKey, _boot: BootId) -> Result<OwnedFd, OpsError> {
            self.log.lock().unwrap().push(Call::Open(key.pid));
            self.step(self.open.clone())?;
            Ok(std::fs::File::open("/dev/null").unwrap().into())
        }

        fn send_signal(&self, _pidfd: &OwnedFd, sig: Signal) -> Result<(), OpsError> {
            self.log.lock().unwrap().push(Call::Send(sig));
            self.step(self.send.clone())
        }
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 100 + u64::from(pid),
            boot_id: BOOT,
            pid,
        }
    }

    /// Runs `kind` on one process target and returns its state, the fake's call log and the
    /// virtual time in ms when the state was ready.
    fn run(kind: ActionKind, open: Step, send: Step) -> (ActionState, Vec<Call>, u64) {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        lab.state
            .set_timer_driver(TimerDriverHandle::with_virtual_clock(Arc::clone(&clock)));
        let ops = Arc::new(FakeOps {
            open,
            send,
            clock: Arc::clone(&clock),
            log: Mutex::new(Vec::new()),
        });
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = mpsc::channel(16);
        let actx = Arc::new(ActionCtx {
            events,
            msgs,
            ops: Arc::clone(&ops) as Arc<dyn ActionOps>,
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
        });
        let out = Arc::new(Mutex::new(None));
        let out_task = Arc::clone(&out);
        let task_clock = Arc::clone(&clock);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = run_target(&cx, &actx, &kind, &ActionTarget::Process(key(7))).await;
                let ms = task_clock.now().as_nanos() / 1_000_000;
                *out_task.lock().unwrap() = Some((state, ms));
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        let (state, ms) = out.lock().unwrap().take().expect("target finished");
        let log = ops.log.lock().unwrap().clone();
        (state, log, ms)
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
        let (state, log, _) = run(ActionKind::SetNice(5), Step::Ok, Step::Ok);
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(log.is_empty());
    }
}
