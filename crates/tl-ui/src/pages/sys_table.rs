//! Shared frame for the Services, Startup Apps and Connections tables (UI-SYS-001, UI-SYS-002, UI-SYS-004, UI-SYS-005, UI-SYS-007).
//!
//! `SysQuery` owns one table's query and its generation (ADR-009): the engine sorts, filters and
//! flattens, and a page shows a view only when `accepts` says its generation is the last one
//! sent. `SysPageState` turns the page's demand on while its tab is active and asks for a
//! refresh on the first visit (ADR-008, R-011). `sys_body` picks what the body shows: skeleton
//! rows before the first data of the session, then the rows or an empty or error state
//! (UI-SYS-004, UI-SYS-005).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    Action, AnyElement, App, Entity, EntityId, IntoElement, Pixels, SharedString, div, px,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionError, ActionTarget};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, Tab};
use tl_model::demand::Demand;
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::table::{ServiceFilter, SysNodeId, SysSortSpec, SysTable, SysTableQuery};
use tl_model::validity::Validity;
use tracing::debug;

use crate::action_tracker::message;
use crate::actions::RefreshCurrentTab;
use crate::app::EngineGlobal;
use crate::components::empty::{TlEmptyState, TlEmptyTone};
use crate::components::skeleton::{TlSkeletonRows, skeleton_row_count};
use crate::demand::DemandAggregator;
use crate::i18n::{tr, tr_en};
use crate::theme::UiMetrics;

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
    /// sends `RefreshNow` for each source and the query.
    pub fn on_activated(
        &mut self,
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
        let Some(client) = cx.try_global::<EngineGlobal>().map(|g| Arc::clone(&g.0)) else {
            return;
        };
        for id in &self.source_ids {
            if let Err(e) = client.send(Command::RefreshNow(*id)) {
                debug!(target: "tasklens::ui", source = ?id, error = ?e, "refresh not sent");
            }
        }
        query.resend(&*client);
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

/// Toast texts for one action kind on one table. Fluent args: `$n` total, `$ok` succeeded,
/// `$failed` failed.
pub struct ActionTexts {
    /// "Starting service...". Empty: no toast until the final event (opener actions).
    pub pending_one: SharedString,
    /// "Starting { $n } services...".
    pub pending_many: SharedString,
    /// "Service started". Empty: the toast is removed and nothing shows.
    pub done_one: SharedString,
    /// "{ $n } services started".
    pub done_many: SharedString,
    /// "{ $ok } of { $n } services started. { $failed } failed."
    pub partial_many: SharedString,
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
