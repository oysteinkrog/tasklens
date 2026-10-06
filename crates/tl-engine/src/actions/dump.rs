//! "Create dump file" (PROC-075, E11-018). With `ptrace_scope = 0` and the same uid the engine
//! runs `tl_ops::dump::dump_to_path` itself. A privileged request instead opens the output file
//! here and passes the descriptor to the helper, which never receives a path (R-145).
//!
//! Core files hold process memory: they are created with mode `0600` and never read or logged.
//! The path is logged at debug only. The action has no deadline; the UI reads the file size for
//! progress.

use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asupersync::Cx;
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use tl_model::actions::{ActionError, ActionState, DumpMode, RejectReason};
use tl_model::health::HelperState;
use tl_model::helper_proto::{HelperError, HelperReplyBody, HelperRequest};
use tl_model::ids::ProcessKey;

use super::{ActionCtx, HelperCallError, PROMPT_DEADLINE, internal_error, map_ops_error};
use crate::config::EngineConfig;

/// The dumps directory is created with this mode when missing.
const DUMPS_DIR_MODE: u32 = 0o700;
/// Core files are created with this mode.
const DUMP_FILE_MODE: u32 = 0o600;
/// The only file name suffix the engine writes.
const DUMP_SUFFIX: &str = ".core";
/// The `OutcomeUnknown` text for a helper call that ran out of time.
const HELPER_NO_ANSWER: &str = "no answer from the administrator helper";
/// The `Internal` text when the helper process ended or broke the protocol.
const HELPER_STOPPED: &str = "administrator helper stopped";
/// The `Internal` text when the helper refused the request.
const HELPER_REFUSED: &str = "refused by the administrator helper";

/// `$XDG_STATE_HOME/tasklens/dumps/` (UI-PWN-034).
pub fn dumps_dir(config: &EngineConfig) -> PathBuf {
    config.state_dir.join("dumps")
}

/// True when `path` is absolute, its parent is `dir` (component compare, no canonicalisation)
/// and its file name matches `^[A-Za-z0-9._-]+\.core$`. The engine writes nowhere else.
pub fn valid_dump_path(path: &str, dir: &Path) -> bool {
    if !Path::new(path).is_absolute() {
        return false;
    }
    // Split on the raw text, so a trailing "/." or "/" cannot hide the real last component.
    let Some((parent, name)) = path.rsplit_once('/') else {
        return false;
    };
    let name_ok = name.len() > DUMP_SUFFIX.len()
        && name.ends_with(DUMP_SUFFIX)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && name != "."
        && name != "..";
    name_ok && Path::new(parent).components().eq(dir.components())
}

