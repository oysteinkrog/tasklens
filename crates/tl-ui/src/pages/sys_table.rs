//! Shared frame for the Services, Startup Apps and Connections tables (UI-SYS-001, UI-SYS-002, UI-SYS-004, UI-SYS-005, UI-SYS-007).
//!
//! `SysQuery` owns one table's query and its generation (ADR-009): the engine sorts, filters and
//! flattens, and a page shows a view only when `accepts` says its generation is the last one
//! sent. `SysPageState` turns the page's demand on while its tab is active and asks for a
//! refresh on the first visit (ADR-008, R-011). `sys_body` picks what the body shows: skeleton
//! rows before the first data of the session, then the rows or an empty or error state
//! (UI-SYS-004, UI-SYS-005).
//!
//! `SysTableFrame` ties these together for one page (E27-A1, UI-SYS-001): it owns the
//! `TlTreeTable`, the query, the page state and the action layer, installs snapshots the query
//! accepts and draws the page root.

use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::assets::IconName;
use gpui_kit::base::v_flex;
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, Entity, EntityId, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, SharedString, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;
use tl_model::actions::{
    ActionError, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget, RejectReason,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, Tab};
use tl_model::demand::Demand;
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::snapshot::{ComponentHeader, ComponentMask, Snapshot};
use tl_model::table::{ServiceFilter, SysNodeId, SysSortSpec, SysTable, SysTableQuery};
use tl_model::validity::Validity;
use tracing::{debug, warn};

#[cfg(any(test, feature = "test-hooks"))]
use crate::action_tracker::PushedToast;
use crate::action_tracker::{
    ActionTracker, ActionTrackerEvent, ToastTexts, action_words, message, root_confirm_dialog,
};
use crate::actions::RefreshCurrentTab;
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::empty::{TlEmptyState, TlEmptyTone};
use crate::components::skeleton::{TlSkeletonRows, skeleton_row_count};
use crate::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use crate::demand::DemandAggregator;
use crate::host::HostContext;
use crate::i18n::{tr, tr_args, tr_en};
use crate::table::TreeTableDelegate;
use crate::table::tree_table::{HEADER_H_ONE, TlTreeTable};
use crate::theme::{UiMetrics, current, metrics};
use crate::window::status::CHIP_H;

/// Query owner for one system table. Generations start at 1 and only grow.
pub struct SysQuery {
    current: SysTableQuery,
    /// Generation of the last query handed to `EngineClient::send`, 0 before the first.
    last_sent: u64,
    /// The last send returned `Err(CommandError::Busy)`.
    pending_resend: bool,
}

impl SysQuery {
    /// Nothing is sent until a setter, `resend` or the first `SysPageState::on_activated`.
    pub fn new(table: SysTable, sort: SysSortSpec, services: Option<ServiceFilter>) -> Self {
        Self {
            current: SysTableQuery {
                table,
                generation: 0,
                sort,
                filter: Arc::from(""),
                expanded: Arc::from([]),
                services,
            },
            last_sent: 0,
            pending_resend: false,
        }
    }

    pub fn query(&self) -> &SysTableQuery {
        &self.current
    }

    pub fn last_sent(&self) -> u64 {
        self.last_sent
    }

    /// `text` arrives debounced by the shell's `FilterBox` (UI-SHL-024). It is never logged.
    pub fn set_filter(&mut self, text: &str, client: &dyn EngineClient) {
        self.current.filter = Arc::from(text.to_lowercase());
        self.send(client);
    }

    pub fn set_sort(&mut self, sort: SysSortSpec, client: &dyn EngineClient) {
        self.current.sort = sort;
        self.send(client);
    }

    pub fn set_expanded(&mut self, expanded: Arc<[SysNodeId]>, client: &dyn EngineClient) {
        self.current.expanded = expanded;
        self.send(client);
    }

    /// Sets the expanded nodes without sending. The next send carries them, so a page can open
    /// with its saved groups expanded before its first query.
    pub fn seed_expanded(&mut self, expanded: Arc<[SysNodeId]>) {
        self.current.expanded = expanded;
    }

    pub fn set_services_filter(&mut self, f: ServiceFilter, client: &dyn EngineClient) {
        self.current.services = Some(f);
        self.send(client);
    }

    /// Sends the current query again with a new generation (after `Busy` and on first visit).
    pub fn resend(&mut self, client: &dyn EngineClient) {
        self.send(client);
    }

    /// Call on every snapshot install: resends once if the last send was refused as busy.
    pub fn on_snapshot(&mut self, client: &dyn EngineClient) {
        if self.pending_resend {
            self.send(client);
        }
    }

    /// True when a view with this generation may be shown.
    pub fn accepts(&self, query_generation: u64) -> bool {
        query_generation == self.last_sent
    }

    /// Sends `current` with the next generation. The generation counts as sent even when the
    /// engine is busy, so older views are refused; `on_snapshot` then resends once (B8-E27-1).
    fn send(&mut self, client: &dyn EngineClient) {
        let generation = self.last_sent + 1;
        self.current.generation = generation;
        self.last_sent = generation;
        self.pending_resend = false;
        match client.send(Command::SetSysTableQuery(self.current.clone())) {
            Ok(()) => {}
            Err(CommandError::Busy) => self.pending_resend = true,
            Err(e) => {
                debug!(target: "tasklens::ui", generation, error = ?e, "system table query not sent");
            }
        }
    }
}

/// Per-page session state shared by the three pages.
pub struct SysPageState {
    /// `Tab::Services`, `Tab::Startup` or `Tab::Connections`.
    pub tab: Tab,
    /// Services: `SystemdSystem`, `SystemdUser`. Startup: `Startup`. Connections: `Connections`.
    pub source_ids: SmallVec<[SourceId; 2]>,
    pub visible: bool,
    pub first_visit_done: bool,
    /// Never resets in the session (APP-059).
    pub first_data_seen: bool,
}

