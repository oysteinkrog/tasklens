//! Helper session flow (UI-FND-028, R-148): sends the helper session commands for the shield
//! Actions and toasts a failed start in the main window.
//!
//! Every window only dispatches `StartHelperSession` and `EndHelperSession`. This module catches
//! both with `App::on_action`, so a dispatch from any window reaches the engine as
//! `Command::StartHelperSession` or `Command::EndHelperSession`. It sends a command only for a
//! dispatched Action, so the helper never starts without a user action.
//!
//! The engine publishes a failed start once, as a new `HealthSnapshot.helper_start` seq. A good
//! start, a start while the session is already up and the helper's idle exit publish no seq. This
//! module keeps the starts it sent, oldest first, and matches each new seq to the oldest one. A
//! start dispatched while a Process Properties or Handle Search window is active belongs to that
//! window, which toasts its own cancel; this module toasts only the other starts, so a cancel
//! never gives two toasts. A cancel shows "Authentication cancelled", any other failure "Could not
//! start the privileged helper".

use std::collections::VecDeque;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity, Global, SharedString};
use gpui_kit::{Subscription, WindowHandle};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::health::{HelperStartOutcome, HelperState};
use tracing::debug;

use crate::components::shield::{EndHelperSession, StartHelperSession};
use crate::components::toast::ToastKind;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_en};
use crate::window::main::push_main_toast;
use crate::windows::handle_search::HandleSearchWindow;
use crate::windows::props::PropsWindow;

const AUTH_CANCELLED_TEXT: &str = "fnd-shield-auth-cancelled";
const START_FAILED_TEXT: &str = "fnd-shield-start-failed";

/// Unanswered starts kept at most. An older entry is dropped first.
const MAX_PENDING: usize = 8;

/// The shield's command sender and start outcome tracker.
pub struct HelperFlow {
    client: Arc<dyn EngineClient>,
    /// One entry per start sent and not answered yet, oldest first: `true` when this module
    /// toasts its failure.
    pending: VecDeque<bool>,
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
        // The dispatching window is being updated while this runs, so its root cannot be read
        // until the dispatch ends.
        let active = cx.active_window();
        let weak = weak.clone();
        cx.defer(move |cx| {
            let toast = !window_toasts_own_start(active, cx);
            let _ = weak.update(cx, |flow, _| flow.start(toast));
        });
    });
    let weak = flow.downgrade();
    App::on_action(cx, move |_: &EndHelperSession, cx| {
        let _ = weak.update(cx, |flow, _| flow.end());
    });
}

/// Whether `window` is a Process Properties or Handle Search window. Both toast the cancel of
/// a start dispatched in them.
fn window_toasts_own_start(window: Option<AnyWindowHandle>, cx: &App) -> bool {
    let Some(window) = window else {
        return false;
    };
    PropsWindow::find(window, cx).is_some()
        || HandleSearchWindow::find(WindowHandle::<Root>::new(window.window_id()), cx).is_some()
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
            pending: VecDeque::new(),
            seen_seq,
            _health,
        }
    }

    /// Sends `StartHelperSession` and records it. `toast` is false when the window that
    /// dispatched the Action toasts the outcome itself.
    fn start(&mut self, toast: bool) {
        if let Err(e) = self.client.send(Command::StartHelperSession) {
            debug!(target: "tasklens::ui", ?e, "start_helper_session not sent");
            return;
        }
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back(toast);
    }

    fn end(&mut self) {
        if let Err(e) = self.client.send(Command::EndHelperSession) {
            debug!(target: "tasklens::ui", ?e, "end_helper_session not sent");
        }
    }

    /// Matches a new `helper_start` seq to the oldest unanswered starts and toasts once when one
    /// of them is this module's. `Unlocked` or `Missing` answers every start still waiting.
    fn take_health(&mut self, hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) {
        let health = &hub.read(cx).current().health;
        let helper = health.helper;
        let start = health.helper_start.clone();
        if let Some((seq, outcome)) = start
            && seq != self.seen_seq
        {
            // Each failed start adds 1. A lower seq is a new engine session counting from 1.
            let answered = if seq > self.seen_seq {
                seq - self.seen_seq
            } else {
                1
            };
            self.seen_seq = seq;
            let mut toast = false;
            for _ in 0..answered {
                match self.pending.pop_front() {
                    Some(own) => toast |= own,
                    None => break,
                }
            }
            if toast {
                let (kind, key) = match outcome {
                    HelperStartOutcome::Cancelled => (ToastKind::Info, AUTH_CANCELLED_TEXT),
                    HelperStartOutcome::NotAuthorized | HelperStartOutcome::Failed(_) => {
                        (ToastKind::Error, START_FAILED_TEXT)
                    }
                };
                let text = flow_text(cx, key);
                push_main_toast(cx, kind, text, None, None);
            }
        }
        if matches!(helper, HelperState::Unlocked | HelperState::Missing) {
            self.pending.clear();
        }
    }

    /// Starts sent and not answered yet, oldest first: `true` when this module toasts the
    /// failure.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn pending(&self) -> Vec<bool> {
        self.pending.iter().copied().collect()
    }
}

fn flow_text(cx: &App, key: &str) -> SharedString {
    if cx.has_global::<Strings>() {
        tr(cx, key)
    } else {
        tr_en(key, &[])
    }
}
