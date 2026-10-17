//! Connections tab (UI-SYS-060 to UI-SYS-070).
//!
//! The page owns the shared system-table frame, turns Connections demand on only while visible,
//! and keeps its status text and refresh toast out of the render path. Rows and selection are
//! keyed by socket cookie, so a refresh may reorder the view without losing selected sockets.

pub mod actions;
mod copy;
pub mod delegate;
mod flows;
mod menu;

pub use self::flows::{ListenConfirmTexts, listen_confirm_texts};

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Context, Entity, EntityId, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Subscription, Task, Window, canvas, div, px,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionState, ActionTarget};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::demand::Demand;
use tl_model::health::{HelperState, SourceId};
use tl_model::snapshot::{ConnCounts, ConnRecord, ConnectionsSnapshot, ProcessSnapshot};
use tl_model::units;

use self::delegate::{ConnectionsDelegate, DEFAULT_SORT};
use self::menu::ConnMenuTargets;
use crate::action_tracker::GlobalActionTracker;
use crate::actions::ClearFilter;
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, TlMenuPopup, open_context_menu_at};
use crate::components::shield::StartHelperSession;
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{tr, tr_args};
use crate::pages::processes::actions::CopyDetails;
use crate::pages::sys_copy::{CopyTexts, copy_cell, copy_row, copy_rows_as_table};
use crate::pages::sys_table::{SysActionsEvent, SysBody, SysTableFrame, loading_status_text};
use crate::table::TreeTableDelegate as _;
use crate::table::actions::{CopyCell, CopyRow, CopyRowsAsTable, OpenProperties};
use crate::table::tree_table::TreeTableEvent;
use crate::theme::{current, metrics};
use crate::util::clipboard_write;
use crate::window::main::{
    FilterChanged, GlobalShellEvents, HelperInstalled, PauseChanged, ShellPage, ShellState,
};
use crate::window::status::StatusChanged;
use crate::windows::connection_properties::open_connection_properties;

/// The most Properties windows one command opens (the window cap).
const MAX_PROPERTIES_WINDOWS: usize = 10;

/// The latest a manual refresh waits for a new Connections component version.
pub const REFRESH_TOAST_AFTER: Duration = Duration::from_secs(2);

/// Element id of the Namespaces control.
const NETNS_BUTTON_ID: &str = "ConnectionsNamespaces";

/// The aggregator source of the page's all-namespaces flag. It sits beside the Remote host
/// source; no live entity has an index this near `u32::MAX`.
fn netns_source() -> EntityId {
    EntityId::from(0xFFFF_FE01)
}

const STATUS_UNSET: (u32, u32, u32, u32, u32, usize) =
    (u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, usize::MAX);

/// The Connections tab entity.
pub struct ConnectionsPage {
    frame: SysTableFrame<ConnectionsDelegate>,
    window: AnyWindowHandle,
    focus: FocusHandle,
    status_buf: String,
    last_status: (u32, u32, u32, u32, u32, usize),
    refresh: Option<(u64, Task<()>)>,
    menu_targets: Option<ConnMenuTargets>,
    copy_texts: CopyTexts,
    copy_prepared: copy::PreparedCopy,
    copy_connections: Arc<ConnectionsSnapshot>,
    copy_processes: Arc<ProcessSnapshot>,
    /// Close requests by socket cookie: the request id and the target's index in it.
    close_requests: FxHashMap<u64, (ActionId, u16)>,
    demand: Entity<DemandAggregator>,
    active: bool,
    /// "Namespaces: All" is chosen. Session only: a new page starts on "Host only".
    netns_all: bool,
    /// Set while a helper start this page asked for has no answer: the `helper_start` seq seen
    /// when it was sent. A newer seq means the start failed or was cancelled.
    netns_start_after: Option<u64>,
    netns_focus: FocusHandle,
    netns_bounds: Rc<Cell<Bounds<Pixels>>>,
    _subs: SmallVec<[Subscription; 3]>,
}

impl EventEmitter<StatusChanged> for ConnectionsPage {}