impl SysPageState {
    pub fn new(tab: Tab, source_ids: &[SourceId]) -> Self {
        Self {
            tab,
            source_ids: source_ids.iter().copied().collect(),
            visible: false,
            first_visit_done: false,
            first_data_seen: false,
        }
    }

    /// The tab became active: turns the page's demand on. The first call of the session also
    /// sends `RefreshNow` for each source and the query to `host`'s engine.
    pub fn on_activated(
        &mut self,
        host: &HostContext,
        query: &mut SysQuery,
        demand: &Entity<DemandAggregator>,
        cx: &mut App,
    ) {
        self.visible = true;
        set_table_demand(self.tab, true, demand, cx);
        if self.first_visit_done {
            return;
        }
        self.first_visit_done = true;
        let Some(client) = host.client() else {
            return;
        };
        for id in &self.source_ids {
            if let Err(e) = client.send(Command::RefreshNow(*id)) {
                debug!(target: "tasklens::ui", source = ?id, error = ?e, "refresh not sent");
            }
        }
        query.resend(&**client);
    }

    /// The tab was left: withdraws the page's demand.
    pub fn on_deactivated(&mut self, demand: &Entity<DemandAggregator>, cx: &mut App) {
        self.visible = false;
        set_table_demand(self.tab, false, demand, cx);
    }

    /// Marks first data seen when `validity` is not `WarmingUp` or there are rows. True only on
    /// the transition.
    pub fn note_data(&mut self, validity: Validity, rows_len: usize) -> bool {
        if self.first_data_seen || (validity == Validity::WarmingUp && rows_len == 0) {
            return false;
        }
        self.first_data_seen = true;
        true
    }
}

/// The aggregator source of each page's part. `EntityId` is a slotmap key, and these indexes
/// near `u32::MAX` are never reached by a live entity.
fn demand_source(tab: Tab) -> EntityId {
    EntityId::from(0xFFFF_FF00 + tab as u64)
}

/// Turns the Demand flag a page owns on or off: Services `services`, Startup `startup`,
/// Connections `connections`. On, the part also sets `any_surface_visible`, without which the
/// aggregator drops it, and carries the main window, so a hidden window turns it off. Other
/// tabs own no system-table flag and change nothing.
pub fn set_table_demand(tab: Tab, on: bool, demand: &Entity<DemandAggregator>, cx: &mut App) {
    let mut part = Demand {
        any_surface_visible: true,
        ..Demand::default()
    };
    match tab {
        Tab::Services => part.services = true,
        Tab::Startup => part.startup = true,
        Tab::Connections => part.connections = true,
        _ => return,
    }
    let source = demand_source(tab);
    demand.update(cx, |d, cx| {
        if on {
            let window = d.main_window();
            d.set(source, window, part, cx);
        } else {
            d.remove(source, cx);
        }
    });
}

/// What the body of a system table shows (UI-SYS-004, UI-SYS-005).
#[derive(Clone, Debug, PartialEq)]
pub enum SysBody {
    /// The first data of the session has not arrived.
    Skeleton,
    /// The table body.
    Rows,
    /// No rows and a filter is active.
    NoMatches,
    /// No rows, no filter, the source is fine.
    NoEntries,
    /// The source failed before the first data. One line of its error.
    FailedFirst { error: SharedString },
    /// Services only: both systemd managers are unavailable.
    ManagerMissing,
}

pub struct SysBodyInput<'a> {
    pub tab: Tab,
    pub first_data_seen: bool,
    pub rows_len: usize,
    /// The text filter is not empty, or (Services) the strip is not at its defaults.
    pub filter_active: bool,
    /// The page's source. Services passes the worse of its two.
    pub source: Option<&'a SourceHealth>,
    /// Services only.
    pub managers_unavailable: bool,
}

/// The body for `input`; the first matching rule wins. A source that fails after the first data
/// keeps `Rows`: the status chip shows the health item instead.
pub fn sys_body(input: &SysBodyInput) -> SysBody {
    if !input.first_data_seen
        && let Some(source) = input.source.filter(|s| s.state == SourceState::Failed)
    {
        let error = source.last_error.as_deref().unwrap_or_default();
        let line = error.lines().next().unwrap_or_default();
        return SysBody::FailedFirst {
            error: SharedString::from(line.to_owned()),
        };
    }
    if !input.first_data_seen {
        return SysBody::Skeleton;
    }
    let services = input.tab == Tab::Services;
    if services && input.managers_unavailable {
        return SysBody::ManagerMissing;
    }
    if input.rows_len == 0 && input.filter_active {
        return SysBody::NoMatches;
    }
    // Services always has its two group rows while a manager answers.
    if input.rows_len == 0 && !services {
        return SysBody::NoEntries;
    }
    SysBody::Rows
}

/// Skeleton rows for a viewport `viewport_h` high: as many as fit, at most 12.
pub fn skeleton_rows(viewport_h: Pixels, row_h: Pixels) -> usize {
    if row_h <= px(0.) {
        return 0;
    }
    skeleton_row_count((viewport_h / row_h).floor() as usize)
}

/// The skeleton drawn in place of the table body, never inside the `uniform_list`. `columns`
/// are the shown column widths.
pub fn render_skeleton(columns: &[Pixels], viewport_h: Pixels, metrics: &UiMetrics) -> AnyElement {
    let rows = skeleton_rows(viewport_h, metrics.row_height);
    TlSkeletonRows::new(rows, columns.iter().copied()).into_any_element()
}

