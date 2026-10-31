//! `ActionTracker`: turns the engine's action events into toasts (E16-A3, ADR-005, ADR-006,
//! ADR-015, R-114, R-129, R-196, R-197, UI-FND-020, UI-PRC-015).
//!
//! One tracker per app reads `EngineClient::action_events`. Owners call [`ActionTracker::track`]
//! with the request and their [`ToastTexts`]; the tracker sends the command, keeps one
//! [`PendingAction`] per request and shows each step as a toast that replaces the previous one
//! in place. Per request the engine sends `Started` or one `Rejected`, then one final state per
//! target, then a summary with `target: None` (ADR-005). A dump's target also sends `Progress`
//! before its final state; the tracker passes it on as [`ActionTrackerEvent::Progress`].
//!
//! - 1 target: every toast uses the request key, `ToastKey(id.0)` unless the owner set
//!   `toast_key`.
//! - 2 targets up to `summary_min_targets - 1`: one toast per target result, keyed
//!   `id.0 << 8 | index` (R-114).
//! - `summary_min_targets` or more with `summary` set: one started toast (Progress unless the
//!   owner chose another kind), then one summary toast whose kind and text the owner builds
//!   from the failures, with a "Details" button when any target failed (R-197).
//!
//! An owner that shows its own toasts sets `ToastTexts::owner_toasts` (the system tables'
//! `SysActions`, UI-SYS-007): the tracker then sends, sweeps and emits events as usual, also
//! `TargetFinished` per target, but pushes no toast, and `results` gives the final states. With
//! `retry_as_admin` such an owner asks [`ActionTracker::can_retry`] and adds the "Retry as
//! administrator" button itself; the retry is announced with `Retried`.
//!
//! A request with a deadline that has had no summary `deadline + 5 s` after it was sent becomes
//! "Outcome unknown" (ADR-005). Nothing is ever retried automatically; "Retry as administrator"
//! is a button the user clicks (ADR-006, P3).
//!
//! Texts follow R-132: the tracker's words are translated once per strings version. The free
//! functions [`message`] and [`short_reason`] have no `App`, so they give the en-US text.

use std::collections::VecDeque;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{
    Action, App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global,
    IntoElement as _, ParentElement as _, SharedString, Styled as _, Task, WeakEntity, div,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    MAX_ACTION_TARGETS, RejectReason,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::health::HelperState;
use tracing::{debug, warn};

use crate::components::dialog::TlDialog;
use crate::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use crate::hub::SnapshotHub;
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;

/// Added to a request's deadline before it becomes "Outcome unknown" (ADR-005).
pub const DEADLINE_GRACE: Duration = Duration::from_secs(5);
/// How often pending requests are checked against their deadline.
pub const SWEEP_PERIOD: Duration = Duration::from_secs(1);
/// The summary threshold when the owner does not choose one (R-114).
pub const SUMMARY_MIN_TARGETS: u16 = 4;
/// Finished requests whose Retry or Details button may still be clicked.
const KEEP_FINISHED: usize = 16;

const PER_TARGET: Duration = Duration::from_secs(2);
const POLKIT: Duration = Duration::from_secs(120);
const UNIT_FILES: Duration = Duration::from_secs(10);

/// "Retry as administrator" on an EPERM toast: re-sends the failed targets of request `.0` with
/// `privileged = true` (UI-PRC-015, ADR-006).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct RetryActionAsAdmin(pub u64);

/// "Details" on a partial summary toast: lists the failed targets of request `.0` (R-197).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ShowActionDetails(pub u64);

/// (ok, total, failures) to the summary toast. `failures` holds each target that did not
/// succeed as (target index, final state), in target order.
type PartialText = Box<dyn Fn(u16, u16, &[(u16, &ActionState)]) -> (ToastKind, SharedString)>;

/// The owner's texts for one request. Per-feature texts belong to the owner (UI-PRC-015).
pub struct ToastTexts {
    /// Shown on `Started`, such as (Info, "End task requested."). `None` shows nothing until a
    /// result, also for many targets (R-197).
    pub started: Option<(ToastKind, SharedString)>,
    /// One target succeeded, such as "Process terminated". Empty shows no toast.
    pub success: SharedString,
    /// One target failed; `message(err)` follows it, such as "Could not end task.".
    pub failure: SharedString,
    /// The texts for `summary_min_targets` or more targets (R-114).
    pub summary: Option<SummaryTexts>,
    /// 4 by default (R-114); process actions pass 2 (UI-PRC-015).
    pub summary_min_targets: u16,
    /// P3: "Retry as administrator" on `PermissionDenied`.
    pub retry_as_admin: bool,
    /// Grouped requests share one toast (R-197). `None` is `ToastKey(id.0)`.
    pub toast_key: Option<ToastKey>,
    /// The owner shows every toast of this request itself: the tracker pushes none, emits
    /// `TargetFinished` for each target result and keeps the final states for `results`.
    pub owner_toasts: bool,
}

impl ToastTexts {
    /// Texts with no started toast, no summary, the default threshold and no retry button.
    pub fn new(success: impl Into<SharedString>, failure: impl Into<SharedString>) -> Self {
        Self {
            started: None,
            success: success.into(),
            failure: failure.into(),
            summary: None,
            summary_min_targets: SUMMARY_MIN_TARGETS,
            retry_as_admin: false,
            toast_key: None,
            owner_toasts: false,
        }
    }
}

/// The summary texts for many targets (R-114).
pub struct SummaryTexts {
    /// "Starting 6 services...", shown as `started_kind`.
    pub started: SharedString,
    /// Progress unless the owner chose another kind, such as Info for a multi-target End
    /// (UI-PRC-015).
    pub started_kind: ToastKind,
    /// "6 services started".
    pub all_ok: SharedString,
    /// The kind and text when any target did not succeed, such as (Warning, "4 of 6 services
    /// started. 2 failed.").
    pub partial: PartialText,
}

impl SummaryTexts {
    /// Summary texts whose started toast is Progress.
    pub fn new(
        started: impl Into<SharedString>,
        all_ok: impl Into<SharedString>,
        partial: impl Fn(u16, u16, &[(u16, &ActionState)]) -> (ToastKind, SharedString) + 'static,
    ) -> Self {
        Self {
            started: started.into(),
            started_kind: ToastKind::Progress,
            all_ok: all_ok.into(),
            partial: Box::new(partial),
        }
    }
}

/// One request in flight.
pub struct PendingAction {
    pub request: ActionRequest,
    pub queue: WeakEntity<ToastQueue>,
    pub texts: ToastTexts,
    pub started_at: Instant,
    /// `None` never times out (service jobs, `Dump`).
    pub deadline: Option<Duration>,
    /// Final state per target index, in arrival order.
    pub results: SmallVec<[(u16, ActionState); 4]>,
}

impl PendingAction {
    fn total(&self) -> u16 {
        self.request.targets.len().max(1) as u16
    }

    /// The toast key of the whole request.
    fn key(&self) -> ToastKey {
        self.texts.toast_key.unwrap_or(ToastKey(self.request.id.0))
    }

    fn mode(&self) -> Mode {
        let n = self.request.targets.len();
        let min = usize::from(self.texts.summary_min_targets.max(2));
        if n >= min && self.texts.summary.is_some() {
            Mode::Summary
        } else if n >= 2 && n < min {
            Mode::PerTarget
        } else {
            Mode::Single
        }
    }

    fn result(&self, index: u16) -> Option<&ActionState> {
        self.results
            .iter()
            .rev()
            .find(|(i, _)| *i == index)
            .map(|(_, s)| s)
    }

    /// Each target's state: its own result, else `fallback`.
    fn effective<'a>(&'a self, fallback: &'a ActionState) -> SmallVec<[&'a ActionState; 4]> {
        (0..self.total())
            .map(|i| self.result(i).unwrap_or(fallback))
            .collect()
    }
}