impl ConnectionsPage {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        demand: &Entity<DemandAggregator>,
        _client: Arc<dyn EngineClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut delegate = ConnectionsDelegate::new(cx);
        delegate.set_page(cx.weak_entity());
        delegate.set_demand(demand.clone());
        let frame = SysTableFrame::new(
            Tab::Connections,
            "ConnectionTable",
            &[SourceId::Connections],
            delegate,
            DEFAULT_SORT,
            None,
            window,
            cx,
        );
        let table = frame.table.clone();
        let focus = table.read(cx).focus_handle().clone();
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(hub, |this, hub, ev: &SnapshotChanged, cx| {
            let snap = Arc::clone(hub.read(cx).current());
            if ev.mask.contains(ComponentMask::HEALTH) {
                this.on_health(&snap.health, cx);
            }
            let changed = this.frame.on_snapshot(&snap, ev.mask, cx);
            let accepted = this.frame.query.accepts(snap.connections.query_generation);
            if ev
                .mask
                .contains(tl_model::snapshot::ComponentMask::CONNECTIONS)
            {
                this.finish_refresh_if_new(snap.connections.header.version, cx);
            }
            if accepted && !this.frame.is_paused() {
                this.copy_connections = Arc::clone(&snap.connections);
                this.copy_processes = Arc::clone(&snap.processes);
            }
            if changed {
                if accepted && !this.frame.is_paused() {
                    this.refresh_status(&snap.connections, cx);
                }
                cx.notify();
            }
        }));
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|g| g.0.clone()) {
            subs.push(cx.subscribe(&events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Connections {
                    this.frame.on_filter_changed(&ev.text, cx);
                }
            }));
            subs.push(cx.subscribe(&events, |this, _, ev: &PauseChanged, cx| {
                if this.frame.set_paused(ev.0, cx) {
                    if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
                        let snap = Arc::clone(hub.read(cx).current());
                        this.refresh_status(&snap.connections, cx);
                        if !ev.0 {
                            this.copy_connections = Arc::clone(&snap.connections);
                            this.copy_processes = Arc::clone(&snap.processes);
                        }
                    }
                    cx.notify();
                }
            }));
        }
        subs.push(cx.observe(&table, |this, _, cx| this.on_table_changed(cx)));
        let actions = frame.actions.clone();
        subs.push(cx.subscribe(&actions, |this, _, ev: &SysActionsEvent, cx| {
            let SysActionsEvent::PendingChanged { target } = ev;
            this.on_pending_changed(target, cx);
        }));
        subs.push(cx.subscribe(&table, |this, _, event: &TreeTableEvent, cx| {
            match event {
                TreeTableEvent::CopiedRows { .. } => {
                    this.show_copy_toast(ToastKind::Success, this.copy_texts.row_ok.clone(), cx);
                }
                // A double-click reaches the page as an event; the mouse handler cannot raise
                // OpenProperties by propagating.
                TreeTableEvent::Activate { .. } => {
                    let window = this.window;
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| {
                            window.dispatch_action(Box::new(OpenProperties), cx);
                        });
                    });
                }
                _ => {}
            }
        }));
        Self {
            frame,
            window: window.window_handle(),
            focus,
            status_buf: loading_status_text(cx).to_string(),
            last_status: STATUS_UNSET,
            refresh: None,
            menu_targets: None,
            copy_texts: CopyTexts {
                row_ok: tr(cx, "connections-copy-row-ok"),
                row_failed: tr(cx, "connections-copy-failed"),
                nothing_selected: tr(cx, "connections-copy-nothing-selected"),
            },
            copy_prepared: copy::PreparedCopy::default(),
            copy_connections: Arc::new(ConnectionsSnapshot::default()),
            copy_processes: Arc::new(ProcessSnapshot::default()),
            close_requests: FxHashMap::default(),
            demand: demand.clone(),
            active: false,
            netns_all: false,
            netns_start_after: None,
            netns_focus: cx.focus_handle(),
            netns_bounds: Rc::default(),
            _subs: subs,
        }
    }

    /// Writes the cached status text without allocating in the render path.
    pub fn status_text(&self, out: &mut String) {
        out.push_str(&self.status_buf);
    }

    /// Turns Connections collection on or off with the tab.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            self.frame.on_activated(cx);
        } else {
            self.frame.on_deactivated(cx);
        }
        self.frame
            .table
            .update(cx, |table, cx| table.delegate.set_active(active, cx));
        self.active = active;
        self.send_netns_demand(cx);
    }

    /// Whether "Namespaces: All" is chosen.
    pub fn netns_all(&self) -> bool {
        self.netns_all
    }

    /// Gives the aggregator `connections_all_netns` while the tab is active and "All" is chosen.
    fn send_netns_demand(&self, cx: &mut Context<Self>) {
        let on = self.active && self.netns_all;
        self.demand.update(cx, |demand, cx| {
            if on {
                let part = Demand {
                    any_surface_visible: true,
                    connections_all_netns: true,
                    ..Demand::default()
                };
                let window = demand.main_window();
                demand.set(netns_source(), window, part, cx);
            } else {
                demand.remove(netns_source(), cx);
            }
        });
    }

    /// Applies the Namespaces choice: demand, the Namespace column and the control label.
    fn set_netns_all(&mut self, all: bool, cx: &mut Context<Self>) {
        if self.netns_all == all {
            return;
        }
        self.netns_all = all;
        if !all {
            self.netns_start_after = None;
        }
        self.frame
            .table
            .update(cx, |table, cx| table.delegate.set_netns_shown(all, cx));
        self.send_netns_demand(cx);
        cx.notify();
    }

    /// A helper start this page asked for ended without a session: back to "Host only". The
    /// toast belongs to the helper flow (R-148), so the page shows none.
    fn on_health(&mut self, health: &tl_model::health::HealthSnapshot, cx: &mut Context<Self>) {
        let Some(after) = self.netns_start_after else {
            return;
        };
        if health
            .helper_start
            .as_ref()
            .is_some_and(|(seq, _)| *seq > after)
        {
            self.set_netns_all(false, cx);
        } else if health.helper == HelperState::Unlocked {
            self.netns_start_after = None;
        }
    }

    fn on_netns_host(
        &mut self,
        _: &actions::ConnectionNetnsHost,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_netns_all(false, cx);
    }

    /// "All": sets the demand and, while the helper session is locked, starts one (polkit).
    fn on_netns_all(
        &mut self,
        _: &actions::ConnectionNetnsAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cx.try_global::<HelperInstalled>().is_some_and(|h| h.0) {
            return;
        }
        self.set_netns_all(true, cx);
        let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
            return;
        };
        let snap = Arc::clone(hub.read(cx).current());
        if snap.health.helper == HelperState::Locked {
            self.netns_start_after = Some(snap.health.helper_start.as_ref().map_or(0, |(s, _)| *s));
            crate::helper_flow::note_start(window.window_handle(), cx);
            window.dispatch_action(Box::new(StartHelperSession), cx);
        }
    }

    /// The Namespaces menu: check items "Host only" and "All". Without the helper "All" is
    /// disabled.
    pub fn build_netns_menu(
        page: &Entity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TlMenuPopup> {
        let (all, focus) = {
            let page = page.read(cx);
            (page.netns_all, page.netns_focus.clone())
        };
        let installed = cx.try_global::<HelperInstalled>().is_some_and(|h| h.0);
        let host_text = tr(cx, "connections-netns-host-only");
        let all_text = tr(cx, "connections-netns-all");
        let needs_helper = tr(cx, "connections-netns-needs-helper");
        TlMenu::build(window, cx, focus, move |menu, _, _| {
            let menu = menu.check_item(host_text, !all, Box::new(actions::ConnectionNetnsHost));
            if installed {
                menu.check_item(all_text, all, Box::new(actions::ConnectionNetnsAll))
            } else {
                menu.item_disabled(all_text, Some(needs_helper))
            }
        })
    }

    /// The label of the Namespaces control.
    pub fn netns_label(&self, cx: &App) -> SharedString {
        tr(
            cx,
            if self.netns_all {
                "connections-netns-label-all"
            } else {
                "connections-netns-label-host"
            },
        )
    }

    /// The floating control at the bottom right of the table body (R-116): `sp4` from the right
    /// and bottom edges, above the status chip.
    fn netns_control(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let page = cx.weak_entity();
        let bounds = Rc::clone(&self.netns_bounds);
        let measure = Rc::clone(&self.netns_bounds);
        let button = TlButton::new(NETNS_BUTTON_ID, TlButtonKind::Ghost)
            .size(TlButtonSize::Sm)
            .label(self.netns_label(cx))
            .trailing_icon(TlIcon::ChevronDown)
            .icon_size(m.icon.xs)
            .focus_handle(self.netns_focus.clone())
            .on_click(move |ev, window, cx| {
                let b = bounds.get();
                let at = if b.size.width > px(0.) {
                    b.bottom_left()
                } else {
                    ev.position()
                };
                let Some(page) = page.upgrade() else {
                    return;
                };
                let menu = Self::build_netns_menu(&page, window, cx);
                open_context_menu_at(at, window, cx, menu);
            });
        div()
            .debug_selector(|| "ConnectionsNamespaces-control".into())
            .absolute()
            .right(m.sp[4])
            .bottom(m.sp[4] + px(crate::window::status::CHIP_H * m.ui_scale))
            .bg(theme.menu_bg)
            .border_1()
            .border_color(theme.divider)
            .rounded(m.radius_card)
            .child(button)
            .child(
                canvas(move |b, _, _| measure.set(b), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .into_any_element()
    }

    /// Sends `RefreshNow(Connections)` and arms the one-shot completion toast.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.frame.refresh(cx);
        let version = cx
            .try_global::<GlobalHub>()
            .map_or(0, |h| h.0.read(cx).current().connections.header.version);
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_TOAST_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh.take().is_some() {
                    this.show_refreshed(cx);
                }
            });
        });
        self.refresh = Some((version, timer));
    }

    pub fn table(&self) -> &Entity<crate::table::tree_table::TlTreeTable<ConnectionsDelegate>> {
        &self.frame.table
    }

    pub fn body(&self) -> SysBody {
        self.frame.body()
    }

    pub fn is_paused(&self) -> bool {
        self.frame.is_paused()
    }

    /// Stores the records captured when the row menu opened.
    pub(super) fn set_menu_targets(&mut self, targets: ConnMenuTargets) {
        self.menu_targets = Some(targets);
    }

    /// Captures the selection at command time. Menu actions call [`Self::take_targets`] first.
    #[allow(dead_code)]
    pub(super) fn capture_targets(&self, cx: &App) -> ConnMenuTargets {
        let table = self.frame.table.read(cx);
        let delegate = &table.delegate;
        let mut cookies: Vec<u64> = table
            .selection
            .ids()
            .filter(|cookie| delegate.row_index(*cookie).is_some())
            .collect();
        cookies.sort_unstable_by_key(|cookie| delegate.row_index(*cookie));
        delegate.capture_menu_targets(&cookies, None)
    }

    /// Takes the menu-open snapshot, or reads the current selection for a keyboard command.
    #[allow(dead_code)]
    pub(super) fn take_targets(&mut self, cx: &App) -> ConnMenuTargets {
        self.menu_targets
            .take()
            .unwrap_or_else(|| self.capture_targets(cx))
    }

    /// The page's action layer, for the close toast tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn actions_for_test(&self) -> &Entity<crate::pages::sys_table::SysActions> {
        &self.frame.actions
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn menu_target_cookies_for_test(&self) -> &[u64] {
        self.menu_targets
            .as_ref()
            .map_or(&[], |targets| targets.cookies.as_slice())
    }

    /// A close started or finished on `target`. Its State cell shows "Closing…" while pending,
    /// and after a success until the row leaves the snapshot (UI-SYS-064 step 5). Any other
    /// result drops the mark.
    fn on_pending_changed(&mut self, target: &ActionTarget, cx: &mut Context<Self>) {
        let ActionTarget::Socket { cookie, .. } = *target else {
            return;
        };
        let closing = if self.frame.actions.read(cx).pending_tag(target).is_some() {
            Some(true)
        } else if self.close_succeeded(cookie, cx) {
            None
        } else {
            Some(false)
        };
        if let Some(closing) = closing {
            self.frame.table.update(cx, |table, cx| {
                table.delegate.set_closing(cookie, closing);
                cx.notify();
            });
            cx.notify();
        }
    }

    /// Whether the finished close on `cookie` succeeded. Forgets the request.
    fn close_succeeded(&mut self, cookie: u64, cx: &App) -> bool {
        let Some((id, index)) = self.close_requests.remove(&cookie) else {
            return false;
        };
        let Some(tracker) = cx.try_global::<GlobalActionTracker>() else {
            return false;
        };
        tracker.0.read(cx).results(id).is_some_and(|results| {
            results
                .iter()
                .rev()
                .find(|(i, _)| *i == index)
                .is_some_and(|(_, state)| *state == ActionState::Succeeded)
        })
    }

    fn on_table_changed(&mut self, cx: &mut Context<Self>) {
        let sort = self
            .frame
            .table
            .update(cx, |table, _| table.delegate.take_sort_request());
        if let Some(sort) = sort
            && let Some(client) = cx.try_global::<EngineGlobal>().map(|g| Arc::clone(&g.0))
        {
            self.frame.query.set_sort(sort, &*client);
        }
        if self.frame.body() != SysBody::Skeleton && self.last_status != STATUS_UNSET {
            self.refresh_status_counts(
                ConnCounts {
                    total: self.last_status.0,
                    tcp: self.last_status.1,
                    udp: self.last_status.2,
                    established: self.last_status.3,
                    listening: self.last_status.4,
                },
                cx,
            );
        }
    }

    fn refresh_status(&mut self, connections: &ConnectionsSnapshot, cx: &mut Context<Self>) {
        if self.frame.body() == SysBody::Skeleton {
            return;
        }
        self.refresh_status_counts(connections.counts, cx);
    }

    fn refresh_status_counts(&mut self, counts: ConnCounts, cx: &mut Context<Self>) {
        let table = self.frame.table.read(cx);
        let selected = table
            .selection
            .ids()
            .filter(|&row| table.delegate.row_index(row).is_some())
            .count();
        let next = (
            counts.total,
            counts.tcp,
            counts.udp,
            counts.established,
            counts.listening,
            selected,
        );
        if next == self.last_status {
            return;
        }
        self.last_status = next;

        let mut total = String::new();
        let mut tcp = String::new();
        let mut udp = String::new();
        let mut established = String::new();
        let mut listening = String::new();
        units::count(&mut total, u64::from(counts.total));
        units::count(&mut tcp, u64::from(counts.tcp));
        units::count(&mut udp, u64::from(counts.udp));
        units::count(&mut established, u64::from(counts.established));
        units::count(&mut listening, u64::from(counts.listening));
        let args = [
            ("total", FluentValue::from(total.as_str())),
            ("tcp", FluentValue::from(tcp.as_str())),
            ("udp", FluentValue::from(udp.as_str())),
            ("established", FluentValue::from(established.as_str())),
            ("listening", FluentValue::from(listening.as_str())),
        ];
        self.status_buf.clear();
        self.status_buf
            .push_str(&tr_args(cx, "connections-status-total", &args));
        if selected >= 2 {
            let mut selected_text = String::new();
            units::count(
                &mut selected_text,
                u64::try_from(selected).unwrap_or(u64::MAX),
            );
            let suffix = tr_args(
                cx,
                "connections-status-selected",
                &[("selected", FluentValue::from(selected_text.as_str()))],
            );
            self.status_buf.push_str(" | ");
            self.status_buf.push_str(&suffix);
        }
        cx.emit(StatusChanged);
    }

    fn finish_refresh_if_new(&mut self, version: u64, cx: &mut Context<Self>) {
        if self.refresh.as_ref().is_some_and(|(at, _)| version > *at) {
            self.refresh = None;
            self.show_refreshed(cx);
        }
    }

    fn show_refreshed(&self, cx: &mut Context<Self>) {
        let text = tr(cx, "connections-refreshed");
        let key = ToastKey(cx.entity_id().as_u64());
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Info, text, Some(key), None, cx)
                });
            }
        });
    }

    /// Selects and centres the row whose socket has `inode`. A socket that is not in the
    /// current view (gone, or hidden by the filter) gives the "not found" toast and leaves the
    /// selection and the filters alone (UI-PWN-012).
    pub fn select_inode(&mut self, inode: u64, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.frame.table.clone();
        let found = (inode != 0)
            .then(|| {
                let delegate = &table.read(cx).delegate;
                (0..delegate.row_count())
                    .find(|ix| delegate.record(*ix).is_some_and(|r| r.inode == inode))
                    .map(|ix| delegate.row_id(ix))
            })
            .flatten();
        if let Some(cookie) = found {
            table.update(cx, |table, cx| table.jump_to(cookie, cx));
            return;
        }
        // Deferred: the main window is mid-update when the route calls this.
        let text = tr(cx, "connections-not-found");
        window.defer(cx, move |window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx));
            }
        });
    }

    fn on_copy_cell(&mut self, _: &CopyCell, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_targets(cx);
        if let Some((cookie, column)) = targets.cell {
            self.prepare_copy(cx, Some(cookie));
            if let tl_model::table::SysColumnId::Connections(column) = column {
                copy_cell(self, cookie, copy::column_alias(column), window, cx);
            }
        }
    }

    fn on_copy_details(&mut self, _: &CopyDetails, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_targets = None;
        self.prepare_copy(cx, None);
        copy_row(self, window, cx);
    }

    fn on_copy_row(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        self.on_copy_details(&CopyDetails, window, cx);
    }

    #[allow(dead_code)]
    fn on_copy_rows_as_table(
        &mut self,
        _: &CopyRowsAsTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_targets = None;
        self.prepare_copy(cx, None);
        copy_rows_as_table(self, window, cx);
    }

    fn on_copy_local(
        &mut self,
        _: &actions::ConnectionCopyLocal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        if let [record] = targets.records.as_slice() {
            self.copy_address(
                record.local.as_ref(),
                "connections-copy-local-ok",
                window,
                cx,
            );
        }
    }

    fn on_copy_foreign(
        &mut self,
        _: &actions::ConnectionCopyForeign,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        if let [record] = targets.records.as_slice() {
            self.copy_address(
                record.foreign.as_ref(),
                "connections-copy-foreign-ok",
                window,
                cx,
            );
        }
    }

    fn on_jump_to_process(
        &mut self,
        _: &actions::ConnectionJumpToProcess,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        if let [record] = targets.records.as_slice() {
            // The tab switch updates this page, so run after the current update ends.
            let record = record.clone();
            window.defer(cx, move |window, cx| {
                flows::run_jump_to_process(&record, window, cx);
            });
        }
    }

    /// Enter, Alt+Enter, Ctrl+I, double-click and the menu's Properties: one window per selected
    /// socket, at most 10.
    fn on_open_properties(&mut self, _: &OpenProperties, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_targets(cx);
        for cookie in targets.cookies.iter().take(MAX_PROPERTIES_WINDOWS) {
            open_connection_properties(*cookie, cx);
        }
    }

    fn on_go_to_service(
        &mut self,
        _: &actions::ConnectionGoToService,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        if let [record] = targets.records.as_slice() {
            let record = record.clone();
            window.defer(cx, move |window, cx| {
                flows::run_go_to_service(&record, window, cx);
            });
        }
    }

    fn copy_address(
        &self,
        value: &str,
        success_key: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let (kind, text) = match clipboard_write(cx, value.to_owned()) {
            Ok(()) => (ToastKind::Success, tr(cx, success_key)),
            Err(()) => (ToastKind::Error, self.copy_texts.row_failed.clone()),
        };
        self.show_copy_toast_on_window(kind, text, window, cx);
    }

    fn show_copy_toast(
        &self,
        kind: ToastKind,
        text: gpui_kit::SharedString,
        cx: &mut Context<Self>,
    ) {
        let Some(window) = cx.active_window() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            self.show_copy_toast_on_window(kind, text, window, cx);
        });
    }

    fn show_copy_toast_on_window(
        &self,
        kind: ToastKind,
        text: gpui_kit::SharedString,
        window: &Window,
        cx: &mut App,
    ) {
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |queue, cx| queue.push(kind, text, None, None, cx));
        }
    }
}

