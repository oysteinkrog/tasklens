//! The targetless kinds `OpenUri`, `ShowInFileManager` and `Launch` (R-134, E11-016). Each runs
//! one job through `ActionCtx::openers`; `run_action` emits `Started` and the one summary event.
//!
//! Privacy: URIs, paths and command text are user data. Never log them above debug (ADR-015).

use std::sync::Arc;

use asupersync::Cx;
use tl_model::actions::{ActionKind, ActionState, RejectReason};

use super::{ActionCtx, LaunchOutcome, TIMED_OUT, deadline, with_deadline};

/// Runs one targetless `kind` under its ADR-005 deadline: 2 s, or 120 s for `Launch` with
/// `as_admin`. The openers' `ActionError`s pass through as `Failed` (R-148 item 3), and a launch
/// that finds the program running is `Rejected(AlreadyRunning)` (R-148 item 4).
pub async fn run(cx: &Cx, actx: &ActionCtx, kind: &ActionKind, privileged: bool) -> ActionState {
    let Some(openers) = actx.openers.as_deref() else {
        return ActionState::Rejected(RejectReason::Unsupported);
    };
    let job = async {
        match kind {
            ActionKind::OpenUri(uri) => openers
                .open_uri(cx, uri)
                .await
                .map(|()| ActionState::Succeeded),
            ActionKind::ShowInFileManager { path, properties } => openers
                .show_in_file_manager(cx, path, *properties)
                .await
                .map(|()| ActionState::Succeeded),
            ActionKind::Launch(spec) => openers.launch(cx, spec).await.map(|out| match out {
                LaunchOutcome::Launched => ActionState::Succeeded,
                LaunchOutcome::AlreadyRunning => {
                    ActionState::Rejected(RejectReason::AlreadyRunning)
                }
            }),
            _ => Ok(ActionState::Rejected(RejectReason::Unsupported)),
        }
    };
    match with_deadline(cx, deadline(kind, None, privileged), job).await {
        Ok(Ok(state)) => state,
        Ok(Err(e)) => ActionState::Failed(e),
        Err(_) => ActionState::OutcomeUnknown(Arc::from(TIMED_OUT)),
    }
}
