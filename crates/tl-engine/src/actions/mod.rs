//! Action admission and the action task (ADR-005, ADR-006, E11). The reducer calls `admit` on
//! `Command::Action` before any action task starts. Each admitted request then runs as one
//! `run_action` task.
//!
//! Module lines for `process`, `cgroup`, `unit`, `startup`, `dump`, `socket` and `recovery` are
//! added by their beads.

pub mod cgroup;
pub mod dump;
mod helper;
pub mod process;
pub mod recovery;
pub mod socket;
pub mod startup;
pub mod targetless;
pub mod unit;

use std::future::Future;
use std::os::fd::{AsFd, OwnedFd};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc;
use asupersync::runtime::TaskHandle;
use asupersync::time::{self, Elapsed};
use futures_lite::FutureExt;
use rustc_hash::{FxHashMap, FxHashSet};
use rustix::io::Errno;
use smallvec::SmallVec;
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    LaunchSpec, MAX_ACTION_TARGETS, MAX_ACTIONS_IN_FLIGHT, RejectReason, SchedPolicy, Signal,
};
use tl_model::health::{HelperState, SourceId};
use tl_model::helper_proto::{HelperReply, HelperRequest};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcFlags;
use tl_model::request::{IdKind, RequestEnd, RequestEvent};
use tl_model::table::{Manager, StartupSource, UnitRef};
use tl_model::time::BootId;
use tl_ops::OpsError;

use crate::bridge::HookCtx;
use crate::config::EngineConfig;
use crate::dbus::ManagerConnSlot;
use crate::reducer::{ActionGate, ActionRoute, AdmissionView};
use crate::update::EngineMsg;

/// Most requests running at once; one more is `Rejected(Busy)`.
pub const MAX_IN_FLIGHT: usize = MAX_ACTIONS_IN_FLIGHT;
/// Most targets in one request; one more is `Rejected(TooManyTargets)`.
pub const MAX_TARGETS: usize = MAX_ACTION_TARGETS;

/// The admission result for one request.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Admission {
    /// Refuse the whole request. Nothing starts.
    Reject(RejectReason),
    /// Start the request. `per_target[i]` is `Some` when target `i` is rejected before start.
    /// Empty for a targetless kind.
    Run {
        per_target: Vec<Option<RejectReason>>,
    },
}

/// True for `OpenUri`, `ShowInFileManager`, `Launch` and `ActivateWindow`: they carry no
/// targets (R-134).
pub fn targetless(kind: &ActionKind) -> bool {
    kind.is_targetless()
}

/// True for the kinds that act on the host's screen: they need its desktop session (ADR-018).
/// The gate refuses them with `NoDesktopSession` while there is none.
pub fn needs_desktop(kind: &ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::OpenUri(_)
            | ActionKind::ShowInFileManager { .. }
            | ActionKind::Launch(_)
            | ActionKind::StartupRunNow
            | ActionKind::ActivateWindow { .. }
    )
}

/// Admission rules in ADR-006 order. A request whose every target is rejected still runs, so the
/// UI gets `Started`, the per-target `Rejected` events and a `Rejected` summary.
pub fn admit(
    req: &ActionRequest,
    view: &AdmissionView<'_>,
    in_flight: usize,
    busy_units: &FxHashSet<UnitRef>,
) -> Admission {
    if in_flight >= MAX_IN_FLIGHT {
        return Admission::Reject(RejectReason::Busy);
    }
    if req.targets.len() > MAX_TARGETS {
        return Admission::Reject(RejectReason::TooManyTargets);
    }
    if targetless(&req.kind) {
        if !req.targets.is_empty() {
            return Admission::Reject(RejectReason::Unsupported);
        }
        return Admission::Run {
            per_target: Vec::new(),
        };
    }
    if req.targets.is_empty() {
        return Admission::Reject(RejectReason::Unsupported);
    }
    // SCHED_DEADLINE is display only. Refused here for both the direct and the helper path.
    if let ActionKind::SetSched {
        policy: SchedPolicy::Deadline,
        ..
    } = req.kind
    {
        return Admission::Reject(RejectReason::Unsupported);
    }
    let per_target = req
        .targets
        .iter()
        .map(|target| match target {
            ActionTarget::Process(key) | ActionTarget::Thread { process: key, .. } => {
                check_process(key, view)
            }
            ActionTarget::Unit(unit) => busy_units.contains(unit).then_some(RejectReason::Busy),
            // Validated by their own modules.
            ActionTarget::Cgroup(_) | ActionTarget::Startup(_) | ActionTarget::Socket { .. } => {
                None
            }
        })
        .collect();
    Admission::Run { per_target }
}

/// PROC-059: identity first, then kernel threads, then tasklens itself and PID 1. Other
/// `PROTECTED` targets were confirmed in the UI and pass. A key that exited within the linger
/// passes too: the pidfd open reports it.
fn check_process(key: &ProcessKey, view: &AdmissionView<'_>) -> Option<RejectReason> {
    if key.boot_id != view.boot_id {
        return Some(RejectReason::IdentityChanged);
    }
    let flags = match view.live.get(key) {
        Some(flags) => *flags,
        None if view.exited_within_linger.contains(key) => ProcFlags::empty(),
        None => return Some(RejectReason::IdentityChanged),
    };
    if flags.contains(ProcFlags::KERNEL_THREAD) {
        return Some(RejectReason::KernelThread);
    }
    if flags.contains(ProcFlags::IS_SELF) || key.pid == 1 {
        return Some(RejectReason::Protected);
    }
    None
}

/// How long `emit` and the `ActionFinished` send wait before they drop the message.
pub const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(1);
/// Deadline for any call that may show a polkit prompt.
pub const PROMPT_DEADLINE: Duration = Duration::from_secs(120);
/// Deadline for one quick target operation (ADR-006).
const QUICK_DEADLINE: Duration = Duration::from_secs(2);
/// Deadline for a user-manager unit file call and a user-unit startup entry (E11-014).
const USER_UNIT_FILE_DEADLINE: Duration = Duration::from_secs(10);
/// The `OutcomeUnknown` text for an elapsed deadline.
pub const TIMED_OUT: &str = "timed out";
/// The `Internal` text for a panic in an action task (ADR-015).
const INTERNAL_ERROR: &str = "internal error";
/// The `OutcomeUnknown` text of an action the gate aborted because the reducer stopped.
pub const ENGINE_STOPPED: &str = "engine stopped";

/// The ADR-006 deadline for one target of `kind`. `None` means no client-side timeout.
///
/// `KillTree` gives the per-target 2 s; `process::kill_tree` multiplies it by the target count
/// and caps it at 30 s (E11-004). A unit kind without a `Unit` target gets the system-manager
/// deadline.
pub fn deadline(
    kind: &ActionKind,
    target: Option<&ActionTarget>,
    privileged: bool,
) -> Option<Duration> {
    if privileged {
        return Some(PROMPT_DEADLINE);
    }
    let by_manager = |user: Duration| match target {
        Some(ActionTarget::Unit(unit)) if unit.manager == Manager::User => Some(user),
        _ => Some(PROMPT_DEADLINE),
    };
    match kind {
        ActionKind::Signal(_)
        | ActionKind::KillTree
        | ActionKind::Suspend
        | ActionKind::Resume
        | ActionKind::SetNice(_)
        | ActionKind::SetSched { .. }
        | ActionKind::SetAffinity(_)
        | ActionKind::SetIoPriority(_) => Some(QUICK_DEADLINE),
        ActionKind::FreezeGroup | ActionKind::ThawGroup | ActionKind::KillGroup => match target {
            Some(ActionTarget::Cgroup(_)) => Some(QUICK_DEADLINE),
            _ => by_manager(QUICK_DEADLINE),
        },
        // The JobRemoved wait has no client timeout (ADR-005, SYS-005). The method call itself
        // uses 120 s inside unit.rs.
        ActionKind::UnitStart
        | ActionKind::UnitStop
        | ActionKind::UnitRestart
        | ActionKind::UnitReload
        | ActionKind::UnitResetFailed => None,
        ActionKind::UnitEnable
        | ActionKind::UnitDisable
        | ActionKind::UnitMask
        | ActionKind::UnitUnmask => by_manager(USER_UNIT_FILE_DEADLINE),
        ActionKind::UnitWriteRecovery(_) | ActionKind::UnitRemoveRecovery => {
            by_manager(QUICK_DEADLINE)
        }
        ActionKind::StartupEnable | ActionKind::StartupDisable | ActionKind::StartupDelete => {
            match target {
                Some(ActionTarget::Startup(entry)) if entry.source == StartupSource::UserUnit => {
                    Some(USER_UNIT_FILE_DEADLINE)
                }
                _ => Some(QUICK_DEADLINE),
            }
        }
        // The launcher's own 30 s JobRemoved wait and 2 s spawn apply.
        ActionKind::StartupRunNow => None,
        // The UI reads the progress.
        ActionKind::Dump { .. } => None,
        ActionKind::SockDestroy => Some(QUICK_DEADLINE),
        ActionKind::OpenUri(_)
        | ActionKind::ShowInFileManager { .. }
        | ActionKind::ActivateWindow { .. } => Some(QUICK_DEADLINE),
        ActionKind::Launch(spec) if spec.as_admin => Some(PROMPT_DEADLINE),
        ActionKind::Launch(_) => Some(QUICK_DEADLINE),
    }
}

/// The state the UI shows for an `OpsError` (ADR-015). EPERM and EACCES are `PermissionDenied`
/// and ESRCH is `NoSuchProcess`, also when they arrive as a raw `Os` errno.
pub fn map_ops_error(e: &OpsError) -> ActionState {
    match e {
        OpsError::Os(n)
            if *n == Errno::PERM.raw_os_error() || *n == Errno::ACCESS.raw_os_error() =>
        {
            ActionState::Failed(ActionError::PermissionDenied)
        }
        OpsError::Os(n) if *n == Errno::SRCH.raw_os_error() => {
            ActionState::Failed(ActionError::NoSuchProcess)
        }
        other => other.to_action_state(),
    }
}

/// The summary state (`ActionState::summarize`, E03-053). When a request with more than one
/// target has a failure, the summary is `Failed(Partial { done, total })` with `done` the count
/// of succeeded targets, so the UI can say "Ended 4 of 5 processes".
pub fn summarize(states: &[ActionState]) -> ActionState {
    let worst = ActionState::summarize(states);
    if states.len() > 1 && matches!(worst, ActionState::Failed(_)) {
        let done = states.iter().filter(|s| s.is_success()).count();
        return ActionState::Failed(ActionError::Partial {
            done: index_u16(done),
            total: index_u16(states.len()),
        });
    }
    worst
}

/// Why a privileged helper call did not return replies. The one definition: helper/mod.rs
/// imports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperCallError {
    /// The helper is not installed.
    Missing,
    /// The user dismissed the polkit prompt.
    Cancelled,
    NotAuthorized,
    /// A call whose implicit session start failed (R-246).
    StartFailed(Arc<str>),
    Timeout,
    /// The helper process ended during the call.
    Exited,
    Protocol(Arc<str>),
}

/// The privileged helper link (E29-OI-6). `HelperHandle` implements it in `helper.rs`
/// (E29-014). `ActionCtx::helper` is `None` when the session runs no helper task.
pub trait PrivilegedOps: Send + Sync + 'static {
    fn call(
        &self,
        cx: &Cx,
        req: HelperRequest,
        fd: Option<OwnedFd>,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<HelperReply>, HelperCallError>> + Send + '_>>;
    fn state(&self) -> HelperState;
}

/// What a launch did (E14-A3).
pub use crate::launcher::LaunchOutcome;

/// The session-bus openers and the shared launcher (E14-020, E14-A3), behind a trait so tests
/// can fake them. The only place E11 touches E14 code.
pub trait Openers: Send + Sync + 'static {
    fn open_uri(
        &self,
        cx: &Cx,
        target: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>>;
    fn show_in_file_manager(
        &self,
        cx: &Cx,
        path: &str,
        properties: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>>;
    fn launch(
        &self,
        cx: &Cx,
        spec: &LaunchSpec,
    ) -> Pin<Box<dyn Future<Output = Result<LaunchOutcome, ActionError>> + Send + '_>>;
    /// Switch to window through the KWin worker. The default has no bridge.
    fn activate_window(
        &self,
        _cx: &Cx,
        _internal_id: &Arc<str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        Box::pin(async { Err(ActionError::Unsupported) })
    }
}

/// The production `Openers`: E14's opener and launcher on the user manager's session connection.
pub struct RealOpeners {
    pub session: ManagerConnSlot,
    /// The host's desktop session. Under the engine service, spawned programs get its
    /// variables (ADR-018).
    pub desktop: crate::desktop::DesktopSession,
}

impl Openers for RealOpeners {
    fn open_uri(
        &self,
        cx: &Cx,
        target: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        let cx = cx.clone();
        let target = target.to_owned();
        let env = self.desktop.overrides();
        Box::pin(
            async move { crate::opener::open_uri(&cx, self.session.get(), env, &target).await },
        )
    }

    fn show_in_file_manager(
        &self,
        cx: &Cx,
        path: &str,
        properties: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        let cx = cx.clone();
        let path = path.to_owned();
        let env = self.desktop.overrides();
        Box::pin(async move {
            crate::opener::show_in_file_manager(&cx, self.session.get(), env, &path, properties)
                .await
        })
    }

    fn launch(
        &self,
        cx: &Cx,
        spec: &LaunchSpec,
    ) -> Pin<Box<dyn Future<Output = Result<LaunchOutcome, ActionError>> + Send + '_>> {
        let cx = cx.clone();
        let spec = spec.clone();
        let desktop = self.desktop.clone();
        Box::pin(
            async move { crate::launcher::launch(&cx, self.session.get(), &desktop, &spec).await },
        )
    }

    fn activate_window(
        &self,
        _cx: &Cx,
        internal_id: &Arc<str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        let internal_id = Arc::clone(internal_id);
        Box::pin(self.desktop.activate_window(internal_id))
    }
}

