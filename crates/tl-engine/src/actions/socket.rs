//! Close TCP connection: `SockDestroy` through the privileged helper (SYS-031, ADR-006, E11-021).
//!
//! The engine never opens a netlink socket for destroy itself: that needs `CAP_NET_ADMIN`. The
//! helper sends `SOCK_DESTROY` matched by the exact 4-tuple and cookie, so a reused 4-tuple is
//! never hit. The first answer tells the reducer whether the kernel has
//! `CONFIG_INET_DIAG_DESTROY` (`SourceUpdate::ConnectionsDestroySupport`).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use asupersync::Cx;
use asupersync::time;
use rustix::io::Errno;
use tl_model::actions::{
    ActionError, ActionKind, ActionState, Family, FourTuple, Proto, RejectReason,
};
use tl_model::health::HelperState;
use tl_model::helper_proto::{HelperError, HelperReply, HelperReplyBody, HelperRequest};

use super::{ActionCtx, EVENT_SEND_TIMEOUT, HelperCallError, PROMPT_DEADLINE, with_deadline};
use crate::update::{EngineMsg, SourceUpdate};

/// Deadline for one destroy call when the helper session is already open.
pub const DESTROY_DEADLINE: Duration = Duration::from_secs(2);
/// The `OutcomeUnknown` text when the helper does not answer in time.
const NO_ANSWER: &str = "no answer from the administrator helper";
/// The `Internal` text when the helper process ended or broke the protocol.
const HELPER_STOPPED: &str = "administrator helper stopped";
/// The `Internal` text for a reply that is not `Ok` or `Err`.
const UNEXPECTED_REPLY: &str = "unexpected reply from the administrator helper";

/// Closes one TCP connection through the helper. Any kind other than `SockDestroy`, a UDP
/// socket, or no installed helper is `Rejected(Unsupported)` and nothing is called. A locked
/// helper still gets the call: its start shows the polkit prompt, so the deadline is
/// `PROMPT_DEADLINE`.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    cookie: u64,
    family: Family,
    proto: Proto,
    four_tuple: FourTuple,
) -> ActionState {
    if !matches!(kind, ActionKind::SockDestroy) || proto != Proto::Tcp {
        return ActionState::Rejected(RejectReason::Unsupported);
    }
    let Some(helper) = actx.helper.as_ref() else {
        return ActionState::Rejected(RejectReason::Unsupported);
    };
    let deadline = match helper.state() {
        HelperState::Missing => return ActionState::Rejected(RejectReason::Unsupported),
        HelperState::Locked => PROMPT_DEADLINE,
        HelperState::Starting | HelperState::Unlocked => DESTROY_DEADLINE,
    };
    let req = HelperRequest::SockDestroy {
        family,
        proto,
        four_tuple,
        cookie,
    };
    let body = match with_deadline(cx, Some(deadline), helper.call(cx, req, None, deadline)).await {
        Ok(Ok(replies)) => final_body(&replies),
        Ok(Err(e)) => Err(map_call_error(&e)),
        Err(_) => Err(ActionState::OutcomeUnknown(Arc::from(NO_ANSWER))),
    };
    let state = match body {
        Ok(HelperReplyBody::Ok) => ActionState::Succeeded,
        Ok(HelperReplyBody::Err(HelperError::Os(n))) if n == Errno::OPNOTSUPP.raw_os_error() => {
            // The kernel lacks `CONFIG_INET_DIAG_DESTROY`.
            send_support(cx, actx, false).await;
            ActionState::Rejected(RejectReason::Unsupported)
        }
        Ok(HelperReplyBody::Err(e)) => map_helper_error(e),
        Ok(_) => internal_error_text(UNEXPECTED_REPLY),
        Err(state) => state,
    };
    if state == ActionState::Succeeded {
        // Only the first success per session is reported. A dropped message clears the flag so
        // the next success tries again.
        if !actx.destroy_support_sent.swap(true, Ordering::Relaxed)
            && !send_support(cx, actx, true).await
        {
            actx.destroy_support_sent.store(false, Ordering::Relaxed);
        }
    }
    tracing::debug!(
        target: "tasklens::action",
        local = %four_tuple.local,
        remote = %four_tuple.remote,
        cookie,
        result = ?state,
        "sock destroy",
    );
    state
}