/// How a request's results are shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// One toast on the request key. Also many targets without summary texts, which show the
    /// summary state (ADR-005 summary rule).
    Single,
    /// One toast per target result (R-114).
    PerTarget,
    /// One Progress toast, then one summary toast (R-114).
    Summary,
}

/// What a tracker subscriber hears, by request id (R-197).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionTrackerEvent {
    Started(ActionId),
    /// A running target reported progress: `bytes` written so far (a dump, ADR-018). It is not a
    /// result: `results` does not hold it.
    Progress {
        id: ActionId,
        index: u16,
        bytes: u64,
    },
    /// One target's final state arrived; read it with `ActionTracker::results`. Only for
    /// requests with `owner_toasts`.
    TargetFinished {
        id: ActionId,
        index: u16,
    },
    /// The summary arrived, or the request timed out or the engine stopped. `failed` counts every
    /// target that did not succeed.
    Finished {
        id: ActionId,
        ok: u16,
        total: u16,
        failed: u16,
    },
    /// "Retry as administrator" re-sent the failed targets of `from` as request `to`, with the
    /// same texts.
    Retried {
        from: ActionId,
        to: ActionId,
    },
}

/// The failed targets of a finished request, for its "Retry as administrator" button.
struct RetryRecord {
    request: ActionRequest,
    queue: WeakEntity<ToastQueue>,
    texts: ToastTexts,
}

/// The failed-target lines of a finished request, for its "Details" button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailsRecord {
    /// The summary toast text, shown as the dialog text.
    pub text: SharedString,
    /// One "<target label>: <message>" line per failed target.
    pub lines: Vec<SharedString>,
}

/// One toast the tracker pushed, for tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushedToast {
    pub kind: ToastKind,
    pub text: SharedString,
    pub key: Option<ToastKey>,
    pub button: Option<SharedString>,
}

pub struct ActionTracker {
    client: Arc<dyn EngineClient>,
    /// The same host's hub, for its tl-helper state.
    hub: Option<Entity<SnapshotHub>>,
    pending: FxHashMap<ActionId, PendingAction>,
    next_id: AtomicU64,
    retries: VecDeque<(ActionId, RetryRecord)>,
    details: VecDeque<(ActionId, DetailsRecord)>,
    /// The final state of every target of the last finished `owner_toasts` requests, for
    /// `results` after `Finished`.
    finished: VecDeque<(ActionId, FinalStates)>,
    /// The deadline sweep runs only while requests are pending.
    sweeping: bool,
    _events: Task<()>,
    #[cfg(any(test, feature = "test-hooks"))]
    pushed: Vec<PushedToast>,
    #[cfg(any(test, feature = "test-hooks"))]
    restricted_notes: u32,
    #[cfg(any(test, feature = "test-hooks"))]
    opened_details: Vec<DetailsRecord>,
}

/// Final states by target index.
type FinalStates = SmallVec<[(u16, ActionState); 4]>;

/// The local host's tracker, set by `TasklensUi::init`. Read only through
/// [`crate::host::HostContext`].
pub struct GlobalActionTracker(pub Entity<ActionTracker>);

impl Global for GlobalActionTracker {}

impl EventEmitter<ActionTrackerEvent> for ActionTracker {}

/// Creates the local host's tracker and sets [`GlobalActionTracker`]. `hub` is the same host's
/// hub.
pub fn install(client: Arc<dyn EngineClient>, hub: Option<Entity<SnapshotHub>>, cx: &mut App) {
    let tracker = cx.new(|cx| ActionTracker::new(client, hub, cx));
    cx.set_global(GlobalActionTracker(tracker));
}

