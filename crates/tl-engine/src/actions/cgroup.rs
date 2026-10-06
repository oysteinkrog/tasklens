//! Freeze App, Thaw App and Kill App on an app cgroup under `app.slice` (ADR-006, E11-011).
//!
//! `tl-ops::cgroup` validates the captured `CgroupRef` before any write. Units outside
//! `app.slice` arrive as `ActionTarget::Unit` and go through systemd instead (R-092).

use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use tl_model::actions::{ActionKind, ActionState, CgroupRef, RejectReason};
use tl_ops::OpsError;
use tl_ops::cgroup::FREEZE_WAIT;

use super::{ActionCtx, QUICK_DEADLINE, TIMED_OUT, internal_error, map_ops_error, with_deadline};

/// The control file and value one kind writes.
fn control(kind: &ActionKind) -> Option<(&'static str, &'static str)> {
    match kind {
        ActionKind::FreezeGroup => Some(("cgroup.freeze", "1")),
        ActionKind::ThawGroup => Some(("cgroup.freeze", "0")),
        ActionKind::KillGroup => Some(("cgroup.kill", "1")),
        _ => None,
    }
}

/// Validates `cg`, writes the control file under the 2 s deadline, then for `FreezeGroup` waits
/// up to `FREEZE_WAIT` for `frozen 1`. Any other kind is `Rejected(Unsupported)`.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    cg: &CgroupRef,
) -> ActionState {
    let Some((file, value)) = control(kind) else {
        return ActionState::Rejected(RejectReason::Unsupported);
    };
    let state = apply(
        cx,
        actx,
        cg,
        file,
        value,
        matches!(kind, ActionKind::FreezeGroup),
    )
    .await;
    let leaf = cg.rel_path.rsplit('/').next().unwrap_or("");
    tracing::debug!(target: "tasklens::action", cgroup = leaf, file, result = ?state, "cgroup action");
    state
}

async fn apply(
    cx: &Cx,
    actx: &ActionCtx,
    cg: &CgroupRef,
    file: &'static str,
    value: &'static str,
    wait_frozen: bool,
) -> ActionState {
    let ops = Arc::clone(&actx.ops);
    let cg = cg.clone();
    let uid = actx.uid;
    let written = blocking(cx, QUICK_DEADLINE, move || {
        let dir = ops.cgroup_open_validated(&cg, uid)?;
        ops.cgroup_write(&dir, file, value)?;
        Ok(dir)
    })
    .await;
    let dir = match written {
        Ok(dir) => dir,
        Err(state) => return state,
    };
    if !wait_frozen {
        return ActionState::Succeeded;
    }
    // The wait has its own `FREEZE_WAIT` inside tl-ops; the deadline only guards a stuck read.
    let ops = Arc::clone(&actx.ops);
    match blocking(cx, FREEZE_WAIT + QUICK_DEADLINE, move || {
        ops.cgroup_wait_frozen(&dir, FREEZE_WAIT)
    })
    .await
    {
        Ok(()) => ActionState::Succeeded,
        Err(state) => state,
    }
}