/// Runs Jump to process for `record`'s owner in the main window. The Connection Properties
/// window calls it from its Process link.
pub fn jump_to_owner(record: &ConnRecord, cx: &mut App) {
    let record = menu::ConnRecordRef::from(record);
    let Some(main) = cx.try_global::<ShellState>().and_then(|s| s.main_window) else {
        return;
    };
    let _ = main.update(cx, |_, window, cx| {
        window.activate_window();
        flows::run_jump_to_process(&record, window, cx);
    });
}

impl Render for ConnectionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let netns_control = self.netns_control(cx);
        div()
            .size_full()
            .relative()
            .on_action(cx.listener(Self::on_netns_host))
            .on_action(cx.listener(Self::on_netns_all))
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_details))
            .on_action(cx.listener(Self::on_copy_row))
            .on_action(cx.listener(Self::on_copy_rows_as_table))
            .on_action(cx.listener(Self::on_copy_local))
            .on_action(cx.listener(Self::on_copy_foreign))
            .on_action(cx.listener(Self::on_jump_to_process))
            .on_action(cx.listener(Self::on_go_to_service))
            .on_action(cx.listener(Self::on_close_tcp))
            .on_action(cx.listener(Self::on_open_properties))
            .child(self.frame.render(None, Box::new(ClearFilter), window, cx))
            .child(netns_control)
    }
}

impl ShellPage for ConnectionsPage {
    fn status_text(&self, out: &mut String) {
        ConnectionsPage::status_text(self, out);
    }

    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        ConnectionsPage::set_active(self, active, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        ConnectionsPage::refresh(self, cx);
    }
}