/// One method per tl-ops entry point used by actions/{process,cgroup,startup,dump}.rs. Each
/// module bead adds its methods. `RealOps` forwards to tl-ops; `FakeOps` (tests) scripts results.
pub trait ActionOps: Send + Sync + 'static {
    /// `tl_ops::process::open_verified`: a pidfd proven to be `key`. The default refuses, so an
    /// ops type without process support never signals.
    fn open_verified(&self, _key: ProcessKey, _boot: BootId) -> Result<OwnedFd, OpsError> {
        Err(OpsError::Unsupported("process ops not wired"))
    }
    /// Sends `sig` through `pidfd`. tl-ops checks `Signal::Other(n)` is 1..=64.
    fn send_signal(&self, _pidfd: &OwnedFd, _sig: Signal) -> Result<(), OpsError> {
        Err(OpsError::Unsupported("process ops not wired"))
    }
    /// Closes a pidfd from `open_verified`. Only `process::kill_tree` calls it, so a fake can log
    /// the close pass.
    fn close_pidfd(&self, pidfd: OwnedFd) {
        drop(pidfd);
    }
    /// `tl_ops::cgroup::open_validated`. The default refuses, so an ops type without cgroup
    /// support never writes.
    fn cgroup_open_validated(
        &self,
        _cg: &tl_model::actions::CgroupRef,
        _uid: u32,
    ) -> Result<tl_ops::cgroup::CgroupDir, OpsError> {
        Err(OpsError::Unsupported("cgroup ops not wired"))
    }
    /// Writes `value` to the control file `file`: `cgroup.freeze` or `cgroup.kill`.
    fn cgroup_write(
        &self,
        _dir: &tl_ops::cgroup::CgroupDir,
        _file: &str,
        _value: &str,
    ) -> Result<(), OpsError> {
        Err(OpsError::Unsupported("cgroup ops not wired"))
    }
    /// `tl_ops::cgroup::wait_frozen` with `FREEZE_POLL`. Not frozen within `wait` is
    /// `OutcomeUnknown("freeze still in progress")`.
    fn cgroup_wait_frozen(
        &self,
        _dir: &tl_ops::cgroup::CgroupDir,
        _wait: Duration,
    ) -> Result<(), OpsError> {
        Err(OpsError::Unsupported("cgroup ops not wired"))
    }
    /// `tl_ops::dump::dump_available`: gdb and gcore exist. The default is false.
    fn dump_available(&self) -> bool {
        false
    }
    /// `tl_ops::dump::dump_to_path`: creates `out` with mode `0600` and dumps `key` into it,
    /// blocking until gdb exits. The default refuses.
    fn dump_to_path(
        &self,
        _key: ProcessKey,
        _boot: BootId,
        _mode: tl_model::actions::DumpMode,
        _out: &std::path::Path,
    ) -> Result<(), OpsError> {
        Err(OpsError::Unsupported("dump ops not wired"))
    }
    /// The tl-ops TID loop for a PID-based change (ADR-006): opens and verifies the pidfd, runs
    /// `op` on every thread, then polls the pidfd. Some threads failed is `Err(Partial)`; a
    /// process that exited during the loop is `Err(OutcomeUnknown)`. The default refuses.
    fn apply_pid_op(
        &self,
        _key: ProcessKey,
        _boot: BootId,
        _op: &process::PidOp,
    ) -> Result<(), OpsError> {
        Err(OpsError::Unsupported("process ops not wired"))
    }
    /// One thread of the verified process (ADR-006, R-097): tl-ops opens and verifies the pidfd,
    /// checks `/proc/<pid>/task/<tid>` exists (else `IdentityChanged`), runs `op` on that TID
    /// (signals through `tgkill`) and polls the pidfd after it. The default refuses.
    fn apply_thread_op(
        &self,
        _process: ProcessKey,
        _boot: BootId,
        _tid: u32,
        _op: &process::ThreadOp,
    ) -> Result<(), OpsError> {
        Err(OpsError::Unsupported("process ops not wired"))
    }
    /// `tl_ops::fs::autostart_disable_system`: writes the user override of `system_file` and
    /// returns its path. The default refuses.
    fn startup_disable_system(
        &self,
        _system_file: &std::path::Path,
        _roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult<std::path::PathBuf> {
        Err(OpsError::Unsupported("startup ops not wired"))
    }
    /// `tl_ops::fs::autostart_enable_override`. The default refuses.
    fn startup_enable_override(
        &self,
        _user_file: &std::path::Path,
        _roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult<tl_ops::fs::EnableOutcome> {
        Err(OpsError::Unsupported("startup ops not wired"))
    }
    /// `tl_ops::fs::autostart_set_hidden`. The default refuses.
    fn startup_set_hidden(
        &self,
        _user_file: &std::path::Path,
        _hidden: bool,
        _roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult {
        Err(OpsError::Unsupported("startup ops not wired"))
    }
    /// `tl_ops::fs::script_set_enabled`. The default refuses.
    fn startup_script_set_enabled(
        &self,
        _script: &std::path::Path,
        _enabled: bool,
        _roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult {
        Err(OpsError::Unsupported("startup ops not wired"))
    }
    /// `tl_ops::fs::delete_startup_file`: unlinks a user startup file. A missing file is
    /// `Err(Os(ENOENT))`. The default refuses.
    fn startup_delete_file(
        &self,
        _path: &std::path::Path,
        _roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult {
        Err(OpsError::Unsupported("startup ops not wired"))
    }
    /// `tl_ops::write_atomic` of `text` with exactly `mode`. The parent must exist. The default
    /// refuses.
    fn write_atomic(&self, _path: &std::path::Path, _text: &str, _mode: u32) -> tl_ops::OpsResult {
        Err(OpsError::Unsupported("file ops not wired"))
    }
    /// Unlinks `path`. A missing file is `Err(Os(ENOENT))`. The default refuses.
    fn remove_file(&self, _path: &std::path::Path) -> tl_ops::OpsResult {
        Err(OpsError::Unsupported("file ops not wired"))
    }
    /// Removes the directory `path` when it is empty. A directory that is not empty, or is
    /// missing, is `Ok`. The default refuses.
    fn remove_dir_if_empty(&self, _path: &std::path::Path) -> tl_ops::OpsResult {
        Err(OpsError::Unsupported("file ops not wired"))
    }
}

/// The production `ActionOps`: each method forwards to tl-ops. Module beads add their methods
/// here next to the trait.
pub struct RealOps;

impl ActionOps for RealOps {
    fn open_verified(&self, key: ProcessKey, boot: BootId) -> Result<OwnedFd, OpsError> {
        tl_ops::process::open_verified(key, boot)
    }

    fn send_signal(&self, pidfd: &OwnedFd, sig: Signal) -> Result<(), OpsError> {
        tl_ops::process::send_signal_fd(pidfd.as_fd(), sig)
    }

    fn cgroup_open_validated(
        &self,
        cg: &tl_model::actions::CgroupRef,
        uid: u32,
    ) -> Result<tl_ops::cgroup::CgroupDir, OpsError> {
        tl_ops::cgroup::open_validated(cg, uid)
    }

    fn cgroup_write(
        &self,
        dir: &tl_ops::cgroup::CgroupDir,
        file: &str,
        value: &str,
    ) -> Result<(), OpsError> {
        match (file, value) {
            ("cgroup.freeze", "1") => tl_ops::cgroup::freeze(dir),
            ("cgroup.freeze", "0") => tl_ops::cgroup::thaw(dir),
            ("cgroup.kill", "1") => tl_ops::cgroup::kill(dir),
            _ => Err(OpsError::InvalidInput("unknown cgroup control write")),
        }
    }

    fn cgroup_wait_frozen(
        &self,
        dir: &tl_ops::cgroup::CgroupDir,
        wait: Duration,
    ) -> Result<(), OpsError> {
        tl_ops::cgroup::wait_frozen(dir, wait, tl_ops::cgroup::FREEZE_POLL)
    }

    fn dump_available(&self) -> bool {
        tl_ops::dump::dump_available()
    }

    fn dump_to_path(
        &self,
        key: ProcessKey,
        boot: BootId,
        mode: tl_model::actions::DumpMode,
        out: &std::path::Path,
    ) -> Result<(), OpsError> {
        tl_ops::dump::dump_to_path(key, boot, mode, out)
    }

    fn apply_pid_op(
        &self,
        key: ProcessKey,
        boot: BootId,
        op: &process::PidOp,
    ) -> Result<(), OpsError> {
        use process::PidOp;
        match op {
            PidOp::Nice(n) => tl_ops::process::set_nice(key, boot, *n),
            PidOp::Affinity(set) => tl_ops::process::set_affinity(key, boot, set),
            PidOp::Sched { policy, rt_prio } => {
                tl_ops::process::set_sched(key, boot, *policy, *rt_prio)
            }
            PidOp::IoPriority(prio) => tl_ops::process::set_io_priority(key, boot, *prio),
        }
    }

    fn apply_thread_op(
        &self,
        process: ProcessKey,
        boot: BootId,
        tid: u32,
        op: &process::ThreadOp,
    ) -> Result<(), OpsError> {
        use process::{PidOp, ThreadOp};
        match op {
            ThreadOp::Signal(sig) => tl_ops::process::thread_send_signal(process, boot, tid, *sig),
            ThreadOp::Pid(PidOp::Nice(n)) => {
                tl_ops::process::thread_set_nice(process, boot, tid, *n)
            }
            ThreadOp::Pid(PidOp::Affinity(set)) => {
                tl_ops::process::thread_set_affinity(process, boot, tid, set)
            }
            ThreadOp::Pid(PidOp::Sched { policy, rt_prio }) => {
                tl_ops::process::thread_set_sched(process, boot, tid, *policy, *rt_prio)
            }
            // `process::run_thread` never sends this (R-097 allows no I/O priority per thread).
            ThreadOp::Pid(PidOp::IoPriority(_)) => {
                Err(OpsError::Unsupported("no I/O priority for one thread"))
            }
        }
    }

    fn startup_disable_system(
        &self,
        system_file: &std::path::Path,
        roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult<std::path::PathBuf> {
        tl_ops::fs::autostart_disable_system(system_file, roots)
    }

    fn startup_enable_override(
        &self,
        user_file: &std::path::Path,
        roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult<tl_ops::fs::EnableOutcome> {
        tl_ops::fs::autostart_enable_override(user_file, roots)
    }

    fn startup_set_hidden(
        &self,
        user_file: &std::path::Path,
        hidden: bool,
        roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult {
        tl_ops::fs::autostart_set_hidden(user_file, hidden, roots)
    }

    fn startup_script_set_enabled(
        &self,
        script: &std::path::Path,
        enabled: bool,
        roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult {
        tl_ops::fs::script_set_enabled(script, enabled, roots)
    }

    fn startup_delete_file(
        &self,
        path: &std::path::Path,
        roots: &tl_ops::fs::StartupRoots,
    ) -> tl_ops::OpsResult {
        tl_ops::fs::delete_startup_file(path, roots)
    }

    fn write_atomic(&self, path: &std::path::Path, text: &str, mode: u32) -> tl_ops::OpsResult {
        tl_ops::write_atomic(path, text.as_bytes(), mode)
    }

    fn remove_file(&self, path: &std::path::Path) -> tl_ops::OpsResult {
        Ok(rustix::fs::unlink(path)?)
    }

    fn remove_dir_if_empty(&self, path: &std::path::Path) -> tl_ops::OpsResult {
        match rustix::fs::rmdir(path) {
            Ok(()) | Err(Errno::NOTEMPTY | Errno::EXIST | Errno::NOENT) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Everything an action task needs. Shared by every action of one session.
pub struct ActionCtx {
    pub events: async_channel::Sender<ActionEvent>,
    pub msgs: mpsc::Sender<EngineMsg>,
    pub ops: Arc<dyn ActionOps>,
    pub boot_id: BootId,
    pub uid: u32,
    pub config: Arc<EngineConfig>,
    /// `get()` is `None` when that manager is not connected (E08-009, B3 open item 8).
    pub systemd_system: ManagerConnSlot,
    pub systemd_user: ManagerConnSlot,
    /// `RealOpeners` in production. `None` refuses the targetless kinds.
    pub openers: Option<Arc<dyn Openers>>,
    /// The session's `HelperHandle` (E29-014). `None` when the session runs no helper task.
    pub helper: Option<Arc<dyn PrivilegedOps>>,
    /// Set once `ConnectionsDestroySupport(true)` was sent this session (E11-021).
    pub destroy_support_sent: AtomicBool,
    /// Picks the unit bus in place of the manager slots. `None` in production; tests set it.
    pub unit_bus_override: Option<unit::UnitBusOverride>,
    /// The startup entry directories, from the environment in production.
    pub startup_paths: startup::StartupPaths,
}

/// The `ActionGate` the reducer calls (E11-A1). The only entry to `run_action`: every request
/// passes `admit` first. It lives in the reducer, so a session restart drops it, and `Drop`
/// aborts every action still in flight (ADR-004 task tree) and ends it as `Lost` (ADR-018).
pub struct Gate {
    ctx: Arc<ActionCtx>,
    in_flight: FxHashMap<ActionId, InFlight>,
    busy_units: FxHashSet<UnitRef>,
    /// Read from the ops once in `new` (R-141). Not re-checked per snapshot.
    caps: tl_model::health::EngineCaps,
    /// Events of actions that had already ended: they reached no session (ADR-018). Every
    /// `EndLock` the gate hands out counts into it.
    late: Arc<AtomicU64>,
}

struct InFlight {
    handle: TaskHandle<()>,
    /// The request's own id, which its events carry.
    client: ActionId,
    route: ActionRoute,
    /// Set by whoever sends the action's `RequestEvent`: its task or `Drop`.
    ended: Arc<EndLock>,
    kind: ActionKind,
    /// The admitted `Unit` targets, freed in `finished`.
    units: SmallVec<[UnitRef; 1]>,
    /// The manager of the first `Unit` target, which picks the systemd source to refresh.
    manager: Option<Manager>,
}

impl Gate {
    /// The actions admitted and not yet `finished`, which admission counts (ADR-006).
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// The count of events dropped because their action had ended. Its tasks share it, so it
    /// keeps counting after the gate is dropped. For debugging only.
    pub fn late(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.late)
    }

    pub fn new(ctx: Arc<ActionCtx>) -> Self {
        let mut caps = tl_model::health::EngineCaps::empty();
        if ctx.ops.dump_available() {
            caps.insert(tl_model::health::EngineCaps::GDB);
        }
        Self {
            caps,
            ctx,
            in_flight: FxHashMap::default(),
            busy_units: FxHashSet::default(),
            late: Arc::default(),
        }
    }
}

impl ActionGate for Gate {
    fn caps(&self) -> tl_model::health::EngineCaps {
        self.caps
    }

    /// Events go to the gate's own channel; the request's id is the engine id.
    fn submit(&mut self, cx: &Cx, req: ActionRequest, view: &AdmissionView<'_>) {
        let route = ActionRoute {
            id: req.id,
            events: self.ctx.events.clone(),
            requests: None,
        };
        self.submit_routed(cx, req, view, route);
    }

    fn submit_routed(
        &mut self,
        cx: &Cx,
        req: ActionRequest,
        view: &AdmissionView<'_>,
        route: ActionRoute,
    ) {
        let admission = match admit(&req, view, self.in_flight.len(), &self.busy_units) {
            Admission::Run { .. }
                if needs_desktop(&req.kind) && !self.ctx.config.desktop.is_present() =>
            {
                Admission::Reject(RejectReason::NoDesktopSession)
            }
            admission => admission,
        };
        let per_target = match admission {
            Admission::Reject(reason) => {
                let state = ActionState::Rejected(reason);
                let end = action_end(&state);
                let ev = event(req.id, None, state);
                if route.events.try_send(ev).is_err() {
                    tracing::debug!(target: "tasklens::action", id = req.id.0, "rejected event dropped: queue full or closed");
                }
                send_end(&route, req.id, end);
                return;
            }
            Admission::Run { per_target } => per_target,
        };
        let mut units = SmallVec::new();
        let mut manager = None;
        for (i, target) in req.targets.iter().enumerate() {
            if let ActionTarget::Unit(unit) = target {
                manager.get_or_insert(unit.manager);
                // A unit rejected as busy belongs to the action that holds it.
                if per_target.get(i).copied().flatten().is_none()
                    && self.busy_units.insert(unit.clone())
                {
                    units.push(unit.clone());
                }
            }
        }
        let id = route.id;
        let client = req.id;
        let kind = req.kind.clone();
        let ctx = Arc::clone(&self.ctx);
        let ended = Arc::new(EndLock::new(Arc::clone(&self.late)));
        let (task_route, task_ended) = (route.clone(), Arc::clone(&ended));
        let handle = cx
            .spawn(move |cx| async move {
                run_with(
                    &cx,
                    &ctx,
                    &task_route,
                    &req,
                    &per_target,
                    dispatch_boxed,
                    &task_ended,
                )
                .await;
            })
            .expect("spawn");
        self.in_flight.insert(
            id,
            InFlight {
                handle,
                client,
                route,
                ended,
                kind,
                units,
                manager,
            },
        );
    }

    fn finished(&mut self, id: ActionId) -> Option<SourceId> {
        let done = self.in_flight.remove(&id)?;
        for unit in &done.units {
            self.busy_units.remove(unit);
        }
        refresh_source(&done.kind, done.manager)
    }
}

impl Drop for Gate {
    /// `TaskHandle` does not cancel on drop, so each action still in flight is aborted here. The
    /// gate then ends it as `Lost`, unless its task already sent the end: the shared `EndLock`
    /// lets exactly one of them send it. A best-effort `OutcomeUnknown` summary for the UI's
    /// `ActionTracker` goes to the action events first, so the end is the last event. Both go
    /// out with the lock held, so an aborted task that is still running pushes nothing after
    /// them.
    fn drop(&mut self) {
        for flight in self.in_flight.values() {
            flight.handle.abort();
            let mut ended = flight.ended.lock();
            if *ended {
                continue;
            }
            *ended = true;
            let summary = ActionState::OutcomeUnknown(Arc::from(ENGINE_STOPPED));
            if flight
                .route
                .events
                .try_send(event(flight.client, None, summary))
                .is_err()
            {
                tracing::debug!(target: "tasklens::action", id = flight.client.0, "summary of an aborted action dropped: queue full or closed");
            }
            send_end(&flight.route, flight.client, RequestEnd::Lost);
        }
    }
}

/// The `RequestEnd` an action with final summary `state` ends with.
pub fn action_end(state: &ActionState) -> RequestEnd {
    match state {
        ActionState::Succeeded | ActionState::Saved(_) => RequestEnd::Done,
        ActionState::Rejected(RejectReason::Busy) => RequestEnd::Busy,
        // A summary is never `Started` or `Progress`.
        ActionState::Rejected(_)
        | ActionState::Failed(_)
        | ActionState::OutcomeUnknown(_)
        | ActionState::Started
        | ActionState::Progress { .. } => RequestEnd::Failed,
    }
}

/// Sends the action's one `RequestEvent` on its route. The channel is unbounded, so only a
/// closed one refuses it: the session is gone.
fn send_end(route: &ActionRoute, id: ActionId, end: RequestEnd) {
    let Some(tx) = route.requests.as_ref() else {
        return;
    };
    let ev = RequestEvent {
        kind: IdKind::Action,
        id: id.0,
        end,
    };
    if tx.try_send(ev).is_err() {
        tracing::debug!(target: "tasklens::action", id = id.0, ?end, "request end dropped: channel closed");
    }
}

/// The source `ActionFinished` refreshes for `kind` (ADR-005). `manager` is the manager of the
/// first `Unit` target; a unit kind without one refreshes the system manager.
fn refresh_source(kind: &ActionKind, manager: Option<Manager>) -> Option<SourceId> {
    let by_manager = || match manager {
        Some(Manager::User) => SourceId::SystemdUser,
        _ => SourceId::SystemdSystem,
    };
    match kind {
        ActionKind::Signal(_)
        | ActionKind::KillTree
        | ActionKind::Suspend
        | ActionKind::Resume
        | ActionKind::SetNice(_)
        | ActionKind::SetSched { .. }
        | ActionKind::SetAffinity(_)
        | ActionKind::SetIoPriority(_)
        | ActionKind::FreezeGroup
        | ActionKind::ThawGroup
        | ActionKind::KillGroup
        | ActionKind::Dump { .. } => Some(SourceId::Processes),
        ActionKind::UnitStart
        | ActionKind::UnitStop
        | ActionKind::UnitRestart
        | ActionKind::UnitReload
        | ActionKind::UnitResetFailed
        | ActionKind::UnitEnable
        | ActionKind::UnitDisable
        | ActionKind::UnitMask
        | ActionKind::UnitUnmask
        | ActionKind::UnitWriteRecovery(_)
        | ActionKind::UnitRemoveRecovery => Some(by_manager()),
        ActionKind::StartupEnable
        | ActionKind::StartupDisable
        | ActionKind::StartupRunNow
        | ActionKind::StartupDelete => Some(SourceId::Startup),
        ActionKind::SockDestroy => Some(SourceId::Connections),
        ActionKind::OpenUri(_)
        | ActionKind::ShowInFileManager { .. }
        | ActionKind::Launch(_)
        | ActionKind::ActivateWindow { .. } => None,
    }
}

/// The production gate factory that `default_hooks` installs, with `RealOps`.
pub fn gate_factory(hook: HookCtx) -> Box<dyn ActionGate> {
    gate_with_ops(hook, Arc::new(RealOps))
}

/// `gate_factory` with `ops` in place of `RealOps`, for tests.
pub fn gate_with_ops(hook: HookCtx, ops: Arc<dyn ActionOps>) -> Box<dyn ActionGate> {
    let openers: Arc<dyn Openers> = Arc::new(RealOpeners {
        session: hook.shared.systemd_user.clone(),
        desktop: hook.config.desktop.clone(),
    });
    Box::new(Gate::new(Arc::new(ActionCtx {
        events: hook.events_tx,
        msgs: hook.msgs_tx,
        ops,
        boot_id: hook.boot_id,
        uid: rustix::process::getuid().as_raw(),
        config: hook.config,
        systemd_system: hook.shared.systemd_system,
        systemd_user: hook.shared.systemd_user,
        openers: Some(openers),
        helper: hook
            .shared
            .helper
            .map(|h| Arc::new(h) as Arc<dyn PrivilegedOps>),
        destroy_support_sent: AtomicBool::new(false),
        unit_bus_override: None,
        startup_paths: startup::StartupPaths::from_env(),
    })))
}

/// The per-target dispatch `run_action` uses. Tests pass their own.
type Dispatch = for<'a> fn(
    &'a Cx,
    &'a ActionCtx,
    &'a ActionKind,
    &'a ActionTarget,
    &'a Progress<'a>,
) -> Pin<Box<dyn Future<Output = ActionState> + Send + 'a>>;

/// One admitted request: `Started`, one final state per target, one summary with
/// `target: None`, then `ActionFinished` to the reducer. A panic reports
/// `Failed(Internal("internal error"))` and `ActionFinished` is still sent (ADR-015).
///
/// A cancelled task (the session's `Gate` was dropped, or shutdown) starts no further target,
/// emits no further event and sends no `ActionFinished`. Cancellation is cooperative, so a
/// target operation already running finishes first.
pub async fn run_action(
    cx: Cx,
    actx: Arc<ActionCtx>,
    req: ActionRequest,
    per_target: Vec<Option<RejectReason>>,
) {
    let route = ActionRoute {
        id: req.id,
        events: actx.events.clone(),
        requests: None,
    };
    run_action_routed(cx, actx, req, per_target, route).await;
}

/// `run_action` for one client session (ADR-018): events carry `req.id` and go to
/// `route.events`; `ActionFinished` carries the engine id `route.id`.
pub async fn run_action_routed(
    cx: Cx,
    actx: Arc<ActionCtx>,
    req: ActionRequest,
    per_target: Vec<Option<RejectReason>>,
    route: ActionRoute,
) {
    let ended = EndLock::default();
    run_with(
        &cx,
        &actx,
        &route,
        &req,
        &per_target,
        dispatch_boxed,
        &ended,
    )
    .await;
}

/// The action task. `ended` is shared with the gate, and every event goes out through it, so
/// none follows the end. The summary push and the end are one step under the lock: the task
/// sends the end only when the gate has not. A task cancelled before it has a summary sends no
/// end; the gate that aborted it does.
async fn run_with(
    cx: &Cx,
    actx: &ActionCtx,
    route: &ActionRoute,
    req: &ActionRequest,
    per_target: &[Option<RejectReason>],
    dispatch: Dispatch,
    ended: &EndLock,
) {
    let events = &Emitter {
        events: &route.events,
        ended,
    };
    emit(events, cx, event(req.id, None, ActionState::Started)).await;
    let body = AssertUnwindSafe(run_body(cx, actx, events, req, per_target, dispatch))
        .catch_unwind()
        .await;
    let summary = match body {
        Ok(Some(summary)) => summary,
        Ok(None) => return,
        Err(_) => internal_error(),
    };
    let end = action_end(&summary);
    let end_now = |ended: &mut bool| {
        *ended = true;
        send_end(route, req.id, end);
    };
    let summary = event(req.id, None, summary);
    match time::timeout(cx.now(), EVENT_SEND_TIMEOUT, events.push(summary, end_now)).await {
        Ok(Some(Ok(()))) => {}
        Ok(None) => {
            tracing::debug!(target: "tasklens::action", id = req.id.0, "the gate ended the action first");
            return;
        }
        Ok(Some(Err(_))) => {
            tracing::warn!(target: "tasklens::action", id = req.id.0, "event channel closed, summary dropped");
        }
        Err(_) => {
            tracing::warn!(target: "tasklens::action", id = req.id.0, "event channel full for 1 s, summary dropped");
            let mut held = ended.lock();
            if *held {
                return;
            }
            end_now(&mut held);
        }
    }
    if cx.is_cancel_requested() {
        tracing::debug!(target: "tasklens::action", id = req.id.0, "action cancelled, ActionFinished not sent");
        return;
    }
    let finished = EngineMsg::ActionFinished { id: route.id };
    match time::timeout(cx.now(), EVENT_SEND_TIMEOUT, actx.msgs.send(cx, finished)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            tracing::warn!(target: "tasklens::action", id = req.id.0, "reducer gone, ActionFinished dropped");
        }
        Err(_) => {
            tracing::warn!(target: "tasklens::action", id = req.id.0, "reducer busy, ActionFinished dropped");
        }
    }
}

/// Emits every target's final state and returns the summary. `None` when the task was
/// cancelled.
async fn run_body(
    cx: &Cx,
    actx: &ActionCtx,
    events: &Emitter<'_>,
    req: &ActionRequest,
    per_target: &[Option<RejectReason>],
    dispatch: Dispatch,
) -> Option<ActionState> {
    if cx.is_cancel_requested() {
        return None;
    }
    let summary = if req.kind.is_targetless() {
        // `privileged` only picks the deadline here: these kinds have no helper route.
        targetless::run(cx, actx, &req.kind, req.privileged).await
    } else if req.privileged {
        // E29-014: through the helper. Without one the request is refused: it never falls back
        // to an unprivileged call.
        match actx.helper.as_deref() {
            Some(h) => helper::run_privileged(cx, actx, events, req, per_target, h).await?,
            None => ActionState::Rejected(RejectReason::Unsupported),
        }
    } else {
        // `KillTree` runs the admitted targets in one `process::kill_tree` call (E11-004).
        let mut tree = match req.kind {
            ActionKind::KillTree => Some(run_tree(cx, actx, req, per_target).await.into_iter()),
            _ => None,
        };
        let mut states = Vec::with_capacity(req.targets.len());
        for (i, target) in req.targets.iter().enumerate() {
            if cx.is_cancel_requested() {
                return None;
            }
            let progress = Progress::new(events, req.id, i);
            let state = match (per_target.get(i).copied().flatten(), tree.as_mut()) {
                (Some(reason), _) => ActionState::Rejected(reason),
                (None, Some(tree)) => tree.next().unwrap_or_else(internal_error),
                (None, None) => run_target(cx, actx, &req.kind, target, &progress, dispatch).await,
            };
            tracing::info!(
                target: "tasklens::action",
                kind = kind_name(&req.kind),
                pid = target_pid(target),
                result = ?state,
            );
            emit(events, cx, event(req.id, Some(i), state.clone())).await;
            states.push(state);
        }
        summarize(&states)
    };
    Some(summary)
}

/// Calls `process::kill_tree` once with the targets admission passed. A panic gives each of them
/// `Failed(Internal)`.
async fn run_tree(
    cx: &Cx,
    actx: &ActionCtx,
    req: &ActionRequest,
    per_target: &[Option<RejectReason>],
) -> Vec<ActionState> {
    let admitted: Vec<ActionTarget> = req
        .targets
        .iter()
        .enumerate()
        .filter(|(i, _)| per_target.get(*i).copied().flatten().is_none())
        .map(|(_, target)| target.clone())
        .collect();
    AssertUnwindSafe(process::kill_tree(cx, actx, &admitted))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| vec![internal_error(); admitted.len()])
}

/// A panic in one target gives that target `Failed(Internal)`; the other targets still run.
async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
    progress: &Progress<'_>,
    dispatch: Dispatch,
) -> ActionState {
    AssertUnwindSafe(dispatch(cx, actx, kind, target, progress))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| internal_error())
}

fn dispatch_boxed<'a>(
    cx: &'a Cx,
    actx: &'a ActionCtx,
    kind: &'a ActionKind,
    target: &'a ActionTarget,
    progress: &'a Progress<'a>,
) -> Pin<Box<dyn Future<Output = ActionState> + Send + 'a>> {
    Box::pin(dispatch(cx, actx, kind, target, progress))
}

/// The `(kind, target)` table (E11-A2). No arm matches every kind, so a new `ActionKind` does not
/// compile until it has a row here: this is what keeps a kind from reaching the wrong module.
///
/// - `Process | Thread` × the process kinds → `process::run_target`
/// - `Cgroup` × `FreezeGroup | ThawGroup | KillGroup` → `cgroup::run_target`
/// - `Unit` × `FreezeGroup | ThawGroup | KillGroup` and the `Unit*` job and file kinds →
///   `unit::run_target`; `Unit` × `UnitWriteRecovery | UnitRemoveRecovery` →
///   `recovery::run_target`
/// - `Startup` × `Startup*` → `startup::run_target`
/// - `Socket` × `SockDestroy` → `socket::run_target`
/// - `Process` × `Dump` → `dump::run_target`, which reports progress through `progress`
///
/// Every other pair is `Rejected(Unsupported)`. `KillTree` runs in `run_tree` and the targetless
/// kinds in `targetless::run`, so they never reach this function.
async fn dispatch(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
    progress: &Progress<'_>,
) -> ActionState {
    use ActionKind as K;
    use ActionTarget as T;
    match (kind, target) {
        (
            K::Signal(_)
            | K::Suspend
            | K::Resume
            | K::SetNice(_)
            | K::SetSched { .. }
            | K::SetAffinity(_)
            | K::SetIoPriority(_),
            T::Process(_) | T::Thread { .. },
        ) => process::run_target(cx, actx, kind, target).await,
        (K::FreezeGroup | K::ThawGroup | K::KillGroup, T::Cgroup(cg)) => {
            cgroup::run_target(cx, actx, kind, cg).await
        }
        (
            K::FreezeGroup
            | K::ThawGroup
            | K::KillGroup
            | K::UnitStart
            | K::UnitStop
            | K::UnitRestart
            | K::UnitReload
            | K::UnitResetFailed
            | K::UnitEnable
            | K::UnitDisable
            | K::UnitMask
            | K::UnitUnmask,
            T::Unit(unit),
        ) => unit::run_target(cx, actx, kind, unit).await,
        (K::UnitWriteRecovery(_) | K::UnitRemoveRecovery, T::Unit(unit)) => {
            recovery::run_target(cx, actx, kind, unit).await
        }
        (
            K::StartupEnable | K::StartupDisable | K::StartupRunNow | K::StartupDelete,
            T::Startup(entry),
        ) => startup::run_target(cx, actx, kind, entry).await,
        (
            K::SockDestroy,
            T::Socket {
                cookie,
                family,
                proto,
                four_tuple,
            },
        ) => socket::run_target(cx, actx, kind, *cookie, *family, *proto, *four_tuple).await,
        // A privileged dump goes through `helper::run_privileged`, so this call is the
        // unprivileged one.
        (K::Dump { mode, name }, T::Process(key)) => {
            dump::run_target(cx, actx, *mode, name, *key, false, progress).await
        }
        // The mismatched pairs, one arm per kind group.
        (
            K::Signal(_)
            | K::Suspend
            | K::Resume
            | K::SetNice(_)
            | K::SetSched { .. }
            | K::SetAffinity(_)
            | K::SetIoPriority(_)
            | K::FreezeGroup
            | K::ThawGroup
            | K::KillGroup
            | K::Dump { .. }
            | K::UnitStart
            | K::UnitStop
            | K::UnitRestart
            | K::UnitReload
            | K::UnitResetFailed
            | K::UnitEnable
            | K::UnitDisable
            | K::UnitMask
            | K::UnitUnmask
            | K::UnitWriteRecovery(_)
            | K::UnitRemoveRecovery
            | K::StartupEnable
            | K::StartupDisable
            | K::StartupRunNow
            | K::StartupDelete
            | K::SockDestroy,
            _,
        )
        // Handled before the target loop (E11-004, E11-016).
        | (
            K::KillTree
            | K::OpenUri(_)
            | K::ShowInFileManager { .. }
            | K::Launch(_)
            | K::ActivateWindow { .. },
            _,
        ) => ActionState::Rejected(RejectReason::Unsupported),
    }
}

/// The lock on an action's end, shared by its task and the gate. Whoever sets it sends the
/// action's `RequestEvent`. The task pushes each of its events with the lock held, so once the
/// gate has ended the action no event of it reaches the session: neither one that waits for room
/// in the channel nor one from a task that checked just before the gate took the lock.
#[derive(Default)]
struct EndLock {
    ended: Mutex<bool>,
    /// The gate's count of events dropped because the action had ended.
    late: Arc<AtomicU64>,
}

impl EndLock {
    fn new(late: Arc<AtomicU64>) -> Self {
        EndLock {
            ended: Mutex::new(false),
            late,
        }
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        self.ended.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts one event dropped because the action had ended.
    fn count_late(&self) {
        self.late.fetch_add(1, Ordering::Relaxed);
    }
}

/// Where an action task's events go: its route's channel, behind its `EndLock`.
struct Emitter<'a> {
    events: &'a async_channel::Sender<ActionEvent>,
    ended: &'a EndLock,
}

impl Emitter<'_> {
    /// Pushes `ev` unless the action has ended; `None`, counted as late, when it had. Each try
    /// holds the end lock, and `pushed` runs under it right after the push, so the gate cannot
    /// end the action between the check and the push.
    async fn push(
        &self,
        ev: ActionEvent,
        mut pushed: impl FnMut(&mut bool),
    ) -> Option<Result<(), async_channel::SendError<ActionEvent>>> {
        let mut send = std::pin::pin!(self.events.send(ev));
        std::future::poll_fn(|task| {
            let mut ended = self.ended.lock();
            if *ended {
                self.ended.count_late();
                return Poll::Ready(None);
            }
            let sent = std::task::ready!(send.as_mut().poll(task));
            pushed(&mut ended);
            Poll::Ready(Some(sent))
        })
        .await
    }
}

/// Where a running target reports progress: `ActionState::Progress` events for one target of
/// one action (ADR-005, ADR-018). A report never waits: one that finds the queue full is
/// dropped, as the next one replaces it. None goes out after the action has ended.
pub struct Progress<'a> {
    events: Option<&'a Emitter<'a>>,
    id: ActionId,
    target: usize,
}

impl<'a> Progress<'a> {
    fn new(events: &'a Emitter<'a>, id: ActionId, target: usize) -> Self {
        Progress {
            events: Some(events),
            id,
            target,
        }
    }

    /// Reports nothing. For calls that have no action events, such as tests of one target.
    pub fn none() -> Progress<'static> {
        Progress {
            events: None,
            id: ActionId(0),
            target: 0,
        }
    }

    /// Sends `Progress { bytes }` for this target unless the action has ended.
    pub fn report(&self, bytes: u64) {
        let Some(events) = self.events else {
            return;
        };
        let ended = events.ended.lock();
        if *ended {
            events.ended.count_late();
            return;
        }
        let ev = event(self.id, Some(self.target), ActionState::Progress { bytes });
        if events.events.try_send(ev).is_err() {
            tracing::debug!(target: "tasklens::action", id = self.id.0, "progress event dropped: queue full or closed");
        }
    }
}