/// What the empty-state button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysEmptyButton {
    /// The page's clear action: `ServicesClearAllFilters`, or `ClearFilter`.
    Clear,
    /// `RefreshCurrentTab`.
    Retry,
}

/// The texts and icon of one empty state, from the UI-SYS-005 table.
#[derive(Clone, Debug, PartialEq)]
pub struct SysEmptyContent {
    pub icon: IconName,
    /// The icon is drawn in `status.warning` (the failed states).
    pub warning: bool,
    pub heading: SharedString,
    pub text: Option<SharedString>,
    pub button: Option<(SharedString, SysEmptyButton)>,
}

/// The empty state for `body` on `tab`. `None` for `Rows`, `Skeleton`, and bodies a tab never
/// shows (Services `NoEntries`, `ManagerMissing` outside Services).
pub fn sys_empty_content(body: &SysBody, tab: Tab, cx: &App) -> Option<SysEmptyContent> {
    let t = |key: &str| tr(cx, key);
    let content = |icon, heading, text: Option<SharedString>, button| SysEmptyContent {
        icon,
        warning: false,
        heading,
        text,
        button,
    };
    let retry = || Some((t("sys-sys005-retry"), SysEmptyButton::Retry));
    Some(match (body, tab) {
        (SysBody::ManagerMissing, Tab::Services) => content(
            IconName::Server,
            t("sys-sys005-systemd-not-found"),
            Some(t("sys-sys005-tasklens-lists-services-from-systemd")),
            retry(),
        ),
        (SysBody::NoMatches, Tab::Services) => content(
            IconName::FunnelX,
            t("sys-sys005-no-matches"),
            Some(t("sys-sys005-no-services-match-the-current")),
            Some((t("sys-sys005-clear-filters"), SysEmptyButton::Clear)),
        ),
        (SysBody::NoMatches, Tab::Startup | Tab::Connections) => {
            let text = if tab == Tab::Startup {
                "sys-sys005-no-startup-entries-match-the"
            } else {
                "sys-sys005-no-connections-match-the-filter"
            };
            content(
                IconName::FunnelX,
                t("sys-sys005-no-matches"),
                Some(t(text)),
                Some((t("sys-sys005-clear-filter"), SysEmptyButton::Clear)),
            )
        }
        (SysBody::NoEntries, Tab::Startup) => content(
            IconName::AppWindow,
            t("sys-sys005-no-startup-entries"),
            Some(t("sys-sys005-nothing-starts-automatically-when-you")),
            None,
        ),
        (SysBody::NoEntries, Tab::Connections) => content(
            IconName::Network,
            t("sys-sys005-no-connections"),
            None,
            None,
        ),
        (SysBody::FailedFirst { error }, Tab::Services | Tab::Startup | Tab::Connections) => {
            let heading = match tab {
                Tab::Services => "sys-sys005-could-not-load-services",
                Tab::Startup => "sys-sys005-could-not-load-startup-entries",
                _ => "sys-sys005-could-not-load-connections",
            };
            SysEmptyContent {
                warning: true,
                ..content(
                    IconName::CircleAlert,
                    t(heading),
                    Some(error.clone()),
                    retry(),
                )
            }
        }
        _ => return None,
    })
}

/// The centred empty or error state for `body` (UI-SYS-005), drawn by `TlEmptyState`.
/// `clear_action` is the page's clear action; Retry dispatches `RefreshCurrentTab`. An empty
/// element for a body with no empty state.
pub fn render_empty(
    body: &SysBody,
    tab: Tab,
    clear_action: Box<dyn Action>,
    cx: &App,
) -> AnyElement {
    let Some(c) = sys_empty_content(body, tab, cx) else {
        return div().into_any_element();
    };
    let mut state = TlEmptyState::new(c.text.unwrap_or_default())
        .kit_icon(c.icon)
        .heading(c.heading);
    if c.warning {
        // The error is one line; an over-long one ends in an ellipsis.
        state = state.tone(TlEmptyTone::Warning).one_line();
    }
    if let Some((label, what)) = c.button {
        let action = match what {
            SysEmptyButton::Clear => clear_action,
            SysEmptyButton::Retry => Box::new(RefreshCurrentTab),
        };
        state = state.clear_filter(action).button_label(label);
    }
    state.into_any_element()
}

/// The system-table action a request runs, for the pending map and the row overlay
/// (UI-SYS-007).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ActionKindTag {
    Start,
    Stop,
    Restart,
    Reload,
    ResetFailed,
    SetStartup,
    StartupEnable,
    StartupDisable,
    StartupRunNow,
    StartupDelete,
    CloseTcp,
    Other,
}

/// Makes the action dispatched by a failed toast's button, such as "View logs", for one target.
pub type FailedButtonAction = Box<dyn Fn(&ActionTarget) -> Box<dyn Action>>;

/// Toast texts for one action kind on one table. The counted texts are Fluent message keys,
/// filled with `tr_args` and the args `$n` total, `$ok` succeeded, `$failed` failed, so plural
/// forms and number formats follow the locale.
pub struct ActionTexts {
    /// "Starting service...". Empty: no toast until the final event (opener actions).
    pub pending_one: SharedString,
    /// Key of "Starting { $n } services...", such as `sys-sys007-starting-n-services`.
    pub pending_many: &'static str,
    /// "Service started". Empty: the toast is removed and nothing shows.
    pub done_one: SharedString,
    /// Key of "{ $n } services started".
    pub done_many: &'static str,
    /// Key of "{ $ok } of { $n } services started. { $failed } failed."
    pub partial_many: &'static str,
    /// "Failed to start service".
    pub failed_prefix: SharedString,
    /// True: "<prefix>: <result>". False: the prefix alone (opener toasts).
    pub failed_suffix: bool,
    /// "Service action finished."
    pub unknown: SharedString,
    /// "A service action is already in progress."
    pub busy: SharedString,
    /// "View logs" and the action it dispatches for the failed target.
    pub failed_button: Option<(SharedString, FailedButtonAction)>,
}