/// The body of the final reply. No reply at all is `Failed(Internal)`.
fn final_body(replies: &[HelperReply]) -> Result<HelperReplyBody, ActionState> {
    replies
        .last()
        .map(|r| r.body.clone())
        .ok_or_else(|| internal_error_text(UNEXPECTED_REPLY))
}

/// `ENOENT` means the connection is already gone.
fn map_helper_error(e: HelperError) -> ActionState {
    match e {
        HelperError::Os(n) if n == Errno::NOENT.raw_os_error() => {
            ActionState::Failed(ActionError::NoSuchProcess)
        }
        HelperError::Os(n) => ActionState::Failed(ActionError::Os(n)),
        HelperError::PermissionDenied | HelperError::NotAllowed => {
            ActionState::Failed(ActionError::PermissionDenied)
        }
        HelperError::NoSuchProcess => ActionState::Failed(ActionError::NoSuchProcess),
        HelperError::IdentityChanged => ActionState::Rejected(RejectReason::IdentityChanged),
        HelperError::Partial { done, total } => {
            ActionState::Failed(ActionError::Partial { done, total })
        }
        HelperError::TooLarge => internal_error_text(UNEXPECTED_REPLY),
    }
}

/// The same `HelperCallError` mapping as the dump path (E11-018).
fn map_call_error(e: &HelperCallError) -> ActionState {
    match e {
        HelperCallError::Missing => ActionState::Rejected(RejectReason::Unsupported),
        HelperCallError::Cancelled => ActionState::Failed(ActionError::AuthCancelled),
        HelperCallError::NotAuthorized => ActionState::Failed(ActionError::NotAuthorized),
        HelperCallError::StartFailed(reason) => {
            ActionState::Failed(ActionError::Internal(Arc::clone(reason)))
        }
        HelperCallError::Timeout => ActionState::OutcomeUnknown(Arc::from(NO_ANSWER)),
        HelperCallError::Exited | HelperCallError::Protocol(_) => {
            internal_error_text(HELPER_STOPPED)
        }
    }
}

fn internal_error_text(text: &str) -> ActionState {
    ActionState::Failed(ActionError::Internal(Arc::from(text)))
}