/// Sends one event with a 1 s timeout. When the UI is not draining, the event is dropped and the
/// action continues. An event of an action that has ended is dropped and counted as late.
async fn emit(events: &Emitter<'_>, cx: &Cx, ev: ActionEvent) {
    let id = ev.id.0;
    if cx.is_cancel_requested() {
        if *events.ended.lock() {
            events.ended.count_late();
        }
        tracing::debug!(target: "tasklens::action", id, "action cancelled, event dropped");
        return;
    }
    match time::timeout(cx.now(), EVENT_SEND_TIMEOUT, events.push(ev, |_| {})).await {
        Ok(Some(Ok(()))) => {}
        Ok(None) => {
            tracing::debug!(target: "tasklens::action", id, "action ended, event dropped");
        }
        Ok(Some(Err(_))) => {
            tracing::warn!(target: "tasklens::action", id, "event channel closed, event dropped");
        }
        Err(_) => {
            tracing::warn!(target: "tasklens::action", id, "event channel full for 1 s, event dropped");
        }
    }
}

/// Runs `job` under the deadline `d`. `None` waits without a timeout.
pub async fn with_deadline<T>(
    cx: &Cx,
    d: Option<Duration>,
    job: impl Future<Output = T>,
) -> Result<T, Elapsed> {
    match d {
        Some(d) => time::timeout(cx.now(), d, job).await,
        None => Ok(job.await),
    }
}