/// D-Bus error names `failure_text` maps (R-117, U-39).
const NO_AGENT: &str = "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired";
const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";

/// "<prefix>: <reason>" for a failed system-table action. A dismissed polkit prompt cannot be
/// told from a denial on systemd D-Bus calls, so both read "not authorized" (R-117, U-39). The
/// reason is en-US, as `action_tracker::message` gives it.
pub fn failure_text(prefix: &str, err: &ActionError) -> SharedString {
    let not_authorized = || tr_en("sys-sys007-not-authorized", &[]);
    let reason = match err {
        ActionError::PermissionDenied => not_authorized(),
        ActionError::UnitJob { result, error_name } => {
            let s = error_name.as_deref().unwrap_or(result);
            match s {
                NO_AGENT => tr_en("sys-sys007-no-authentication-agent-is-running", &[]),
                ACCESS_DENIED => not_authorized(),
                _ => SharedString::from(s.to_owned()),
            }
        }
        _ => message(err),
    };
    format!("{prefix}: {reason}").into()
}

/// Targets from which one summary toast replaces the per-target toasts (R-114).
const SUMMARY_MIN: usize = 4;
/// Finished requests whose "Details" button may still be clicked.
const KEEP_DETAILS: usize = 16;

/// "Details" on a partial summary toast of a system-table action: lists the failed targets of
/// request `.0` (R-197).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ShowSysActionDetails(pub u64);

/// What a system-table page hears from its `SysActions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SysActionsEvent {
    /// `target` became pending or stopped being pending; the page repaints its row.
    PendingChanged { target: ActionTarget },
}

/// One request in flight.
struct Tracked {
    targets: SmallVec<[ActionTarget; 1]>,
    texts: ActionTexts,
    queue: WeakEntity<ToastQueue>,
    /// Per target: its final toast is shown and it is no longer pending.
    done: SmallVec<[bool; 4]>,
    /// The request's kind and the remote host's name when it is a desktop command on that PC:
    /// its toasts say "on <host>" (ADR-018).
    remote: Option<(ActionKind, Arc<str>)>,
    /// The remote host's name when the request is a root action there: a remote-admin refusal
    /// names it.
    host: Option<Arc<str>>,
}

impl Tracked {
    /// Targets counted for the toasts: a targetless request counts as one.
    fn total(&self) -> usize {
        self.targets.len().max(1)
    }

    fn summary(&self) -> bool {
        self.targets.len() >= SUMMARY_MIN
    }
}

/// The failed-target lines of a finished summary, for its "Details" button.
struct DetailsRecord {
    text: SharedString,
    lines: Rc<[SharedString]>,
}

/// One per system-table page: sends its actions through the app's `ActionTracker` with
/// `owner_toasts`, shows every toast of them itself (UI-SYS-007) and keeps the pending map the
/// row overlay and the menu read.
pub struct SysActions {
    tracker: Entity<ActionTracker>,
    pending: FxHashMap<ActionTarget, (ActionId, ActionKindTag)>,
    requests: FxHashMap<ActionId, Tracked>,
    details: std::collections::VecDeque<(ActionId, DetailsRecord)>,
    _tracker_events: Subscription,
    #[cfg(any(test, feature = "test-hooks"))]
    pushed: Vec<PushedToast>,
}

impl EventEmitter<SysActionsEvent> for SysActions {}

impl SysActions {
    /// Follows `tracker`'s events for the ids this `SysActions` submitted.
    pub fn new(tracker: &Entity<ActionTracker>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(tracker, |this, _, ev: &ActionTrackerEvent, cx| {
            this.on_tracker(*ev, cx);
        });
        let this = cx.weak_entity();
        App::on_action(cx, move |a: &ShowSysActionDetails, cx| {
            let _ = this.update(cx, |t, cx| t.show_details(ActionId(a.0), cx));
        });
        Self {
            tracker: tracker.clone(),
            pending: FxHashMap::default(),
            requests: FxHashMap::default(),
            details: std::collections::VecDeque::new(),
            _tracker_events: sub,
            #[cfg(any(test, feature = "test-hooks"))]
            pushed: Vec::new(),
        }
    }