impl ActionTracker {
    /// Starts the event loop and registers the toast button handlers. The deadline sweep starts
    /// with the first tracked request. `hub` is the same host's hub; it tells whether that host
    /// has tl-helper. Without it the helper reads as missing.
    pub fn new(
        client: Arc<dyn EngineClient>,
        hub: Option<Entity<SnapshotHub>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = client.action_events();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(ev) = events.recv().await {
                if this.update(cx, |t, cx| t.on_event(ev, cx)).is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |t, cx| t.on_closed(cx));
        });

        let this = cx.weak_entity();
        App::on_action(cx, move |a: &RetryActionAsAdmin, cx| {
            let _ = this.update(cx, |t, cx| t.retry(ActionId(a.0), cx));
        });
        let this = cx.weak_entity();
        App::on_action(cx, move |a: &ShowActionDetails, cx| {
            let _ = this.update(cx, |t, cx| t.show_details(ActionId(a.0), cx));
        });

        Self {
            client,
            hub,
            pending: FxHashMap::default(),
            next_id: AtomicU64::new(1),
            retries: VecDeque::new(),
            details: VecDeque::new(),
            finished: VecDeque::new(),
            sweeping: false,
            _events: task,
            #[cfg(any(test, feature = "test-hooks"))]
            pushed: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            restricted_notes: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            opened_details: Vec::new(),
        }
    }

    /// Makes the next id `first`. A remote host's tracker starts at its own base, so ids from
    /// two trackers never meet (tasklens-q4x76.27).
    pub fn start_ids_at(&self, first: u64) {
        self.next_id.store(first, Ordering::Relaxed);
    }

    /// A new request id, from 1 (ADR-005).
    pub fn next_id(&self) -> ActionId {
        ActionId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Sends `request` and records it (R-197). A request whose id is 0 gets [`Self::next_id`].
    /// On `Busy` or `EngineStopped` nothing is recorded.
    pub fn track(
        &mut self,
        mut request: ActionRequest,
        queue: WeakEntity<ToastQueue>,
        texts: ToastTexts,
        cx: &mut Context<Self>,
    ) -> Result<ActionId, CommandError> {
        if request.id.0 == 0 {
            request.id = self.next_id();
        }
        let id = request.id;
        self.client.send(Command::Action(request.clone()))?;
        let deadline = request_deadline(&request);
        debug!(target: "tasklens::ui", ?id, ?deadline, "action tracked");
        self.pending.insert(
            id,
            PendingAction {
                request,
                queue,
                texts,
                started_at: cx.background_executor().now(),
                deadline,
                results: SmallVec::new(),
            },
        );
        self.start_sweep(cx);
        Ok(id)
    }

    /// [`Self::track`] for owners: a refused send shows the Warning "Too many actions are
    /// running. Try again shortly." on `queue`.
    pub fn track_or_toast(
        &mut self,
        request: ActionRequest,
        queue: WeakEntity<ToastQueue>,
        texts: ToastTexts,
        cx: &mut Context<Self>,
    ) -> Option<ActionId> {
        match self.track(request, queue.clone(), texts, cx) {
            Ok(id) => Some(id),
            Err(CommandError::Busy) => {
                let text = action_words(cx).send_busy.clone();
                self.push(&queue, ToastKind::Warning, text, None, None, cx);
                None
            }
            Err(CommandError::EngineStopped) => {
                warn!(target: "tasklens::ui", "action not sent: the engine has stopped");
                None
            }
        }
    }

    /// Requests in flight.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The target results of `owner_toasts` request `id`, as (target index, final state). While
    /// it runs: the results so far. After `Finished`: every target, those the engine reported
    /// only in the summary with the summary state; after a timeout or a closed engine, only the
    /// targets that had a result. The last 16 finished requests are kept.
    pub fn results(&self, id: ActionId) -> Option<&[(u16, ActionState)]> {
        if let Some(entry) = self.pending.get(&id) {
            return Some(&entry.results);
        }
        self.finished
            .iter()
            .find(|(f, _)| *f == id)
            .map(|(_, states)| &states[..])
    }

    fn keep_finished(&mut self, id: ActionId, states: FinalStates) {
        if self.finished.len() == KEEP_FINISHED {
            self.finished.pop_front();
        }
        self.finished.push_back((id, states));
    }

    fn on_event(&mut self, ev: ActionEvent, cx: &mut Context<Self>) {
        let Some(entry) = self.pending.get_mut(&ev.id) else {
            debug!(target: "tasklens::ui", id = ?ev.id, "event for an unknown action");
            return;
        };
        match (ev.target, ev.state) {
            (None, ActionState::Started) => self.on_started(ev.id, cx),
            (Some(_), ActionState::Started) | (None, ActionState::Progress { .. }) => {}
            (Some(index), ActionState::Progress { bytes }) => {
                cx.emit(ActionTrackerEvent::Progress {
                    id: ev.id,
                    index,
                    bytes,
                });
            }
            (Some(index), state) => {
                entry.results.push((index, state.clone()));
                if entry.texts.owner_toasts {
                    cx.emit(ActionTrackerEvent::TargetFinished { id: ev.id, index });
                } else if entry.mode() == Mode::PerTarget {
                    let helper = helper_state(self.hub.as_ref(), cx);
                    self.push_target(ev.id, index, &state, helper, cx);
                }
            }
            (None, state) => self.on_summary(ev.id, state, cx),
        }
    }

    fn on_started(&mut self, id: ActionId, cx: &mut Context<Self>) {
        cx.emit(ActionTrackerEvent::Started(id));
        let Some(entry) = self.pending.get(&id) else {
            return;
        };
        if entry.texts.owner_toasts {
            return;
        }
        let Some((kind, text)) = entry.texts.started.clone() else {
            return;
        };
        let (kind, text) = match (entry.mode(), &entry.texts.summary) {
            (Mode::Summary, Some(summary)) => (summary.started_kind, summary.started.clone()),
            _ => (kind, text),
        };
        let (queue, key) = (entry.queue.clone(), entry.key());
        self.push(&queue, kind, text, Some(key), None, cx);
    }

    /// The toast of one target's result in per-target mode.
    fn push_target(
        &mut self,
        id: ActionId,
        index: u16,
        state: &ActionState,
        helper: HelperState,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.pending.get(&id) else {
            return;
        };
        let words = action_words(cx);
        let retry = retry_allowed(entry, helper);
        let (kind, text, button) = result_toast(id, state, &entry.texts, retry, &words);
        let queue = entry.queue.clone();
        let key = ToastKey(id.0 << 8 | u64::from(index));
        self.push(&queue, kind, text, Some(key), button, cx);
    }

    fn on_summary(&mut self, id: ActionId, summary: ActionState, cx: &mut Context<Self>) {
        let Some(entry) = self.pending.remove(&id) else {
            return;
        };
        let words = action_words(cx);
        let helper = helper_state(self.hub.as_ref(), cx);
        let states = entry.effective(&summary);
        let total = entry.total();
        let ok = states.iter().filter(|s| s.is_success()).count() as u16;
        let failed = total - ok;
        let eperm: SmallVec<[u16; 4]> = (0..total)
            .filter(|&i| is_eperm(states[usize::from(i)]))
            .collect();
        let retry = !eperm.is_empty() && retry_allowed(&entry, helper);
        if entry.texts.owner_toasts {
            let finals = states.iter().enumerate();
            let finals = finals.map(|(i, s)| (i as u16, (*s).clone())).collect();
            drop(states);
            if retry {
                self.keep_retry(id, retry_record(entry, &eperm));
            }
            self.keep_finished(id, finals);
            cx.emit(ActionTrackerEvent::Finished {
                id,
                ok,
                total,
                failed,
            });
            return;
        }
        cx.emit(ActionTrackerEvent::Finished {
            id,
            ok,
            total,
            failed,
        });

        if !eperm.is_empty() && helper == HelperState::Missing {
            self.note_restricted(eperm.len() as u32, cx);
        }
        let queue = entry.queue.clone();
        let key = entry.key();

        match entry.mode() {
            Mode::Single => {
                let state = if entry.request.targets.len() == 1 {
                    states[0].clone()
                } else {
                    ActionState::summarize(states.iter().copied())
                };
                let (kind, text, button) = result_toast(id, &state, &entry.texts, retry, &words);
                self.push(&queue, kind, text, Some(key), button, cx);
            }
            Mode::PerTarget => {
                // Targets the engine reported only through the summary.
                for i in 0..total {
                    if entry.result(i).is_none() {
                        let (kind, text, button) =
                            result_toast(id, &summary, &entry.texts, retry, &words);
                        let target_key = ToastKey(id.0 << 8 | u64::from(i));
                        self.push(&queue, kind, text, Some(target_key), button, cx);
                    }
                }
                if entry.texts.started.is_some()
                    && let Some(queue) = queue.upgrade()
                {
                    queue.update(cx, |q, cx| q.remove(key, cx));
                }
            }
            Mode::Summary => {
                let Some(texts) = &entry.texts.summary else {
                    return;
                };
                if ok == total {
                    self.push(
                        &queue,
                        ToastKind::Success,
                        texts.all_ok.clone(),
                        Some(key),
                        None,
                        cx,
                    );
                } else {
                    let failures: SmallVec<[(u16, &ActionState); 4]> = (0..total)
                        .map(|i| (i, states[usize::from(i)]))
                        .filter(|(_, s)| !s.is_success())
                        .collect();
                    let (kind, text) = (texts.partial)(ok, total, &failures);
                    let button = if retry {
                        retry_button(id, &words)
                    } else {
                        let lines = failed_lines(&entry, &states, &words);
                        self.keep_details(
                            id,
                            DetailsRecord {
                                text: text.clone(),
                                lines,
                            },
                        );
                        ToastAction {
                            label: words.details.clone(),
                            action: Box::new(ShowActionDetails(id.0)),
                        }
                    };
                    self.push(&queue, kind, text, Some(key), Some(button), cx);
                }
            }
        }

        drop(states);
        if retry {
            self.keep_retry(id, retry_record(entry, &eperm));
        }
    }

    /// Every pending request past its deadline plus 5 s becomes "Outcome unknown". Returns
    /// whether requests are still pending.
    fn sweep(&mut self, cx: &mut Context<Self>) -> bool {
        let now = cx.background_executor().now();
        let expired: SmallVec<[ActionId; 4]> = self
            .pending
            .iter()
            .filter(|(_, e)| {
                e.deadline.is_some_and(|d| {
                    now.saturating_duration_since(e.started_at) > d + DEADLINE_GRACE
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.expire(id, cx);
        }
        let keep = !self.pending.is_empty();
        self.sweeping = keep;
        keep
    }

    /// The event channel closed: every pending request becomes "Outcome unknown".
    fn on_closed(&mut self, cx: &mut Context<Self>) {
        warn!(target: "tasklens::ui", pending = self.pending.len(), "action events closed");
        let ids: SmallVec<[ActionId; 4]> = self.pending.keys().copied().collect();
        for id in ids {
            self.expire(id, cx);
        }
    }

    fn expire(&mut self, id: ActionId, cx: &mut Context<Self>) {
        let Some(entry) = self.pending.remove(&id) else {
            return;
        };
        debug!(target: "tasklens::ui", ?id, "action outcome unknown");
        let total = entry.total();
        let ok = entry.results.iter().filter(|(_, s)| s.is_success()).count() as u16;
        if entry.texts.owner_toasts {
            self.keep_finished(id, entry.results);
            cx.emit(ActionTrackerEvent::Finished {
                id,
                ok,
                total,
                failed: total - ok.min(total),
            });
            return;
        }
        cx.emit(ActionTrackerEvent::Finished {
            id,
            ok,
            total,
            failed: total - ok.min(total),
        });
        let text = action_words(cx).outcome_unknown.clone();
        let queue = entry.queue.clone();
        self.push(
            &queue,
            ToastKind::Warning,
            text,
            Some(entry.key()),
            None,
            cx,
        );
    }

    fn start_sweep(&mut self, cx: &mut Context<Self>) {
        if self.sweeping {
            return;
        }
        self.sweeping = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(SWEEP_PERIOD).await;
                match this.update(cx, |t, cx| t.sweep(cx)) {
                    Ok(true) => {}
                    _ => return,
                }
            }
        })
        .detach();
    }

    fn retry(&mut self, id: ActionId, cx: &mut Context<Self>) {
        let Some(ix) = self.retries.iter().position(|(r, _)| *r == id) else {
            debug!(target: "tasklens::ui", ?id, "retry already sent or forgotten");
            return;
        };
        let Some((_, record)) = self.retries.remove(ix) else {
            return;
        };
        if let Some(to) = self.track_or_toast(record.request, record.queue, record.texts, cx) {
            cx.emit(ActionTrackerEvent::Retried { from: id, to });
        }
    }

    /// Whether finished request `id` has a "Retry as administrator" waiting. An owner that
    /// shows its own toasts adds the button, `RetryActionAsAdmin(id)`, only then.
    pub fn can_retry(&self, id: ActionId) -> bool {
        self.retries.iter().any(|(r, _)| *r == id)
    }

    fn show_details(&mut self, id: ActionId, cx: &mut Context<Self>) {
        let Some(record) = self
            .details
            .iter()
            .find(|(r, _)| *r == id)
            .map(|(_, d)| d.clone())
        else {
            return;
        };
        let Some(window) = cx.active_window() else {
            warn!(target: "tasklens::ui", "no window for the action details");
            return;
        };
        #[cfg(any(test, feature = "test-hooks"))]
        self.opened_details.push(record.clone());
        let close = action_words(cx).close.clone();
        let lines: Rc<[SharedString]> = record.lines.into();
        let dialog = TlDialog::confirm(record.text, close)
            .without_cancel()
            .extra(move |_, _| {
                div()
                    .flex()
                    .flex_col()
                    .children(lines.iter().map(|line| div().child(line.clone())))
                    .into_any_element()
            });
        // The button's window is being updated while its action runs.
        cx.defer(move |cx| {
            let opened = window.update(cx, |_, window, cx| dialog.open(window, cx, |_, _, _| {}));
            if !matches!(opened, Ok(Ok(()))) {
                warn!(target: "tasklens::ui", "action details dialog did not open");
            }
        });
    }

    /// Tells the status chip how many targets were refused while tl-helper is missing
    /// (E20-016, UI-FND-028). Without a main window this does nothing.
    fn note_restricted(&mut self, count: u32, cx: &mut Context<Self>) {
        let Some(main) = crate::window::main::main_window_view(cx) else {
            return;
        };
        let chip = main.read(cx).status_chip().clone();
        chip.update(cx, |c, cx| c.note_restricted_refusal(count, cx));
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.restricted_notes += 1;
        }
    }

    fn keep_retry(&mut self, id: ActionId, record: RetryRecord) {
        if self.retries.len() == KEEP_FINISHED {
            self.retries.pop_front();
        }
        self.retries.push_back((id, record));
    }

    fn keep_details(&mut self, id: ActionId, record: DetailsRecord) {
        if self.details.len() == KEEP_FINISHED {
            self.details.pop_front();
        }
        self.details.push_back((id, record));
    }

    fn push(
        &mut self,
        queue: &WeakEntity<ToastQueue>,
        kind: ToastKind,
        text: SharedString,
        key: Option<ToastKey>,
        button: Option<ToastAction>,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            return;
        }
        #[cfg(any(test, feature = "test-hooks"))]
        self.pushed.push(PushedToast {
            kind,
            text: text.clone(),
            key,
            button: button.as_ref().map(|b| b.label.clone()),
        });
        if let Some(queue) = queue.upgrade() {
            queue.update(cx, |q, cx| q.push(kind, text, key, button, cx));
        }
    }

    /// Every toast pushed so far, oldest first.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn pushed(&self) -> &[PushedToast] {
        &self.pushed
    }

    /// How often the status chip was told about a restricted refusal.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn restricted_notes(&self) -> u32 {
        self.restricted_notes
    }

    /// The Details dialogs opened so far, oldest first.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn opened_details(&self) -> &[DetailsRecord] {
        &self.opened_details
    }
}