/// Tells the reducer whether the kernel supports destroy. Returns false when the message was
/// dropped: the reducer is gone or did not take it within 1 s.
async fn send_support(cx: &Cx, actx: &ActionCtx, supported: bool) -> bool {
    let msg = EngineMsg::Source(SourceUpdate::ConnectionsDestroySupport(supported));
    match time::timeout(cx.now(), EVENT_SEND_TIMEOUT, actx.msgs.send(cx, msg)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(target: "tasklens::action", supported, "destroy support message dropped");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::net::SocketAddr;
    use std::os::fd::OwnedFd;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use asupersync::channel::mpsc;
    use asupersync::types::{Budget, RegionId};
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::time::BootId;

    use super::*;
    use crate::actions::{ActionOps, PrivilegedOps};
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;

    const COOKIE: u64 = 0x1234_5678_9abc;

    struct NoOps;

    impl ActionOps for NoOps {}

    /// Scripted helper. `reply: None` never answers.
    struct FakeHelper {
        state: HelperState,
        reply: Option<HelperReplyBody>,
        calls: Mutex<Vec<(HelperRequest, Duration)>>,
    }

    impl FakeHelper {
        fn answering(body: HelperReplyBody) -> Arc<Self> {
            Arc::new(Self {
                state: HelperState::Unlocked,
                reply: Some(body),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<(HelperRequest, Duration)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl PrivilegedOps for FakeHelper {
        fn call(
            &self,
            _cx: &Cx,
            req: HelperRequest,
            fd: Option<OwnedFd>,
            deadline: Duration,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<HelperReply>, HelperCallError>> + Send + '_>>
        {
            assert!(fd.is_none());
            self.calls.lock().unwrap().push((req, deadline));
            match self.reply.clone() {
                Some(body) => Box::pin(async move {
                    Ok(vec![HelperReply {
                        seq: 1,
                        last: true,
                        body,
                    }])
                }),
                None => Box::pin(std::future::pending()),
            }
        }

        fn state(&self) -> HelperState {
            self.state
        }
    }

    fn tuple() -> FourTuple {
        FourTuple {
            local: "192.168.1.10:51234".parse::<SocketAddr>().unwrap(),
            remote: "93.184.216.34:443".parse::<SocketAddr>().unwrap(),
        }
    }

    struct Rig {
        lab: LabRuntime,
        root: RegionId,
        actx: Arc<ActionCtx>,
        msgs: mpsc::Receiver<EngineMsg>,
        out: Arc<Mutex<Vec<ActionState>>>,
    }

    impl Rig {
        fn new(helper: Option<Arc<dyn PrivilegedOps>>) -> Self {
            let (events, _events_rx) = async_channel::bounded(16);
            let (msgs_tx, msgs) = mpsc::channel(16);
            let actx = Arc::new(ActionCtx {
                events,
                msgs: msgs_tx,
                ops: Arc::new(NoOps),
                boot_id: BootId(1),
                uid: 1000,
                config: Arc::new(EngineConfig::new(
                    EngineMode::Full,
                    PathBuf::from("/nonexistent"),
                )),
                systemd_system: ManagerConnSlot::new(),
                systemd_user: ManagerConnSlot::new(),
                openers: None,
                helper,
                destroy_support_sent: AtomicBool::new(false),
            });
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
            let root = lab.state.create_root_region(Budget::INFINITE);
            Self {
                lab,
                root,
                actx,
                msgs,
                out: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Starts one `run_target` and runs until idle.
        fn start(&mut self, kind: ActionKind, proto: Proto) {
            let actx = Arc::clone(&self.actx);
            let out = Arc::clone(&self.out);
            let (task, _handle) = self
                .lab
                .state
                .create_task(self.root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let state =
                        run_target(&cx, &actx, &kind, COOKIE, Family::Inet, proto, tuple()).await;
                    out.lock().unwrap().push(state);
                })
                .expect("task is created");
            self.lab.scheduler.lock().schedule(task, 0);
            self.lab.run_until_idle();
        }

        fn run(&mut self) -> ActionState {
            self.start(ActionKind::SockDestroy, Proto::Tcp);
            self.out.lock().unwrap().pop().expect("run_target finished")
        }

        fn advance(&mut self, d: Duration) {
            self.lab.advance_time(d.as_nanos() as u64);
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        fn support_msgs(&mut self) -> Vec<bool> {
            let mut out = Vec::new();
            while let Ok(msg) = self.msgs.try_recv() {
                match msg {
                    EngineMsg::Source(SourceUpdate::ConnectionsDestroySupport(s)) => out.push(s),
                    other => panic!("unexpected message {other:?}"),
                }
            }
            out
        }

        fn passed(mut self) {
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    fn os_err(n: i32) -> HelperReplyBody {
        HelperReplyBody::Err(HelperError::Os(n))
    }

    #[test]
    fn sock_destroy_ok_reports_support() {
        let helper = FakeHelper::answering(HelperReplyBody::Ok);
        let mut rig = Rig::new(Some(helper.clone()));
        assert_eq!(rig.run(), ActionState::Succeeded);
        let expected = HelperRequest::SockDestroy {
            family: Family::Inet,
            proto: Proto::Tcp,
            four_tuple: tuple(),
            cookie: COOKIE,
        };
        assert_eq!(helper.calls(), vec![(expected, DESTROY_DEADLINE)]);
        assert_eq!(rig.support_msgs(), vec![true]);
        rig.passed();
    }

    #[test]
    fn sock_destroy_support_sent_once() {
        let helper = FakeHelper::answering(HelperReplyBody::Ok);
        let mut rig = Rig::new(Some(helper.clone()));
        assert_eq!(rig.run(), ActionState::Succeeded);
        assert_eq!(rig.run(), ActionState::Succeeded);
        assert_eq!(helper.calls().len(), 2);
        assert_eq!(rig.support_msgs(), vec![true]);
        rig.passed();
    }

    #[test]
    fn sock_destroy_eopnotsupp() {
        let helper = FakeHelper::answering(os_err(95));
        let mut rig = Rig::new(Some(helper));
        assert_eq!(rig.run(), ActionState::Rejected(RejectReason::Unsupported));
        assert_eq!(rig.support_msgs(), vec![false]);
        rig.passed();
    }

    #[test]
    fn sock_destroy_enoent() {
        let helper = FakeHelper::answering(os_err(2));
        let mut rig = Rig::new(Some(helper));
        assert_eq!(rig.run(), ActionState::Failed(ActionError::NoSuchProcess));
        assert!(rig.support_msgs().is_empty());
        rig.passed();
    }

    #[test]
    fn sock_destroy_errors_mapped() {
        for (body, state) in [
            (
                HelperReplyBody::Err(HelperError::PermissionDenied),
                ActionState::Failed(ActionError::PermissionDenied),
            ),
            (os_err(5), ActionState::Failed(ActionError::Os(5))),
        ] {
            let mut rig = Rig::new(Some(FakeHelper::answering(body)));
            assert_eq!(rig.run(), state);
            assert!(rig.support_msgs().is_empty());
            rig.passed();
        }
    }

    #[test]
    fn sock_destroy_udp_unsupported() {
        let helper = FakeHelper::answering(HelperReplyBody::Ok);
        let mut rig = Rig::new(Some(helper.clone()));
        rig.start(ActionKind::SockDestroy, Proto::Udp);
        let state = rig.out.lock().unwrap().pop();
        assert_eq!(
            state,
            Some(ActionState::Rejected(RejectReason::Unsupported))
        );
        assert!(helper.calls().is_empty());
        rig.passed();
    }

    #[test]
    fn sock_destroy_wrong_kind_unsupported() {
        let helper = FakeHelper::answering(HelperReplyBody::Ok);
        let mut rig = Rig::new(Some(helper.clone()));
        rig.start(ActionKind::KillTree, Proto::Tcp);
        let state = rig.out.lock().unwrap().pop();
        assert_eq!(
            state,
            Some(ActionState::Rejected(RejectReason::Unsupported))
        );
        assert!(helper.calls().is_empty());
        rig.passed();
    }

    #[test]
    fn sock_destroy_without_helper() {
        let mut rig = Rig::new(None);
        assert_eq!(rig.run(), ActionState::Rejected(RejectReason::Unsupported));
        rig.passed();

        let helper = Arc::new(FakeHelper {
            state: HelperState::Missing,
            reply: Some(HelperReplyBody::Ok),
            calls: Mutex::new(Vec::new()),
        });
        let mut rig = Rig::new(Some(helper.clone()));
        assert_eq!(rig.run(), ActionState::Rejected(RejectReason::Unsupported));
        assert!(helper.calls().is_empty());
        assert!(rig.support_msgs().is_empty());
        rig.passed();
    }

    /// A locked helper still gets the call, with the prompt deadline.
    #[test]
    fn sock_destroy_locked_uses_prompt_deadline() {
        let helper = Arc::new(FakeHelper {
            state: HelperState::Locked,
            reply: Some(HelperReplyBody::Ok),
            calls: Mutex::new(Vec::new()),
        });
        let mut rig = Rig::new(Some(helper.clone()));
        assert_eq!(rig.run(), ActionState::Succeeded);
        let calls = helper.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, PROMPT_DEADLINE);
        rig.passed();
    }

    #[test]
    fn sock_destroy_timeout() {
        let helper = Arc::new(FakeHelper {
            state: HelperState::Unlocked,
            reply: None,
            calls: Mutex::new(Vec::new()),
        });
        let mut rig = Rig::new(Some(helper));
        rig.start(ActionKind::SockDestroy, Proto::Tcp);
        assert!(rig.out.lock().unwrap().is_empty());
        rig.advance(DESTROY_DEADLINE);
        let state = rig.out.lock().unwrap().pop();
        assert_eq!(
            state,
            Some(ActionState::OutcomeUnknown(Arc::from(
                "no answer from the administrator helper"
            )))
        );
        assert!(rig.support_msgs().is_empty());
        rig.passed();
    }

    #[test]
    fn sock_destroy_call_errors_mapped() {
        for (err, state) in [
            (
                HelperCallError::Cancelled,
                ActionState::Failed(ActionError::AuthCancelled),
            ),
            (
                HelperCallError::NotAuthorized,
                ActionState::Failed(ActionError::NotAuthorized),
            ),
            (
                HelperCallError::Timeout,
                ActionState::OutcomeUnknown(Arc::from(NO_ANSWER)),
            ),
            (
                HelperCallError::Exited,
                ActionState::Failed(ActionError::Internal(Arc::from(HELPER_STOPPED))),
            ),
            (
                HelperCallError::Missing,
                ActionState::Rejected(RejectReason::Unsupported),
            ),
        ] {
            assert_eq!(map_call_error(&err), state);
        }
    }
}