fn internal_error() -> ActionState {
    ActionState::Failed(ActionError::Internal(Arc::from(INTERNAL_ERROR)))
}

fn event(id: ActionId, target: Option<usize>, state: ActionState) -> ActionEvent {
    ActionEvent {
        id,
        target: target.map(index_u16),
        state,
    }
}

/// Admission caps targets at 256, so this never saturates.
fn index_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

fn target_pid(target: &ActionTarget) -> Option<u32> {
    match target {
        ActionTarget::Process(key) | ActionTarget::Thread { process: key, .. } => Some(key.pid),
        _ => None,
    }
}

/// The kind's name for logs. Carries no user data (ADR-015).
fn kind_name(kind: &ActionKind) -> &'static str {
    match kind {
        ActionKind::Signal(_) => "Signal",
        ActionKind::KillTree => "KillTree",
        ActionKind::Suspend => "Suspend",
        ActionKind::Resume => "Resume",
        ActionKind::SetNice(_) => "SetNice",
        ActionKind::SetSched { .. } => "SetSched",
        ActionKind::SetAffinity(_) => "SetAffinity",
        ActionKind::SetIoPriority(_) => "SetIoPriority",
        ActionKind::FreezeGroup => "FreezeGroup",
        ActionKind::ThawGroup => "ThawGroup",
        ActionKind::KillGroup => "KillGroup",
        ActionKind::Dump { .. } => "Dump",
        ActionKind::UnitStart => "UnitStart",
        ActionKind::UnitStop => "UnitStop",
        ActionKind::UnitRestart => "UnitRestart",
        ActionKind::UnitReload => "UnitReload",
        ActionKind::UnitResetFailed => "UnitResetFailed",
        ActionKind::UnitEnable => "UnitEnable",
        ActionKind::UnitDisable => "UnitDisable",
        ActionKind::UnitMask => "UnitMask",
        ActionKind::UnitUnmask => "UnitUnmask",
        ActionKind::UnitWriteRecovery(_) => "UnitWriteRecovery",
        ActionKind::UnitRemoveRecovery => "UnitRemoveRecovery",
        ActionKind::StartupEnable => "StartupEnable",
        ActionKind::StartupDisable => "StartupDisable",
        ActionKind::StartupRunNow => "StartupRunNow",
        ActionKind::StartupDelete => "StartupDelete",
        ActionKind::SockDestroy => "SockDestroy",
        ActionKind::OpenUri(_) => "OpenUri",
        ActionKind::ShowInFileManager { .. } => "ShowInFileManager",
        ActionKind::Launch(_) => "Launch",
        ActionKind::ActivateWindow { .. } => "ActivateWindow",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustc_hash::FxHashMap;
    use smallvec::SmallVec;
    use tl_model::actions::{ActionId, CgroupRef, Signal};
    use tl_model::table::Manager;
    use tl_model::time::BootId;

    use super::*;

    const BOOT: BootId = BootId(1);

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 100 + u64::from(pid),
            boot_id: BOOT,
            pid,
        }
    }

    fn request(kind: ActionKind, targets: Vec<ActionTarget>) -> ActionRequest {
        ActionRequest {
            id: ActionId(1),
            kind,
            targets: SmallVec::from_vec(targets),
            privileged: false,
        }
    }

    fn term(targets: Vec<ActionTarget>) -> ActionRequest {
        request(ActionKind::Signal(Signal::Term), targets)
    }

    struct Fixture {
        live: FxHashMap<ProcessKey, ProcFlags>,
        exited: FxHashSet<ProcessKey>,
        busy: FxHashSet<UnitRef>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                live: FxHashMap::default(),
                exited: FxHashSet::default(),
                busy: FxHashSet::default(),
            }
        }

        fn admit(&self, req: &ActionRequest, in_flight: usize) -> Admission {
            let view = AdmissionView {
                boot_id: BOOT,
                live: &self.live,
                exited_within_linger: &self.exited,
            };
            admit(req, &view, in_flight, &self.busy)
        }

        fn per_target(&self, req: &ActionRequest) -> Vec<Option<RejectReason>> {
            match self.admit(req, 0) {
                Admission::Run { per_target } => per_target,
                other => panic!("expected Run, got {other:?}"),
            }
        }
    }

    fn unit(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::User,
            name: Arc::from(name),
        }
    }

    #[test]
    fn admission_limits() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        let one = term(vec![ActionTarget::Process(key(500))]);
        assert_eq!(f.admit(&one, 16), Admission::Reject(RejectReason::Busy));
        assert!(matches!(f.admit(&one, 15), Admission::Run { .. }));

        let many = |n: u32| {
            term(
                (0..n)
                    .map(|i| ActionTarget::Process(key(1000 + i)))
                    .collect(),
            )
        };
        assert_eq!(
            f.admit(&many(257), 0),
            Admission::Reject(RejectReason::TooManyTargets)
        );
        match f.admit(&many(256), 0) {
            Admission::Run { per_target } => assert_eq!(per_target.len(), 256),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn admission_identity_and_flags() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        f.live.insert(key(2), ProcFlags::KERNEL_THREAD);
        f.live.insert(key(600), ProcFlags::IS_SELF);
        f.live.insert(key(1), ProcFlags::PROTECTED);
        f.live.insert(key(700), ProcFlags::PROTECTED);
        f.exited.insert(key(800));
        let other_boot = ProcessKey {
            boot_id: BootId(2),
            ..key(500)
        };
        let req = term(vec![
            ActionTarget::Process(key(500)),
            ActionTarget::Process(other_boot),
            ActionTarget::Process(key(900)),
            ActionTarget::Process(key(800)),
            ActionTarget::Process(key(2)),
            ActionTarget::Process(key(600)),
            ActionTarget::Process(key(1)),
            ActionTarget::Process(key(700)),
            ActionTarget::Thread {
                process: other_boot,
                tid: 501,
            },
            ActionTarget::Thread {
                process: key(500),
                tid: 501,
            },
        ]);
        assert_eq!(
            f.per_target(&req),
            vec![
                None,
                Some(RejectReason::IdentityChanged),
                Some(RejectReason::IdentityChanged),
                None,
                Some(RejectReason::KernelThread),
                Some(RejectReason::Protected),
                Some(RejectReason::Protected),
                None,
                Some(RejectReason::IdentityChanged),
                None,
            ]
        );

        // Every target rejected still runs, so the UI gets per-target events.
        let all_bad = term(vec![ActionTarget::Process(key(900))]);
        assert_eq!(
            f.per_target(&all_bad),
            vec![Some(RejectReason::IdentityChanged)]
        );
    }

    #[test]
    fn admission_targetless_kinds() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        assert_eq!(
            f.admit(&term(vec![]), 0),
            Admission::Reject(RejectReason::Unsupported)
        );
        let open = |targets| {
            request(
                ActionKind::OpenUri(Arc::from("https://example.org")),
                targets,
            )
        };
        assert_eq!(
            f.admit(&open(vec![]), 0),
            Admission::Run {
                per_target: Vec::new()
            }
        );
        assert_eq!(
            f.admit(&open(vec![ActionTarget::Process(key(500))]), 0),
            Admission::Reject(RejectReason::Unsupported)
        );
        let show = request(
            ActionKind::ShowInFileManager {
                path: Arc::from("/home"),
                properties: false,
            },
            vec![],
        );
        assert!(targetless(&show.kind));
        assert!(!targetless(&ActionKind::KillTree));
        assert_eq!(
            f.admit(&show, 0),
            Admission::Run {
                per_target: Vec::new()
            }
        );
        // The in-flight limit applies to targetless kinds too.
        assert_eq!(
            f.admit(&open(vec![]), 16),
            Admission::Reject(RejectReason::Busy)
        );
    }

    #[test]
    fn admission_rejects_deadline() {
        let mut f = Fixture::new();
        f.live.insert(key(500), ProcFlags::empty());
        let deadline = ActionKind::SetSched {
            policy: SchedPolicy::Deadline,
            rt_prio: 0,
        };
        for privileged in [false, true] {
            let mut req = request(deadline.clone(), vec![ActionTarget::Process(key(500))]);
            req.privileged = privileged;
            assert_eq!(
                f.admit(&req, 0),
                Admission::Reject(RejectReason::Unsupported)
            );
        }
        // Other policies on the same target still run.
        let other = request(
            ActionKind::SetSched {
                policy: SchedPolicy::Other,
                rt_prio: 0,
            },
            vec![ActionTarget::Process(key(500))],
        );
        assert_eq!(f.per_target(&other), vec![None]);
    }

    #[test]
    fn admission_unit_busy() {
        let mut f = Fixture::new();
        f.busy.insert(unit("busy.service"));
        let req = request(
            ActionKind::UnitRestart,
            vec![
                ActionTarget::Unit(unit("busy.service")),
                ActionTarget::Unit(unit("idle.service")),
                ActionTarget::Cgroup(CgroupRef {
                    rel_path: Arc::from("user.slice/app.slice/app-foo.scope"),
                    cgroup_id: 42,
                }),
            ],
        );
        assert_eq!(
            f.per_target(&req),
            vec![Some(RejectReason::Busy), None, None]
        );
    }

    struct NoOps;
    impl ActionOps for NoOps {}

    /// An `ActionCtx` over `NoOps` whose channels stay open while the returned receivers live.
    fn ctx() -> (
        Arc<ActionCtx>,
        async_channel::Receiver<ActionEvent>,
        mpsc::Receiver<EngineMsg>,
    ) {
        ctx_with_ops(Arc::new(NoOps))
    }

    /// `ctx()` with `ops` in place of `NoOps`.
    fn ctx_with_ops(
        ops: Arc<dyn ActionOps>,
    ) -> (
        Arc<ActionCtx>,
        async_channel::Receiver<ActionEvent>,
        mpsc::Receiver<EngineMsg>,
    ) {
        let config = EngineConfig::new(
            crate::config::EngineMode::Full,
            std::path::PathBuf::from("/nonexistent"),
        );
        ctx_with_config(ops, config)
    }

    /// `ctx_with_ops` with `config`.
    fn ctx_with_config(
        ops: Arc<dyn ActionOps>,
        config: EngineConfig,
    ) -> (
        Arc<ActionCtx>,
        async_channel::Receiver<ActionEvent>,
        mpsc::Receiver<EngineMsg>,
    ) {
        let (events_tx, events) = async_channel::bounded(256);
        let (msgs_tx, msgs) = mpsc::channel(256);
        let ctx = Arc::new(ActionCtx {
            events: events_tx,
            msgs: msgs_tx,
            ops,
            boot_id: BOOT,
            uid: 1000,
            config: Arc::new(config),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: None,
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: None,
            startup_paths: crate::actions::startup::StartupPaths::from_env(),
        });
        (ctx, events, msgs)
    }

    struct DumpOps(bool);
    impl ActionOps for DumpOps {
        fn dump_available(&self) -> bool {
            self.0
        }
    }

    #[test]
    fn gate_caps_follow_dump_available() {
        use tl_model::health::EngineCaps;
        for available in [true, false] {
            let (ctx, _events, _msgs) = ctx_with_ops(Arc::new(DumpOps(available)));
            let caps = Gate::new(ctx).caps();
            assert_eq!(caps.contains(EngineCaps::GDB), available);
            assert!(!caps.contains(EngineCaps::FILE_MANAGER1));
        }
    }

    /// A gate over `ctx()`.
    fn gate() -> (
        Gate,
        async_channel::Receiver<ActionEvent>,
        mpsc::Receiver<EngineMsg>,
    ) {
        let (ctx, events, msgs) = ctx();
        (Gate::new(ctx), events, msgs)
    }

    /// ADR-018, q4x76.17: a dropped gate (the reducer stopped) ends each action still in flight
    /// as `Lost`, once. Action 1 already sent its own end and gets no second one; action 2 had
    /// not run yet and gets `Lost`, a best-effort `OutcomeUnknown` summary and nothing else.
    #[test]
    fn dropped_gate_ends_each_action_in_flight_once() {
        use asupersync::types::Budget;
        use asupersync::{LabConfig, LabRuntime};

        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (a_tx, a_rx) = async_channel::bounded(16);
        let (b_tx, b_rx) = async_channel::bounded(16);
        let (a_req_tx, a_req) = async_channel::unbounded();
        let (b_req_tx, b_req) = async_channel::unbounded();
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let (mut gate, _own_events, mut msgs) = gate();
                let f = Fixture::new();
                let view = AdmissionView {
                    boot_id: BOOT,
                    live: &f.live,
                    exited_within_linger: &f.exited,
                };
                let open = |id| {
                    let mut req = request(
                        ActionKind::OpenUri(Arc::from("https://example.org")),
                        vec![],
                    );
                    req.id = ActionId(id);
                    req
                };
                let route = ActionRoute {
                    id: ActionId(10),
                    events: a_tx,
                    requests: Some(a_req_tx),
                };
                gate.submit_routed(&cx, open(1), &view, route);
                assert!(matches!(
                    msgs.recv(&cx).await,
                    Ok(EngineMsg::ActionFinished { id: ActionId(10) })
                ));
                // `finished` is not called, so action 1 is still in flight.
                let route = ActionRoute {
                    id: ActionId(11),
                    events: b_tx,
                    requests: Some(b_req_tx),
                };
                gate.submit_routed(&cx, open(2), &view, route);
                assert_eq!(gate.in_flight.len(), 2);
                drop(gate);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert!(handle.is_finished());

        let ends = |rx: &async_channel::Receiver<RequestEvent>| {
            std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>()
        };
        let end = |id, end| RequestEvent {
            kind: IdKind::Action,
            id,
            end,
        };
        assert_eq!(ends(&a_req), [end(1, RequestEnd::Failed)]);
        assert_eq!(ends(&b_req), [end(2, RequestEnd::Lost)]);
        let states = |rx: &async_channel::Receiver<ActionEvent>| {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|e| (e.id, e.target, e.state))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            states(&a_rx),
            [
                (ActionId(1), None, ActionState::Started),
                (
                    ActionId(1),
                    None,
                    ActionState::Rejected(RejectReason::Unsupported)
                ),
            ]
        );
        assert_eq!(
            states(&b_rx),
            [(
                ActionId(2),
                None,
                ActionState::OutcomeUnknown(Arc::from(ENGINE_STOPPED))
            )]
        );
    }

    /// q4x76.17: an aborted task whose event waits for room in the channel pushes nothing after
    /// the gate ended the action. The session's channel holds one event and is full, so the task
    /// waits on `Started` when the gate is dropped. When the reader frees the room the task wakes
    /// after the `Lost` end and must drop `Started`, and the gate's late count goes up by one.
    #[test]
    fn dropped_gate_ends_an_action_waiting_for_room() {
        use asupersync::types::Budget;
        use asupersync::{LabConfig, LabRuntime};

        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (events_tx, events) = async_channel::bounded(1);
        let (requests_tx, requests) = async_channel::unbounded();
        events_tx
            .try_send(event(ActionId(99), None, ActionState::Started))
            .expect("room for the filler");
        let held: Arc<std::sync::Mutex<Option<Gate>>> = Arc::default();
        let slot = Arc::clone(&held);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let (mut gate, _own_events, _msgs) = gate();
                let f = Fixture::new();
                let view = AdmissionView {
                    boot_id: BOOT,
                    live: &f.live,
                    exited_within_linger: &f.exited,
                };
                let mut req = request(
                    ActionKind::OpenUri(Arc::from("https://example.org")),
                    vec![],
                );
                req.id = ActionId(1);
                let route = ActionRoute {
                    id: ActionId(10),
                    events: events_tx,
                    requests: Some(requests_tx),
                };
                gate.submit_routed(&cx, req, &view, route);
                *slot.lock().expect("not poisoned") = Some(gate);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert!(handle.is_finished());
        assert!(requests.is_empty(), "the action waits to send Started");
        let gate = held.lock().expect("not poisoned").take().expect("gate");
        let late = gate.late();
        assert_eq!(late.load(Ordering::Relaxed), 0);
        // The reducer stops: `Reducer::stopped` drops the gate outside any action task.
        drop(gate);
        let end = RequestEvent {
            kind: IdKind::Action,
            id: 1,
            end: RequestEnd::Lost,
        };
        assert_eq!(requests.try_recv().ok(), Some(end));
        // The reader takes the filler; the waiting task wakes after the end.
        assert_eq!(events.try_recv().ok().map(|e| e.id), Some(ActionId(99)));
        lab.run_until_idle();
        let after: Vec<ActionEvent> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(after.is_empty(), "events after the end: {after:?}");
        assert!(requests.is_empty(), "one end only");
        assert_eq!(
            late.load(Ordering::Relaxed),
            1,
            "the dropped Started is counted"
        );
    }

    /// ADR-018: two sessions send action id 1. Each run's events carry id 1 and go only to its
    /// own route; `ActionFinished` and `finished` use the engine ids.
    /// q4x76.12, ADR-018: while the host has no desktop session, the kinds that act on its
    /// screen are refused with `NoDesktopSession` and nothing runs. Once the session is there
    /// they run.
    #[test]
    fn desktop_commands_need_a_desktop_session() {
        use asupersync::types::Budget;
        use asupersync::{LabConfig, LabRuntime};

        use crate::desktop::{DesktopEnv, DesktopSource};

        let config = EngineConfig::new(
            crate::config::EngineMode::Full,
            std::path::PathBuf::from("/nonexistent"),
        )
        .with_desktop(DesktopSource::UserManager);
        let desktop = config.desktop.clone();
        let (ctx, _own, mut msgs) = ctx_with_config(Arc::new(NoOps), config);
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (tx, rx) = async_channel::bounded(16);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut gate = Gate::new(ctx);
                let f = Fixture::new();
                let view = AdmissionView {
                    boot_id: BOOT,
                    live: &f.live,
                    exited_within_linger: &f.exited,
                };
                let route = |id| ActionRoute {
                    id: ActionId(id),
                    events: tx.clone(),
                    requests: None,
                };
                let desktop_kinds = [
                    ActionKind::OpenUri(Arc::from("https://example.org")),
                    ActionKind::ShowInFileManager {
                        path: Arc::from("/home/u/a.txt"),
                        properties: false,
                    },
                    ActionKind::Launch(LaunchSpec {
                        what: tl_model::actions::LaunchWhat::Command(Arc::from("kate")),
                        cwd: None,
                        as_admin: false,
                    }),
                    // Switch to window (q4x76.12 review).
                    ActionKind::ActivateWindow {
                        internal_id: Arc::from("{0a1b2c3d-0000-4000-8000-00000000000a}"),
                    },
                ];
                for (i, kind) in desktop_kinds.into_iter().enumerate() {
                    gate.submit_routed(&cx, request(kind, vec![]), &view, route(10 + i as u64));
                }
                assert_eq!(gate.in_flight.len(), 0, "nothing runs without a session");
                desktop.set(Some(DesktopEnv::from_lines(&["WAYLAND_DISPLAY=wayland-1"])));
                let open = request(
                    ActionKind::OpenUri(Arc::from("https://example.org")),
                    vec![],
                );
                gate.submit_routed(&cx, open, &view, route(20));
                assert_eq!(gate.in_flight.len(), 1, "admitted with a session");
                match msgs.recv(&cx).await {
                    Ok(EngineMsg::ActionFinished { id }) => assert_eq!(id, ActionId(20)),
                    _ => panic!("expected ActionFinished"),
                }
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        drop(handle);

        let states: Vec<ActionState> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|e| e.state)
            .collect();
        let refused = ActionState::Rejected(RejectReason::NoDesktopSession);
        assert_eq!(
            states,
            [
                refused.clone(),
                refused.clone(),
                refused.clone(),
                refused,
                ActionState::Started,
                // No openers in this ctx.
                ActionState::Rejected(RejectReason::Unsupported),
            ]
        );
    }

    #[test]
    fn routed_actions_report_to_their_own_route() {
        use asupersync::types::Budget;
        use asupersync::{LabConfig, LabRuntime};

        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (a_tx, a_rx) = async_channel::bounded(16);
        let (b_tx, b_rx) = async_channel::bounded(16);
        let finished: Arc<std::sync::Mutex<Vec<ActionId>>> = Arc::default();
        let out = Arc::clone(&finished);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let (mut gate, own_events, mut msgs) = gate();
                let f = Fixture::new();
                let view = AdmissionView {
                    boot_id: BOOT,
                    live: &f.live,
                    exited_within_linger: &f.exited,
                };
                // No openers, so the run ends with one `Rejected(Unsupported)` summary.
                let open = request(
                    ActionKind::OpenUri(Arc::from("https://example.org")),
                    vec![],
                );
                for (engine, events) in [(10, a_tx), (11, b_tx)] {
                    let route = ActionRoute {
                        id: ActionId(engine),
                        events,
                        requests: None,
                    };
                    gate.submit_routed(&cx, open.clone(), &view, route);
                }
                assert_eq!(gate.in_flight.len(), 2, "keyed by engine id");
                for _ in 0..2 {
                    match msgs.recv(&cx).await {
                        Ok(EngineMsg::ActionFinished { id }) => {
                            assert!(gate.finished(id).is_none());
                            out.lock().expect("not poisoned").push(id);
                        }
                        _ => panic!("expected ActionFinished"),
                    }
                }
                assert!(
                    own_events.try_recv().is_err(),
                    "nothing on the gate's own channel"
                );
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        drop(handle);

        let mut ids = finished.lock().expect("not poisoned").clone();
        ids.sort();
        assert_eq!(ids, [ActionId(10), ActionId(11)]);
        for rx in [a_rx, b_rx] {
            let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert_eq!(
                events
                    .iter()
                    .map(|e| (e.id, e.state.clone()))
                    .collect::<Vec<_>>(),
                [
                    (ActionId(1), ActionState::Started),
                    (
                        ActionId(1),
                        ActionState::Rejected(RejectReason::Unsupported)
                    ),
                ]
            );
        }
    }

    #[test]
    fn finished_refresh_source() {
        use asupersync::types::Budget;
        use asupersync::{LabConfig, LabRuntime};

        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let (mut gate, _events, _msgs) = gate();
                let f = Fixture::new();
                let view = AdmissionView {
                    boot_id: BOOT,
                    live: &f.live,
                    exited_within_linger: &f.exited,
                };
                let submit = |gate: &mut Gate, id: u64, kind, targets| {
                    let mut req = request(kind, targets);
                    req.id = ActionId(id);
                    gate.submit(&cx, req, &view);
                };

                let user = unit("app.service");
                submit(
                    &mut gate,
                    1,
                    ActionKind::UnitStart,
                    vec![ActionTarget::Unit(user.clone())],
                );
                assert!(gate.busy_units.contains(&user));
                // The busy unit is rejected per target and stays with action 1.
                submit(
                    &mut gate,
                    2,
                    ActionKind::UnitStop,
                    vec![ActionTarget::Unit(user.clone())],
                );
                assert_eq!(gate.finished(ActionId(2)), Some(SourceId::SystemdUser));
                assert!(gate.busy_units.contains(&user));
                assert_eq!(gate.finished(ActionId(1)), Some(SourceId::SystemdUser));
                assert!(gate.busy_units.is_empty());
                assert_eq!(gate.finished(ActionId(1)), None, "already finished");

                let system = UnitRef {
                    manager: Manager::System,
                    name: Arc::from("sshd.service"),
                };
                submit(
                    &mut gate,
                    3,
                    ActionKind::UnitRestart,
                    vec![ActionTarget::Unit(system)],
                );
                assert_eq!(gate.finished(ActionId(3)), Some(SourceId::SystemdSystem));
                submit(
                    &mut gate,
                    4,
                    ActionKind::OpenUri(Arc::from("https://example.org")),
                    vec![],
                );
                assert_eq!(gate.finished(ActionId(4)), None);
                assert!(gate.in_flight.is_empty());
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert!(handle.is_finished(), "the checks ran");

        for (kind, want) in [
            (ActionKind::Signal(Signal::Term), Some(SourceId::Processes)),
            (ActionKind::KillGroup, Some(SourceId::Processes)),
            (ActionKind::StartupRunNow, Some(SourceId::Startup)),
            (ActionKind::SockDestroy, Some(SourceId::Connections)),
            (
                ActionKind::UnitRemoveRecovery,
                Some(SourceId::SystemdSystem),
            ),
            (
                ActionKind::ShowInFileManager {
                    path: Arc::from("/home"),
                    properties: false,
                },
                None,
            ),
        ] {
            assert_eq!(refresh_source(&kind, None), want, "{kind:?}");
        }
        // Freeze, thaw and kill on a unit target refresh Processes too.
        assert_eq!(
            refresh_source(&ActionKind::FreezeGroup, Some(Manager::User)),
            Some(SourceId::Processes)
        );
    }

    /// One value of every `ActionKind` variant. `kind_name` has one arm per variant, so the
    /// distinct-name check in each table test fails when a variant is missing here.
    fn every_kind() -> Vec<ActionKind> {
        use tl_model::actions::{DumpMode, LaunchWhat, RecoveryEdit};
        vec![
            ActionKind::Signal(Signal::Term),
            ActionKind::KillTree,
            ActionKind::Suspend,
            ActionKind::Resume,
            ActionKind::SetNice(5),
            ActionKind::SetSched {
                policy: tl_model::actions::SchedPolicy::Other,
                rt_prio: 0,
            },
            ActionKind::SetAffinity(tl_model::system::CpuSet::empty()),
            ActionKind::SetIoPriority(tl_model::actions::IoPriority::Idle),
            ActionKind::FreezeGroup,
            ActionKind::ThawGroup,
            ActionKind::KillGroup,
            ActionKind::Dump {
                mode: DumpMode::Mini,
                name: Arc::from("x.core"),
            },
            ActionKind::UnitStart,
            ActionKind::UnitStop,
            ActionKind::UnitRestart,
            ActionKind::UnitReload,
            ActionKind::UnitResetFailed,
            ActionKind::UnitEnable,
            ActionKind::UnitDisable,
            ActionKind::UnitMask,
            ActionKind::UnitUnmask,
            ActionKind::UnitWriteRecovery(RecoveryEdit::default()),
            ActionKind::UnitRemoveRecovery,
            ActionKind::StartupEnable,
            ActionKind::StartupDisable,
            ActionKind::StartupRunNow,
            ActionKind::StartupDelete,
            ActionKind::SockDestroy,
            ActionKind::OpenUri(Arc::from("https://example.org")),
            ActionKind::ShowInFileManager {
                path: Arc::from("/home"),
                properties: false,
            },
            ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command(Arc::from("true")),
                as_admin: false,
                cwd: None,
            }),
            ActionKind::ActivateWindow {
                internal_id: Arc::from("{0a1b2c3d-0000-4000-8000-00000000000a}"),
            },
        ]
    }

    #[test]
    fn every_kind_is_complete() {
        let kinds = every_kind();
        let names: FxHashSet<&str> = kinds.iter().map(kind_name).collect();
        assert_eq!(names.len(), kinds.len(), "a kind is listed twice");
        assert_eq!(
            kinds.len(),
            32,
            "a new ActionKind needs a row in every_kind"
        );
    }

    /// One target of each type the deadline and dispatch tables tell apart.
    fn every_target() -> Vec<ActionTarget> {
        use tl_model::actions::{Family, FourTuple, Proto};
        use tl_model::table::{StartupRef, StartupSource};
        let unit = |manager| {
            ActionTarget::Unit(UnitRef {
                manager,
                name: Arc::from("foo.service"),
            })
        };
        let startup = |source, id: &str| {
            ActionTarget::Startup(StartupRef {
                source,
                id: Arc::from(id),
            })
        };
        let addr: std::net::SocketAddr = "127.0.0.1:1".parse().expect("address");
        vec![
            ActionTarget::Process(key(500)),
            ActionTarget::Thread {
                process: key(500),
                tid: 501,
            },
            ActionTarget::Cgroup(CgroupRef {
                rel_path: Arc::from("user.slice/app.slice/app-foo.scope"),
                cgroup_id: 7,
            }),
            unit(Manager::User),
            unit(Manager::System),
            startup(StartupSource::AutostartUser, "foo.desktop"),
            startup(StartupSource::UserUnit, "foo.service"),
            ActionTarget::Socket {
                cookie: 1,
                family: Family::Inet,
                proto: Proto::Tcp,
                four_tuple: FourTuple {
                    local: addr,
                    remote: addr,
                },
            },
        ]
    }

    /// The E11-002 deadline list, written per target type.
    fn adr006(kind: &ActionKind, target: Option<&ActionTarget>) -> Option<Duration> {
        use tl_model::table::StartupSource;
        let s = |n| Some(Duration::from_secs(n));
        let user = matches!(target, Some(ActionTarget::Unit(u)) if u.manager == Manager::User);
        let user_or_system = |u, sys| if user { s(u) } else { s(sys) };
        match kind {
            ActionKind::Signal(_)
            | ActionKind::KillTree
            | ActionKind::Suspend
            | ActionKind::Resume
            | ActionKind::SetNice(_)
            | ActionKind::SetSched { .. }
            | ActionKind::SetAffinity(_)
            | ActionKind::SetIoPriority(_) => s(2),
            ActionKind::FreezeGroup | ActionKind::ThawGroup | ActionKind::KillGroup => match target
            {
                Some(ActionTarget::Cgroup(_)) => s(2),
                _ => user_or_system(2, 120),
            },
            ActionKind::UnitStart
            | ActionKind::UnitStop
            | ActionKind::UnitRestart
            | ActionKind::UnitReload
            | ActionKind::UnitResetFailed => None,
            ActionKind::UnitEnable
            | ActionKind::UnitDisable
            | ActionKind::UnitMask
            | ActionKind::UnitUnmask => user_or_system(10, 120),
            ActionKind::StartupEnable | ActionKind::StartupDisable | ActionKind::StartupDelete => {
                match target {
                    Some(ActionTarget::Startup(e)) if e.source == StartupSource::UserUnit => s(10),
                    _ => s(2),
                }
            }
            ActionKind::StartupRunNow | ActionKind::Dump { .. } => None,
            ActionKind::SockDestroy => s(2),
            ActionKind::UnitWriteRecovery(_) | ActionKind::UnitRemoveRecovery => {
                user_or_system(2, 120)
            }
            ActionKind::OpenUri(_)
            | ActionKind::ShowInFileManager { .. }
            | ActionKind::ActivateWindow { .. } => s(2),
            ActionKind::Launch(spec) if spec.as_admin => s(120),
            ActionKind::Launch(_) => s(2),
        }
    }

    #[test]
    fn deadline_table_matches_adr006() {
        let mut targets: Vec<Option<ActionTarget>> = every_target().into_iter().map(Some).collect();
        targets.push(None);
        for kind in every_kind() {
            for target in &targets {
                let t = target.as_ref();
                assert_eq!(
                    deadline(&kind, t, false),
                    adr006(&kind, t),
                    "{kind:?} on {t:?}"
                );
                assert_eq!(
                    deadline(&kind, t, true),
                    Some(PROMPT_DEADLINE),
                    "privileged {kind:?} on {t:?}"
                );
            }
        }
        let admin = ActionKind::Launch(LaunchSpec {
            what: tl_model::actions::LaunchWhat::Command(Arc::from("true")),
            as_admin: true,
            cwd: None,
        });
        assert_eq!(deadline(&admin, None, false), Some(PROMPT_DEADLINE));
    }

    #[test]
    fn finished_refresh_source_every_kind() {
        for kind in every_kind() {
            for manager in [None, Some(Manager::User), Some(Manager::System)] {
                let systemd = match manager {
                    Some(Manager::User) => SourceId::SystemdUser,
                    _ => SourceId::SystemdSystem,
                };
                let want = match &kind {
                    ActionKind::Signal(_)
                    | ActionKind::KillTree
                    | ActionKind::Suspend
                    | ActionKind::Resume
                    | ActionKind::SetNice(_)
                    | ActionKind::SetSched { .. }
                    | ActionKind::SetAffinity(_)
                    | ActionKind::SetIoPriority(_)
                    | ActionKind::Dump { .. } => Some(SourceId::Processes),
                    // Freeze, thaw and kill change process state, also on a unit target.
                    ActionKind::FreezeGroup | ActionKind::ThawGroup | ActionKind::KillGroup => {
                        Some(SourceId::Processes)
                    }
                    ActionKind::UnitStart
                    | ActionKind::UnitStop
                    | ActionKind::UnitRestart
                    | ActionKind::UnitReload
                    | ActionKind::UnitResetFailed
                    | ActionKind::UnitEnable
                    | ActionKind::UnitDisable
                    | ActionKind::UnitMask
                    | ActionKind::UnitUnmask
                    | ActionKind::UnitWriteRecovery(_)
                    | ActionKind::UnitRemoveRecovery => Some(systemd),
                    ActionKind::StartupEnable
                    | ActionKind::StartupDisable
                    | ActionKind::StartupRunNow
                    | ActionKind::StartupDelete => Some(SourceId::Startup),
                    ActionKind::SockDestroy => Some(SourceId::Connections),
                    ActionKind::OpenUri(_)
                    | ActionKind::ShowInFileManager { .. }
                    | ActionKind::Launch(_)
                    | ActionKind::ActivateWindow { .. } => None,
                };
                assert_eq!(refresh_source(&kind, manager), want, "{kind:?} {manager:?}");
            }
        }
    }

    /// True for the pairs the dispatch table sends to a module.
    fn routed(kind: &ActionKind, target: &ActionTarget) -> bool {
        use ActionKind as K;
        use ActionTarget as T;
        match target {
            T::Process(_) | T::Thread { .. } => {
                matches!(
                    kind,
                    K::Signal(_)
                        | K::Suspend
                        | K::Resume
                        | K::SetNice(_)
                        | K::SetSched { .. }
                        | K::SetAffinity(_)
                        | K::SetIoPriority(_)
                ) || (matches!(target, T::Process(_)) && matches!(kind, K::Dump { .. }))
            }
            T::Cgroup(_) => matches!(kind, K::FreezeGroup | K::ThawGroup | K::KillGroup),
            T::Unit(_) => matches!(
                kind,
                K::FreezeGroup
                    | K::ThawGroup
                    | K::KillGroup
                    | K::UnitStart
                    | K::UnitStop
                    | K::UnitRestart
                    | K::UnitReload
                    | K::UnitResetFailed
                    | K::UnitEnable
                    | K::UnitDisable
                    | K::UnitMask
                    | K::UnitUnmask
                    | K::UnitWriteRecovery(_)
                    | K::UnitRemoveRecovery
            ),
            T::Startup(_) => matches!(
                kind,
                K::StartupEnable | K::StartupDisable | K::StartupRunNow | K::StartupDelete
            ),
            T::Socket { .. } => matches!(kind, K::SockDestroy),
        }
    }

    #[test]
    fn dispatch_rejects_mismatched_pairs() {
        use asupersync::types::Budget;
        use asupersync::{LabConfig, LabRuntime};
        use tl_model::table::{StartupRef, StartupSource};

        let unsupported = ActionState::Rejected(RejectReason::Unsupported);
        let named = [
            (
                ActionKind::Signal(Signal::Term),
                ActionTarget::Unit(unit("foo.service")),
            ),
            (ActionKind::UnitStart, ActionTarget::Process(key(500))),
            (
                ActionKind::StartupEnable,
                ActionTarget::Cgroup(CgroupRef {
                    rel_path: Arc::from("user.slice/app.slice/app-foo.scope"),
                    cgroup_id: 7,
                }),
            ),
            (ActionKind::SockDestroy, ActionTarget::Process(key(500))),
            (
                ActionKind::UnitStop,
                ActionTarget::Startup(StartupRef {
                    source: StartupSource::AutostartUser,
                    id: Arc::from("foo.desktop"),
                }),
            ),
        ];
        let mut pairs: Vec<(ActionKind, ActionTarget)> = named.to_vec();
        for kind in every_kind() {
            for target in every_target() {
                if !routed(&kind, &target) {
                    pairs.push((kind.clone(), target));
                }
            }
        }
        assert!(pairs.len() > 150, "the sweep covers the mismatched pairs");

        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let (actx, _events, _msgs) = ctx();
                for (kind, target) in &pairs {
                    let state = dispatch(&cx, &actx, kind, target, &Progress::none()).await;
                    assert_eq!(state, unsupported, "{kind:?} on {target:?}");
                }
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert!(handle.is_finished(), "the checks ran");
    }
}

