//! "Retry as administrator": a request with `privileged = true` runs through the helper instead
//! of tl-ops (PROC-058, PROC-076, ADR-005, ADR-006, E29-014). There is no automatic retry and no
//! automatic escalation: only `run_body` calls this, and only for a privileged request.
//!
//! Also the `PrivilegedOps` impl for `HelperHandle`, which `actions/{socket,recovery,dump}.rs`
//! call through `ActionCtx.helper` (R-147).
//!
//! Privacy: the log records kind, target pid and result only (ADR-015).

use std::future::Future;
use std::os::fd::OwnedFd;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use futures_lite::FutureExt;
use rustix::io::Errno;
use tl_model::actions::{
    ActionError, ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, CgroupRef,
    DumpMode, RejectReason, Signal,
};
use tl_model::health::HelperState;
use tl_model::helper_proto::{
    HelperCgroup, HelperError, HelperReply, HelperReplyBody, HelperRequest, HelperTarget,
};
use tl_model::ids::ProcessKey;

use super::{
    ActionCtx, Emitter, HelperCallError, PROMPT_DEADLINE, PrivilegedOps, QUICK_DEADLINE, dump,
    emit, event, internal_error, kind_name, summarize, target_pid, with_deadline,
};
use crate::helper::HelperHandle;

/// The `OutcomeUnknown` text when the helper does not answer in time.
const NO_ANSWER: &str = "no answer from the administrator helper";
/// The `Internal` text when the helper process ended or broke the protocol.
const HELPER_STOPPED: &str = "administrator helper stopped";
/// The `Internal` text when the helper refused the request.
const HELPER_REFUSED: &str = "refused by the administrator helper";
/// The `Internal` text for a reply that is not `Ok` or `Err`.
const UNEXPECTED_REPLY: &str = "unexpected reply from the administrator helper";
/// The `OutcomeUnknown` text for ESRCH after the change.
const EXITED_DURING: &str = "process exited during the change";
/// The `OutcomeUnknown` text for EINPROGRESS from a freeze.
const FREEZE_IN_PROGRESS: &str = "freeze still in progress";

impl PrivilegedOps for HelperHandle {
    fn call(
        &self,
        cx: &Cx,
        req: HelperRequest,
        fd: Option<OwnedFd>,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<HelperReply>, HelperCallError>> + Send + '_>> {
        let cx = cx.clone();
        Box::pin(async move { HelperHandle::call(self, &cx, req, fd, deadline).await })
    }

    fn state(&self) -> HelperState {
        HelperHandle::state(self)
    }
}

/// What one target sends. `Dump` goes through `dump::run_target`, which opens the file and
/// passes its descriptor to the helper (R-145).
enum Route {
    Call(HelperRequest),
    Dump(DumpArgs),
}

/// The arguments `dump::run_target` needs besides the context.
struct DumpArgs {
    mode: DumpMode,
    path: Arc<str>,
    key: ProcessKey,
}

/// Runs an admitted privileged request: one final state per target in ADR-005 order, and
/// returns the summary for `run_body` to emit. `None` when the task was cancelled; nothing more
/// is emitted then.
///
/// A kind or target type the helper does not take refuses the whole request with one
/// `Rejected(Unsupported)` and calls nothing. So does a helper that is not installed.
pub(super) async fn run_privileged(
    cx: &Cx,
    actx: &ActionCtx,
    events: &Emitter<'_>,
    req: &ActionRequest,
    per_target: &[Option<RejectReason>],
    helper: &dyn PrivilegedOps,
) -> Option<ActionState> {
    if helper.state() == HelperState::Missing {
        return Some(ActionState::Rejected(RejectReason::Unsupported));
    }
    let routes = match plan(&req.kind, &req.targets) {
        Some(routes) => routes,
        None => return Some(ActionState::Rejected(RejectReason::Unsupported)),
    };
    let admitted = |i: usize| per_target.get(i).copied().flatten().is_none();
    let mut run = Run {
        helper,
        fatal: None,
    };
    let mut states: Vec<ActionState> = per_target_states(per_target, req.targets.len());

    if matches!(req.kind, ActionKind::KillTree) {
        // ADR-006: SIGSTOP every target, then SIGKILL every target that stopped.
        for (i, target) in req.targets.iter().enumerate() {
            if !admitted(i) {
                continue;
            }
            if cx.is_cancel_requested() {
                return None;
            }
            states[i] = run.signal(cx, target, Signal::Stop).await;
        }
        for (i, target) in req.targets.iter().enumerate() {
            if !admitted(i) || states[i] != ActionState::Succeeded {
                continue;
            }
            if cx.is_cancel_requested() {
                return None;
            }
            states[i] = run.signal(cx, target, Signal::Kill).await;
        }
        for (i, target) in req.targets.iter().enumerate() {
            log(&req.kind, target, &states[i]);
            emit(events, cx, event(req.id, Some(i), states[i].clone())).await;
        }
        return Some(summarize(&states));
    }

    for (i, (target, route)) in req.targets.iter().zip(routes).enumerate() {
        if cx.is_cancel_requested() {
            return None;
        }
        if admitted(i) {
            states[i] = match route {
                Route::Call(call) => run.call(cx, call).await,
                Route::Dump(d) => {
                    AssertUnwindSafe(dump::run_target(cx, actx, d.mode, &d.path, d.key, true))
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| internal_error())
                }
            };
        }
        log(&req.kind, target, &states[i]);
        emit(events, cx, event(req.id, Some(i), states[i].clone())).await;
    }
    Some(summarize(&states))
}