    /// Sends `kind` on `targets`, marks every target pending and shows the pending toast(s) on
    /// `window`'s toast queue. `None` when nothing was sent: a target is already pending with
    /// the same tag (Warning `texts.busy`), the engine queue is full, or the engine stopped.
    ///
    /// A root action on a remote host (a unit of the system manager, a socket) asks first,
    /// "Change service as root on home?", and is sent after the user confirms, so the result is
    /// `None` at once (ADR-018).
    pub fn submit(
        &mut self,
        kind: ActionKind,
        tag: ActionKindTag,
        targets: SmallVec<[ActionTarget; 1]>,
        texts: ActionTexts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ActionId> {
        let probe = ActionRequest {
            id: ActionId(0),
            kind: kind.clone(),
            targets: targets.clone(),
            privileged: false,
        };
        let Some(host) = self.tracker.read(cx).remote_root_host(&probe) else {
            return self.send(kind, tag, targets, texts, window, cx);
        };
        let dialog = root_confirm_dialog(&kind, &host, cx);
        let this = cx.weak_entity();
        let opened = dialog.open(window, cx, move |result, window, cx| {
            if !matches!(result, DialogResult::Confirmed { .. }) {
                return;
            }
            let _ = this.update(cx, |t, cx| {
                t.send(kind, tag, targets, texts, window, cx);
            });
        });
        if opened.is_err() {
            warn!(target: "tasklens::ui", "root confirmation did not open; nothing sent");
        }
        None
    }

    fn send(
        &mut self,
        kind: ActionKind,
        tag: ActionKindTag,
        targets: SmallVec<[ActionTarget; 1]>,
        texts: ActionTexts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ActionId> {
        let queue = ToastQueue::of_window(window, cx).map(|q| q.downgrade());
        let Some(queue) = queue else {
            warn!(target: "tasklens::ui", "no toast queue; the action was not sent");
            return None;
        };
        if targets.iter().any(|t| self.is_pending(t, tag)) {
            let busy = texts.busy.clone();
            self.push(&queue, ToastKind::Warning, busy, None, None, cx);
            return None;
        }
        let remote = self
            .tracker
            .read(cx)
            .remote_desktop_host(&kind)
            .map(|host| (kind.clone(), host));
        let id = self.tracker.read(cx).next_id();
        let request = ActionRequest {
            id,
            kind,
            targets: targets.clone(),
            privileged: false,
        };
        let host = self.tracker.read(cx).remote_root_host(&request);
        let tracker_texts = ToastTexts {
            owner_toasts: true,
            ..ToastTexts::new(SharedString::default(), SharedString::default())
        };
        let sent = self.tracker.update(cx, |t, cx| {
            t.track(request, queue.clone(), tracker_texts, cx)
        });
        match sent {
            Ok(_) => {}
            Err(CommandError::Busy) => {
                let text = action_words(cx).send_busy.clone();
                self.push(&queue, ToastKind::Warning, text, None, None, cx);
                return None;
            }
            Err(CommandError::EngineStopped) => {
                warn!(target: "tasklens::ui", "action not sent: the engine has stopped");
                return None;
            }
        }

        let tracked = Tracked {
            host,
            done: SmallVec::from_elem(false, targets.len().max(1)),
            targets,
            texts,
            queue,
            remote,
        };
        for target in &tracked.targets {
            self.pending.insert(target.clone(), (id, tag));
            cx.emit(SysActionsEvent::PendingChanged {
                target: target.clone(),
            });
        }
        if !tracked.texts.pending_one.is_empty() {
            let queue = tracked.queue.clone();
            if tracked.summary() {
                let n = tracked.targets.len();
                let text = counted(cx, tracked.texts.pending_many, n, 0, 0);
                let key = ToastKey(id.0);
                self.push(&queue, ToastKind::Progress, text, Some(key), None, cx);
            } else {
                for index in 0..tracked.total() {
                    let key = target_key(id, index, tracked.targets.len());
                    let text = tracked.texts.pending_one.clone();
                    self.push(&queue, ToastKind::Progress, text, Some(key), None, cx);
                }
            }
        }
        self.requests.insert(id, tracked);
        Some(id)
    }

    /// True while `target` has a request with `tag` in flight (the menu disables that item).
    pub fn is_pending(&self, target: &ActionTarget, tag: ActionKindTag) -> bool {
        self.pending.get(target).is_some_and(|(_, t)| *t == tag)
    }

    /// The tag of the request in flight on `target`, for the row overlay.
    pub fn pending_tag(&self, target: &ActionTarget) -> Option<ActionKindTag> {
        self.pending.get(target).map(|(_, tag)| *tag)
    }

    fn on_tracker(&mut self, ev: ActionTrackerEvent, cx: &mut Context<Self>) {
        match ev {
            // The pending toast stays as it is.
            ActionTrackerEvent::Started(_)
            | ActionTrackerEvent::Progress { .. }
            | ActionTrackerEvent::Retried { .. } => {}
            ActionTrackerEvent::TargetFinished { id, index } => {
                if !self.requests.contains_key(&id) {
                    return;
                }
                let state = self.final_state(id, index, cx);
                self.finish_target(id, usize::from(index), state, cx);
            }
            ActionTrackerEvent::Finished { id, .. } => {
                let Some(total) = self.requests.get(&id).map(Tracked::total) else {
                    return;
                };
                for index in 0..total {
                    if self.requests[&id].done[index] {
                        continue;
                    }
                    // A target with no result after a timeout or a closed engine.
                    let state = self
                        .final_state(id, index as u16, cx)
                        .unwrap_or_else(|| ActionState::OutcomeUnknown(Arc::from("no result")));
                    self.finish_target(id, index, Some(state), cx);
                }
                self.finish_summary(id, cx);
                self.requests.remove(&id);
            }
        }
    }

    /// The tracker's final state of target `index` of request `id`.
    fn final_state(&self, id: ActionId, index: u16, cx: &App) -> Option<ActionState> {
        let results = self.tracker.read(cx).results(id)?;
        let state = results.iter().rev().find(|(i, _)| *i == index)?;
        Some(state.1.clone())
    }

    /// Clears target `index`'s pending mark and, below the summary threshold, shows its final
    /// toast.
    fn finish_target(
        &mut self,
        id: ActionId,
        index: usize,
        state: Option<ActionState>,
        cx: &mut Context<Self>,
    ) {
        let Some(tracked) = self.requests.get_mut(&id) else {
            return;
        };
        let Some(done) = tracked.done.get_mut(index) else {
            return;
        };
        if *done {
            return;
        }
        *done = true;
        let target = tracked.targets.get(index).cloned();
        if let Some(target) = &target
            && self.pending.get(target).is_some_and(|(p, _)| *p == id)
        {
            self.pending.remove(target);
            cx.emit(SysActionsEvent::PendingChanged {
                target: target.clone(),
            });
        }
        let tracked = &self.requests[&id];
        let (Some(state), false) = (state, tracked.summary()) else {
            return;
        };
        let key = target_key(id, index, tracked.targets.len());
        let queue = tracked.queue.clone();
        match final_toast(
            &state,
            &tracked.texts,
            target.as_ref(),
            tracked.remote.as_ref(),
            tracked.host.as_deref(),
            cx,
        ) {
            Some((kind, text, button)) => self.push(&queue, kind, text, Some(key), button, cx),
            None => {
                if let Some(queue) = queue.upgrade() {
                    queue.update(cx, |q, cx| q.remove(key, cx));
                }
            }
        }
    }

    /// The summary toast of a request with 4 or more targets.
    fn finish_summary(&mut self, id: ActionId, cx: &mut Context<Self>) {
        let Some(tracked) = self.requests.get(&id).filter(|t| t.summary()) else {
            return;
        };
        let n = tracked.targets.len();
        let results: SmallVec<[(usize, ActionState); 4]> = (0..n)
            .map(|i| {
                let state = self.final_state(id, i as u16, cx);
                (
                    i,
                    state.unwrap_or(ActionState::OutcomeUnknown(Arc::from("no result"))),
                )
            })
            .collect();
        let ok = results.iter().filter(|(_, s)| s.is_success()).count();
        let failed = n - ok;
        let queue = tracked.queue.clone();
        let key = ToastKey(id.0);
        if failed == 0 {
            let text = counted(cx, tracked.texts.done_many, n, ok, 0);
            self.push(&queue, ToastKind::Success, text, Some(key), None, cx);
            return;
        }
        let words = action_words(cx);
        let lines: Rc<[SharedString]> = results
            .iter()
            .filter(|(_, s)| !s.is_success())
            .map(|(i, s)| {
                let label = words.target_label(&tracked.targets[*i]);
                let reason = match s {
                    ActionState::Failed(err) => words.message(err),
                    ActionState::Rejected(reason) => words.reject(*reason).1,
                    _ => words.outcome_unknown.clone(),
                };
                SharedString::from(format!("{label}: {reason}"))
            })
            .collect();
        let text = counted(cx, tracked.texts.partial_many, n, ok, failed);
        if self.details.len() == KEEP_DETAILS {
            self.details.pop_front();
        }
        self.details.push_back((
            id,
            DetailsRecord {
                text: text.clone(),
                lines,
            },
        ));
        let button = ToastAction {
            label: tr(cx, "sys-sys007-details"),
            action: Box::new(ShowSysActionDetails(id.0)),
        };
        self.push(
            &queue,
            ToastKind::Warning,
            text,
            Some(key),
            Some(button),
            cx,
        );
    }

    /// The "Details" dialog: titled "tasklens", one line per failed target, one button "Close"
    /// (R-197).
    fn show_details(&mut self, id: ActionId, cx: &mut Context<Self>) {
        let Some((_, record)) = self.details.iter().find(|(r, _)| *r == id) else {
            return;
        };
        let Some(window) = cx.active_window() else {
            warn!(target: "tasklens::ui", "no window for the action details");
            return;
        };
        let lines = Rc::clone(&record.lines);
        let dialog = TlDialog::confirm(record.text.clone(), tr(cx, "sys-sys007-close"))
            .without_cancel()
            .extra(move |_, _| {
                div()
                    .flex()
                    .flex_col()
                    .children(lines.iter().map(|line| div().child(line.clone())))
                    .into_any_element()
            });
        // The toast button's window is being updated while its action runs.
        cx.defer(move |cx| {
            let opened = window.update(cx, |_, window, cx| dialog.open(window, cx, |_, _, _| {}));
            if !matches!(opened, Ok(Ok(()))) {
                warn!(target: "tasklens::ui", "action details dialog did not open");
            }
        });
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

    /// The lines of the Details dialog of request `id`, if it has one.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn details_lines(&self, id: ActionId) -> Option<Vec<SharedString>> {
        let (_, record) = self.details.iter().find(|(r, _)| *r == id)?;
        Some(record.lines.to_vec())
    }
}

/// `ToastKey(id.0)` for 0 or 1 target; for 2 or 3 targets one key per target, the FxHash of
/// (id, index) (R-114).
fn target_key(id: ActionId, index: usize, targets: usize) -> ToastKey {
    if targets <= 1 {
        return ToastKey(id.0);
    }
    let mut hasher = FxHasher::default();
    (id.0, index).hash(&mut hasher);
    ToastKey(hasher.finish())
}

/// The toast of one final state, per the UI-SYS-007 table. `None` removes the toast (an empty
/// `done_one`).
fn final_toast(
    state: &ActionState,
    texts: &ActionTexts,
    target: Option<&ActionTarget>,
    remote: Option<&(ActionKind, Arc<str>)>,
    host: Option<&str>,
    cx: &mut App,
) -> Option<(ToastKind, SharedString, Option<ToastAction>)> {
    let toast = match state {
        // A desktop command on a remote host says where it ran or failed (ADR-018).
        ActionState::Succeeded | ActionState::Saved(_) if remote.is_some() => {
            let (kind, host) = remote?;
            let text = action_words(cx).on_host_success(kind, host);
            (ToastKind::Success, text, None)
        }
        ActionState::Failed(err)
            if remote.is_some() && !matches!(err, ActionError::AuthCancelled) =>
        {
            let (_, host) = remote?;
            let words = action_words(cx);
            let prefix = words.on_host_failure(&texts.failed_prefix, host);
            let text = if texts.failed_suffix {
                failure_text(&prefix, err)
            } else {
                prefix
            };
            (ToastKind::Error, text, None)
        }
        ActionState::Rejected(reason)
            if remote.is_some() && *reason == RejectReason::NoDesktopSession =>
        {
            let (_, host) = remote?;
            let (_, text) = action_words(cx).reject_on(*reason, host);
            (ToastKind::Error, text, None)
        }
        ActionState::Succeeded | ActionState::Saved(_) if texts.done_one.is_empty() => return None,
        ActionState::Succeeded | ActionState::Saved(_) => {
            (ToastKind::Success, texts.done_one.clone(), None)
        }
        // A dismissed helper prompt is a choice, not a failure (UI-SYS-064 step 3).
        ActionState::Failed(ActionError::AuthCancelled) => (
            ToastKind::Warning,
            tr(cx, "fnd-shield-auth-cancelled"),
            None,
        ),
        ActionState::Failed(err) => {
            let text = if texts.failed_suffix {
                failure_text(&texts.failed_prefix, err)
            } else {
                texts.failed_prefix.clone()
            };
            let button = texts
                .failed_button
                .as_ref()
                .zip(target)
                .map(|((label, make), target)| ToastAction {
                    label: label.clone(),
                    action: make(target),
                });
            (ToastKind::Error, text, button)
        }
        ActionState::Rejected(RejectReason::Busy) => (ToastKind::Warning, texts.busy.clone(), None),
        // R-255: the engine's "already running" answer is information, not a failure.
        ActionState::Rejected(RejectReason::AlreadyRunning) => (
            ToastKind::Info,
            action_words(cx).reject(RejectReason::AlreadyRunning).1,
            None,
        ),
        ActionState::Rejected(reason @ RejectReason::RemoteAdminOff) if host.is_some() => {
            let (_, text) = action_words(cx).reject_on(*reason, host?);
            (ToastKind::Warning, text, None)
        }
        ActionState::Rejected(reason) => {
            let reason = action_words(cx).reject(*reason).1;
            let text = format!("{}: {reason}", texts.failed_prefix).into();
            (ToastKind::Error, text, None)
        }
        ActionState::OutcomeUnknown(_) | ActionState::Started | ActionState::Progress { .. } => {
            (ToastKind::Warning, texts.unknown.clone(), None)
        }
    };
    Some(toast)
}

/// Message `key` with the args `$n`, `$ok` and `$failed`.
fn counted(cx: &App, key: &str, n: usize, ok: usize, failed: usize) -> SharedString {
    let args = [
        ("n", FluentValue::from(n)),
        ("ok", FluentValue::from(ok)),
        ("failed", FluentValue::from(failed)),
    ];
    tr_args(cx, key, &args)
}

// ---- The frame (E27-A1, UI-SYS-001, UI-SYS-002) ----

/// What a system-table delegate adds to `TreeTableDelegate` for the frame. The reads take the
/// incoming snapshot, so the frame can check it before anything is installed.
pub trait SysDelegate: TreeTableDelegate {
    /// The table's component header in `snap`.
    fn component_header<'a>(&self, snap: &'a Snapshot) -> &'a ComponentHeader;
    /// The table's row count in `snap`.
    fn rows_len(&self, snap: &Snapshot) -> usize;
    /// The `query_generation` of the table's rows in `snap`, checked with `SysQuery::accepts`.
    fn query_generation(&self, snap: &Snapshot) -> u64;
    /// Installs the rows of `snap`. The frame calls it only when `accepts` is true.
    fn install(&mut self, snap: &Arc<Snapshot>);
    /// The mask bits this table draws, for example `ComponentMask::SERVICES`.
    fn mask(&self) -> ComponentMask;
}

/// "Loading…", the status text while the body is `Skeleton`, translated (R-132). Pages keep it
/// per strings version, since `status_text` has no `cx`.
pub fn loading_status_text(cx: &App) -> SharedString {
    tr(cx, "sys-loading")
}

/// The shared frame of one system-table page. The page owns it, routes the hub's
/// `SnapshotChanged`, the shell's `FilterChanged` and `PauseChanged` and its activation to it,
/// and notifies itself when a method returns true. The frame never notifies the page.
pub struct SysTableFrame<D: SysDelegate> {
    /// The host this table shows. Set when it opens; never changes.
    host: HostContext,
    pub table: Entity<TlTreeTable<D>>,
    pub query: SysQuery,
    pub state: SysPageState,
    pub actions: Entity<SysActions>,
    /// "ServiceTable", "StartupTable" or "ConnectionTable".
    key_context: &'static str,
    /// What the body shows, updated on each snapshot and filter change.
    body: SysBody,
    /// Ctrl-pause: the table keeps the rows it shows.
    paused: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    render_count: u64,
}

/// The engine's table for a system-table tab. Services for any other tab.
fn sys_table_of(tab: Tab) -> SysTable {
    match tab {
        Tab::Startup => SysTable::Startup,
        Tab::Connections => SysTable::Connections,
        _ => SysTable::Services,
    }
}

impl<D: SysDelegate> SysTableFrame<D> {
    /// Builds the table around `delegate`. Nothing is sent until the first `on_activated`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: &HostContext,
        tab: Tab,
        key_context: &'static str,
        sources: &[SourceId],
        delegate: D,
        sort: SysSortSpec,
        services: Option<ServiceFilter>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let tracker = host
            .tracker()
            .cloned()
            .expect("a system table needs the host's action tracker");
        let actions = cx.new(|cx| SysActions::new(&tracker, cx));
        Self {
            host: host.clone(),
            table,
            query: SysQuery::new(sys_table_of(tab), sort, services),
            state: SysPageState::new(tab, sources),
            actions,
            key_context,
            body: SysBody::Skeleton,
            paused: false,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: 0,
        }
    }