/// The tl-helper state of the tracker's host. Missing without a hub.
fn helper_state(hub: Option<&Entity<SnapshotHub>>, cx: &App) -> HelperState {
    hub.map_or(HelperState::Missing, |hub| {
        hub.read(cx).current().health.helper
    })
}

fn is_eperm(state: &ActionState) -> bool {
    matches!(state, ActionState::Failed(ActionError::PermissionDenied))
}

/// "Retry as administrator" shows from P3 on, while tl-helper is installed, and never for a
/// request that already ran privileged.
fn retry_allowed(entry: &PendingAction, helper: HelperState) -> bool {
    entry.texts.retry_as_admin && helper != HelperState::Missing && !entry.request.privileged
}

/// The retry of `entry`: its `eperm` targets, privileged, with a new id.
fn retry_record(entry: PendingAction, eperm: &[u16]) -> RetryRecord {
    let mut request = entry.request;
    request.id = ActionId(0);
    request.privileged = true;
    request.targets = eperm
        .iter()
        .filter_map(|&i| request.targets.get(usize::from(i)).cloned())
        .collect();
    RetryRecord {
        request,
        queue: entry.queue,
        texts: entry.texts,
    }
}

fn retry_button(id: ActionId, words: &ActionWords) -> ToastAction {
    ToastAction {
        label: words.retry_admin.clone(),
        action: Box::new(RetryActionAsAdmin(id.0)),
    }
}