/// `Rejected(reason)` for a target admission refused, `Succeeded` as a placeholder otherwise.
fn per_target_states(per_target: &[Option<RejectReason>], len: usize) -> Vec<ActionState> {
    (0..len)
        .map(|i| match per_target.get(i).copied().flatten() {
            Some(reason) => ActionState::Rejected(reason),
            None => ActionState::Succeeded,
        })
        .collect()
}

/// The helper call for each target, or `None` when any `(kind, target)` pair has no helper
/// request. `KillTree` gets placeholder routes: it sends its signals itself.
fn plan(kind: &ActionKind, targets: &[ActionTarget]) -> Option<Vec<Route>> {
    targets.iter().map(|t| route(kind, t)).collect()
}

fn route(kind: &ActionKind, target: &ActionTarget) -> Option<Route> {
    use ActionKind as K;
    use ActionTarget as T;
    let signal = |signal: Signal| {
        helper_target(target).map(|target| Route::Call(HelperRequest::Signal { target, signal }))
    };
    let call = Route::Call;
    match (kind, target) {
        (K::Signal(s), T::Process(_) | T::Thread { .. }) => signal(*s),
        (K::KillTree, T::Process(_)) => signal(Signal::Kill),
        (K::Suspend, T::Process(_)) => signal(Signal::Stop),
        (K::Resume, T::Process(_)) => signal(Signal::Cont),
        (K::SetNice(nice), T::Process(_) | T::Thread { .. }) => {
            Some(call(HelperRequest::SetNice {
                target: helper_target(target)?,
                nice: *nice,
            }))
        }
        (K::SetSched { policy, rt_prio }, T::Process(_) | T::Thread { .. }) => {
            Some(call(HelperRequest::SetSched {
                target: helper_target(target)?,
                policy: *policy,
                rt_prio: *rt_prio,
            }))
        }
        (K::SetAffinity(set), T::Process(_) | T::Thread { .. }) => {
            Some(call(HelperRequest::SetAffinity {
                target: helper_target(target)?,
                set: *set,
            }))
        }
        (K::SetIoPriority(prio), T::Process(key)) => Some(call(HelperRequest::SetIoPriority {
            key: *key,
            prio: *prio,
        })),
        (K::FreezeGroup, T::Cgroup(cg)) => Some(call(HelperRequest::FreezeGroup(cgroup(cg)))),
        (K::ThawGroup, T::Cgroup(cg)) => Some(call(HelperRequest::ThawGroup(cgroup(cg)))),
        (K::KillGroup, T::Cgroup(cg)) => Some(call(HelperRequest::KillGroup(cgroup(cg)))),
        (K::Dump { mode, path }, T::Process(key)) => Some(Route::Dump(DumpArgs {
            mode: *mode,
            path: Arc::clone(path),
            key: *key,
        })),
        _ => None,
    }
}

fn helper_target(target: &ActionTarget) -> Option<HelperTarget> {
    match target {
        ActionTarget::Process(key) => Some(HelperTarget::Process(*key)),
        ActionTarget::Thread { process, tid } => Some(HelperTarget::Thread {
            process: *process,
            tid: *tid,
        }),
        _ => None,
    }
}

