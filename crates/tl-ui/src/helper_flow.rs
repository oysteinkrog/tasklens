//! Helper session flow (UI-FND-028, R-148): sends the helper session commands for the shield
//! Actions and toasts a failed start in the main window.
//!
//! Every window only dispatches `StartHelperSession` and `EndHelperSession`. This module catches
//! both with `App::on_action`, so a dispatch from any window reaches the engine as
//! `Command::StartHelperSession` or `Command::EndHelperSession`. It sends a command only for a
//! dispatched Action, so the helper never starts without a user action.
//!
//! The engine publishes a failed start once, as a new `HealthSnapshot.helper_start` seq. A good
//! start, a start while the session is already up and the helper's idle exit publish no seq. The
//! engine answers starts in the order it gets them, so this module keeps the starts it sent,
//! oldest first, and matches each new seq to the oldest one.
//!
//! A Process Properties or Handle Search window that dispatched a start toasts the first cancel
//! seq after it, whichever start that seq answers. This module keeps a claim for each such
//! window and counts the toasts those windows give. It toasts in the main window only when the
//! failed starts a snapshot answers need more toasts than the windows give, so each cancel gets
//! one toast. A cancel shows "Authentication cancelled", any other failure of this module's own
//! start "Could not start the privileged helper". A window's start that fails without a cancel
//! gets no toast, as before this module.

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

/// A window that toasts the next cancel seq because it dispatched a start.
#[derive(Clone, Copy)]
struct Claim {
    window: AnyWindowHandle,
    /// Process Properties drops its claim on any new seq. Handle Search keeps it until a cancel.
    props: bool,
}

/// The shield's command sender and start outcome tracker.
pub struct HelperFlow {
    client: Arc<dyn EngineClient>,
    /// One entry per start sent and not answered yet, oldest first: `true` for this module's
    /// own start, `false` for a window's start.
    pending: VecDeque<bool>,
    /// Windows that toast the next cancel seq.
    claims: Vec<Claim>,
    /// Toasts a window gave for a cancel of another start. Each one stands for the later failure
    /// of a window start still waiting, so that failure gets no second toast.
    credit: usize,
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
            let owner = start_owner(active, cx);
            let _ = weak.update(cx, |flow, _| flow.start(owner));
        });
    });
    let weak = flow.downgrade();
    App::on_action(cx, move |_: &EndHelperSession, cx| {
        let _ = weak.update(cx, |flow, _| flow.end());
    });
}

/// Who handles a start dispatched while `window` was active.
#[derive(Clone, Copy)]
enum Owner {
    /// This module toasts its failure.
    Module,
    /// A window toasts its own cancel, and `Some` claims the next cancel seq.
    Window(Option<Claim>),
}

/// The owner of a start dispatched while `window` was active. A window claims the next cancel
/// only when it marked its start: Handle Search when "Search all" waits for the helper, Process
/// Properties when it shows the helper as `Locked` and the process has not exited.
fn start_owner(window: Option<AnyWindowHandle>, cx: &App) -> Owner {
    let Some(window) = window else {
        return Owner::Module;
    };
    if let Some(props) = PropsWindow::find(window, cx) {
        let props = props.read(cx);
        let marked = props.helper() == HelperState::Locked && !props.exited();
        return Owner::Window(marked.then_some(Claim {
            window,
            props: true,
        }));
    }
    if let Some(search) =
        HandleSearchWindow::find(WindowHandle::<Root>::new(window.window_id()), cx)
    {
        let marked = search.read(cx).is_pending_all();
        return Owner::Window(marked.then_some(Claim {
            window,
            props: false,
        }));
    }
    Owner::Module
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
            claims: Vec::new(),
            credit: 0,
            seen_seq,
            _health,
        }
    }

    /// Sends `StartHelperSession` and records it with its owner.
    fn start(&mut self, owner: Owner) {
        if let Err(e) = self.client.send(Command::StartHelperSession) {
            debug!(target: "tasklens::ui", ?e, "start_helper_session not sent");
            return;
        }
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back(matches!(owner, Owner::Module));
        if let Owner::Window(Some(claim)) = owner {
            let id = claim.window.window_id();
            if !self.claims.iter().any(|c| c.window.window_id() == id) {
                self.claims.push(claim);
            }
        }
    }

    fn end(&mut self) {
        if let Err(e) = self.client.send(Command::EndHelperSession) {
            debug!(target: "tasklens::ui", ?e, "end_helper_session not sent");
        }
    }

    /// Matches a new `helper_start` seq to the oldest unanswered starts and toasts once when the
    /// windows do not toast every failure it answers. `Unlocked` answers every start still
    /// waiting and ends every claim. `Missing` answers every start but keeps the claims, as the
    /// windows keep waiting.
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
            let cancelled = outcome == HelperStartOutcome::Cancelled;
            // Failed starts that need a toast: every own start, and a window's start on a cancel.
            let mut needed = 0;
            for _ in 0..answered {
                match self.pending.pop_front() {
                    Some(own) => needed += usize::from(own || cancelled),
                    None => break,
                }
            }
            let given = if cancelled {
                let open = cx.windows();
                let given = self
                    .claims
                    .iter()
                    .filter(|c| open.iter().any(|w| w.window_id() == c.window.window_id()))
                    .count();
                self.claims.clear();
                given
            } else {
                self.claims.retain(|c| !c.props);
                0
            };
            let covered = self.credit + given;
            let waiting = self.pending.iter().filter(|own| !**own).count();
            self.credit = covered.saturating_sub(needed).min(waiting);
            if needed > covered {
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
        match helper {
            HelperState::Unlocked => {
                self.pending.clear();
                self.claims.clear();
                self.credit = 0;
            }
            HelperState::Missing => {
                self.pending.clear();
                self.credit = 0;
            }
            HelperState::Locked | HelperState::Starting => {}
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