    /// One hub change. False unless `mask` shares a bit with the delegate's mask. Resends a
    /// busy query, notes the first data, and installs the rows only when the query accepts
    /// their generation and Ctrl-pause is off. True when the page must notify.
    pub fn on_snapshot(&mut self, snap: &Arc<Snapshot>, mask: ComponentMask, cx: &mut App) -> bool {
        let table_mask = self.table.read(cx).delegate.mask();
        if !mask.intersects(table_mask) {
            return false;
        }
        if let Some(client) = self.host.client() {
            self.query.on_snapshot(&**client);
        }
        self.apply(snap, cx)
    }

    /// Notes the first data, installs `snap` when allowed and updates the body.
    fn apply(&mut self, snap: &Arc<Snapshot>, cx: &mut App) -> bool {
        let (validity, rows_len, generation) = {
            let d = &self.table.read(cx).delegate;
            (
                d.component_header(snap).validity,
                d.rows_len(snap),
                d.query_generation(snap),
            )
        };
        let first = self.state.note_data(validity, rows_len);
        let install = !self.paused && self.query.accepts(generation);
        if install {
            self.table.update(cx, |t, cx| {
                t.delegate.install(snap);
                t.on_new_version(cx);
            });
        }
        let body = self.body_from(snap, cx);
        let changed = body != self.body;
        self.body = body;
        first || install || changed
    }