/// Runs `job` on the blocking pool under the deadline `d`. The error is the target's final state.
async fn blocking<T: Send + 'static>(
    cx: &Cx,
    d: Duration,
    job: impl FnOnce() -> Result<T, OpsError> + Send + 'static,
) -> Result<T, ActionState> {
    let mut handle = cx
        .spawn_blocking(move |_| job())
        .map_err(|_| internal_error())?;
    match with_deadline(cx, Some(d), handle.join(cx)).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(e))) => Err(map_ops_error(&e)),
        Ok(Err(_)) => Err(internal_error()),
        Err(_) => Err(ActionState::OutcomeUnknown(Arc::from(TIMED_OUT))),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::OwnedFd;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::actions::{ActionError, Signal};
    use tl_model::time::BootId;
    use tl_ops::cgroup::CgroupDir;

    use super::*;
    use crate::actions::ActionOps;
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;

    const APP_SCOPE: &str = "user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope";

    /// Scripted cgroup ops. `open` hands out a directory fd of the temp dir, so nothing writes
    /// to a real cgroup.
    struct FakeOps {
        open: Result<(), OpsError>,
        write: Result<(), OpsError>,
        frozen: Result<(), OpsError>,
        log: Mutex<Vec<String>>,
    }

    impl FakeOps {
        fn ok() -> Self {
            Self {
                open: Ok(()),
                write: Ok(()),
                frozen: Ok(()),
                log: Mutex::new(Vec::new()),
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl ActionOps for FakeOps {
        fn cgroup_open_validated(&self, cg: &CgroupRef, uid: u32) -> Result<CgroupDir, OpsError> {
            assert_eq!(&*cg.rel_path, APP_SCOPE);
            assert_eq!(uid, 1000);
            self.log.lock().unwrap().push("open".into());
            self.open.clone()?;
            let dir = File::open(env!("CARGO_MANIFEST_DIR")).expect("crate dir opens");
            Ok(CgroupDir::from_validated_fd(OwnedFd::from(dir)))
        }

        fn cgroup_write(&self, _: &CgroupDir, file: &str, value: &str) -> Result<(), OpsError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("write({file}, {value:?})"));
            self.write.clone()
        }

        fn cgroup_wait_frozen(&self, _: &CgroupDir, wait: Duration) -> Result<(), OpsError> {
            assert_eq!(wait, FREEZE_WAIT);
            self.log.lock().unwrap().push("wait_frozen".into());
            self.frozen.clone()
        }
    }

    /// Runs one target through `run_target` on the lab runtime and returns its state.
    fn run(ops: Arc<FakeOps>, kind: ActionKind) -> ActionState {
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = asupersync::channel::mpsc::channel(16);
        let actx = Arc::new(ActionCtx {
            events,
            msgs,
            ops,
            boot_id: BootId(1),
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
        let cg = CgroupRef {
            rel_path: Arc::from(APP_SCOPE),
            cgroup_id: 42,
        };
        let out = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&out);
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = run_target(&cx, &actx, &kind, &cg).await;
                *slot.lock().unwrap() = Some(state);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        out.lock().unwrap().take().expect("run_target finished")
    }

    #[test]
    fn cgroup_freeze_waits_frozen() {
        let ops = Arc::new(FakeOps::ok());
        let state = run(Arc::clone(&ops), ActionKind::FreezeGroup);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            ops.log(),
            ["open", r#"write(cgroup.freeze, "1")"#, "wait_frozen"]
        );
    }

    #[test]
    fn cgroup_freeze_not_frozen_unknown() {
        let ops = Arc::new(FakeOps {
            frozen: Err(OpsError::OutcomeUnknown("freeze still in progress")),
            ..FakeOps::ok()
        });
        let state = run(ops, ActionKind::FreezeGroup);
        assert_eq!(
            state,
            ActionState::OutcomeUnknown(Arc::from("freeze still in progress"))
        );
    }

    #[test]
    fn cgroup_thaw_write() {
        let ops = Arc::new(FakeOps::ok());
        let state = run(Arc::clone(&ops), ActionKind::ThawGroup);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(ops.log(), ["open", r#"write(cgroup.freeze, "0")"#]);
    }

    /// tl-ops reports a bad path, suffix or uid as `NotOwner` and a changed inode as
    /// `IdentityChanged`. Either way nothing is written.
    #[test]
    fn cgroup_invalid_rejected() {
        for (err, reason) in [
            (OpsError::NotOwner, RejectReason::NotOwner),
            (OpsError::IdentityChanged, RejectReason::IdentityChanged),
            (OpsError::Unsupported("x"), RejectReason::Unsupported),
        ] {
            let ops = Arc::new(FakeOps {
                open: Err(err),
                ..FakeOps::ok()
            });
            let state = run(Arc::clone(&ops), ActionKind::FreezeGroup);
            assert_eq!(state, ActionState::Rejected(reason));
            assert_eq!(ops.log(), ["open"]);
        }
    }

    #[test]
    fn cgroup_own_protected() {
        let ops = Arc::new(FakeOps {
            open: Err(OpsError::Protected),
            ..FakeOps::ok()
        });
        let state = run(Arc::clone(&ops), ActionKind::KillGroup);
        assert_eq!(state, ActionState::Rejected(RejectReason::Protected));
        assert_eq!(ops.log(), ["open"]);
    }

    #[test]
    fn cgroup_kill_write() {
        let ops = Arc::new(FakeOps::ok());
        let state = run(Arc::clone(&ops), ActionKind::KillGroup);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(ops.log(), ["open", r#"write(cgroup.kill, "1")"#]);
    }

    #[test]
    fn cgroup_write_errno_mapped() {
        for (err, state) in [
            (
                OpsError::Os(libc_eacces()),
                ActionState::Failed(ActionError::PermissionDenied),
            ),
            (
                OpsError::PermissionDenied,
                ActionState::Failed(ActionError::PermissionDenied),
            ),
            (OpsError::Os(5), ActionState::Failed(ActionError::Os(5))),
        ] {
            let ops = Arc::new(FakeOps {
                write: Err(err),
                ..FakeOps::ok()
            });
            assert_eq!(run(Arc::clone(&ops), ActionKind::FreezeGroup), state);
            // A failed write never waits for frozen.
            assert_eq!(ops.log(), ["open", r#"write(cgroup.freeze, "1")"#]);
        }
    }

    fn libc_eacces() -> i32 {
        rustix::io::Errno::ACCESS.raw_os_error()
    }

    #[test]
    fn cgroup_wrong_kind_unsupported() {
        let ops = Arc::new(FakeOps::ok());
        let state = run(Arc::clone(&ops), ActionKind::Signal(Signal::Term));
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(ops.log().is_empty());
    }
}
