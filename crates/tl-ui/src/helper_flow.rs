//! Helper session flow (UI-FND-028, R-148): sends the helper session commands for the shield
//! Actions and toasts each failed start once.
//!
//! Every window only dispatches `StartHelperSession` and `EndHelperSession`. This module catches
//! both with `App::on_action`, so a dispatch from any window reaches the engine as
//! `Command::StartHelperSession` or `Command::EndHelperSession`. It sends a command only for a
//! dispatched Action, so the helper never starts without a user action.
//!
//! This module is the only code that toasts a failed helper start. Each start records the window
//! that asked for it: the window that called [`note_start`] during the dispatch (Process
//! Properties, Handle Search), else the active window. The engine publishes a failed start once,
//! as a new `HealthSnapshot.helper_start` seq; a good start and the helper's idle exit publish
//! none. Each new seq gives one toast in the window of the last recorded start, or in the main
//! window when that window is closed or no start is recorded, and then clears the record. A
//! cancel shows "Authentication cancelled", any other failure "Could not start the privileged
//! helper".

use std::sync::Arc;

use gpui_kit::Subscription;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity, Global, SharedString};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::health::{HelperStartOutcome, HelperState};
use tracing::debug;

use crate::components::shield::{EndHelperSession, StartHelperSession};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_en};
use crate::window::main::push_main_toast;

const AUTH_CANCELLED_TEXT: &str = "fnd-shield-auth-cancelled";
const START_FAILED_TEXT: &str = "fnd-shield-start-failed";

/// The shield's command sender and start outcome toaster.
pub struct HelperFlow {
    client: Arc<dyn EngineClient>,
    /// The window [`note_start`] named for the start being dispatched now.
    noted: Option<AnyWindowHandle>,
    /// The window of the last start sent, until a start outcome or an active session.
    requester: Option<AnyWindowHandle>,
    /// The last `helper_start` seq seen, 0 before the first.
    seen_seq: u64,
    _health: Subscription,
}

/// The app's helper flow, set by [`install`].
pub struct GlobalHelperFlow(pub Entity<HelperFlow>);

impl Global for GlobalHelperFlow {}

/// Creates the flow and registers the two Action handlers. Does nothing without the hub.
pub fn install(cx: &mut App) {
    let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
        debug!(target: "tasklens::ui", "no hub, helper flow not installed");
        return;
    };
    let flow = cx.new(|cx| HelperFlow::new(&hub, cx));
    cx.set_global(GlobalHelperFlow(flow.clone()));

    let weak = flow.downgrade();
    App::on_action(cx, move |_: &StartHelperSession, cx| {
        let active = cx.active_window();
        let _ = weak.update(cx, |flow, _| flow.start(active));
    });
    let weak = flow.downgrade();
    App::on_action(cx, move |_: &EndHelperSession, cx| {
        let _ = weak.update(cx, |flow, _| flow.end());
    });
}

/// Names `window` as the requester of the `StartHelperSession` it dispatches now. A window calls
/// it before the Action reaches this module: before `dispatch_action`, or in a capture-phase
/// handler. Does nothing when the flow is not installed.
pub fn note_start(window: AnyWindowHandle, cx: &mut App) {
    if let Some(flow) = cx.try_global::<GlobalHelperFlow>().map(|g| g.0.clone()) {
        flow.update(cx, |flow, _| flow.noted = Some(window));
    }
}

impl HelperFlow {
    fn new(hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let hub_ref = hub.read(cx);
        let client = hub_ref.client().clone();
        let seen_seq = hub_ref
            .current()
            .health
            .helper_start
            .as_ref()
            .map_or(0, |(seq, _)| *seq);
        let _health = cx.subscribe(hub, |this, hub, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::HEALTH) {
                this.take_health(&hub, cx);
            }
        });
        Self {
            client,
            noted: None,
            requester: None,
            seen_seq,
            _health,
        }
    }

    /// Sends `StartHelperSession` and records its requester: the noted window, else `active`.
    fn start(&mut self, active: Option<AnyWindowHandle>) {
        let window = self.noted.take().or(active);
        if let Err(e) = self.client.send(Command::StartHelperSession) {
            debug!(target: "tasklens::ui", ?e, "start_helper_session not sent");
            return;
        }
        self.requester = window;
    }

    fn end(&mut self) {
        if let Err(e) = self.client.send(Command::EndHelperSession) {
            debug!(target: "tasklens::ui", ?e, "end_helper_session not sent");
        }
    }

    /// Toasts each new `helper_start` seq once, in the requester's window, and clears the
    /// requester. An active session or a missing helper also clears it.
    fn take_health(&mut self, hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) {
        let health = &hub.read(cx).current().health;
        let helper = health.helper;
        let start = health.helper_start.clone();
        if let Some((seq, outcome)) = start
            && seq != self.seen_seq
        {
            self.seen_seq = seq;
            let (kind, key) = match outcome {
                HelperStartOutcome::Cancelled => (ToastKind::Info, AUTH_CANCELLED_TEXT),
                HelperStartOutcome::NotAuthorized | HelperStartOutcome::Failed(_) => {
                    (ToastKind::Error, START_FAILED_TEXT)
                }
            };
            let text = flow_text(cx, key);
            toast_in(self.requester.take(), kind, text, cx);
        }
        if matches!(helper, HelperState::Unlocked | HelperState::Missing) {
            self.requester = None;
        }
    }

    /// The window of the last start sent and not answered yet.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn requester(&self) -> Option<AnyWindowHandle> {
        self.requester
    }
}

/// Pushes the toast into `window`'s stack when it is open and has one, else into the main
/// window's.
fn toast_in(window: Option<AnyWindowHandle>, kind: ToastKind, text: SharedString, cx: &mut App) {
    let queue = window.and_then(|w| {
        w.update(cx, |_, window, cx| ToastQueue::of_window(window, cx))
            .ok()
            .flatten()
    });
    match queue {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => push_main_toast(cx, kind, text, None, None),
    }
}

fn flow_text(cx: &App, key: &str) -> SharedString {
    if cx.has_global::<Strings>() {
        tr(cx, key)
    } else {
        tr_en(key, &[])
    }
}
