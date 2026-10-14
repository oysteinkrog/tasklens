//! Startup Apps tab (UI-SYS-040 to UI-SYS-050).
//!
//! The page owns the shared system-table frame, turns Startup demand on only while visible and
//! keeps its status text and refresh toast out of the render path. Rows and selection are keyed
//! by `StartupRef`, so a rescan may reorder the view without losing the selected entries.

pub mod actions;
pub mod delegate;

use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    Context, Entity, EventEmitter, FocusHandle, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Task, Window, div,
};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::health::SourceId;
use tl_model::snapshot::{StartupCounts, StartupSnapshot};
use tl_model::table::{StartupColumn, SysColumnId, SysSortSpec};
use tl_model::units;

use self::delegate::StartupDelegate;
use crate::actions::ClearFilter;
use crate::app::EngineGlobal;
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::demand::DemandAggregator;
use crate::hub::{GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{tr, tr_args};
use crate::pages::sys_table::{SysBody, SysTableFrame, loading_status_text};
use crate::table::TreeTableDelegate as _;
use crate::table::actions::OpenProperties;
use crate::table::tree_table::TreeTableEvent;
use crate::window::main::{FilterChanged, GlobalShellEvents, ShellPage};
use crate::window::status::StatusChanged;

/// The latest a manual refresh waits for a new Startup component version.
pub const REFRESH_TOAST_AFTER: Duration = Duration::from_secs(2);

const STATUS_UNSET: (u32, u32, u32, usize) = (u32::MAX, u32::MAX, u32::MAX, usize::MAX);

/// The Startup Apps tab entity.
pub struct StartupPage {
    frame: SysTableFrame<StartupDelegate>,
    focus: FocusHandle,
    status_buf: String,
    last_status: (u32, u32, u32, usize),
    refresh: Option<(u64, Task<()>)>,
    _subs: SmallVec<[Subscription; 3]>,
}

impl EventEmitter<StatusChanged> for StartupPage {}

impl StartupPage {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        _demand: &Entity<DemandAggregator>,
        _client: Arc<dyn EngineClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = StartupDelegate::new(StartupDelegate::env_home(), cx);
        let sort = SysSortSpec {
            column: SysColumnId::Startup(StartupColumn::Status),
            ascending: true,
        };
        let frame = SysTableFrame::new(
            Tab::Startup,
            "StartupTable",
            &[SourceId::Startup],
            delegate,
            sort,
            None,
            window,
            cx,
        );
        let focus = frame.table.read(cx).focus_handle().clone();
        let table = frame.table.clone();
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(hub, |this, hub, ev: &SnapshotChanged, cx| {
            let snap = Arc::clone(hub.read(cx).current());
            let changed = this.frame.on_snapshot(&snap, ev.mask, cx);
            let accepted = this.frame.query.accepts(snap.startup.query_generation);
            if ev.mask.contains(tl_model::snapshot::ComponentMask::STARTUP) {
                this.finish_refresh_if_new(snap.startup.header.version, cx);
            }
            if changed {
                if accepted {
                    this.refresh_status(&snap.startup, cx);
                }
                cx.notify();
            }
        }));
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|g| g.0.clone()) {
            subs.push(cx.subscribe(&events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Startup {
                    this.frame.on_filter_changed(&ev.text, cx);
                }
            }));
        }
        subs.push(cx.subscribe(&table, |_, _, ev: &TreeTableEvent, cx| {
            let TreeTableEvent::Activate { .. } = ev else {
                return;
            };
            let Some(window) = cx.active_window() else {
                return;
            };
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(OpenProperties), cx);
                });
            });
        }));
        subs.push(cx.observe(&table, |this, _, cx| this.on_table_changed(cx)));
        Self {
            frame,
            focus,
            status_buf: loading_status_text(cx).to_string(),
            last_status: STATUS_UNSET,
            refresh: None,
            _subs: subs,
        }
    }

    /// Writes the cached status text without allocating in the render path.
    pub fn status_text(&self, out: &mut String) {
        out.push_str(&self.status_buf);
    }

    /// Turns Startup collection and live watches on or off with the tab.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            self.frame.on_activated(cx);
        } else {
            self.frame.on_deactivated(cx);
        }
    }

    /// Sends `RefreshNow(Startup)` and arms the one-shot completion toast.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.frame.refresh(cx);
        let version = cx
            .try_global::<GlobalHub>()
            .map_or(0, |h| h.0.read(cx).current().startup.header.version);
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_TOAST_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh.is_some() {
                    this.show_refreshed(cx);
                    this.refresh = None;
                }
            });
        });
        self.refresh = Some((version, timer));
    }

    pub fn table(&self) -> &Entity<crate::table::tree_table::TlTreeTable<StartupDelegate>> {
        &self.frame.table
    }

    pub fn body(&self) -> SysBody {
        self.frame.body()
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
            let counts = StartupCounts {
                total: self.last_status.0,
                enabled: self.last_status.1,
                disabled: self.last_status.2,
            };
            self.refresh_status_counts(counts, cx);
        }
    }

    fn refresh_status(&mut self, startup: &StartupSnapshot, cx: &mut Context<Self>) {
        if self.frame.body() == SysBody::Skeleton {
            return;
        }
        self.refresh_status_counts(startup.counts, cx);
    }

    fn refresh_status_counts(&mut self, counts: StartupCounts, cx: &mut Context<Self>) {
        let table = self.frame.table.read(cx);
        let selected = table
            .selection
            .ids()
            .filter(|&row| table.delegate.row_index(row).is_some())
            .count();
        let next = (counts.total, counts.enabled, counts.disabled, selected);
        if next == self.last_status {
            return;
        }
        self.last_status = next;

        let mut total = String::new();
        let mut enabled = String::new();
        let mut disabled = String::new();
        units::count(&mut total, u64::from(counts.total));
        units::count(&mut enabled, u64::from(counts.enabled));
        units::count(&mut disabled, u64::from(counts.disabled));
        let args = [
            ("total", FluentValue::from(total.as_str())),
            ("enabled", FluentValue::from(enabled.as_str())),
            ("disabled", FluentValue::from(disabled.as_str())),
        ];
        self.status_buf.clear();
        self.status_buf
            .push_str(&tr_args(cx, "startup-status-total", &args));
        if selected >= 2 {
            let mut selected_text = String::new();
            units::count(
                &mut selected_text,
                u64::try_from(selected).unwrap_or(u64::MAX),
            );
            let suffix = tr_args(
                cx,
                "startup-status-selected",
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
        let text = tr(cx, "startup-refreshed");
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
}

impl Render for StartupPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.frame.render(None, Box::new(ClearFilter), window, cx))
    }
}

impl ShellPage for StartupPage {
    fn status_text(&self, out: &mut String) {
        StartupPage::status_text(self, out);
    }

    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        StartupPage::set_active(self, active, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        StartupPage::refresh(self, cx);
    }
}