/// The toast of one final state on one target or a one-target request.
fn result_toast(
    id: ActionId,
    state: &ActionState,
    texts: &ToastTexts,
    retry: bool,
    words: &ActionWords,
) -> (ToastKind, SharedString, Option<ToastAction>) {
    match state {
        ActionState::Succeeded | ActionState::Saved(_) => {
            (ToastKind::Success, texts.success.clone(), None)
        }
        ActionState::Failed(ActionError::PermissionDenied) if retry => (
            ToastKind::Error,
            words.eperm.clone(),
            Some(retry_button(id, words)),
        ),
        // An empty failure text shows the reason alone (Open file location).
        ActionState::Failed(err) if texts.failure.is_empty() => {
            (ToastKind::Error, words.message(err), None)
        }
        ActionState::Failed(err) => (
            ToastKind::Error,
            format!("{} {}", texts.failure, words.message(err)).into(),
            None,
        ),
        ActionState::Rejected(reason) => {
            let (kind, text) = words.reject(*reason);
            (kind, text, None)
        }
        ActionState::OutcomeUnknown(_) | ActionState::Started | ActionState::Progress { .. } => {
            (ToastKind::Warning, words.outcome_unknown.clone(), None)
        }
    }
}

/// One "<target label>: <message>" line per target that did not succeed (R-197).
fn failed_lines(
    entry: &PendingAction,
    states: &[&ActionState],
    words: &ActionWords,
) -> Vec<SharedString> {
    let mut lines = Vec::new();
    for (target, state) in entry.request.targets.iter().zip(states) {
        let reason = match state {
            ActionState::Succeeded
            | ActionState::Saved(_)
            | ActionState::Started
            | ActionState::Progress { .. } => continue,
            ActionState::Failed(err) => words.message(err),
            ActionState::Rejected(reason) => words.reject(*reason).1,
            ActionState::OutcomeUnknown(_) => words.outcome_unknown.clone(),
        };
        lines.push(format!("{}: {}", words.target_label(target), reason).into());
    }
    lines
}