#[cfg(test)]
mod run_tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use smallvec::SmallVec;
    use tl_model::actions::{LaunchWhat, Signal};
    use tl_model::table::StartupRef;

    use super::*;
    use crate::config::EngineMode;

    const BOOT: BootId = BootId(1);

    struct NoOps;
    impl ActionOps for NoOps {}

    struct Rig {
        lab: LabRuntime,
        actx: Arc<ActionCtx>,
        events: async_channel::Receiver<ActionEvent>,
        msgs: mpsc::Receiver<EngineMsg>,
    }

    impl Rig {
        fn new() -> Self {
            let (events_tx, events) = async_channel::bounded(256);
            let (msgs_tx, msgs) = mpsc::channel(256);
            let actx = Arc::new(ActionCtx {
                events: events_tx,
                msgs: msgs_tx,
                ops: Arc::new(NoOps),
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
                destroy_support_sent: AtomicBool::new(false),
                unit_bus_override: None,
                startup_paths: crate::actions::startup::StartupPaths::from_env(),
            });
            Self {
                lab: LabRuntime::new(LabConfig::new(7).max_steps(1_000_000)),
                actx,
                events,
                msgs,
            }
        }

        fn start(
            &mut self,
            req: ActionRequest,
            per_target: Vec<Option<RejectReason>>,
            dispatch: Dispatch,
        ) {
            let actx = Arc::clone(&self.actx);
            let root = self.lab.state.create_root_region(Budget::INFINITE);
            let (task, _handle) = self
                .lab
                .state
                .create_task(root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let route = ActionRoute {
                        id: req.id,
                        events: actx.events.clone(),
                        requests: None,
                    };
                    let ended = EndLock::default();
                    run_with(&cx, &actx, &route, &req, &per_target, dispatch, &ended).await;
                })
                .expect("task is created");
            self.lab.scheduler.lock().schedule(task, 0);
            self.lab.run_until_idle();
        }

        fn advance(&mut self, d: Duration) {
            self.lab.advance_time(d.as_nanos() as u64);
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        fn events(&self) -> Vec<(Option<u16>, ActionState)> {
            let mut out = Vec::new();
            while let Ok(ev) = self.events.try_recv() {
                assert_eq!(ev.id, ActionId(1));
                out.push((ev.target, ev.state));
            }
            out
        }

        fn finished(&mut self) -> usize {
            let mut n = 0;
            while let Ok(msg) = self.msgs.try_recv() {
                assert!(matches!(msg, EngineMsg::ActionFinished { id: ActionId(1) }));
                n += 1;
            }
            n
        }

        fn passed(mut self) {
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 100 + u64::from(pid),
            boot_id: BOOT,
            pid,
        }
    }

    fn term(pids: &[u32]) -> ActionRequest {
        ActionRequest {
            id: ActionId(1),
            kind: ActionKind::Signal(Signal::Term),
            targets: pids
                .iter()
                .map(|p| ActionTarget::Process(key(*p)))
                .collect(),
            privileged: false,
        }
    }

    type Boxed<'a> = Pin<Box<dyn Future<Output = ActionState> + Send + 'a>>;

    /// Takes 3 s under the 2 s signal deadline.
    fn slow<'a>(
        cx: &'a Cx,
        _: &'a ActionCtx,
        kind: &'a ActionKind,
        t: &'a ActionTarget,
        _: &'a Progress<'a>,
    ) -> Boxed<'a> {
        Box::pin(async move {
            let d = deadline(kind, Some(t), false);
            match with_deadline(cx, d, time::sleep(cx.now(), Duration::from_secs(3))).await {
                Ok(()) => ActionState::Succeeded,
                Err(_) => ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
            }
        })
    }

    /// PID 2 gets EPERM, every other target succeeds.
    fn eperm_on_2<'a>(
        _: &'a Cx,
        _: &'a ActionCtx,
        _: &'a ActionKind,
        t: &'a ActionTarget,
        _: &'a Progress<'a>,
    ) -> Boxed<'a> {
        Box::pin(async move {
            match target_pid(t) {
                Some(2) => map_ops_error(&OpsError::Os(Errno::PERM.raw_os_error())),
                _ => ActionState::Succeeded,
            }
        })
    }

    fn panics<'a>(
        _: &'a Cx,
        _: &'a ActionCtx,
        _: &'a ActionKind,
        _: &'a ActionTarget,
        _: &'a Progress<'a>,
    ) -> Boxed<'a> {
        Box::pin(async move { panic!("fake op panicked") })
    }

    fn succeeds<'a>(
        _: &'a Cx,
        _: &'a ActionCtx,
        _: &'a ActionKind,
        _: &'a ActionTarget,
        _: &'a Progress<'a>,
    ) -> Boxed<'a> {
        Box::pin(async move { ActionState::Succeeded })
    }

    fn failed(e: ActionError) -> ActionState {
        ActionState::Failed(e)
    }

    const SAVED: &str = "/s/tasklens/dumps/x_1.core";

    /// Reports 100 and then 250 bytes, then ends as a dump does.
    fn reports_progress<'a>(
        _: &'a Cx,
        _: &'a ActionCtx,
        _: &'a ActionKind,
        _: &'a ActionTarget,
        progress: &'a Progress<'a>,
    ) -> Boxed<'a> {
        Box::pin(async move {
            progress.report(100);
            progress.report(250);
            ActionState::Saved(Arc::from(SAVED))
        })
    }

    /// ADR-005 as ADR-018 changes it: `Started`, the target's progress in order, its final state,
    /// then the summary. A one-target dump's summary keeps the path, and the action ends `Done`.
    #[test]
    fn progress_events_in_order() {
        let mut rig = Rig::new();
        let req = ActionRequest {
            id: ActionId(1),
            kind: ActionKind::Dump {
                mode: tl_model::actions::DumpMode::Mini,
                name: Arc::from("x_1.core"),
            },
            targets: smallvec::smallvec![ActionTarget::Process(key(1))],
            privileged: false,
        };
        rig.start(req, vec![None], reports_progress);
        let saved = ActionState::Saved(Arc::from(SAVED));
        assert_eq!(
            rig.events(),
            vec![
                (None, ActionState::Started),
                (Some(0), ActionState::Progress { bytes: 100 }),
                (Some(0), ActionState::Progress { bytes: 250 }),
                (Some(0), saved.clone()),
                (None, saved.clone()),
            ]
        );
        assert_eq!(action_end(&saved), RequestEnd::Done);
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    /// ADR-018, q4x76.17: the action's end is its last event. An event queue of one holds each
    /// event back until the client reads the one before, and the end waits for the summary.
    #[test]
    fn end_follows_the_summary() {
        let mut rig = Rig::new();
        let (events_tx, events) = async_channel::bounded(1);
        let (requests_tx, requests) = async_channel::unbounded();
        let actx = Arc::clone(&rig.actx);
        let root = rig.lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = rig
            .lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let req = term(&[1000]);
                let route = ActionRoute {
                    id: req.id,
                    events: events_tx,
                    requests: Some(requests_tx),
                };
                let ended = EndLock::default();
                run_with(&cx, &actx, &route, &req, &[None], succeeds, &ended).await;
            })
            .expect("task is created");
        rig.lab.scheduler.lock().schedule(task, 0);
        rig.lab.run_until_idle();
        // Each event, and whether the end was sent when the client read it.
        let mut seen = Vec::new();
        while let Ok(ev) = events.try_recv() {
            seen.push((ev.target, ev.state, !requests.is_empty()));
            rig.lab.run_until_idle();
        }
        assert_eq!(
            seen,
            [
                (None, ActionState::Started, false),
                (Some(0), ActionState::Succeeded, false),
                (None, ActionState::Succeeded, true),
            ]
        );
        let end = RequestEvent {
            kind: IdKind::Action,
            id: 1,
            end: RequestEnd::Done,
        };
        assert_eq!(requests.try_recv(), Ok(end));
        assert!(requests.is_empty());
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn timeout_is_outcome_unknown() {
        let mut rig = Rig::new();
        rig.start(term(&[1000]), vec![None], slow);
        assert_eq!(rig.events(), vec![(None, ActionState::Started)]);
        rig.advance(Duration::from_secs(2));
        let unknown = ActionState::OutcomeUnknown(Arc::from("timed out"));
        assert_eq!(
            rig.events(),
            vec![(Some(0), unknown.clone()), (None, unknown)]
        );
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn partial_failure_summary() {
        let mut rig = Rig::new();
        rig.start(term(&[1, 2, 3]), vec![None, None, None], eperm_on_2);
        assert_eq!(
            rig.events(),
            vec![
                (None, ActionState::Started),
                (Some(0), ActionState::Succeeded),
                (Some(1), failed(ActionError::PermissionDenied)),
                (Some(2), ActionState::Succeeded),
                (None, failed(ActionError::Partial { done: 2, total: 3 })),
            ]
        );
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn rejected_targets_skip_dispatch() {
        let mut rig = Rig::new();
        rig.start(
            term(&[1, 2]),
            vec![Some(RejectReason::IdentityChanged), None],
            succeeds,
        );
        let rejected = ActionState::Rejected(RejectReason::IdentityChanged);
        assert_eq!(
            rig.events(),
            vec![
                (None, ActionState::Started),
                (Some(0), rejected.clone()),
                (Some(1), ActionState::Succeeded),
                (None, rejected),
            ]
        );
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn panic_is_internal_error() {
        let mut rig = Rig::new();
        rig.start(term(&[1000]), vec![None], panics);
        let internal = failed(ActionError::Internal(Arc::from("internal error")));
        assert_eq!(
            rig.events(),
            vec![
                (None, ActionState::Started),
                (Some(0), internal.clone()),
                (None, internal),
            ]
        );
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn emit_drops_after_1s() {
        let mut rig = Rig::new();
        let fill = event(ActionId(99), None, ActionState::Started);
        for _ in 0..256 {
            rig.actx.events.try_send(fill.clone()).expect("room left");
        }
        let done = Arc::new(AtomicBool::new(false));
        let actx = Arc::clone(&rig.actx);
        let flag = Arc::clone(&done);
        let root = rig.lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = rig
            .lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let out = Emitter {
                    events: &actx.events,
                    ended: &EndLock::default(),
                };
                emit(&out, &cx, event(ActionId(1), None, ActionState::Started)).await;
                flag.store(true, Ordering::SeqCst);
            })
            .expect("task is created");
        rig.lab.scheduler.lock().schedule(task, 0);
        rig.lab.run_until_idle();
        assert!(
            !done.load(Ordering::SeqCst),
            "emit waits while the channel is full"
        );
        rig.advance(EVENT_SEND_TIMEOUT);
        assert!(done.load(Ordering::SeqCst), "emit returned after 1 s");
        assert_eq!(rig.events.len(), 256, "the event was dropped");
        rig.passed();
    }

    #[test]
    fn action_continues_when_events_are_not_drained() {
        let mut rig = Rig::new();
        let fill = event(ActionId(1), None, ActionState::Started);
        for _ in 0..256 {
            rig.actx.events.try_send(fill.clone()).expect("room left");
        }
        rig.start(term(&[1000]), vec![None], succeeds);
        // Started, the target state and the summary each wait 1 s.
        for _ in 0..3 {
            assert_eq!(rig.finished(), 0);
            rig.advance(EVENT_SEND_TIMEOUT);
        }
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn privileged_without_helper_rejected() {
        let mut rig = Rig::new();
        let mut req = term(&[1000]);
        req.privileged = true;
        // A dispatch that would succeed must not be reached: no unprivileged fallback.
        rig.start(req, vec![None], succeeds);
        assert_eq!(
            rig.events(),
            vec![
                (None, ActionState::Started),
                (None, ActionState::Rejected(RejectReason::Unsupported)),
            ]
        );
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    #[test]
    fn targetless_without_openers_rejected() {
        let mut rig = Rig::new();
        let req = ActionRequest {
            id: ActionId(1),
            kind: ActionKind::OpenUri(Arc::from("https://example.org")),
            targets: SmallVec::new(),
            privileged: false,
        };
        rig.start(req, Vec::new(), succeeds);
        assert_eq!(
            rig.events(),
            vec![
                (None, ActionState::Started),
                (None, ActionState::Rejected(RejectReason::Unsupported)),
            ]
        );
        assert_eq!(rig.finished(), 1);
        rig.passed();
    }

    fn unit(manager: Manager) -> ActionTarget {
        ActionTarget::Unit(UnitRef {
            manager,
            name: Arc::from("foo.service"),
        })
    }

    #[test]
    fn deadline_table() {
        let s = Duration::from_secs;
        let process = ActionTarget::Process(key(1000));
        let term = ActionKind::Signal(Signal::Term);
        assert_eq!(deadline(&term, Some(&process), false), Some(s(2)));
        assert_eq!(
            deadline(&ActionKind::KillTree, Some(&process), false),
            Some(s(2))
        );
        let system = unit(Manager::System);
        let user = unit(Manager::User);
        assert_eq!(deadline(&ActionKind::UnitStart, Some(&system), false), None);
        assert_eq!(
            deadline(&ActionKind::UnitEnable, Some(&system), false),
            Some(s(120))
        );
        assert_eq!(
            deadline(&ActionKind::UnitEnable, Some(&user), false),
            Some(s(10))
        );
        assert_eq!(
            deadline(&ActionKind::FreezeGroup, Some(&user), false),
            Some(s(2))
        );
        assert_eq!(
            deadline(&ActionKind::FreezeGroup, Some(&system), false),
            Some(s(120))
        );
        let user_unit_entry = ActionTarget::Startup(StartupRef {
            source: StartupSource::UserUnit,
            id: Arc::from("foo.service"),
        });
        let autostart = ActionTarget::Startup(StartupRef {
            source: StartupSource::AutostartUser,
            id: Arc::from("foo.desktop"),
        });
        assert_eq!(
            deadline(&ActionKind::StartupDisable, Some(&user_unit_entry), false),
            Some(s(10))
        );
        assert_eq!(
            deadline(&ActionKind::StartupDisable, Some(&autostart), false),
            Some(s(2))
        );
        assert_eq!(
            deadline(&ActionKind::StartupRunNow, Some(&autostart), false),
            None
        );
        let launch = |as_admin| {
            ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command(Arc::from("true")),
                as_admin,
                cwd: None,
            })
        };
        assert_eq!(deadline(&launch(true), None, false), Some(s(120)));
        assert_eq!(deadline(&launch(false), None, false), Some(s(2)));
        for kind in [
            term,
            ActionKind::UnitStart,
            ActionKind::StartupRunNow,
            launch(false),
        ] {
            assert_eq!(
                deadline(&kind, Some(&process), true),
                Some(s(120)),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn summarize_order() {
        let unknown = ActionState::OutcomeUnknown(Arc::from("x"));
        assert_eq!(
            summarize(&[
                ActionState::Rejected(RejectReason::IdentityChanged),
                unknown.clone()
            ]),
            unknown
        );
        assert!(matches!(
            summarize(&[
                ActionState::Rejected(RejectReason::Protected),
                failed(ActionError::NoSuchProcess),
            ]),
            ActionState::Failed(_)
        ));
        assert_eq!(
            summarize(&[ActionState::Succeeded, ActionState::Succeeded]),
            ActionState::Succeeded
        );
        assert_eq!(
            summarize(&[
                ActionState::Succeeded,
                failed(ActionError::PermissionDenied)
            ]),
            failed(ActionError::Partial { done: 1, total: 2 })
        );
        // One target keeps its own failure.
        assert_eq!(
            summarize(&[failed(ActionError::PermissionDenied)]),
            failed(ActionError::PermissionDenied)
        );
    }

    #[test]
    fn map_ops_error_table() {
        let os = |e: Errno| OpsError::Os(e.raw_os_error());
        let cases = [
            (
                OpsError::IdentityChanged,
                ActionState::Rejected(RejectReason::IdentityChanged),
            ),
            (
                OpsError::NotOwner,
                ActionState::Rejected(RejectReason::NotOwner),
            ),
            (
                OpsError::Unsupported("x"),
                ActionState::Rejected(RejectReason::Unsupported),
            ),
            (
                OpsError::PermissionDenied,
                failed(ActionError::PermissionDenied),
            ),
            (os(Errno::PERM), failed(ActionError::PermissionDenied)),
            (os(Errno::ACCESS), failed(ActionError::PermissionDenied)),
            (os(Errno::SRCH), failed(ActionError::NoSuchProcess)),
            (
                OpsError::Partial { done: 1, total: 3 },
                failed(ActionError::Partial { done: 1, total: 3 }),
            ),
            (
                OpsError::InvalidInput("bad"),
                failed(ActionError::Internal(Arc::from("bad"))),
            ),
            (
                os(Errno::INVAL),
                failed(ActionError::Os(Errno::INVAL.raw_os_error())),
            ),
        ];
        for (e, want) in cases {
            assert_eq!(map_ops_error(&e), want, "{e:?}");
        }
    }
}

/// `RealOps` forwards to tl-ops instead of the refusing defaults. No real process or cgroup is
/// touched: the process calls fail their checks before any syscall that changes state, and the
/// cgroup writes go to plain files in a temp directory.
#[cfg(test)]
mod real_ops_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tl_model::actions::CgroupRef;
    use tl_ops::cgroup::CgroupDir;

    use super::*;

    const NOT_WIRED: &str = "cgroup ops not wired";

    /// A directory with the three cgroup control files, removed on drop.
    struct FakeCgroup(PathBuf);

    impl FakeCgroup {
        fn new() -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/tmp")
                .join(format!(
                    "tl-engine-realops-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            for name in ["cgroup.freeze", "cgroup.kill"] {
                fs::write(dir.join(name), b"").expect("write control file");
            }
            fs::write(dir.join("cgroup.events"), b"populated 1\nfrozen 1\n").expect("write events");
            Self(dir)
        }

        fn dir(&self) -> CgroupDir {
            let fd = rustix::fs::open(
                &self.0,
                rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .expect("open temp dir");
            CgroupDir::from_validated_fd(fd)
        }

        fn read(&self, name: &str) -> String {
            fs::read_to_string(self.0.join(name)).expect("read control file")
        }
    }

    impl Drop for FakeCgroup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn real_ops_process_forwards() {
        let pid = std::process::id();
        let key = ProcessKey {
            start_ticks: 1,
            boot_id: BootId(1),
            pid,
        };
        // The boot check fails before any pidfd is opened.
        assert_eq!(
            RealOps.open_verified(key, BootId(2)).err(),
            Some(OpsError::IdentityChanged)
        );
        // The range check fails before the syscall, so /dev/null is never signalled.
        let fd: OwnedFd = fs::File::open("/dev/null").expect("open /dev/null").into();
        assert_eq!(
            RealOps.send_signal(&fd, Signal::Other(0)),
            Err(OpsError::InvalidInput("signal number out of range"))
        );
    }

    #[test]
    fn real_ops_cgroup_forwards() {
        // A path outside app.slice fails validation before any file is opened.
        let cg = CgroupRef {
            rel_path: Arc::from("system.slice/x.service"),
            cgroup_id: 1,
        };
        let open = RealOps.cgroup_open_validated(&cg, 1000).err();
        assert!(open.is_some());
        assert_ne!(open, Some(OpsError::Unsupported(NOT_WIRED)));

        let fake = FakeCgroup::new();
        let dir = fake.dir();
        assert_eq!(RealOps.cgroup_write(&dir, "cgroup.freeze", "1"), Ok(()));
        assert_eq!(fake.read("cgroup.freeze"), "1");
        assert_eq!(RealOps.cgroup_write(&dir, "cgroup.freeze", "0"), Ok(()));
        assert_eq!(fake.read("cgroup.freeze"), "0");
        assert_eq!(RealOps.cgroup_write(&dir, "cgroup.kill", "1"), Ok(()));
        assert_eq!(fake.read("cgroup.kill"), "1");
        assert_eq!(
            RealOps.cgroup_write(&dir, "cgroup.procs", "1"),
            Err(OpsError::InvalidInput("unknown cgroup control write"))
        );
        assert_eq!(
            RealOps.cgroup_wait_frozen(&dir, Duration::from_millis(100)),
            Ok(())
        );
    }
}