    /// The body for the rows shown, the filter and the page's source health in `snap`.
    fn body_from(&self, snap: &Snapshot, cx: &App) -> SysBody {
        let health = &snap.health;
        let source = self
            .state
            .source_ids
            .iter()
            .map(|id| health.get(*id))
            .find(|s| s.state == SourceState::Failed)
            .or_else(|| self.state.source_ids.first().map(|id| health.get(*id)));
        let query = self.query.query();
        sys_body(&SysBodyInput {
            tab: self.state.tab,
            first_data_seen: self.state.first_data_seen,
            rows_len: self.table.read(cx).delegate.row_count(),
            filter_active: !query.filter.is_empty()
                || query
                    .services
                    .as_ref()
                    .is_some_and(|strip| *strip != ServiceFilter::default()),
            source,
            managers_unavailable: false,
        })
    }

    /// Ctrl-pause from the shell's `PauseChanged` (R-042, UI-PRC-017). On release the hub's
    /// current snapshot is installed once. True when the page must notify.
    pub fn set_paused(&mut self, paused: bool, cx: &mut App) -> bool {
        if paused == self.paused {
            return false;
        }
        self.paused = paused;
        self.table.update(cx, |t, _| t.paused = paused);
        if paused {
            return false;
        }
        let Some(snap) = self.host.hub().map(|h| Arc::clone(h.read(cx).current())) else {
            return false;
        };
        self.apply(&snap, cx)
    }

