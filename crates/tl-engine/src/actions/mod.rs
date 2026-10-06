//! Action admission and the action task (ADR-005, ADR-006, E11). The reducer calls `admit` on
//! `Command::Action` before any action task starts. Each admitted request then runs as one
//! `run_action` task.
//!
//! Module lines for `process`, `cgroup`, `unit`, `startup`, `dump`, `socket` and `recovery` are
//! added by their beads.

pub mod cgroup;
pub mod process;

use std::future::Future;
use std::os::fd::OwnedFd;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc;
use asupersync::time::{self, Elapsed};
use futures_lite::FutureExt;
use rustc_hash::FxHashSet;
use rustix::io::Errno;
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    LaunchSpec, MAX_ACTION_TARGETS, MAX_ACTIONS_IN_FLIGHT, RejectReason, Signal,
};
use tl_model::health::HelperState;
use tl_model::helper_proto::{HelperReply, HelperRequest};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcFlags;
use tl_model::table::{Manager, StartupSource, UnitRef};
use tl_model::time::BootId;
use tl_ops::OpsError;

use crate::config::EngineConfig;
use crate::dbus::ManagerConnSlot;
use crate::reducer::AdmissionView;
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

/// True for `OpenUri`, `ShowInFileManager` and `Launch`: they carry no targets (R-134).
pub fn targetless(kind: &ActionKind) -> bool {
    kind.is_targetless()
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
        ActionKind::OpenUri(_) | ActionKind::ShowInFileManager { .. } => Some(QUICK_DEADLINE),
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
        let done = states
            .iter()
            .filter(|s| matches!(s, ActionState::Succeeded))
            .count();
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

/// The privileged helper link, filled by E29-014 (E29-OI-6). `ActionCtx::helper` is `None`
/// before P3 and when the helper is not installed.
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

/// What a launch did (E14-A3). E11-016 switches this to a re-export of E14-A3's enum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LaunchOutcome {
    Launched,
    AlreadyRunning,
}

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
    /// `None` until E11-016 installs `RealOpeners` (needs E14-A3).
    pub openers: Option<Arc<dyn Openers>>,
    /// `None` until E29-014.
    pub helper: Option<Arc<dyn PrivilegedOps>>,
}

/// The per-target dispatch `run_action` uses. Tests pass their own.
type Dispatch = for<'a> fn(
    &'a Cx,
    &'a ActionCtx,
    &'a ActionKind,
    &'a ActionTarget,
) -> Pin<Box<dyn Future<Output = ActionState> + Send + 'a>>;

/// One admitted request: `Started`, one final state per target, one summary with
/// `target: None`, then `ActionFinished` to the reducer. A panic reports
/// `Failed(Internal("internal error"))` and `ActionFinished` is still sent (ADR-015).
pub async fn run_action(
    cx: Cx,
    actx: Arc<ActionCtx>,
    req: ActionRequest,
    per_target: Vec<Option<RejectReason>>,
) {
    run_with(&cx, &actx, &req, &per_target, dispatch_boxed).await;
}

async fn run_with(
    cx: &Cx,
    actx: &ActionCtx,
    req: &ActionRequest,
    per_target: &[Option<RejectReason>],
    dispatch: Dispatch,
) {
    emit(actx, cx, event(req.id, None, ActionState::Started)).await;
    let body = AssertUnwindSafe(run_body(cx, actx, req, per_target, dispatch))
        .catch_unwind()
        .await;
    if body.is_err() {
        emit(actx, cx, event(req.id, None, internal_error())).await;
    }
    let finished = EngineMsg::ActionFinished { id: req.id };
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

/// Emits every final state and the summary.
async fn run_body(
    cx: &Cx,
    actx: &ActionCtx,
    req: &ActionRequest,
    per_target: &[Option<RejectReason>],
    dispatch: Dispatch,
) {
    let summary = if req.privileged {
        // E29-014 chain-edits the routing arm here. Until then a privileged request is refused:
        // it never falls back to an unprivileged call.
        ActionState::Rejected(RejectReason::Unsupported)
    } else if req.kind.is_targetless() {
        // `targetless::run` (E11-016) replaces this arm.
        ActionState::Rejected(RejectReason::Unsupported)
    } else {
        // `KillTree` calls `process::kill_tree` once here (E11-004).
        let mut states = Vec::with_capacity(req.targets.len());
        for (i, target) in req.targets.iter().enumerate() {
            let state = match per_target.get(i).copied().flatten() {
                Some(reason) => ActionState::Rejected(reason),
                None => run_target(cx, actx, &req.kind, target, dispatch).await,
            };
            tracing::info!(
                target: "tasklens::action",
                kind = kind_name(&req.kind),
                pid = target_pid(target),
                result = ?state,
            );
            emit(actx, cx, event(req.id, Some(i), state.clone())).await;
            states.push(state);
        }
        summarize(&states)
    };
    emit(actx, cx, event(req.id, None, summary)).await;
}

/// A panic in one target gives that target `Failed(Internal)`; the other targets still run.
async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
    dispatch: Dispatch,
) -> ActionState {
    AssertUnwindSafe(dispatch(cx, actx, kind, target))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| internal_error())
}

fn dispatch_boxed<'a>(
    cx: &'a Cx,
    actx: &'a ActionCtx,
    kind: &'a ActionKind,
    target: &'a ActionTarget,
) -> Pin<Box<dyn Future<Output = ActionState> + Send + 'a>> {
    Box::pin(dispatch(cx, actx, kind, target))
}

/// The `(kind, target)` table. `Process | Thread` → `process::run_target`, `Cgroup` →
/// `cgroup::run_target`, `Unit` → `unit::run_target`, `Startup` → `startup::run_target`,
/// `Socket` → `socket::run_target`. Each module bead adds its arm; a pair with no module is
/// `Rejected(Unsupported)`.
async fn dispatch(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    target: &ActionTarget,
) -> ActionState {
    match target {
        ActionTarget::Cgroup(cg) => cgroup::run_target(cx, actx, kind, cg).await,
        ActionTarget::Process(_) | ActionTarget::Thread { .. } => {
            process::run_target(cx, actx, kind, target).await
        }
        ActionTarget::Unit(_) | ActionTarget::Startup(_) | ActionTarget::Socket { .. } => {
            ActionState::Rejected(RejectReason::Unsupported)
        }
    }
}

/// Sends one event with a 1 s timeout. When the UI is not draining, the event is dropped and the
/// action continues.
async fn emit(actx: &ActionCtx, cx: &Cx, ev: ActionEvent) {
    let id = ev.id.0;
    match time::timeout(cx.now(), EVENT_SEND_TIMEOUT, actx.events.send(ev)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
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
                    run_with(&cx, &actx, &req, &per_target, dispatch).await;
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
    ) -> Boxed<'a> {
        Box::pin(async move { panic!("fake op panicked") })
    }

    fn succeeds<'a>(
        _: &'a Cx,
        _: &'a ActionCtx,
        _: &'a ActionKind,
        _: &'a ActionTarget,
    ) -> Boxed<'a> {
        Box::pin(async move { ActionState::Succeeded })
    }

    fn failed(e: ActionError) -> ActionState {
        ActionState::Failed(e)
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
                emit(&actx, &cx, event(ActionId(1), None, ActionState::Started)).await;
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
