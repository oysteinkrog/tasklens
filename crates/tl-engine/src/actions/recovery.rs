//! Recovery tab edits (SYS-013, ADR-006, E11-020). `UnitWriteRecovery` writes a drop-in with only
//! the changed keys and `UnitRemoveRecovery` deletes it. The owning manager then reloads.
//!
//! A user unit's drop-in is written here with `write_atomic` under
//! `$XDG_CONFIG_HOME/systemd/user/<unit>.d/`. A system unit's drop-in is written by the helper
//! (`HelperRequest::WriteServiceDropIn`) at its own fixed path under `/etc/systemd/system`; the
//! engine never writes there. Both use the one renderer in `tl_ops::dropin` (R-248).
//!
//! The unit name and every value pass `validate_drop_in` before any path is built or any call is
//! made. Log the unit name at debug only.

use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asupersync::Cx;
use rustix::io::Errno;
use tl_model::actions::{
    ActionError, ActionKind, ActionState, ActionTarget, RecoveryEdit, RejectReason,
};
use tl_model::health::HelperState;
use tl_model::helper_proto::{
    DropInKey, HelperError, HelperReplyBody, HelperRequest, validate_drop_in,
};
use tl_model::table::{Manager, UnitRef};
use tl_ops::OpsError;

pub use tl_ops::dropin::{HEADER, render};

use super::{
    ActionCtx, HelperCallError, PROMPT_DEADLINE, TIMED_OUT, deadline, internal_error,
    map_ops_error, unit, with_deadline,
};

/// The file name of the tasklens drop-in in `<unit>.d`.
pub const DROP_IN_NAME: &str = "50-tasklens-recovery.conf";
/// The mode of a user drop-in file.
const DROP_IN_MODE: u32 = 0o644;
/// The mode of a `<unit>.d` directory the engine creates.
const DROP_IN_DIR_MODE: u32 = 0o755;
/// The `OutcomeUnknown` text when the file changed but `Manager.Reload()` failed.
const RELOAD_FAILED: &str = "written, but the manager reload failed";
/// The `Internal` text when the owning manager has no connection.
const NOT_CONNECTED: &str = "manager not connected";
/// The `OutcomeUnknown` text for a helper call that ran out of time.
const HELPER_NO_ANSWER: &str = "no answer from the administrator helper";
/// The `Internal` text when the helper process ended or broke the protocol.
const HELPER_STOPPED: &str = "administrator helper stopped";
/// The `Internal` text when the helper refused the request.
const HELPER_REFUSED: &str = "refused by the administrator helper";

/// The drop-in keys for `edit`: one per field that is set. Times are written in microseconds
/// with the `us` suffix; `OnFailure` joins the unit names with one space.
pub fn keys_of(edit: &RecoveryEdit) -> Vec<(DropInKey, String)> {
    let usec = |n: u64| format!("{n}us");
    let fields = [
        (
            DropInKey::Restart,
            edit.restart.as_deref().map(str::to_owned),
        ),
        (DropInKey::RestartSec, edit.restart_usec.map(usec)),
        (
            DropInKey::RestartSteps,
            edit.restart_steps.map(|n| n.to_string()),
        ),
        (
            DropInKey::RestartMaxDelaySec,
            edit.restart_max_delay_usec.map(usec),
        ),
        (
            DropInKey::StartLimitBurst,
            edit.start_limit_burst.map(|n| n.to_string()),
        ),
        (
            DropInKey::StartLimitIntervalSec,
            edit.start_limit_interval_usec.map(usec),
        ),
        (
            DropInKey::StartLimitAction,
            edit.start_limit_action.as_deref().map(str::to_owned),
        ),
        (
            DropInKey::FailureAction,
            edit.failure_action.as_deref().map(str::to_owned),
        ),
        (
            DropInKey::OnFailure,
            edit.on_failure.as_deref().map(|names| names.join(" ")),
        ),
    ];
    fields
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect()
}

/// `<config_home>/systemd/user/<unit>.d/50-tasklens-recovery.conf`. The caller has validated
/// `unit`.
pub fn user_drop_in_path(config_home: &Path, unit: &str) -> PathBuf {
    drop_in_under(&config_home.join("systemd").join("user"), unit)
}