/// The plain-text form of `err`, in en-US (ADR-015, R-196, R-240).
pub fn message(err: &ActionError) -> SharedString {
    EN_WORDS.with(|w| w.message(err))
}

/// The first sentence of [`message`] with a lower-case first letter and without its full stop,
/// for partial summaries such as "Ended 4 of 5 processes (1 permission denied)" (R-196).
pub fn short_reason(err: &ActionError) -> SharedString {
    first_sentence(&message(err)).into()
}

fn first_sentence(text: &str) -> String {
    let sentence = text.split_once(". ").map_or(text, |(first, _)| first);
    let sentence = sentence.strip_suffix('.').unwrap_or(sentence);
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The toast kind and en-US text of a refused request (R-196).
pub fn reject_text(reason: RejectReason) -> (ToastKind, SharedString) {
    EN_WORDS.with(|w| w.reject(reason))
}

/// The client-side deadline of a request of `kind` with `targets` targets (ADR-006, R-196).
/// `None` never times out. A privileged request may show a polkit prompt and gets at least
/// 120 s; see [`request_deadline`].
pub fn deadline_for(kind: &ActionKind, targets: usize) -> Option<Duration> {
    let per_target = PER_TARGET * targets.max(1) as u32;
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
        | ActionKind::StartupEnable
        | ActionKind::StartupDisable
        | ActionKind::StartupDelete => Some(per_target),
        ActionKind::UnitStart
        | ActionKind::UnitStop
        | ActionKind::UnitRestart
        | ActionKind::UnitReload
        | ActionKind::UnitResetFailed
        | ActionKind::UnitMask
        | ActionKind::UnitUnmask
        | ActionKind::Dump { .. } => None,
        ActionKind::UnitEnable | ActionKind::UnitDisable => Some(UNIT_FILES),
        ActionKind::Launch(spec) if spec.as_admin => Some(POLKIT),
        ActionKind::OpenUri(_)
        | ActionKind::ShowInFileManager { .. }
        | ActionKind::Launch(_)
        | ActionKind::StartupRunNow => Some(PER_TARGET),
        ActionKind::SockDestroy
        | ActionKind::UnitWriteRecovery(_)
        | ActionKind::UnitRemoveRecovery => Some(POLKIT),
    }
}

/// [`deadline_for`], raised to 120 s when the request is privileged (ADR-006).
pub fn request_deadline(request: &ActionRequest) -> Option<Duration> {
    let deadline = deadline_for(&request.kind, request.targets.len())?;
    Some(if request.privileged {
        deadline.max(POLKIT)
    } else {
        deadline
    })
}

/// The tracker's texts, translated once per strings version (R-132).
pub struct ActionWords {
    permission_denied: SharedString,
    no_such_process: SharedString,
    partial: Template,
    unit_job: Template,
    os: Template,
    internal: Template,
    not_authorized: SharedString,
    auth_cancelled: SharedString,
    unsupported: SharedString,
    pub outcome_unknown: SharedString,
    pub eperm: SharedString,
    pub retry_admin: SharedString,
    pub details: SharedString,
    pub close: SharedString,
    pub send_busy: SharedString,
    reject_busy: SharedString,
    reject_too_many: Template,
    reject_identity: SharedString,
    reject_kernel: SharedString,
    reject_protected: SharedString,
    reject_not_owner: SharedString,
    reject_unsupported: SharedString,
    reject_running: SharedString,
    reject_no_desktop: SharedString,
    target_pid: Template,
    target_tid: Template,
    target_socket: Template,
}

