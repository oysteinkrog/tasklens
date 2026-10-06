//! Startup Apps actions on an `ActionTarget::Startup` (SYS-022, ADR-006, E11-014). Part 1:
//! Enable and Disable.
//!
//! A system XDG entry is disabled by writing a user override and enabled by removing it. A user
//! XDG entry toggles `Hidden` in place. A Plasma script toggles its executable bit. A systemd
//! user unit goes through `unit::set_unit_file_state` on the user manager. File work runs on the
//! blocking pool through `ActionOps`, and `tl-ops` confines every path to the roots a second
//! time. Changes take effect at next login; running programs are not stopped.
//!
//! The entry id is checked by `valid_id` before any path is built. Log the id at debug only and
//! never file contents (ADR-015).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use asupersync::Cx;
use tl_model::actions::{ActionError, ActionKind, ActionState, ActionTarget, RejectReason};
use tl_model::table::{Manager, StartupRef, StartupSource, UnitRef};
use tl_ops::OpsError;
use tl_ops::fs::StartupRoots;

use super::{ActionCtx, TIMED_OUT, deadline, internal_error, map_ops_error, unit, with_deadline};

/// The unit suffixes a user-unit startup entry may have.
const UNIT_SUFFIXES: [&str; 8] = [
    ".service", ".socket", ".target", ".timer", ".mount", ".path", ".slice", ".scope",
];
/// The `InvalidInput` text of `autostart_disable_system` when the user file already exists.
const USER_ENTRY_EXISTS: &str = "user entry already exists";
const SYSTEM_ENTRY_NOT_FOUND: &str = "system entry not found";
const NOT_CONNECTED: &str = "manager not connected";

/// Where startup entries live. `ActionCtx` holds one, built once by `gate_factory`; tests point it
/// at a temp directory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartupPaths {
    /// The user roots under `$XDG_CONFIG_HOME` (default `$HOME/.config`).
    pub roots: StartupRoots,
    /// Each `$XDG_CONFIG_DIRS` entry (default `/etc/xdg`) plus `/autostart`, in precedence order.
    pub system_autostart_dirs: Vec<PathBuf>,
}

impl StartupPaths {
    /// Reads `HOME`, `XDG_CONFIG_HOME` and `XDG_CONFIG_DIRS`. Empty or relative values are
    /// ignored, as the XDG Base Directory spec says.
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(std::env::home_dir);
        Self::from_vars(
            home,
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("XDG_CONFIG_DIRS"),
        )
    }

    fn from_vars(
        home: Option<PathBuf>,
        config_home: Option<OsString>,
        config_dirs: Option<OsString>,
    ) -> Self {
        let config_home = config_home
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.unwrap_or_default().join(".config"));
        let mut system_autostart_dirs: Vec<PathBuf> = config_dirs
            .as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .filter(|p| p.is_absolute())
            .map(|p| p.join("autostart"))
            .collect();
        if system_autostart_dirs.is_empty() {
            system_autostart_dirs.push(PathBuf::from("/etc/xdg/autostart"));
        }
        Self {
            roots: StartupRoots::from_config_home(&config_home),
            system_autostart_dirs,
        }
    }
}

/// The user file of `r`: `roots.autostart/<id>`, `roots.systemd_user/<id>`,
/// `roots.plasma_env/<id>` or `roots.plasma_shutdown/<id>`. A system entry has none.
pub fn user_file(paths: &StartupPaths, r: &StartupRef) -> Option<PathBuf> {
    let root = match r.source {
        StartupSource::AutostartUser => &paths.roots.autostart,
        StartupSource::UserUnit => &paths.roots.systemd_user,
        StartupSource::PlasmaEnv => &paths.roots.plasma_env,
        StartupSource::PlasmaShutdown => &paths.roots.plasma_shutdown,
        StartupSource::AutostartSystem => return None,
    };
    Some(root.join(&*r.id))
}

/// The first existing `<dir>/<id>` over `system_autostart_dirs` (SYS-019 lookup precedence).
/// It reads the file system, so call it on the blocking pool.
pub fn system_file(paths: &StartupPaths, id: &str) -> Option<PathBuf> {
    paths
        .system_autostart_dirs
        .iter()
        .map(|dir| dir.join(id))
        .find(|p| p.exists())
}