/// `<systemd_user>/<unit>.d/50-tasklens-recovery.conf`.
fn drop_in_under(systemd_user: &Path, unit: &str) -> PathBuf {
    systemd_user.join(format!("{unit}.d")).join(DROP_IN_NAME)
}

/// Writes or removes the recovery drop-in of `unit`, then reloads its manager. Any other kind is
/// `Rejected(Unsupported)`. A bad unit name or value is `Rejected(Unsupported)` before anything
/// is written or called.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    unit: &UnitRef,
) -> ActionState {
    let keys = match kind {
        ActionKind::UnitWriteRecovery(edit) => keys_of(edit),
        ActionKind::UnitRemoveRecovery => Vec::new(),
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    if validate_drop_in(&unit.name, &keys).is_err() {
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    let state = match unit.manager {
        Manager::User => user(cx, actx, kind, unit, keys).await,
        Manager::System => system(cx, actx, unit, keys).await,
    };
    tracing::debug!(target: "tasklens::action", unit = &*unit.name, result = ?state, "recovery drop-in");
    state
}

/// Writes or removes the user drop-in on the blocking pool under the 2 s deadline, then reloads
/// the user manager.
async fn user(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    unit: &UnitRef,
    keys: Vec<(DropInKey, String)>,
) -> ActionState {
    let Some(bus) = unit::bus_for(actx, Manager::User) else {
        return not_connected();
    };
    let limit = deadline(kind, Some(&ActionTarget::Unit(unit.clone())), false);
    let path = drop_in_under(&actx.startup_paths.roots.systemd_user, &unit.name);
    let ops = Arc::clone(&actx.ops);
    let job = move || -> Result<(), OpsError> {
        let dir = path.parent().unwrap_or(Path::new(""));
        if keys.is_empty() {
            match ops.remove_file(&path) {
                Ok(()) => {}
                Err(OpsError::Os(n)) if n == Errno::NOENT.raw_os_error() => {}
                Err(e) => return Err(e),
            }
            ops.remove_dir_if_empty(dir)
        } else {
            DirBuilder::new()
                .recursive(true)
                .mode(DROP_IN_DIR_MODE)
                .create(dir)?;
            ops.write_atomic(&path, &render(&keys), DROP_IN_MODE)
        }
    };
    let mut handle = match cx.spawn_blocking(move |_| job()) {
        Ok(handle) => handle,
        Err(_) => return internal_error(),
    };
    match with_deadline(cx, limit, handle.join(cx)).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => return map_ops_error(&e),
        Ok(Err(_)) => return internal_error(),
        Err(_) => return ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
    }
    reload(cx, &*bus, Manager::User).await
}

/// Asks the helper to write or remove the system drop-in, then reloads the system manager. No
/// installed helper is `Rejected(Unsupported)` (UI tooltip "Needs the tasklens helper").
async fn system(
    cx: &Cx,
    actx: &ActionCtx,
    unit: &UnitRef,
    keys: Vec<(DropInKey, String)>,
) -> ActionState {
    let Some(helper) = actx
        .helper
        .as_ref()
        .filter(|h| h.state() != HelperState::Missing)
    else {
        return ActionState::Rejected(RejectReason::Unsupported);
    };
    let Some(bus) = unit::bus_for(actx, Manager::System) else {
        return not_connected();
    };
    let req = HelperRequest::WriteServiceDropIn {
        unit: unit.name.to_string(),
        keys,
    };
    let written = match helper.call(cx, req, None, PROMPT_DEADLINE).await {
        Ok(replies) => match replies.last().map(|r| &r.body) {
            Some(HelperReplyBody::Ok) => ActionState::Succeeded,
            Some(HelperReplyBody::Err(e)) => map_helper_error(*e),
            _ => ActionState::Failed(ActionError::Internal(Arc::from(HELPER_STOPPED))),
        },
        Err(e) => map_call_error(e),
    };
    if written != ActionState::Succeeded {
        return written;
    }
    reload(cx, &*bus, Manager::System).await
}

/// `Manager.Reload()` after a change. A failure is `OutcomeUnknown`: the file changed, but the
/// manager may not have read it.
async fn reload(cx: &Cx, bus: &dyn unit::UnitBus, manager: Manager) -> ActionState {
    match unit::reload(cx, bus, manager).await {
        Ok(()) => ActionState::Succeeded,
        Err(e) => {
            tracing::debug!(target: "tasklens::action", error = ?reload_error_name(&e), "manager reload failed");
            ActionState::OutcomeUnknown(Arc::from(RELOAD_FAILED))
        }
    }
}

/// Only the error name: the message can hold a unit name.
fn reload_error_name(e: &unit::UnitBusError) -> &str {
    match e {
        unit::UnitBusError::Call { name, .. } => name,
        unit::UnitBusError::Timeout => "timed out",
        unit::UnitBusError::Closed => "closed",
        unit::UnitBusError::NoConnection => "no connection",
    }
}

fn not_connected() -> ActionState {
    ActionState::Failed(ActionError::Internal(Arc::from(NOT_CONNECTED)))
}

/// The E29-014 mapping of a helper error, as on the dump path (E11-018).
fn map_helper_error(e: HelperError) -> ActionState {
    match e {
        HelperError::IdentityChanged => ActionState::Rejected(RejectReason::IdentityChanged),
        HelperError::PermissionDenied => ActionState::Failed(ActionError::PermissionDenied),
        HelperError::NoSuchProcess => ActionState::Failed(ActionError::NoSuchProcess),
        HelperError::Partial { done, total } => {
            ActionState::Failed(ActionError::Partial { done, total })
        }
        HelperError::Os(n) => ActionState::Failed(ActionError::Os(n)),
        HelperError::NotAllowed | HelperError::TooLarge => {
            ActionState::Failed(ActionError::Internal(Arc::from(HELPER_REFUSED)))
        }
    }
}

/// The E29-014 mapping of a failed helper call, as on the dump path (E11-018, R-148 item 3).
fn map_call_error(e: HelperCallError) -> ActionState {
    match e {
        HelperCallError::Missing => ActionState::Rejected(RejectReason::Unsupported),
        HelperCallError::Cancelled => ActionState::Failed(ActionError::AuthCancelled),
        HelperCallError::NotAuthorized => ActionState::Failed(ActionError::NotAuthorized),
        HelperCallError::StartFailed(reason) => ActionState::Failed(ActionError::Internal(reason)),
        HelperCallError::Timeout => ActionState::OutcomeUnknown(Arc::from(HELPER_NO_ANSWER)),
        HelperCallError::Exited | HelperCallError::Protocol(_) => {
            ActionState::Failed(ActionError::Internal(Arc::from(HELPER_STOPPED)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::future::Future;
    use std::os::fd::OwnedFd;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use asupersync::channel::mpsc;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::helper_proto::HelperReply;
    use tl_model::time::BootId;
    use tl_ops::fs::StartupRoots;
    use zbus::zvariant::{OwnedValue, Value};

    use super::*;
    use crate::actions::startup::StartupPaths;
    use crate::actions::unit::{BusFuture, JobStream, UnitBus, UnitBusError, UnitBusOverride};
    use crate::actions::{ActionOps, PrivilegedOps};
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;

    fn edit_full() -> RecoveryEdit {
        RecoveryEdit {
            restart: Some("on-failure".into()),
            restart_usec: Some(5_000_000),
            start_limit_burst: Some(3),
            start_limit_interval_usec: Some(60_000_000),
            ..Default::default()
        }
    }

    #[test]
    fn render_full() {
        assert_eq!(
            render(&keys_of(&edit_full())),
            format!(
                "{HEADER}[Unit]\nStartLimitIntervalSec=60000000us\nStartLimitBurst=3\n\
                 [Service]\nRestart=on-failure\nRestartSec=5000000us\n"
            )
        );
    }

    #[test]
    fn render_unit_only() {
        let edit = RecoveryEdit {
            on_failure: Some(Arc::from(vec![
                Arc::<str>::from("a.service"),
                Arc::from("b.service"),
            ])),
            ..Default::default()
        };
        assert_eq!(
            render(&keys_of(&edit)),
            format!("{HEADER}[Unit]\nOnFailure=a.service b.service\n")
        );
    }

    #[test]
    fn keys_of_table() {
        let edit = RecoveryEdit {
            restart: Some("always".into()),
            restart_usec: Some(100),
            restart_steps: Some(4),
            restart_max_delay_usec: Some(7),
            start_limit_burst: Some(5),
            start_limit_interval_usec: Some(9),
            start_limit_action: Some("reboot".into()),
            failure_action: Some("none".into()),
            on_failure: Some(Arc::from(vec![Arc::<str>::from("x.service")])),
        };
        let pairs = |keys: Vec<(DropInKey, String)>| -> Vec<(&'static str, String)> {
            keys.into_iter().map(|(k, v)| (k.as_str(), v)).collect()
        };
        assert_eq!(
            pairs(keys_of(&edit)),
            [
                ("Restart", "always"),
                ("RestartSec", "100us"),
                ("RestartSteps", "4"),
                ("RestartMaxDelaySec", "7us"),
                ("StartLimitBurst", "5"),
                ("StartLimitIntervalSec", "9us"),
                ("StartLimitAction", "reboot"),
                ("FailureAction", "none"),
                ("OnFailure", "x.service"),
            ]
            .map(|(k, v)| (k, v.to_owned()))
        );
        assert!(keys_of(&RecoveryEdit::default()).is_empty());
    }

    #[test]
    fn user_drop_in_path_shape() {
        assert_eq!(
            user_drop_in_path(Path::new("/home/u/.config"), "foo.service"),
            PathBuf::from("/home/u/.config/systemd/user/foo.service.d/50-tasklens-recovery.conf")
        );
        // The same path the action builds from the startup roots.
        let roots = StartupRoots::from_config_home(Path::new("/home/u/.config"));
        assert_eq!(
            drop_in_under(&roots.systemd_user, "foo.service"),
            user_drop_in_path(Path::new("/home/u/.config"), "foo.service")
        );
    }

    /// A config home under `target/tmp`, removed on drop. Never the real `~/.config`.
    struct ConfigDir(PathBuf);

    impl ConfigDir {
        fn new(name: &str) -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/tmp")
                .join(format!("tl-engine-recovery-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create config dir");
            Self(dir)
        }

        fn drop_in(&self) -> PathBuf {
            user_drop_in_path(&self.0, "foo.service")
        }
    }

    impl Drop for ConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Logs every file call; writes nothing.
    #[derive(Default)]
    struct FakeOps {
        log: Mutex<Vec<String>>,
    }

    impl ActionOps for FakeOps {
        fn write_atomic(&self, path: &Path, text: &str, mode: u32) -> tl_ops::OpsResult {
            self.log.lock().unwrap().push(format!(
                "write_atomic({}, {text:?}, {mode:o})",
                path.display()
            ));
            Ok(())
        }

        fn remove_file(&self, path: &Path) -> tl_ops::OpsResult {
            self.log
                .lock()
                .unwrap()
                .push(format!("remove_file({})", path.display()));
            Err(OpsError::Os(Errno::NOENT.raw_os_error()))
        }

        fn remove_dir_if_empty(&self, path: &Path) -> tl_ops::OpsResult {
            self.log
                .lock()
                .unwrap()
                .push(format!("remove_dir_if_empty({})", path.display()));
            Ok(())
        }
    }

    /// Logs `(manager, method)` for each call and answers from `reload`.
    struct FakeBus {
        manager: Manager,
        reload: Result<(), UnitBusError>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl UnitBus for FakeBus {
        fn call<'a>(
            &'a self,
            _cx: &'a Cx,
            method: &'a str,
            args: &'a [Value<'static>],
            timeout: Duration,
        ) -> BusFuture<'a, OwnedValue> {
            self.log.lock().unwrap().push(format!(
                "{:?} {method} {} {}s",
                self.manager,
                args.len(),
                timeout.as_secs()
            ));
            let reply = self.reload.clone().map(|()| OwnedValue::from(true));
            Box::pin(async move { reply })
        }

        fn job_removed<'a>(&'a self, _cx: &'a Cx) -> BusFuture<'a, JobStream> {
            unreachable!("recovery never waits for a job")
        }

        fn get_property<'a>(
            &'a self,
            _cx: &'a Cx,
            _unit_path: &'a str,
            _iface: &'a str,
            _name: &'a str,
        ) -> BusFuture<'a, OwnedValue> {
            unreachable!("recovery reads no property")
        }
    }

    struct FakeHelper {
        state: HelperState,
        calls: Mutex<Vec<HelperRequest>>,
    }

    impl PrivilegedOps for FakeHelper {
        fn call(
            &self,
            _cx: &Cx,
            req: HelperRequest,
            fd: Option<OwnedFd>,
            _deadline: Duration,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<HelperReply>, HelperCallError>> + Send + '_>>
        {
            assert!(fd.is_none());
            self.calls.lock().unwrap().push(req);
            Box::pin(async move {
                Ok(vec![HelperReply {
                    seq: 1,
                    last: true,
                    body: HelperReplyBody::Ok,
                }])
            })
        }

        fn state(&self) -> HelperState {
            self.state
        }
    }

    struct Rig {
        ops: Arc<FakeOps>,
        bus_log: Arc<Mutex<Vec<String>>>,
        actx: Arc<ActionCtx>,
    }

    fn rig(
        dir: &ConfigDir,
        helper: Option<Arc<FakeHelper>>,
        reload: Result<(), UnitBusError>,
    ) -> Rig {
        let ops = Arc::new(FakeOps::default());
        let bus_log = Arc::new(Mutex::new(Vec::new()));
        let pick_log = Arc::clone(&bus_log);
        let pick: UnitBusOverride = Arc::new(move |manager| {
            Some(Arc::new(FakeBus {
                manager,
                reload: reload.clone(),
                log: Arc::clone(&pick_log),
            }) as Arc<dyn UnitBus>)
        });
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = mpsc::channel(16);
        let actx = Arc::new(ActionCtx {
            events,
            msgs,
            ops: Arc::clone(&ops) as Arc<dyn ActionOps>,
            boot_id: BootId(1),
            uid: 1000,
            config: Arc::new(EngineConfig::new(EngineMode::Full, dir.0.join("state"))),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: helper.map(|h| h as Arc<dyn PrivilegedOps>),
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: Some(pick),
            startup_paths: StartupPaths {
                roots: StartupRoots::from_config_home(&dir.0),
                system_autostart_dirs: Vec::new(),
            },
        });
        Rig { ops, bus_log, actx }
    }

    fn run(rig: &Rig, kind: ActionKind, manager: Manager, name: &str) -> ActionState {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let out = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&out);
        let actx = Arc::clone(&rig.actx);
        let unit = UnitRef {
            manager,
            name: name.into(),
        };
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = run_target(&cx, &actx, &kind, &unit).await;
                *slot.lock().unwrap() = Some(state);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        out.lock().unwrap().take().expect("run_target finished")
    }

    fn ops_log(rig: &Rig) -> Vec<String> {
        rig.ops.log.lock().unwrap().clone()
    }

    fn bus_log(rig: &Rig) -> Vec<String> {
        rig.bus_log.lock().unwrap().clone()
    }

    #[test]
    fn user_write_then_reload() {
        let dir = ConfigDir::new("write");
        let rig = rig(&dir, None, Ok(()));
        let kind = ActionKind::UnitWriteRecovery(edit_full());
        assert_eq!(
            run(&rig, kind, Manager::User, "foo.service"),
            ActionState::Succeeded
        );
        let text = render(&keys_of(&edit_full()));
        assert_eq!(
            ops_log(&rig),
            [format!(
                "write_atomic({}, {text:?}, 644)",
                dir.drop_in().display()
            )]
        );
        assert_eq!(bus_log(&rig), ["User Reload 0 10s"]);
        assert!(dir.drop_in().parent().unwrap().is_dir());
    }

    #[test]
    fn user_remove_then_reload() {
        let dir = ConfigDir::new("remove");
        let rig = rig(&dir, None, Ok(()));
        // `remove_file` answers ENOENT, which is fine.
        assert_eq!(
            run(
                &rig,
                ActionKind::UnitRemoveRecovery,
                Manager::User,
                "foo.service"
            ),
            ActionState::Succeeded
        );
        let path = dir.drop_in();
        assert_eq!(
            ops_log(&rig),
            [
                format!("remove_file({})", path.display()),
                format!("remove_dir_if_empty({})", path.parent().unwrap().display()),
            ]
        );
        assert_eq!(bus_log(&rig), ["User Reload 0 10s"]);
    }

    #[test]
    fn user_reload_failure_is_unknown() {
        let dir = ConfigDir::new("reload");
        let rig = rig(&dir, None, Err(UnitBusError::Timeout));
        assert_eq!(
            run(
                &rig,
                ActionKind::UnitWriteRecovery(edit_full()),
                Manager::User,
                "foo.service"
            ),
            ActionState::OutcomeUnknown(Arc::from(RELOAD_FAILED))
        );
    }

    #[test]
    fn system_via_helper_then_reload() {
        let dir = ConfigDir::new("system");
        let helper = Arc::new(FakeHelper {
            state: HelperState::Locked,
            calls: Mutex::default(),
        });
        let rig = rig(&dir, Some(Arc::clone(&helper)), Ok(()));
        assert_eq!(
            run(
                &rig,
                ActionKind::UnitWriteRecovery(edit_full()),
                Manager::System,
                "foo.service"
            ),
            ActionState::Succeeded
        );
        assert_eq!(
            *helper.calls.lock().unwrap(),
            [HelperRequest::WriteServiceDropIn {
                unit: "foo.service".into(),
                keys: keys_of(&edit_full()),
            }]
        );
        assert_eq!(bus_log(&rig), ["System Reload 0 120s"]);
        assert!(ops_log(&rig).is_empty());
    }

    #[test]
    fn system_without_helper_is_unsupported() {
        let dir = ConfigDir::new("nohelper");
        for helper in [
            None,
            Some(Arc::new(FakeHelper {
                state: HelperState::Missing,
                calls: Mutex::default(),
            })),
        ] {
            let rig = rig(&dir, helper.clone(), Ok(()));
            assert_eq!(
                run(
                    &rig,
                    ActionKind::UnitRemoveRecovery,
                    Manager::System,
                    "foo.service"
                ),
                ActionState::Rejected(RejectReason::Unsupported)
            );
            assert!(bus_log(&rig).is_empty());
            if let Some(helper) = helper {
                assert!(helper.calls.lock().unwrap().is_empty());
            }
        }
    }

    /// Security regression: a path escape in the unit name or a second line in a value writes
    /// nothing and calls nothing.
    #[test]
    fn rejects_bad_unit_or_value() {
        let dir = ConfigDir::new("bad");
        let helper = Arc::new(FakeHelper {
            state: HelperState::Unlocked,
            calls: Mutex::default(),
        });
        let rig = rig(&dir, Some(Arc::clone(&helper)), Ok(()));
        let injected = RecoveryEdit {
            restart: Some("always\nExecStartPre=/bin/sh".into()),
            ..Default::default()
        };
        for manager in [Manager::User, Manager::System] {
            for (kind, name) in [
                (
                    ActionKind::UnitWriteRecovery(edit_full()),
                    "../evil.service",
                ),
                (ActionKind::UnitRemoveRecovery, "../evil.service"),
                (ActionKind::UnitWriteRecovery(edit_full()), "a/b.service"),
                (
                    ActionKind::UnitWriteRecovery(injected.clone()),
                    "foo.service",
                ),
            ] {
                assert_eq!(
                    run(&rig, kind, manager, name),
                    ActionState::Rejected(RejectReason::Unsupported)
                );
            }
        }
        assert!(ops_log(&rig).is_empty());
        assert!(bus_log(&rig).is_empty());
        assert!(helper.calls.lock().unwrap().is_empty());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }

    #[test]
    fn other_kind_is_unsupported() {
        let dir = ConfigDir::new("other");
        let rig = rig(&dir, None, Ok(()));
        assert_eq!(
            run(&rig, ActionKind::UnitStart, Manager::User, "foo.service"),
            ActionState::Rejected(RejectReason::Unsupported)
        );
        assert!(ops_log(&rig).is_empty());
    }
}