    /// The shell's debounced filter text for this tab.
    pub fn on_filter_changed(&mut self, text: &str, _cx: &mut App) {
        if let Some(client) = self.host.client() {
            self.query.set_filter(text, &**client);
        }
    }

    /// The tab became active: demand on, and on the first visit a refresh and the query.
    pub fn on_activated(&mut self, cx: &mut App) {
        let Some(demand) = self.host.demand().cloned() else {
            return;
        };
        self.state
            .on_activated(&self.host, &mut self.query, &demand, cx);
    }

    /// The tab was left: demand off.
    pub fn on_deactivated(&mut self, cx: &mut App) {
        let Some(demand) = self.host.demand().cloned() else {
            return;
        };
        self.state.on_deactivated(&demand, cx);
    }

    /// F5 (`RefreshCurrentTab`): `RefreshNow` for each source. Services adds its own toast.
    pub fn refresh(&mut self, _cx: &mut App) {
        let Some(client) = self.host.client() else {
            return;
        };
        for id in &self.state.source_ids {
            if let Err(e) = client.send(Command::RefreshNow(*id)) {
                debug!(target: "tasklens::ui", source = ?id, error = ?e, "refresh not sent");
            }
        }
    }

    pub fn body(&self) -> SysBody {
        self.body.clone()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// `render` calls, for the page re-render tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u64 {
        self.render_count
    }

    /// The page root (UI-SYS-001): `window_bg`, the optional strip, then the table with its
    /// one-line header. The body ends a status-chip height above the bottom. A skeleton or an
    /// empty state covers the table body below the header; the header still shows. The
    /// page's key context wraps the table's "Table" context.
    pub fn render(
        &mut self,
        strip: Option<AnyElement>,
        clear_action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.render_count += 1;
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let header_h = px(HEADER_H_ONE * m.ui_scale);
        let chip_h = px(CHIP_H * m.ui_scale);
        let cover = match &self.body {
            SysBody::Rows => None,
            SysBody::Skeleton => {
                let layout = self.table.read(cx).columns();
                let widths: SmallVec<[Pixels; 16]> = layout
                    .visible_columns()
                    .iter()
                    .map(|c| layout.width(*c))
                    .collect();
                let viewport_h = window.viewport_size().height - header_h - chip_h;
                Some(render_skeleton(&widths, viewport_h, &m))
            }
            body => Some(render_empty(body, self.state.tab, clear_action, cx)),
        };
        v_flex()
            .debug_selector(|| "SysTableFrame".into())
            .size_full()
            .bg(theme.window_bg)
            .key_context(self.key_context)
            .children(strip)
            .child(
                div()
                    .debug_selector(|| "SysTableFrame-table".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(theme.list_bg)
                    .pb(chip_h)
                    .child(self.table.clone())
                    .children(cover.map(|cover| {
                        div()
                            .debug_selector(|| "SysTableFrame-cover".into())
                            .absolute()
                            .top(header_h)
                            .left_0()
                            .right_0()
                            .bottom(chip_h)
                            .bg(theme.list_bg)
                            .child(cover)
                    })),
            )
            .into_any_element()
    }
}