/// A file name that is safe to join to a root: non-empty, no `/`, not `.` or `..`, no NUL.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\0'])
}

/// `valid_id` plus the suffix of the source: `.desktop` for autostart entries, a known unit
/// suffix for user units. A Plasma script may have any name.
fn valid_for(source: StartupSource, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    match source {
        StartupSource::AutostartUser | StartupSource::AutostartSystem => {
            id.len() > ".desktop".len() && id.ends_with(".desktop")
        }
        StartupSource::UserUnit => UNIT_SUFFIXES
            .iter()
            .any(|s| id.len() > s.len() && id.ends_with(s)),
        StartupSource::PlasmaEnv | StartupSource::PlasmaShutdown => true,
    }
}

/// Runs one startup target. This part handles `StartupEnable` and `StartupDisable`; any other
/// kind is `Rejected(Unsupported)`. A bad id is `Rejected(Unsupported)` before any I/O.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    r: &StartupRef,
) -> ActionState {
    let enable = match kind {
        ActionKind::StartupEnable => true,
        ActionKind::StartupDisable => false,
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    if !valid_for(r.source, &r.id) {
        tracing::debug!(target: "tasklens::action", "startup entry id refused");
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    let state = set_enabled(cx, actx, kind, r, enable).await;
    tracing::debug!(target: "tasklens::action", id = &*r.id, enable, result = ?state, "startup action");
    state
}

async fn set_enabled(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    r: &StartupRef,
    enable: bool,
) -> ActionState {
    let limit = deadline(kind, Some(&ActionTarget::Startup(r.clone())), false);
    if r.source == StartupSource::UserUnit {
        let Some(bus) = unit::bus_for(actx, Manager::User) else {
            return ActionState::Failed(ActionError::Internal(Arc::from(NOT_CONNECTED)));
        };
        let unit = UnitRef {
            manager: Manager::User,
            name: Arc::clone(&r.id),
        };
        let kind = if enable {
            ActionKind::UnitEnable
        } else {
            ActionKind::UnitDisable
        };
        return match with_deadline(
            cx,
            limit,
            unit::set_unit_file_state(cx, &*bus, &unit, &kind),
        )
        .await
        {
            Ok(state) => state,
            Err(_) => ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
        };
    }
    let ops = Arc::clone(&actx.ops);
    let paths = actx.startup_paths.clone();
    let r = r.clone();
    let job = move || -> Result<ActionState, OpsError> {
        let roots = &paths.roots;
        let user = || roots.autostart.join(&*r.id);
        match (r.source, enable) {
            (StartupSource::AutostartSystem, false) => {
                let Some(system) = system_file(&paths, &r.id) else {
                    return Ok(ActionState::Failed(ActionError::Internal(Arc::from(
                        SYSTEM_ENTRY_NOT_FOUND,
                    ))));
                };
                match ops.startup_disable_system(&system, roots) {
                    Ok(_) => {}
                    // The user already has a file of this name: hide that one instead.
                    Err(OpsError::InvalidInput(USER_ENTRY_EXISTS)) => {
                        ops.startup_set_hidden(&user(), true, roots)?;
                    }
                    Err(e) => return Err(e),
                }
            }
            (StartupSource::AutostartSystem, true) => {
                let file = user();
                // No override means the system entry already applies.
                if std::fs::symlink_metadata(&file).is_ok() {
                    ops.startup_enable_override(&file, roots)?;
                }
            }
            (StartupSource::AutostartUser, _) => {
                ops.startup_set_hidden(&user(), !enable, roots)?;
            }
            (StartupSource::PlasmaEnv | StartupSource::PlasmaShutdown, _) => {
                let Some(script) = user_file(&paths, &r) else {
                    return Ok(internal_error());
                };
                ops.startup_script_set_enabled(&script, enable, roots)?;
            }
            // Handled above.
            (StartupSource::UserUnit, _) => return Ok(internal_error()),
        }
        Ok(ActionState::Succeeded)
    };
    let mut handle = match cx.spawn_blocking(move |_| job()) {
        Ok(handle) => handle,
        Err(_) => return internal_error(),
    };
    match with_deadline(cx, limit, handle.join(cx)).await {
        Ok(Ok(Ok(state))) => state,
        Ok(Ok(Err(e))) => map_ops_error(&e),
        Ok(Err(_)) => internal_error(),
        Err(_) => ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::time::BootId;
    use tl_ops::fs::EnableOutcome;
    use zbus::zvariant::{OwnedValue, Value};

    use super::*;
    use crate::actions::ActionOps;
    use crate::actions::unit::{BusFuture, JobStream, UnitBus, UnitBusError, UnitBusOverride};
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;

    /// Scripted startup ops that log each call with its path relative to the fixture root.
    struct FakeOps {
        base: PathBuf,
        disable_system: Result<(), OpsError>,
        result: Result<(), OpsError>,
        log: Mutex<Vec<String>>,
    }

    impl FakeOps {
        fn new(base: &Path) -> Self {
            Self {
                base: base.to_owned(),
                disable_system: Ok(()),
                result: Ok(()),
                log: Mutex::default(),
            }
        }

        fn push(&self, call: &str, path: &Path, roots: &StartupRoots, arg: &str) {
            assert_eq!(
                *roots,
                StartupRoots::from_config_home(&self.base.join("config"))
            );
            let rel = path
                .strip_prefix(&self.base)
                .expect("path under the fixture");
            self.log
                .lock()
                .unwrap()
                .push(format!("{call}({}{arg})", rel.display()));
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl ActionOps for FakeOps {
        fn startup_disable_system(
            &self,
            system_file: &Path,
            roots: &StartupRoots,
        ) -> tl_ops::OpsResult<PathBuf> {
            self.push("disable_system", system_file, roots, "");
            self.disable_system.clone()?;
            Ok(roots.autostart.join(system_file.file_name().unwrap()))
        }

        fn startup_enable_override(
            &self,
            user_file: &Path,
            roots: &StartupRoots,
        ) -> tl_ops::OpsResult<EnableOutcome> {
            self.push("enable_override", user_file, roots, "");
            self.result.clone()?;
            Ok(EnableOutcome::OverrideDeleted)
        }

        fn startup_set_hidden(
            &self,
            user_file: &Path,
            hidden: bool,
            roots: &StartupRoots,
        ) -> tl_ops::OpsResult {
            self.push("set_hidden", user_file, roots, &format!(", {hidden}"));
            self.result.clone()
        }

        fn startup_script_set_enabled(
            &self,
            script: &Path,
            enabled: bool,
            roots: &StartupRoots,
        ) -> tl_ops::OpsResult {
            self.push("script_set_enabled", script, roots, &format!(", {enabled}"));
            self.result.clone()
        }
    }

    /// A user manager that answers `GetUnitFileState` with `disabled` and every other call with
    /// an empty reply.
    #[derive(Default)]
    struct FakeUnitBus {
        calls: Mutex<Vec<String>>,
    }

    fn show(v: &Value<'_>) -> String {
        match v {
            Value::Str(s) => s.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Array(a) => format!("[{}]", a.iter().map(show).collect::<Vec<_>>().join(", ")),
            other => format!("{other:?}"),
        }
    }

    impl UnitBus for FakeUnitBus {
        fn call<'a>(
            &'a self,
            _cx: &'a Cx,
            method: &'a str,
            args: &'a [Value<'static>],
            _timeout: Duration,
        ) -> BusFuture<'a, OwnedValue> {
            let args: Vec<String> = args.iter().map(show).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{method}({})", args.join(", ")));
            let reply = if method == "GetUnitFileState" {
                Value::from("disabled")
            } else {
                Value::Bool(true)
            };
            let reply = OwnedValue::try_from(reply).unwrap();
            Box::pin(async move { Ok(reply) })
        }

        fn job_removed<'a>(&'a self, _cx: &'a Cx) -> BusFuture<'a, JobStream> {
            Box::pin(async { Err(UnitBusError::NoConnection) })
        }

        fn get_property<'a>(
            &'a self,
            _cx: &'a Cx,
            _unit_path: &'a str,
            _iface: &'a str,
            _name: &'a str,
        ) -> BusFuture<'a, OwnedValue> {
            Box::pin(async { Err(UnitBusError::NoConnection) })
        }
    }

    /// A fixture with `config` as `$XDG_CONFIG_HOME` and `etc/xdg` as the only config dir,
    /// under the cargo target tmp dir. Nothing outside it is touched.
    fn fixture(name: &str) -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp/tl-engine-startup")
            .join(name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("config/autostart")).unwrap();
        std::fs::create_dir_all(base.join("etc/xdg/autostart")).unwrap();
        base
    }

    fn paths(base: &Path) -> StartupPaths {
        StartupPaths {
            roots: StartupRoots::from_config_home(&base.join("config")),
            system_autostart_dirs: vec![base.join("etc/xdg/autostart")],
        }
    }

    fn actx(base: &Path, ops: Arc<FakeOps>, bus: Option<Arc<FakeUnitBus>>) -> Arc<ActionCtx> {
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = asupersync::channel::mpsc::channel(16);
        let pick: UnitBusOverride =
            Arc::new(move |_| bus.clone().map(|bus| bus as Arc<dyn UnitBus>));
        Arc::new(ActionCtx {
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
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: Some(pick),
            startup_paths: paths(base),
        })
    }

    fn entry(source: StartupSource, id: &str) -> StartupRef {
        StartupRef {
            source,
            id: id.into(),
        }
    }

    /// Runs one target through `run_target` on the lab runtime and returns its state.
    fn run(actx: Arc<ActionCtx>, kind: ActionKind, r: StartupRef) -> ActionState {
        let out = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&out);
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = run_target(&cx, &actx, &kind, &r).await;
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
    fn paths_from_env_defaults() {
        let p = StartupPaths::from_vars(Some(PathBuf::from("/home/u")), None, None);
        assert_eq!(
            p.roots,
            StartupRoots::from_config_home(Path::new("/home/u/.config"))
        );
        assert_eq!(p.roots.autostart, Path::new("/home/u/.config/autostart"));
        assert_eq!(p.system_autostart_dirs, [Path::new("/etc/xdg/autostart")]);

        // Empty and relative values are ignored.
        let p = StartupPaths::from_vars(
            Some(PathBuf::from("/home/u")),
            Some("rel".into()),
            Some(":rel".into()),
        );
        assert_eq!(p.roots.autostart, Path::new("/home/u/.config/autostart"));
        assert_eq!(p.system_autostart_dirs, [Path::new("/etc/xdg/autostart")]);

        let p = StartupPaths::from_vars(
            Some(PathBuf::from("/home/u")),
            Some("/cfg".into()),
            Some("/a:/b/".into()),
        );
        assert_eq!(p.roots.systemd_user, Path::new("/cfg/systemd/user"));
        assert_eq!(
            p.system_autostart_dirs,
            [Path::new("/a/autostart"), Path::new("/b/autostart")]
        );
    }

    #[test]
    fn valid_id_table() {
        for (id, ok) in [
            ("foo.desktop", true),
            ("x.sh", true),
            (".hidden.desktop", true),
            ("", false),
            (".", false),
            ("..", false),
            ("a/b.desktop", false),
            ("../evil.desktop", false),
            ("/abs.desktop", false),
            ("nul\0.desktop", false),
        ] {
            assert_eq!(valid_id(id), ok, "{id:?}");
        }
        for (source, id, ok) in [
            (StartupSource::AutostartUser, "foo.desktop", true),
            (StartupSource::AutostartSystem, "foo.desktop", true),
            (StartupSource::AutostartUser, ".desktop", false),
            (StartupSource::AutostartUser, "foo.service", false),
            (StartupSource::UserUnit, "foo.service", true),
            (StartupSource::UserUnit, "foo.timer", true),
            (StartupSource::UserUnit, "foo.desktop", false),
            (StartupSource::UserUnit, ".service", false),
            (StartupSource::PlasmaEnv, "x.sh", true),
            (StartupSource::PlasmaShutdown, "x", true),
        ] {
            assert_eq!(valid_for(source, id), ok, "{source:?} {id:?}");
        }
    }

    #[test]
    fn user_file_by_source() {
        let p = paths(Path::new("/f"));
        let file = |source, id| user_file(&p, &entry(source, id));
        assert_eq!(
            file(StartupSource::AutostartUser, "a.desktop").unwrap(),
            Path::new("/f/config/autostart/a.desktop")
        );
        assert_eq!(
            file(StartupSource::UserUnit, "a.service").unwrap(),
            Path::new("/f/config/systemd/user/a.service")
        );
        assert_eq!(
            file(StartupSource::PlasmaEnv, "a.sh").unwrap(),
            Path::new("/f/config/plasma-workspace/env/a.sh")
        );
        assert_eq!(
            file(StartupSource::PlasmaShutdown, "a.sh").unwrap(),
            Path::new("/f/config/plasma-workspace/shutdown/a.sh")
        );
        assert_eq!(file(StartupSource::AutostartSystem, "a.desktop"), None);
    }

    #[test]
    fn system_file_precedence() {
        let base = fixture("precedence");
        std::fs::create_dir_all(base.join("second/autostart")).unwrap();
        std::fs::write(base.join("second/autostart/a.desktop"), "").unwrap();
        let mut p = paths(&base);
        p.system_autostart_dirs.push(base.join("second/autostart"));
        assert_eq!(
            system_file(&p, "a.desktop").unwrap(),
            base.join("second/autostart/a.desktop")
        );
        std::fs::write(base.join("etc/xdg/autostart/a.desktop"), "").unwrap();
        assert_eq!(
            system_file(&p, "a.desktop").unwrap(),
            base.join("etc/xdg/autostart/a.desktop")
        );
        assert_eq!(system_file(&p, "b.desktop"), None);
    }

    #[test]
    fn startup_user_entry_toggle() {
        let base = fixture("user-toggle");
        let ops = Arc::new(FakeOps::new(&base));
        let ctx = actx(&base, Arc::clone(&ops), None);
        let r = entry(StartupSource::AutostartUser, "foo.desktop");
        let state = run(Arc::clone(&ctx), ActionKind::StartupDisable, r.clone());
        assert_eq!(state, ActionState::Succeeded);
        let state = run(ctx, ActionKind::StartupEnable, r);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            ops.log(),
            [
                "set_hidden(config/autostart/foo.desktop, true)",
                "set_hidden(config/autostart/foo.desktop, false)",
            ]
        );
    }

    #[test]
    fn startup_system_disable_writes_override() {
        let base = fixture("system-disable");
        std::fs::write(base.join("etc/xdg/autostart/bar.desktop"), "").unwrap();
        let ops = Arc::new(FakeOps::new(&base));
        let r = entry(StartupSource::AutostartSystem, "bar.desktop");
        let state = run(
            actx(&base, Arc::clone(&ops), None),
            ActionKind::StartupDisable,
            r,
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(ops.log(), ["disable_system(etc/xdg/autostart/bar.desktop)"]);
    }

    #[test]
    fn startup_system_disable_not_found() {
        let base = fixture("system-missing");
        let ops = Arc::new(FakeOps::new(&base));
        let r = entry(StartupSource::AutostartSystem, "bar.desktop");
        let state = run(
            actx(&base, Arc::clone(&ops), None),
            ActionKind::StartupDisable,
            r,
        );
        assert_eq!(
            state,
            ActionState::Failed(ActionError::Internal(Arc::from("system entry not found")))
        );
        assert!(ops.log().is_empty());
    }

    #[test]
    fn startup_system_disable_falls_back_to_existing_user_entry() {
        let base = fixture("system-fallback");
        std::fs::write(base.join("etc/xdg/autostart/bar.desktop"), "").unwrap();
        let ops = Arc::new(FakeOps {
            disable_system: Err(OpsError::InvalidInput("user entry already exists")),
            ..FakeOps::new(&base)
        });
        let r = entry(StartupSource::AutostartSystem, "bar.desktop");
        let state = run(
            actx(&base, Arc::clone(&ops), None),
            ActionKind::StartupDisable,
            r,
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            ops.log(),
            [
                "disable_system(etc/xdg/autostart/bar.desktop)",
                "set_hidden(config/autostart/bar.desktop, true)",
            ]
        );
    }

    #[test]
    fn startup_system_enable_override() {
        let base = fixture("system-enable");
        let ops = Arc::new(FakeOps::new(&base));
        let ctx = actx(&base, Arc::clone(&ops), None);
        let r = entry(StartupSource::AutostartSystem, "bar.desktop");
        // No override: already enabled, nothing to do.
        let state = run(Arc::clone(&ctx), ActionKind::StartupEnable, r.clone());
        assert_eq!(state, ActionState::Succeeded);
        assert!(ops.log().is_empty());

        std::fs::write(base.join("config/autostart/bar.desktop"), "").unwrap();
        let state = run(ctx, ActionKind::StartupEnable, r);
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(ops.log(), ["enable_override(config/autostart/bar.desktop)"]);
    }

    #[test]
    fn startup_user_unit_enable() {
        let base = fixture("user-unit");
        let ops = Arc::new(FakeOps::new(&base));
        let bus = Arc::new(FakeUnitBus::default());
        let r = entry(StartupSource::UserUnit, "foo.service");
        let state = run(
            actx(&base, Arc::clone(&ops), Some(Arc::clone(&bus))),
            ActionKind::StartupEnable,
            r,
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            *bus.calls.lock().unwrap(),
            [
                "GetUnitFileState(foo.service)",
                "EnableUnitFiles([foo.service], false, true)",
                "Reload()",
            ]
        );
        assert!(ops.log().is_empty());
    }

    #[test]
    fn startup_user_unit_not_connected() {
        let base = fixture("user-unit-none");
        let ops = Arc::new(FakeOps::new(&base));
        let r = entry(StartupSource::UserUnit, "foo.service");
        let state = run(actx(&base, ops, None), ActionKind::StartupDisable, r);
        assert_eq!(
            state,
            ActionState::Failed(ActionError::Internal(Arc::from("manager not connected")))
        );
    }

    #[test]
    fn startup_script_toggle() {
        let base = fixture("script");
        let ops = Arc::new(FakeOps::new(&base));
        let ctx = actx(&base, Arc::clone(&ops), None);
        let state = run(
            Arc::clone(&ctx),
            ActionKind::StartupDisable,
            entry(StartupSource::PlasmaEnv, "x.sh"),
        );
        assert_eq!(state, ActionState::Succeeded);
        let state = run(
            ctx,
            ActionKind::StartupEnable,
            entry(StartupSource::PlasmaShutdown, "y.sh"),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            ops.log(),
            [
                "script_set_enabled(config/plasma-workspace/env/x.sh, false)",
                "script_set_enabled(config/plasma-workspace/shutdown/y.sh, true)",
            ]
        );
    }

    /// Security regression: a bad id never reaches a path or a call.
    #[test]
    fn startup_bad_id_rejected() {
        let base = fixture("bad-id");
        std::fs::write(base.join("etc/xdg/autostart/evil.desktop"), "").unwrap();
        let ops = Arc::new(FakeOps::new(&base));
        let bus = Arc::new(FakeUnitBus::default());
        let ctx = actx(&base, Arc::clone(&ops), Some(Arc::clone(&bus)));
        for source in [
            StartupSource::AutostartUser,
            StartupSource::AutostartSystem,
            StartupSource::UserUnit,
            StartupSource::PlasmaEnv,
            StartupSource::PlasmaShutdown,
        ] {
            for id in ["../evil.desktop", "a/b.desktop", "..", ""] {
                for kind in [ActionKind::StartupEnable, ActionKind::StartupDisable] {
                    let state = run(Arc::clone(&ctx), kind, entry(source, id));
                    assert_eq!(
                        state,
                        ActionState::Rejected(RejectReason::Unsupported),
                        "{source:?} {id:?}"
                    );
                }
            }
        }
        assert!(ops.log().is_empty());
        assert!(bus.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_not_owner() {
        let base = fixture("not-owner");
        let ops = Arc::new(FakeOps {
            result: Err(OpsError::NotOwner),
            ..FakeOps::new(&base)
        });
        let r = entry(StartupSource::AutostartUser, "foo.desktop");
        let state = run(actx(&base, ops, None), ActionKind::StartupDisable, r);
        assert_eq!(state, ActionState::Rejected(RejectReason::NotOwner));
    }

    #[test]
    fn startup_other_kind_unsupported() {
        let base = fixture("other-kind");
        let ops = Arc::new(FakeOps::new(&base));
        let r = entry(StartupSource::AutostartUser, "foo.desktop");
        let state = run(
            actx(&base, Arc::clone(&ops), None),
            ActionKind::StartupRunNow,
            r,
        );
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(ops.log().is_empty());
    }
}