impl ActionWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        Self {
            permission_denied: t("fnd-action-permission-denied"),
            no_such_process: t("fnd-action-no-such-process"),
            partial: tmpl("fnd-action-partial", &["done", "total"]),
            unit_job: tmpl("fnd-action-unit-job", &["result"]),
            os: tmpl("fnd-action-os", &["code"]),
            internal: tmpl("fnd-action-internal", &["text"]),
            not_authorized: t("fnd-action-not-authorized"),
            auth_cancelled: t("fnd-action-auth-cancelled"),
            unsupported: t("fnd-action-unsupported"),
            outcome_unknown: t("fnd-action-outcome-unknown"),
            eperm: t("fnd-action-eperm"),
            retry_admin: t("fnd-action-retry-admin"),
            details: t("fnd-action-details"),
            close: t("fnd-action-close"),
            send_busy: t("fnd-action-send-busy"),
            reject_busy: t("fnd-action-reject-busy"),
            reject_too_many: tmpl("fnd-action-reject-too-many-targets", &["limit"]),
            reject_identity: t("fnd-action-reject-identity-changed"),
            reject_kernel: t("fnd-action-reject-kernel-thread"),
            reject_protected: t("fnd-action-reject-protected"),
            reject_not_owner: t("fnd-action-reject-not-owner"),
            reject_unsupported: t("fnd-action-reject-unsupported"),
            reject_running: t("fnd-action-reject-already-running"),
            reject_no_desktop: t("fnd-action-reject-no-desktop-session"),
            target_pid: tmpl("fnd-action-target-pid", &["pid"]),
            target_tid: tmpl("fnd-action-target-tid", &["tid"]),
            target_socket: tmpl("fnd-action-target-socket", &["local", "remote"]),
        }
    }

    fn en() -> Self {
        Self::with(|key| tr_en(key, &[]), Template::en)
    }

    fn translated(cx: &App) -> Self {
        Self::with(|key| tr(cx, key), |key, args| Template::new(cx, key, args))
    }

    pub fn message(&self, err: &ActionError) -> SharedString {
        match err {
            ActionError::PermissionDenied => self.permission_denied.clone(),
            ActionError::NoSuchProcess => self.no_such_process.clone(),
            ActionError::Partial { done, total } => fill(&self.partial, &[done, total]),
            ActionError::UnitJob { result, .. } => fill(&self.unit_job, &[result]),
            ActionError::Os(code) => fill(&self.os, &[code]),
            ActionError::Internal(text) => fill(&self.internal, &[text]),
            ActionError::NotAuthorized => self.not_authorized.clone(),
            ActionError::AuthCancelled => self.auth_cancelled.clone(),
            ActionError::Unsupported => self.unsupported.clone(),
        }
    }

    pub fn reject(&self, reason: RejectReason) -> (ToastKind, SharedString) {
        let text = match reason {
            RejectReason::Busy => self.reject_busy.clone(),
            RejectReason::TooManyTargets => fill(&self.reject_too_many, &[&MAX_ACTION_TARGETS]),
            RejectReason::IdentityChanged => self.reject_identity.clone(),
            RejectReason::KernelThread => self.reject_kernel.clone(),
            RejectReason::Protected => self.reject_protected.clone(),
            RejectReason::NotOwner => self.reject_not_owner.clone(),
            RejectReason::Unsupported => self.reject_unsupported.clone(),
            RejectReason::NoDesktopSession => self.reject_no_desktop.clone(),
            RejectReason::AlreadyRunning => {
                return (ToastKind::Info, self.reject_running.clone());
            }
        };
        (ToastKind::Warning, text)
    }

    /// The reason a target did not succeed, as the first sentence of its text in lower case
    /// without the full stop, such as "permission denied" (R-196). `None` for `Succeeded`,
    /// `Saved`, `Started` and `Progress`.
    pub fn short_reason(&self, state: &ActionState) -> Option<SharedString> {
        let text = match state {
            ActionState::Succeeded
            | ActionState::Saved(_)
            | ActionState::Started
            | ActionState::Progress { .. } => return None,
            ActionState::Failed(err) => self.message(err),
            ActionState::Rejected(reason) => self.reject(*reason).1,
            ActionState::OutcomeUnknown(_) => self.outcome_unknown.clone(),
        };
        Some(first_sentence(&text).into())
    }

    /// How a Details line names a target: the PID, thread id, cgroup path, unit or entry name,
    /// or the socket's two addresses.
    pub fn target_label(&self, target: &ActionTarget) -> SharedString {
        match target {
            ActionTarget::Process(key) => fill(&self.target_pid, &[&key.pid]),
            ActionTarget::Thread { tid, .. } => fill(&self.target_tid, &[tid]),
            ActionTarget::Cgroup(cgroup) => cgroup.rel_path.to_string().into(),
            ActionTarget::Unit(unit) => unit.name.to_string().into(),
            ActionTarget::Startup(entry) => entry.id.to_string().into(),
            ActionTarget::Socket { four_tuple, .. } => fill(
                &self.target_socket,
                &[&four_tuple.local, &four_tuple.remote],
            ),
        }
    }
}

fn fill(template: &Template, values: &[&dyn fmt::Display]) -> SharedString {
    let mut out = String::new();
    let _ = template.write(&mut out, values);
    out.into()
}

thread_local! {
    static EN_WORDS: ActionWords = ActionWords::en();
}

struct ActionWordsCache {
    version: u64,
    words: Rc<ActionWords>,
}

impl Global for ActionWordsCache {}