fn cgroup(cg: &CgroupRef) -> HelperCgroup {
    HelperCgroup {
        rel_path: cg.rel_path.to_string(),
        cgroup_id: cg.cgroup_id,
    }
}

/// The calls of one request. A dismissed or refused prompt, a failed start or a missing helper
/// ends every later target with the same state and sends nothing more (R-148 item 3).
struct Run<'a> {
    helper: &'a dyn PrivilegedOps,
    fatal: Option<ActionState>,
}

impl Run<'_> {
    async fn signal(&mut self, cx: &Cx, target: &ActionTarget, signal: Signal) -> ActionState {
        match helper_target(target) {
            Some(target) => {
                self.call(cx, HelperRequest::Signal { target, signal })
                    .await
            }
            None => ActionState::Rejected(RejectReason::Unsupported),
        }
    }

    /// One call with the 2 s deadline (ADR-006). A locked helper first starts its session and
    /// shows the polkit prompt, so that call also gets `PROMPT_DEADLINE`.
    async fn call(&mut self, cx: &Cx, req: HelperRequest) -> ActionState {
        if let Some(state) = &self.fatal {
            return state.clone();
        }
        let deadline = match self.helper.state() {
            HelperState::Locked => QUICK_DEADLINE + PROMPT_DEADLINE,
            _ => QUICK_DEADLINE,
        };
        let call = self.helper.call(cx, req, None, QUICK_DEADLINE);
        let result = AssertUnwindSafe(with_deadline(cx, Some(deadline), call))
            .catch_unwind()
            .await;
        match result {
            Err(_) => internal_error(),
            Ok(Err(_)) => ActionState::OutcomeUnknown(Arc::from(NO_ANSWER)),
            Ok(Ok(Ok(replies))) => match replies.last().map(|r| &r.body) {
                Some(HelperReplyBody::Ok) => ActionState::Succeeded,
                Some(HelperReplyBody::Err(e)) => map_helper_error(*e),
                _ => internal_text(UNEXPECTED_REPLY),
            },
            Ok(Ok(Err(e))) => {
                let state = map_call_error(&e);
                if matches!(
                    e,
                    HelperCallError::Missing
                        | HelperCallError::Cancelled
                        | HelperCallError::NotAuthorized
                        | HelperCallError::StartFailed(_)
                ) {
                    self.fatal = Some(state.clone());
                }
                state
            }
        }
    }
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
        HelperError::Os(n) if n == Errno::SRCH.raw_os_error() => {
            ActionState::OutcomeUnknown(Arc::from(EXITED_DURING))
        }
        HelperError::Os(n) if n == Errno::INPROGRESS.raw_os_error() => {
            ActionState::OutcomeUnknown(Arc::from(FREEZE_IN_PROGRESS))
        }
        HelperError::Os(n) => ActionState::Failed(ActionError::Os(n)),
        HelperError::NotAllowed | HelperError::TooLarge => internal_text(HELPER_REFUSED),
    }
}

/// The E29-014 mapping of a failed helper call (R-148 item 3).
fn map_call_error(e: &HelperCallError) -> ActionState {
    match e {
        HelperCallError::Missing => ActionState::Rejected(RejectReason::Unsupported),
        HelperCallError::Cancelled => ActionState::Failed(ActionError::AuthCancelled),
        HelperCallError::NotAuthorized => ActionState::Failed(ActionError::NotAuthorized),
        HelperCallError::StartFailed(reason) => {
            ActionState::Failed(ActionError::Internal(Arc::clone(reason)))
        }
        HelperCallError::Timeout => ActionState::OutcomeUnknown(Arc::from(NO_ANSWER)),
        HelperCallError::Exited | HelperCallError::Protocol(_) => internal_text(HELPER_STOPPED),
    }
}

fn internal_text(text: &str) -> ActionState {
    ActionState::Failed(ActionError::Internal(Arc::from(text)))
}

fn log(kind: &ActionKind, target: &ActionTarget, state: &ActionState) {
    tracing::info!(
        target: "tasklens::action",
        kind = kind_name(kind),
        pid = target_pid(target),
        privileged = true,
        result = ?state,
    );
}
