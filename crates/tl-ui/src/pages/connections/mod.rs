//! Connections tab (UI-SYS-060 to UI-SYS-070).
//!
//! The page owns the shared system-table frame, turns Connections demand on only while visible,
//! and keeps its status text and refresh toast out of the render path. Rows and selection are
//! keyed by socket cookie, so a refresh may reorder the view without losing selected sockets.

pub mod actions;
mod copy;
pub mod delegate;
mod menu;

use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Task, Window, div,
};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::health::SourceId;
use tl_model::snapshot::{ConnCounts, ConnectionsSnapshot};
use tl_model::units;

use self::delegate::{ConnectionsDelegate, DEFAULT_SORT};
use self::menu::ConnMenuTargets;
use crate::actions::ClearFilter;
use crate::app::EngineGlobal;
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::demand::DemandAggregator;
use crate::hub::{GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{tr, tr_args};
use crate::pages::processes::actions::CopyDetails;
use crate::pages::sys_copy::{CopyTexts, copy_cell, copy_row, copy_rows_as_table};
use crate::pages::sys_table::{SysBody, SysTableFrame, loading_status_text};
use crate::table::TreeTableDelegate as _;
use crate::table::actions::{CopyCell, CopyRow, CopyRowsAsTable};
use crate::table::tree_table::TreeTableEvent;
use crate::util::clipboard_write;
use crate::window::main::{FilterChanged, GlobalShellEvents, PauseChanged, ShellPage};
use crate::window::status::StatusChanged;

/// The latest a manual refresh waits for a new Connections component version.
pub const REFRESH_TOAST_AFTER: Duration = Duration::from_secs(2);

const STATUS_UNSET: (u32, u32, u32, u32, u32, usize) =
    (u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, usize::MAX);

/// The Connections tab entity.
pub struct ConnectionsPage {
    frame: SysTableFrame<ConnectionsDelegate>,
    focus: FocusHandle,
    status_buf: String,
    last_status: (u32, u32, u32, u32, u32, usize),
    refresh: Option<(u64, Task<()>)>,
    menu_targets: Option<ConnMenuTargets>,
    copy_texts: CopyTexts,
    copy_prepared: copy::PreparedCopy,
    _subs: SmallVec<[Subscription; 3]>,
}

impl EventEmitter<StatusChanged> for ConnectionsPage {}

impl ConnectionsPage {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        _demand: &Entity<DemandAggregator>,
        _client: Arc<dyn EngineClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut delegate = ConnectionsDelegate::new(cx);
        delegate.set_page(cx.weak_entity());
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
            let changed = this.frame.on_snapshot(&snap, ev.mask, cx);
            let accepted = this.frame.query.accepts(snap.connections.query_generation);
            if ev
                .mask
                .contains(tl_model::snapshot::ComponentMask::CONNECTIONS)
            {
                this.finish_refresh_if_new(snap.connections.header.version, cx);
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
                    }
                    cx.notify();
                }
            }));
        }
        subs.push(cx.observe(&table, |this, _, cx| this.on_table_changed(cx)));
        subs.push(cx.subscribe(&table, |this, _, event: &TreeTableEvent, cx| {
            if matches!(event, TreeTableEvent::CopiedRows { .. }) {
                this.show_copy_toast(ToastKind::Success, this.copy_texts.row_ok.clone(), cx);
            }
        }));
        Self {
            frame,
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

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn menu_target_cookies_for_test(&self) -> &[u64] {
        self.menu_targets
            .as_ref()
            .map_or(&[], |targets| targets.cookies.as_slice())
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

impl Render for ConnectionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_details))
            .on_action(cx.listener(Self::on_copy_row))
            .on_action(cx.listener(Self::on_copy_rows_as_table))
            .on_action(cx.listener(Self::on_copy_local))
            .on_action(cx.listener(Self::on_copy_foreign))
            .child(self.frame.render(None, Box::new(ClearFilter), window, cx))
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