/// The tracker's texts for the current strings. Translated only when the strings version
/// changed; without strings they are in en-US.
pub fn action_words(cx: &mut App) -> Rc<ActionWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return Rc::new(ActionWords::en());
    };
    if let Some(cache) = cx.try_global::<ActionWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(ActionWords::translated(cx));
    cx.set_global(ActionWordsCache {
        version,
        words: words.clone(),
    });
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::actions::{DumpMode, LaunchSpec, LaunchWhat, RecoveryEdit, Signal};
    use tl_model::system::CpuSet;

    fn s(text: SharedString) -> String {
        text.to_string()
    }

    #[test]
    fn message_texts() {
        assert_eq!(
            s(message(&ActionError::PermissionDenied)),
            "Permission denied. This process belongs to another user."
        );
        assert_eq!(
            s(message(&ActionError::NoSuchProcess)),
            "The process has already exited."
        );
        assert_eq!(
            s(message(&ActionError::Partial { done: 3, total: 5 })),
            "Done for 3 of 5 targets."
        );
        assert_eq!(
            s(message(&ActionError::UnitJob {
                result: "timeout".into(),
                error_name: Some("org.freedesktop.systemd1.NoSuchUnit".into()),
            })),
            "The service job ended with result: timeout."
        );
        assert_eq!(
            s(message(&ActionError::Os(5))),
            "The system call failed (error 5)."
        );
        assert_eq!(
            s(message(&ActionError::Internal("worker gone".into()))),
            "Internal error: worker gone."
        );
        assert_eq!(
            s(message(&ActionError::NotAuthorized)),
            "Administrator access was refused."
        );
        assert_eq!(
            s(message(&ActionError::AuthCancelled)),
            "Authentication was cancelled."
        );
        assert_eq!(
            s(message(&ActionError::Unsupported)),
            "Not supported on this system."
        );
    }

    #[test]
    fn short_reason_texts() {
        assert_eq!(
            s(short_reason(&ActionError::PermissionDenied)),
            "permission denied"
        );
        assert_eq!(
            s(short_reason(&ActionError::NoSuchProcess)),
            "the process has already exited"
        );
        assert_eq!(
            s(short_reason(&ActionError::Os(13))),
            "the system call failed (error 13)"
        );
        let state = |state: ActionState| EN_WORDS.with(|w| w.short_reason(&state)).map(s);
        assert_eq!(
            state(ActionState::Failed(ActionError::PermissionDenied)).as_deref(),
            Some("permission denied")
        );
        assert_eq!(
            state(ActionState::Rejected(RejectReason::KernelThread)).as_deref(),
            Some("kernel threads cannot be signalled")
        );
        assert_eq!(
            state(ActionState::OutcomeUnknown("timeout".into())).as_deref(),
            Some("outcome unknown")
        );
        assert_eq!(state(ActionState::Succeeded), None);
    }

    #[test]
    fn reject_texts() {
        use RejectReason::*;
        let warn = |text: &str| (ToastKind::Warning, text.to_owned());
        let cases = [
            (
                Busy,
                warn("Another action on this process is still running."),
            ),
            (
                TooManyTargets,
                warn("Too many processes selected. The limit is 256."),
            ),
            (
                IdentityChanged,
                warn("The selected process changed; wait for the list to update and try again."),
            ),
            (KernelThread, warn("Kernel threads cannot be signalled.")),
            (
                Protected,
                warn("This process is protected. Confirm the warning to end it."),
            ),
            (
                NotOwner,
                warn("This process belongs to another user. Administrator access is needed."),
            ),
            (
                Unsupported,
                warn("This action is not available for this process."),
            ),
            (AlreadyRunning, (ToastKind::Info, "Already running".into())),
        ];
        for (reason, want) in cases {
            let (kind, text) = reject_text(reason);
            assert_eq!((kind, s(text)), want, "{reason:?}");
        }
    }

    #[test]
    fn deadline_table() {
        let secs = |n: u64| Some(Duration::from_secs(n));
        let per_target = [
            ActionKind::Signal(Signal::Term),
            ActionKind::KillTree,
            ActionKind::Suspend,
            ActionKind::Resume,
            ActionKind::SetNice(5),
            ActionKind::SetSched {
                policy: tl_model::actions::SchedPolicy::Batch,
                rt_prio: 0,
            },
            ActionKind::SetAffinity(CpuSet::default()),
            ActionKind::SetIoPriority(tl_model::actions::IoPriority::Idle),
            ActionKind::FreezeGroup,
            ActionKind::ThawGroup,
            ActionKind::KillGroup,
            ActionKind::StartupEnable,
            ActionKind::StartupDisable,
            ActionKind::StartupDelete,
        ];
        for kind in &per_target {
            assert_eq!(deadline_for(kind, 1), secs(2), "{kind:?}");
            assert_eq!(deadline_for(kind, 3), secs(6), "{kind:?}");
        }
        let jobs = [
            ActionKind::UnitStart,
            ActionKind::UnitStop,
            ActionKind::UnitRestart,
            ActionKind::UnitReload,
            ActionKind::UnitResetFailed,
            ActionKind::UnitMask,
            ActionKind::UnitUnmask,
        ];
        for kind in &jobs {
            assert_eq!(deadline_for(kind, 4), None, "{kind:?}");
        }
        assert_eq!(deadline_for(&ActionKind::UnitEnable, 1), secs(10));
        assert_eq!(deadline_for(&ActionKind::UnitDisable, 5), secs(10));

        let launch = |as_admin| {
            ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command("true".into()),
                as_admin,
                cwd: None,
            })
        };
        assert_eq!(deadline_for(&ActionKind::OpenUri("x".into()), 0), secs(2));
        assert_eq!(
            deadline_for(
                &ActionKind::ShowInFileManager {
                    path: "/".into(),
                    properties: false,
                },
                0
            ),
            secs(2)
        );
        assert_eq!(deadline_for(&launch(false), 0), secs(2));
        assert_eq!(deadline_for(&launch(true), 0), secs(120));
        assert_eq!(deadline_for(&ActionKind::StartupRunNow, 1), secs(2));
        assert_eq!(deadline_for(&ActionKind::SockDestroy, 1), secs(120));
        assert_eq!(
            deadline_for(&ActionKind::UnitWriteRecovery(RecoveryEdit::default()), 1),
            secs(120)
        );
        assert_eq!(deadline_for(&ActionKind::UnitRemoveRecovery, 1), secs(120));
        assert_eq!(
            deadline_for(
                &ActionKind::Dump {
                    mode: DumpMode::Full,
                    name: "core.core".into(),
                },
                1
            ),
            None
        );

        let mut request = ActionRequest {
            id: ActionId(1),
            kind: ActionKind::Signal(Signal::Kill),
            targets: SmallVec::new(),
            privileged: true,
        };
        assert_eq!(request_deadline(&request), secs(120), "privileged");
        request.kind = ActionKind::UnitStart;
        assert_eq!(request_deadline(&request), None, "jobs stay untimed");
    }
}