/// Runs one `Dump` on one `Process` target. `privileged` routes it through the helper.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    mode: DumpMode,
    path: &str,
    key: ProcessKey,
    privileged: bool,
) -> ActionState {
    let dir = dumps_dir(&actx.config);
    if !valid_dump_path(path, &dir) {
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    // UI tooltip "Install gdb to create dumps".
    if !actx.ops.dump_available() {
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    let path = PathBuf::from(path);
    let state = if privileged {
        through_helper(cx, actx, mode, dir, path.clone(), key).await
    } else {
        direct(cx, actx, mode, dir, path.clone(), key).await
    };
    tracing::debug!(target: "tasklens::action", pid = key.pid, ?mode, path = %path.display(), result = ?state);
    state
}

/// Dumps in this process on the blocking pool, with no deadline. tl-ops removes the file when
/// the dump fails.
async fn direct(
    cx: &Cx,
    actx: &ActionCtx,
    mode: DumpMode,
    dir: PathBuf,
    path: PathBuf,
    key: ProcessKey,
) -> ActionState {
    let ops = Arc::clone(&actx.ops);
    let boot = actx.boot_id;
    let Ok(mut job) = cx.spawn_blocking(move |_| {
        create_dir(&dir).map_err(tl_ops::OpsError::Os)?;
        ops.dump_to_path(key, boot, mode, &path)
    }) else {
        return internal_error();
    };
    match job.join(cx).await {
        Err(_) => internal_error(),
        Ok(Ok(())) => ActionState::Succeeded,
        // UI toast "Creating a dump needs administrator rights" with the retry button.
        Ok(Err(e)) => map_ops_error(&e),
    }
}

/// Creates the file here with `O_EXCL` and mode `0600`, then asks the helper to write into its
/// descriptor. Any failure after the file exists removes it.
async fn through_helper(
    cx: &Cx,
    actx: &ActionCtx,
    mode: DumpMode,
    dir: PathBuf,
    path: PathBuf,
    key: ProcessKey,
) -> ActionState {
    let Some(helper) = actx
        .helper
        .as_ref()
        .filter(|h| h.state() != HelperState::Missing)
    else {
        return ActionState::Rejected(RejectReason::Unsupported);
    };
    let open_path = path.clone();
    let Ok(mut job) = cx.spawn_blocking(move |_| {
        create_dir(&dir)?;
        rustix::fs::open(
            &open_path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(DUMP_FILE_MODE),
        )
        .map_err(|e| e.raw_os_error())
    }) else {
        return internal_error();
    };
    let fd = match job.join(cx).await {
        Err(_) => return internal_error(),
        // EEXIST lands here, before anything was created.
        Ok(Err(errno)) => return ActionState::Failed(ActionError::Os(errno)),
        Ok(Ok(fd)) => fd,
    };
    let state = match helper
        .call(
            cx,
            HelperRequest::Dump { key, mode },
            Some(fd),
            PROMPT_DEADLINE,
        )
        .await
    {
        Ok(replies) => match replies.last().map(|r| &r.body) {
            Some(HelperReplyBody::Ok) => ActionState::Succeeded,
            Some(HelperReplyBody::Err(e)) => map_helper_error(*e),
            _ => ActionState::Failed(ActionError::Internal(Arc::from(HELPER_STOPPED))),
        },
        Err(e) => map_call_error(e),
    };
    if state != ActionState::Succeeded {
        let Ok(mut unlink) = cx.spawn_blocking(move |_| rustix::fs::unlink(&path)) else {
            return state;
        };
        let _ = unlink.join(cx).await;
    }
    state
}

/// Creates `dir` with mode `0700` when missing. Returns the errno on failure.
fn create_dir(dir: &Path) -> Result<(), i32> {
    DirBuilder::new()
        .recursive(true)
        .mode(DUMPS_DIR_MODE)
        .create(dir)
        .map_err(|e| e.raw_os_error().unwrap_or(Errno::IO.raw_os_error()))
}

/// The E29-014 mapping of a helper error.
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

/// The E29-014 mapping of a failed helper call (R-148 item 3).
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
    use std::os::unix::fs::PermissionsExt;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use asupersync::channel::mpsc;
    use asupersync::time::{TimeSource, TimerDriverHandle, VirtualClock};
    use asupersync::types::{Budget, Time};
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::actions::{ActionKind, ActionTarget, CgroupRef};
    use tl_model::helper_proto::HelperReply;
    use tl_model::time::BootId;
    use tl_ops::OpsError;

    use super::*;
    use crate::actions::{ActionOps, PrivilegedOps, RealOps};
    use crate::config::EngineMode;
    use crate::dbus::ManagerConnSlot;

    const BOOT: BootId = BootId(1);
    const NAME: &str = "firefox_1234_20261003_120000.core";

    fn key() -> ProcessKey {
        ProcessKey {
            start_ticks: 1234,
            boot_id: BOOT,
            pid: 1234,
        }
    }

    /// A state dir under `target/tmp`, removed on drop.
    struct StateDir(PathBuf);

    impl StateDir {
        fn new(name: &str) -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/tmp")
                .join(format!("tl-engine-dump-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create state dir");
            Self(dir)
        }

        fn config(&self) -> EngineConfig {
            EngineConfig::new(EngineMode::Full, self.0.clone())
        }

        fn dump_path(&self) -> PathBuf {
            dumps_dir(&self.config()).join(NAME)
        }
    }

    impl Drop for StateDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Clone)]
    enum Step {
        Ok,
        Err(OpsError),
        /// Advances the virtual clock by this much inside the blocking job, then succeeds.
        Block(Duration),
    }

    struct FakeOps {
        available: bool,
        step: Step,
        clock: Arc<VirtualClock>,
        log: Mutex<Vec<(ProcessKey, DumpMode, PathBuf)>>,
    }

    impl ActionOps for FakeOps {
        fn dump_available(&self) -> bool {
            self.available
        }

        fn dump_to_path(
            &self,
            key: ProcessKey,
            boot: BootId,
            mode: DumpMode,
            out: &Path,
        ) -> Result<(), OpsError> {
            assert_eq!(boot, BOOT);
            self.log
                .lock()
                .unwrap()
                .push((key, mode, out.to_path_buf()));
            match self.step.clone() {
                Step::Ok => Ok(()),
                Step::Err(e) => Err(e),
                Step::Block(d) => {
                    self.clock.advance(d.as_nanos() as u64);
                    Ok(())
                }
            }
        }
    }

    /// One helper call as the fake saw it: the request, whether an fd came with it and that fd's
    /// file mode.
    type HelperCall = (HelperRequest, Option<u32>);

    struct FakeHelper {
        state: HelperState,
        answer: Result<HelperReplyBody, HelperCallError>,
        calls: Mutex<Vec<HelperCall>>,
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
            let fd_mode = fd.map(|fd| rustix::fs::fstat(&fd).expect("fstat").st_mode & 0o7777);
            self.calls.lock().unwrap().push((req, fd_mode));
            let answer = self.answer.clone().map(|body| {
                vec![HelperReply {
                    seq: 1,
                    last: true,
                    body,
                }]
            });
            Box::pin(async move { answer })
        }

        fn state(&self) -> HelperState {
            self.state
        }
    }

    fn helper(answer: Result<HelperReplyBody, HelperCallError>) -> Arc<FakeHelper> {
        Arc::new(FakeHelper {
            state: HelperState::Unlocked,
            answer,
            calls: Mutex::new(Vec::new()),
        })
    }

    struct Run {
        state: ActionState,
        log: Vec<(ProcessKey, DumpMode, PathBuf)>,
        ms: u64,
    }

    /// Runs one dump of `key()` to `path` and returns its state, the fake's call log and the
    /// virtual time in ms when the state was ready.
    fn run(
        dir: &StateDir,
        available: bool,
        step: Step,
        helper: Option<Arc<FakeHelper>>,
        path: &str,
        privileged: bool,
    ) -> Run {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
        lab.state
            .set_timer_driver(TimerDriverHandle::with_virtual_clock(Arc::clone(&clock)));
        let ops = Arc::new(FakeOps {
            available,
            step,
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
            config: Arc::new(dir.config()),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: helper.map(|h| h as Arc<dyn PrivilegedOps>),
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: None,
        });
        let out = Arc::new(Mutex::new(None));
        let out_task = Arc::clone(&out);
        let task_clock = Arc::clone(&clock);
        let path = path.to_string();
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = run_target(&cx, &actx, DumpMode::Full, &path, key(), privileged).await;
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
        Run { state, log, ms }
    }

    fn path_str(p: &Path) -> &str {
        p.to_str().expect("utf-8 test path")
    }

    #[test]
    fn dump_path_validation() {
        let dir = Path::new("/home/u/.local/state/tasklens/dumps");
        let d = "/home/u/.local/state/tasklens/dumps";
        assert!(valid_dump_path(&format!("{d}/{NAME}"), dir));
        assert!(valid_dump_path(&format!("{d}/a-b_c.1.core"), dir));
        // Security regression: nothing outside the dumps dir, no other suffix, no sub dirs.
        for bad in [
            format!("{d}/../x.core"),
            "/tmp/x.core".to_string(),
            format!("{d}/x.txt"),
            format!("{d}/sub/x.core"),
            format!("{d}/.core"),
            format!("{d}/x.core/."),
            format!("{d}/x.core/"),
            format!("{d}/x y.core"),
            format!("{d}/x\0.core"),
            format!("{d}/../dumps/x.core"),
            "x.core".to_string(),
            "dumps/x.core".to_string(),
            d.to_string(),
            String::new(),
        ] {
            assert!(!valid_dump_path(&bad, dir), "{bad:?}");
        }
    }

    #[test]
    fn dumps_dir_is_under_state_dir() {
        let config = EngineConfig::new(EngineMode::Full, PathBuf::from("/s/tasklens"));
        assert_eq!(dumps_dir(&config), Path::new("/s/tasklens/dumps"));
    }

    #[test]
    fn dump_direct_ok() {
        let dir = StateDir::new("direct-ok");
        let path = dir.dump_path();
        let r = run(&dir, true, Step::Ok, None, path_str(&path), false);
        assert_eq!(r.state, ActionState::Succeeded);
        assert_eq!(r.log, [(key(), DumpMode::Full, path.clone())]);
        // The dumps dir was created with mode 0700.
        let meta = fs::metadata(path.parent().unwrap()).expect("dumps dir exists");
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn dump_direct_permission_denied() {
        let dir = StateDir::new("direct-eperm");
        let path = dir.dump_path();
        let r = run(
            &dir,
            true,
            Step::Err(OpsError::PermissionDenied),
            None,
            path_str(&path),
            false,
        );
        assert_eq!(r.state, ActionState::Failed(ActionError::PermissionDenied));
    }

    #[test]
    fn dump_direct_identity_changed() {
        let dir = StateDir::new("direct-identity");
        let path = dir.dump_path();
        let r = run(
            &dir,
            true,
            Step::Err(OpsError::IdentityChanged),
            None,
            path_str(&path),
            false,
        );
        assert_eq!(
            r.state,
            ActionState::Rejected(RejectReason::IdentityChanged)
        );
    }

    #[test]
    fn dump_invalid_path_rejected() {
        let dir = StateDir::new("invalid-path");
        let r = run(&dir, true, Step::Ok, None, "/tmp/x.core", false);
        assert_eq!(r.state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(r.log.is_empty());
    }

    #[test]
    fn dump_privileged_passes_fd() {
        let dir = StateDir::new("priv-ok");
        let path = dir.dump_path();
        let h = helper(Ok(HelperReplyBody::Ok));
        let r = run(
            &dir,
            true,
            Step::Ok,
            Some(Arc::clone(&h)),
            path_str(&path),
            true,
        );
        assert_eq!(r.state, ActionState::Succeeded);
        // The engine never dumps itself on the privileged path.
        assert!(r.log.is_empty());
        let calls = h.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            [(
                HelperRequest::Dump {
                    key: key(),
                    mode: DumpMode::Full
                },
                Some(0o600)
            )]
        );
        let meta = fs::metadata(&path).expect("dump file exists");
        assert_eq!(meta.permissions().mode() & 0o7777, 0o600);
    }

    #[test]
    fn dump_privileged_eexist() {
        let dir = StateDir::new("priv-eexist");
        let path = dir.dump_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"keep").unwrap();
        let h = helper(Ok(HelperReplyBody::Ok));
        let r = run(
            &dir,
            true,
            Step::Ok,
            Some(Arc::clone(&h)),
            path_str(&path),
            true,
        );
        assert_eq!(
            r.state,
            ActionState::Failed(ActionError::Os(Errno::EXIST.raw_os_error()))
        );
        assert!(h.calls.lock().unwrap().is_empty());
        assert_eq!(fs::read(&path).unwrap(), b"keep");
    }

    #[test]
    fn dump_privileged_identity_changed_cleans_up() {
        let dir = StateDir::new("priv-identity");
        let path = dir.dump_path();
        let h = helper(Ok(HelperReplyBody::Err(HelperError::IdentityChanged)));
        let r = run(
            &dir,
            true,
            Step::Ok,
            Some(Arc::clone(&h)),
            path_str(&path),
            true,
        );
        assert_eq!(
            r.state,
            ActionState::Rejected(RejectReason::IdentityChanged)
        );
        assert_eq!(h.calls.lock().unwrap().len(), 1);
        assert!(!path.exists());
    }

    #[test]
    fn dump_privileged_call_errors_clean_up() {
        let cases = [
            (
                Err(HelperCallError::Timeout),
                ActionState::OutcomeUnknown(Arc::from(HELPER_NO_ANSWER)),
            ),
            (
                Err(HelperCallError::Cancelled),
                ActionState::Failed(ActionError::AuthCancelled),
            ),
            (
                Err(HelperCallError::NotAuthorized),
                ActionState::Failed(ActionError::NotAuthorized),
            ),
            (
                Err(HelperCallError::Exited),
                ActionState::Failed(ActionError::Internal(Arc::from(HELPER_STOPPED))),
            ),
            (
                Err(HelperCallError::Protocol(Arc::from("bad frame"))),
                ActionState::Failed(ActionError::Internal(Arc::from(HELPER_STOPPED))),
            ),
            (
                Err(HelperCallError::Missing),
                ActionState::Rejected(RejectReason::Unsupported),
            ),
            (
                Ok(HelperReplyBody::Err(HelperError::PermissionDenied)),
                ActionState::Failed(ActionError::PermissionDenied),
            ),
            (
                Ok(HelperReplyBody::Err(HelperError::Os(5))),
                ActionState::Failed(ActionError::Os(5)),
            ),
        ];
        for (i, (answer, want)) in cases.into_iter().enumerate() {
            let dir = StateDir::new(&format!("priv-err-{i}"));
            let path = dir.dump_path();
            let r = run(
                &dir,
                true,
                Step::Ok,
                Some(helper(answer)),
                path_str(&path),
                true,
            );
            assert_eq!(r.state, want, "case {i}");
            assert!(!path.exists(), "case {i}: file left behind");
        }
    }

    #[test]
    fn dump_privileged_without_helper_unsupported() {
        let dir = StateDir::new("priv-missing");
        let path = dir.dump_path();
        let r = run(&dir, true, Step::Ok, None, path_str(&path), true);
        assert_eq!(r.state, ActionState::Rejected(RejectReason::Unsupported));

        let missing = Arc::new(FakeHelper {
            state: HelperState::Missing,
            answer: Ok(HelperReplyBody::Ok),
            calls: Mutex::new(Vec::new()),
        });
        let r = run(
            &dir,
            true,
            Step::Ok,
            Some(Arc::clone(&missing)),
            path_str(&path),
            true,
        );
        assert_eq!(r.state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(missing.calls.lock().unwrap().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn dump_without_gdb_unsupported() {
        let dir = StateDir::new("no-gdb");
        let path = dir.dump_path();
        let r = run(&dir, false, Step::Ok, None, path_str(&path), false);
        assert_eq!(r.state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(r.log.is_empty());

        let h = helper(Ok(HelperReplyBody::Ok));
        let r = run(
            &dir,
            false,
            Step::Ok,
            Some(Arc::clone(&h)),
            path_str(&path),
            true,
        );
        assert_eq!(r.state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(h.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn dump_has_no_deadline() {
        let dir = StateDir::new("no-deadline");
        let path = dir.dump_path();
        let r = run(
            &dir,
            true,
            Step::Block(Duration::from_secs(3)),
            None,
            path_str(&path),
            false,
        );
        assert_eq!(r.state, ActionState::Succeeded);
        // The fake moved the clock 3 s, past every quick deadline.
        assert!(r.ms >= 3_000, "{} ms", r.ms);
    }

    #[test]
    fn dump_on_other_targets_unsupported() {
        let dir = StateDir::new("dispatch");
        let path = dir.dump_path();
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = mpsc::channel(16);
        let ops = Arc::new(FakeOps {
            available: true,
            step: Step::Ok,
            clock: Arc::new(VirtualClock::starting_at(Time::ZERO)),
            log: Mutex::new(Vec::new()),
        });
        let actx = Arc::new(ActionCtx {
            events,
            msgs,
            ops: Arc::clone(&ops) as Arc<dyn ActionOps>,
            boot_id: BOOT,
            uid: 1000,
            config: Arc::new(dir.config()),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: None,
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: None,
        });
        let kind = ActionKind::Dump {
            mode: DumpMode::Mini,
            path: Arc::from(path_str(&path)),
        };
        let out = Arc::new(Mutex::new(Vec::new()));
        let out_task = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let targets = [
                    ActionTarget::Thread {
                        process: key(),
                        tid: 1235,
                    },
                    ActionTarget::Cgroup(CgroupRef {
                        rel_path: Arc::from("user.slice/app.slice/app-foo.scope"),
                        cgroup_id: 42,
                    }),
                    ActionTarget::Process(key()),
                ];
                for t in &targets {
                    let state = crate::actions::dispatch(&cx, &actx, &kind, t).await;
                    out_task.lock().unwrap().push(state);
                }
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        assert_eq!(
            *out.lock().unwrap(),
            [
                ActionState::Rejected(RejectReason::Unsupported),
                ActionState::Rejected(RejectReason::Unsupported),
                ActionState::Succeeded,
            ]
        );
        assert_eq!(
            *ops.log.lock().unwrap(),
            [(key(), DumpMode::Mini, path.clone())]
        );
    }

    #[test]
    fn real_ops_dump_forwards() {
        assert_eq!(RealOps.dump_available(), tl_ops::dump::dump_available());
        let dir = StateDir::new("real-ops");
        let path = dir.0.join(NAME);
        // A wrong boot id fails before any dump; tl-ops removes the file it created.
        let err = RealOps
            .dump_to_path(key(), BootId(2), DumpMode::Mini, &path)
            .expect_err("dump fails");
        assert_ne!(err, OpsError::Unsupported("dump ops not wired"));
        assert!(!path.exists());
    }

    /// ROOT-RUN: with `ptrace_scope` 1 a dump of a process that is not a child of gdb needs
    /// root. Run with `sudo` and `--ignored`.
    #[test]
    #[ignore = "ROOT-RUN: a real gdb dump of a non-child needs root when ptrace_scope is 1"]
    fn real_ops_dump_writes_core() {
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let key = ProcessKey {
            start_ticks: tl_ops::process::read_start_ticks(pid).expect("start ticks"),
            boot_id: BOOT,
            pid,
        };
        let dir = StateDir::new("real-dump");
        let path = dir.0.join(NAME);
        let result = RealOps.dump_to_path(key, BOOT, DumpMode::Mini, &path);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(result, Ok(()));
        let meta = fs::metadata(&path).expect("core exists");
        assert!(meta.len() > 0);
        assert_eq!(meta.permissions().mode() & 0o7777, 0o600);
    }
}
