//! Services tab (UI-SYS-010 to UI-SYS-024).
//!
//! `ServicesPage` puts the tab together: the shared system-table frame with the Services
//! delegate, the filter strip, the status text, F5 refresh, the first-visit loading toast and
//! the handlers that route every Services command to the flows, menus, copy and Properties code.
//! Menu items act on the targets captured when the menu opened; keyboard commands read the
//! selection when they run.
//!
//! Child process rows get the process row menu. Only End Task and End Process run on this tab;
//! they follow the "Disk is in use" window's end path. The other process items do nothing here
//! until tasklens-pdu.58.

pub mod actions;
pub mod copy;
pub mod delegate;
pub mod filter_strip;
pub mod flows;
pub mod menu;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, Subscription, Task, WeakEntity, Window, div, point,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, ActionTarget, Signal};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::health::{EngineCaps, SourceId};
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::ProcFlags;
use tl_model::settings::UiState;
use tl_model::snapshot::{
    ComponentMask, ProcessSnapshot, ServiceBackend, ServiceCounts, Snapshot, StartupType,
};
use tl_model::table::{SysColumnId, SysNodeId, UnitRef};
use tl_model::units;
use tracing::warn;

use self::actions::{
    SERVICE_TABLE_CONTEXT, ServiceFileProperties, ServiceJumpToProcess, ServiceOpenFileLocation,
    ServiceOpenUnitFile, ServiceReload, ServiceResetFailed, ServiceRestart, ServiceSetStartupType,
    ServiceStart, ServiceStop, ServiceViewLogs, ServicesClearAllFilters, ServicesRemoveFilters,
    ServicesResetUnitTypes, ServicesToggleManagerFilter, ServicesToggleStateFilter,
    ServicesToggleUnitType,
};
use self::delegate::{ServicesDelegate, row_key};
use self::filter_strip::{
    ServiceFilterState, ServicesOpenUnitTypes, apply_remove_filters, apply_reset_unit_types,
    apply_toggle_manager, apply_toggle_state, apply_toggle_unit_type, render_strip,
    unit_types_menu,
};
use self::flows::UnitVerb;
use self::menu::{MenuChoice, ServiceMenuTargets, menu_choice, service_menu};
use crate::action_tracker::{GlobalActionTracker, ToastTexts, action_words};
use crate::actions::ClearFilter;
use crate::app::EngineGlobal;
use crate::components::menu::{TlMenu, close_context_menu, open_context_menu_at};
use crate::components::toast::{SERVICES_LOADING_TOAST, ToastKey, ToastKind, ToastQueue};
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::{tr, tr_args};
use crate::pages::processes::actions::{CapturedTargets, CopyDetails, EndProcess, EndTask};
use crate::pages::processes::confirm::{ConfirmResult, confirm};
use crate::pages::processes::menu::{MenuCaps, row_menu as process_row_menu};
use crate::pages::processes::toasts::{refuse, toast_words};
use crate::pages::sys_copy::{CopyTexts, SysCopySource, copy_cell, copy_row, copy_rows_as_table};
use crate::pages::sys_table::{SysActionsEvent, SysBody, SysTableFrame, loading_status_text};
use crate::settings_global::TlSettings;
use crate::table::TreeTableDelegate as _;
use crate::table::actions::{CollapseAll, CopyCell, CopyRow, CopyRowsAsTable, OpenProperties};
use crate::table::tree_table::{TlTreeTable, TreeTableEvent};
use crate::theme::metrics;
use crate::window::main::{FilterChanged, GlobalShellEvents, PauseChanged, ShellPage};
use crate::window::status::StatusChanged;
use crate::windows::disk_in_use::{confirm_kind, record_target};
use crate::windows::service_properties::{ServicePropertiesWindow, ServicePropsTab};

/// The latest a manual refresh waits for a new services version.
pub const REFRESH_TOAST_AFTER: Duration = Duration::from_secs(2);

/// Most Service Properties windows one command opens.
pub const MAX_PROPERTIES_WINDOWS: usize = 10;

/// Unit types menu width at 100% scale (UI-SYS-012).
const UNIT_TYPES_MENU_MIN_W: f32 = 180.;

const STATUS_UNSET: (u32, u32, u32, usize) = (u32::MAX, u32::MAX, u32::MAX, usize::MAX);

/// What the open row menu acts on.
#[derive(Clone)]
enum MenuTargets {
    Unit(ServiceMenuTargets),
    /// Child process rows (UI-SYS-016).
    Process(CapturedTargets),
}

/// The Services tab entity.
pub struct ServicesPage {
    frame: SysTableFrame<ServicesDelegate>,
    strip: ServiceFilterState,
    unit_types_menu_open: bool,
    /// The loading toast was pushed this session (it is pushed once at most).
    loading_toast_shown: bool,
    /// The loading toast is still in the queue.
    loading_toast_up: bool,
    /// Services version at F5 and the 2 s timer.
    refresh_pending: Option<(u64, Task<()>)>,
    /// Total, running, stopped, selected count.
    status_cache: (u32, u32, u32, usize),
    status_buf: String,
    menu_targets: Option<MenuTargets>,
    /// Row keys of the rows the open row menu acts on, in view order, for its copy items.
    menu_rows: Option<SmallVec<[u64; 8]>>,
    focus: FocusHandle,
    window: AnyWindowHandle,
    /// Where the strip was drawn last frame, for the Unit types menu anchor.
    strip_bounds: Rc<Cell<Bounds<Pixels>>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<StatusChanged> for ServicesPage {}

impl ServicesPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (state, strip) = match cx.try_global::<TlSettings>() {
            Some(_) => (
                TlSettings::state(cx).clone(),
                ServiceFilterState::from_settings(&TlSettings::settings(cx).services),
            ),
            None => (UiState::default(), ServiceFilterState::default()),
        };
        let mut delegate = ServicesDelegate::new(&state);
        delegate.set_page(cx.weak_entity());
        delegate.set_window(Some(window.window_handle()));
        let sort = delegate.sort();
        let frame = SysTableFrame::new(
            Tab::Services,
            SERVICE_TABLE_CONTEXT,
            &[SourceId::SystemdSystem, SourceId::SystemdUser],
            delegate,
            sort,
            Some(strip.to_filter()),
            window,
            cx,
        );
        let focus = frame.table.read(cx).focus_handle().clone();
        let table = frame.table.clone();
        let mut subs = Vec::with_capacity(6);
        if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
            subs.push(cx.subscribe(&hub, |this, hub, ev: &SnapshotChanged, cx| {
                let snap = Arc::clone(hub.read(cx).current());
                this.on_snapshot(&snap, ev.mask, cx);
            }));
        }
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|g| g.0.clone()) {
            subs.push(cx.subscribe(&events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Services {
                    this.frame.on_filter_changed(&ev.text, cx);
                }
            }));
            subs.push(cx.subscribe(&events, |this, _, ev: &PauseChanged, cx| {
                if this.frame.set_paused(ev.0, cx) {
                    cx.notify();
                }
            }));
        }
        // A double-click reaches the page as an event; the mouse handler cannot raise
        // OpenProperties by propagating.
        subs.push(cx.subscribe(&table, |this, _, ev: &TreeTableEvent, cx| {
            let TreeTableEvent::Activate { .. } = ev else {
                return;
            };
            let window = this.window;
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(OpenProperties), cx);
                });
            });
        }));
        subs.push(cx.observe(&table, |this, _, cx| this.on_table_changed(cx)));
        let actions = frame.actions.clone();
        subs.push(cx.subscribe(&actions, |this, _, ev: &SysActionsEvent, cx| {
            let SysActionsEvent::PendingChanged { target } = ev;
            this.on_pending_changed(target, cx);
        }));
        Self {
            frame,
            strip,
            unit_types_menu_open: false,
            loading_toast_shown: false,
            loading_toast_up: false,
            refresh_pending: None,
            status_cache: STATUS_UNSET,
            status_buf: loading_status_text(cx).to_string(),
            menu_targets: None,
            menu_rows: None,
            focus,
            window: window.window_handle(),
            strip_bounds: Rc::default(),
            _subs: subs,
        }
    }

    /// Writes the cached status text without allocating in the render path (UI-SYS-022).
    pub fn status_text(&self, out: &mut String) {
        out.push_str(&self.status_buf);
    }

    pub fn table(&self) -> &Entity<TlTreeTable<ServicesDelegate>> {
        &self.frame.table
    }

    pub fn body(&self) -> SysBody {
        self.frame.body()
    }

    pub fn strip(&self) -> &ServiceFilterState {
        &self.strip
    }

    /// The text filter of the last query sent.
    pub fn filter_text(&self) -> &str {
        &self.frame.query.query().filter
    }

    /// Turns Services demand on or off with the tab. The first activation of the session shows
    /// the loading toast while the skeleton is up (UI-SYS-004, SYS-016).
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            self.frame.on_activated(cx);
        } else {
            self.frame.on_deactivated(cx);
        }
        self.frame
            .table
            .update(cx, |table, cx| table.delegate.set_active(active, cx));
        if active && !self.loading_toast_shown {
            self.loading_toast_shown = true;
            if self.frame.body() == SysBody::Skeleton {
                let text = tr(cx, "services-sys004-loading-service-data");
                self.push_toast(ToastKind::Progress, text, SERVICES_LOADING_TOAST, cx);
                self.loading_toast_up = true;
            }
        }
    }

    /// F5: `RefreshNow` for both managers, then "Services refreshed." once, on the first newer
    /// services version or after 2 s, whichever comes first.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.frame.refresh(cx);
        let version = cx
            .try_global::<GlobalHub>()
            .map_or(0, |h| h.0.read(cx).current().services.header.version);
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_TOAST_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh_pending.take().is_some() {
                    this.show_refreshed(cx);
                }
            });
        });
        self.refresh_pending = Some((version, timer));
    }

    /// Forgets the targets captured when the row menu opened.
    pub(super) fn clear_menu_targets(&mut self) {
        self.menu_targets = None;
        self.menu_rows = None;
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, mask: ComponentMask, cx: &mut Context<Self>) {
        let changed = self.frame.on_snapshot(snap, mask, cx);
        if mask.contains(ComponentMask::SERVICES)
            && self
                .refresh_pending
                .as_ref()
                .is_some_and(|(at, _)| snap.services.header.version > *at)
        {
            self.refresh_pending = None;
            self.show_refreshed(cx);
        }
        if self.loading_toast_up && self.frame.body() != SysBody::Skeleton {
            self.loading_toast_up = false;
            self.remove_toast(SERVICES_LOADING_TOAST, cx);
        }
        if !changed {
            return;
        }
        if self.frame.query.accepts(snap.services.query_generation)
            && self.frame.body() != SysBody::Skeleton
        {
            self.refresh_status(snap.services.counts, cx);
        }
        cx.notify();
    }

    /// Forwards the delegate's sort and expansion requests to the query, and recounts the
    /// selection.
    fn on_table_changed(&mut self, cx: &mut Context<Self>) {
        let (sort, expanded) = self.frame.table.update(cx, |table, _| {
            (
                table.delegate.take_sort_request(),
                table.delegate.take_expanded_request(),
            )
        });
        if (sort.is_some() || expanded.is_some())
            && let Some(client) = engine(cx)
        {
            if let Some(sort) = sort {
                self.frame.query.set_sort(sort, &*client);
            }
            if let Some(expanded) = expanded {
                self.frame.query.set_expanded(expanded, &*client);
            }
        }
        if self.frame.body() != SysBody::Skeleton && self.status_cache != STATUS_UNSET {
            let (total, running, stopped, _) = self.status_cache;
            let counts = ServiceCounts {
                total,
                running,
                stopped,
            };
            self.refresh_status(counts, cx);
        }
    }

    /// A service action started or finished on `target`: its Status cell shows or drops the
    /// overlay (UI-SYS-018).
    fn on_pending_changed(&mut self, target: &ActionTarget, cx: &mut Context<Self>) {
        let ActionTarget::Unit(unit) = target else {
            return;
        };
        let tag = self.frame.actions.read(cx).pending_tag(target);
        let unit = unit.clone();
        self.frame.table.update(cx, |table, cx| {
            table.delegate.set_pending(unit, tag);
            cx.notify();
        });
        cx.notify();
    }

    /// "<total> Total | <running> Running | <stopped> Stopped", plus " | <n> selected" when a
    /// row is selected. Emits `StatusChanged` only when one of the four numbers changed.
    fn refresh_status(&mut self, counts: ServiceCounts, cx: &mut Context<Self>) {
        let table = self.frame.table.read(cx);
        let selected = table
            .selection
            .ids()
            .filter(|&row| table.delegate.row_index(row).is_some())
            .count();
        let next = (counts.total, counts.running, counts.stopped, selected);
        if next == self.status_cache {
            return;
        }
        self.status_cache = next;
        self.status_buf.clear();
        write_status(counts, selected, &mut self.status_buf, cx);
        cx.emit(StatusChanged);
    }

    fn show_refreshed(&self, cx: &mut Context<Self>) {
        let text = tr(cx, "services-sys004-services-refreshed");
        let key = ToastKey(cx.entity_id().as_u64());
        self.push_toast(ToastKind::Success, text, key, cx);
    }

    /// A toast in the page's own window: a timer may fire while a Properties window is active.
    fn push_toast(
        &self,
        kind: ToastKind,
        text: SharedString,
        key: ToastKey,
        cx: &mut Context<Self>,
    ) {
        let _ = self.window.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.push(kind, text, Some(key), None, cx));
            }
        });
    }

    fn remove_toast(&self, key: ToastKey, cx: &mut Context<Self>) {
        let _ = self.window.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.remove(key, cx));
            }
        });
    }

    // ---- Targets ----

    /// The selected rows' nodes in view order.
    fn selected_nodes(&self, cx: &App) -> SmallVec<[SysNodeId; 8]> {
        let table = self.frame.table.read(cx);
        let delegate = &table.delegate;
        let mut rows: SmallVec<[usize; 8]> = table
            .selection
            .ids()
            .filter_map(|id| delegate.row_index(id))
            .collect();
        rows.sort_unstable();
        let services = delegate.services();
        rows.into_iter()
            .map(|ix| services.rows[ix].node.clone())
            .collect()
    }

    /// The menu-open unit targets, or the unit rows of the current selection for a keyboard
    /// command.
    fn take_unit_targets(&mut self, cx: &App) -> ServiceMenuTargets {
        if let Some(MenuTargets::Unit(targets)) = self.menu_targets.take() {
            return targets;
        }
        let nodes = self.selected_nodes(cx);
        let services = Arc::clone(self.frame.table.read(cx).delegate.services());
        ServiceMenuTargets::capture(&services, &nodes, None, self.frame.actions.read(cx))
    }

    /// The menu-open child process keys, or the child rows of the current selection.
    fn take_process_keys(&mut self, cx: &App) -> SmallVec<[ProcessKey; 1]> {
        if let Some(MenuTargets::Process(targets)) = self.menu_targets.take() {
            return targets.process_keys();
        }
        self.selected_nodes(cx)
            .iter()
            .filter_map(|node| match node {
                SysNodeId::UnitProcess(_, key) => Some(*key),
                _ => None,
            })
            .collect()
    }

    // ---- Unit commands ----

    fn run_verb(&mut self, verb: UnitVerb, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_unit_targets(cx);
        let actions = self.frame.actions.clone();
        flows::run_unit_verb(verb, &targets.units, &actions, window, cx);
    }

    fn on_start(&mut self, _: &ServiceStart, window: &mut Window, cx: &mut Context<Self>) {
        self.run_verb(UnitVerb::Start, window, cx);
    }

    /// Ctrl+E with only child process rows selected ends those processes (UI-SYS-023).
    fn on_stop(&mut self, _: &ServiceStop, window: &mut Window, cx: &mut Context<Self>) {
        let from_menu = self.menu_targets.is_some();
        let targets = self.take_unit_targets(cx);
        if !targets.units.is_empty() {
            let actions = self.frame.actions.clone();
            flows::run_unit_verb(UnitVerb::Stop, &targets.units, &actions, window, cx);
            return;
        }
        if from_menu {
            return;
        }
        let nodes = self.selected_nodes(cx);
        if nodes.is_empty()
            || !nodes
                .iter()
                .all(|node| matches!(node, SysNodeId::UnitProcess(..)))
        {
            return;
        }
        let keys = self.take_process_keys(cx);
        self.end(&keys, false, window, cx);
    }

    fn on_restart(&mut self, _: &ServiceRestart, window: &mut Window, cx: &mut Context<Self>) {
        self.run_verb(UnitVerb::Restart, window, cx);
    }

    fn on_reload(&mut self, _: &ServiceReload, window: &mut Window, cx: &mut Context<Self>) {
        self.run_verb(UnitVerb::Reload, window, cx);
    }

    fn on_reset_failed(
        &mut self,
        _: &ServiceResetFailed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_verb(UnitVerb::ResetFailed, window, cx);
    }

    fn on_set_startup(
        &mut self,
        a: &ServiceSetStartupType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_unit_targets(cx);
        let units: SmallVec<[(UnitRef, StartupType); 1]> = targets
            .records
            .iter()
            .map(|r| (r.unit.clone(), r.startup))
            .collect();
        let actions = self.frame.actions.clone();
        flows::run_set_startup(a.startup.clone(), &units, &actions, window, cx);
    }

    fn on_file_properties(
        &mut self,
        _: &ServiceFileProperties,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_unit_targets(cx);
        if let Some(record) = targets.records.first() {
            let actions = self.frame.actions.clone();
            flows::run_file_properties(record, &actions, window, cx);
        }
    }

    fn on_open_file_location(
        &mut self,
        _: &ServiceOpenFileLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_unit_targets(cx);
        if let Some(record) = targets.records.first() {
            let actions = self.frame.actions.clone();
            flows::run_open_file_location(record, &actions, window, cx);
        }
    }

    fn on_open_unit_file(
        &mut self,
        _: &ServiceOpenUnitFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_unit_targets(cx);
        if let Some(record) = targets.records.first() {
            let actions = self.frame.actions.clone();
            flows::run_open_unit_file(record, &actions, window, cx);
        }
    }

    fn on_jump_to_process(
        &mut self,
        _: &ServiceJumpToProcess,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_unit_targets(cx);
        let Some(record) = targets.records.first() else {
            return;
        };
        let Some(snap) = cx
            .try_global::<GlobalHub>()
            .map(|h| Arc::clone(h.0.read(cx).current()))
        else {
            return;
        };
        flows::run_jump_to_process(record, &snap, window, cx);
    }

    fn on_view_logs(&mut self, _: &ServiceViewLogs, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_unit_targets(cx);
        if let Some(unit) = targets.units.first() {
            ServicePropertiesWindow::open(unit.clone(), ServicePropsTab::Logs, cx);
        }
    }

    /// Enter, Alt+Enter, Ctrl+I, double-click and the menu's Properties: one window per selected
    /// unit, at most 10. The table toggles a group row itself. Child rows belong to
    /// tasklens-hk6.23.
    fn on_open_properties(&mut self, _: &OpenProperties, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_unit_targets(cx);
        for unit in targets.units.iter().take(MAX_PROPERTIES_WINDOWS) {
            ServicePropertiesWindow::open(unit.clone(), ServicePropsTab::General, cx);
        }
    }

    // ---- Child process rows ----

    fn on_end_task(&mut self, _: &EndTask, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.take_process_keys(cx);
        self.end(&keys, false, window, cx);
    }

    fn on_end_process(&mut self, _: &EndProcess, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.take_process_keys(cx);
        self.end(&keys, true, window, cx);
    }

    /// Ends `keys`: SIGKILL with `kill`, else SIGTERM. Follows `DiskInUseWindow::end`: exited
    /// processes are dropped, refused ones stop the action with a toast, and the R-036
    /// confirmation runs first when one applies.
    fn end(
        &mut self,
        keys: &[ProcessKey],
        kill: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        let Some(snap) = cx
            .try_global::<GlobalHub>()
            .map(|h| Arc::clone(h.0.read(cx).current()))
        else {
            return;
        };
        let live: Vec<_> = keys
            .iter()
            .filter_map(|k| snap.processes.get(*k))
            .filter(|r| !r.flags.contains(ProcFlags::EXITED))
            .map(record_target)
            .collect();
        if live.is_empty() {
            let text = toast_words(cx).exited.clone();
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx));
            }
            return;
        }
        let ask = confirm_kind(&live, kill, cx);
        let targets: SmallVec<[ActionTarget; 4]> = live
            .iter()
            .filter_map(|t| match t.node {
                NodeId::Process(key) => Some(ActionTarget::Process(key)),
                _ => None,
            })
            .collect();
        let captured = CapturedTargets::new(live, None);
        if refuse(&captured, cx) {
            return;
        }
        let kind = ActionKind::Signal(if kill { Signal::Kill } else { Signal::Term });
        let Some(texts) = toast_words(cx).texts_for(&kind, &captured) else {
            return;
        };
        let texts = texts.into_toast_texts(action_words(cx));
        let request = ActionRequest {
            // The tracker gives id 0 its next ActionId.
            id: ActionId(0),
            kind,
            targets: targets.into_iter().collect(),
            privileged: false,
        };
        let Some(ask) = ask else {
            submit_end(request, texts, window, cx);
            return;
        };
        let parent = window.window_handle();
        // The dialog opens modal to this window, which is being updated now.
        cx.defer(move |cx| {
            confirm(ask, parent, cx, move |result, cx| {
                if result == ConfirmResult::Confirmed {
                    let _ = parent.update(cx, |_, window, cx| {
                        submit_end(request, texts, window, cx);
                    });
                }
            });
        });
    }

    // ---- Copy and expansion ----

    fn on_copy_cell(&mut self, a: &CopyCell, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.menu_rows.take();
        let row = match self.menu_targets.take() {
            Some(MenuTargets::Unit(t)) => t
                .units
                .first()
                .map(|u| row_key(&SysNodeId::Unit(u.clone()))),
            Some(MenuTargets::Process(_)) => rows.and_then(|rows| rows.first().copied()),
            None => self.frame.table.read(cx).selection.focus(),
        };
        let Some(row) = row else {
            return;
        };
        let col = a.col;
        self.frame
            .table
            .update(cx, |table, cx| copy_cell(table, row, col, window, cx));
    }

    fn on_copy_row(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_blocks(window, cx);
    }

    /// The menu's "Copy row" and the process menu's "Copy details".
    fn on_copy_details(&mut self, _: &CopyDetails, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_blocks(window, cx);
    }

    /// One block per row: the rows captured when the menu opened, else the selection.
    fn copy_blocks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_targets = None;
        let rows = self.menu_rows.take();
        self.frame.table.update(cx, |table, cx| match rows {
            Some(rows) => copy_row(&MenuRows { table, rows }, window, cx),
            None => copy_row(table, window, cx),
        });
    }

    /// Runs before the table's own TSV handler, so the Services toasts and cell text apply.
    fn on_copy_rows_as_table(
        &mut self,
        _: &CopyRowsAsTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.menu_targets = None;
        let rows = self.menu_rows.take();
        self.frame.table.update(cx, |table, cx| match rows {
            Some(rows) => copy_rows_as_table(&MenuRows { table, rows }, window, cx),
            None => copy_rows_as_table(table, window, cx),
        });
    }

    /// Ctrl+L: every unit collapses and both group rows are expanded (UI-SYS-014).
    fn on_collapse_all(&mut self, _: &CollapseAll, _: &mut Window, cx: &mut Context<Self>) {
        self.frame
            .table
            .update(cx, |table, cx| table.delegate.collapse_units(cx));
    }

    // ---- Filter strip ----

    fn on_toggle_state(
        &mut self,
        a: &ServicesToggleStateFilter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = engine(cx) else {
            return;
        };
        apply_toggle_state(
            &mut self.strip,
            a.state.clone(),
            &mut self.frame.query,
            &*client,
            cx,
        );
        cx.notify();
    }

    fn on_toggle_manager(
        &mut self,
        a: &ServicesToggleManagerFilter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = engine(cx) else {
            return;
        };
        apply_toggle_manager(
            &mut self.strip,
            a.manager.clone(),
            &mut self.frame.query,
            &*client,
        );
        cx.notify();
    }

    fn on_toggle_unit_type(
        &mut self,
        a: &ServicesToggleUnitType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = engine(cx) else {
            return;
        };
        if apply_toggle_unit_type(
            &mut self.strip,
            a.kind.clone(),
            &mut self.frame.query,
            &*client,
            window,
            cx,
        ) {
            cx.notify();
        }
    }

    fn on_reset_unit_types(
        &mut self,
        _: &ServicesResetUnitTypes,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = engine(cx) else {
            return;
        };
        apply_reset_unit_types(&mut self.strip, &mut self.frame.query, &*client, cx);
        cx.notify();
    }

    fn on_remove_filters(
        &mut self,
        _: &ServicesRemoveFilters,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = engine(cx) else {
            return;
        };
        apply_remove_filters(&mut self.strip, &mut self.frame.query, &*client, cx);
        cx.notify();
    }

    /// The "No matches" button: the shell clears the text filter, the strip goes to defaults.
    fn on_clear_all_filters(
        &mut self,
        _: &ServicesClearAllFilters,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.dispatch_action(Box::new(ClearFilter), cx);
        self.on_remove_filters(&ServicesRemoveFilters, window, cx);
    }

    /// Opens the Unit types menu below the strip, under the pointer's x (UI-SYS-012).
    fn on_open_unit_types(
        &mut self,
        _: &ServicesOpenUnitTypes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let strip = self.strip.clone();
        let menu = TlMenu::build(window, cx, self.focus.clone(), move |menu, window, cx| {
            let min_w = gpui_kit::px(UNIT_TYPES_MENU_MIN_W * metrics(cx).ui_scale);
            unit_types_menu(&strip, menu.min_w(min_w), window, cx)
        });
        let bounds = self.strip_bounds.get();
        let x = window
            .mouse_position()
            .x
            .clamp(bounds.origin.x, bounds.origin.x.max(bounds.right()));
        let at = point(x, bounds.bottom());
        cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, _, cx| {
            this.unit_types_menu_open = false;
            cx.notify();
        })
        .detach();
        open_context_menu_at(at, window, cx, menu);
        self.unit_types_menu_open = true;
        cx.notify();
    }

    fn render_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.frame.body() == SysBody::Skeleton;
        let openrc = self.frame.table.read(cx).delegate.backend() == ServiceBackend::OpenRc;
        let strip = render_strip(
            &self.strip,
            disabled,
            self.unit_types_menu_open,
            openrc,
            window,
            cx,
        );
        let bounds = Rc::clone(&self.strip_bounds);
        div()
            .w_full()
            .flex_none()
            .on_children_prepainted(move |children, _, _| {
                if let Some(b) = children.first() {
                    bounds.set(*b);
                }
            })
            .child(strip)
            .into_any_element()
    }
}

/// The engine client, absent only in tests that install none.
fn engine(cx: &App) -> Option<Arc<dyn EngineClient>> {
    cx.try_global::<EngineGlobal>().map(|g| Arc::clone(&g.0))
}

/// Writes the UI-SYS-022 status text for `counts` and `selected` rows to `out`.
fn write_status(counts: ServiceCounts, selected: usize, out: &mut String, cx: &App) {
    let mut total = String::new();
    let mut running = String::new();
    let mut stopped = String::new();
    units::count(&mut total, u64::from(counts.total));
    units::count(&mut running, u64::from(counts.running));
    units::count(&mut stopped, u64::from(counts.stopped));
    let args = [
        ("total", FluentValue::from(total.as_str())),
        ("running", FluentValue::from(running.as_str())),
        ("stopped", FluentValue::from(stopped.as_str())),
    ];
    out.push_str(&tr_args(cx, "services-sys022-status-total", &args));
    if selected > 0 {
        let mut n = String::new();
        units::count(&mut n, u64::try_from(selected).unwrap_or(u64::MAX));
        let suffix = tr_args(
            cx,
            "services-sys022-status-selected",
            &[("selected", FluentValue::from(n.as_str()))],
        );
        out.push_str(" | ");
        out.push_str(&suffix);
    }
}

/// Sends an end request through the app's `ActionTracker`; its toasts go to `window`.
fn submit_end(request: ActionRequest, texts: ToastTexts, window: &mut Window, cx: &mut App) {
    let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
        warn!(target: "tasklens::ui", "no action tracker; the end action was not sent");
        return;
    };
    let Some(queue) = ToastQueue::of_window(window, cx) else {
        warn!(target: "tasklens::ui", "no toast stack; the end action was not sent");
        return;
    };
    let queue = queue.downgrade();
    tracker.update(cx, |t, cx| {
        let _ = t.track_or_toast(request, queue, texts, cx);
    });
}

/// The child process targets of `nodes` that `processes` still lists, in order.
fn child_targets(processes: &ProcessSnapshot, nodes: &[SysNodeId]) -> CapturedTargets {
    let targets = nodes.iter().filter_map(|node| match node {
        SysNodeId::UnitProcess(_, key) => processes.get(*key).map(record_target),
        _ => None,
    });
    CapturedTargets::new(targets, None)
}

/// The row menu for `nodes`, called by the delegate while the table is being updated. A unit in
/// the selection gets the unit menu; only child rows get the process row menu; group rows get
/// none (UI-SYS-016, UI-SYS-017). The captured targets and row keys are stored on the page.
pub(super) fn row_menu(
    page: &WeakEntity<ServicesPage>,
    delegate: &ServicesDelegate,
    nodes: &[SysNodeId],
    clicked_col: Option<SysColumnId>,
    menu: TlMenu,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let Some(page) = page.upgrade() else {
        return menu;
    };
    let rows: SmallVec<[u64; 8]> = nodes
        .iter()
        .filter(|node| !matches!(node, SysNodeId::ServiceGroup(_)))
        .map(row_key)
        .collect();
    match menu_choice(nodes) {
        MenuChoice::Unit => {
            let actions = page.read(cx).frame.actions.clone();
            let targets = ServiceMenuTargets::capture(
                delegate.services(),
                nodes,
                clicked_col,
                actions.read(cx),
            );
            if targets.units.is_empty() {
                return no_menu(&page, menu, window, cx);
            }
            let stored = MenuTargets::Unit(targets.clone());
            page.update(cx, |page, _| {
                page.menu_targets = Some(stored);
                page.menu_rows = Some(rows);
            });
            service_menu(menu, &targets, delegate.backend(), window, cx)
        }
        MenuChoice::Process => {
            let targets = child_targets(delegate.processes(), nodes);
            if targets.items.is_empty() {
                return no_menu(&page, menu, window, cx);
            }
            let engine_caps = cx
                .try_global::<GlobalHub>()
                .map(|h| h.0.read(cx).current().health.caps)
                .unwrap_or_default();
            let caps = MenuCaps {
                gdb: engine_caps.contains(EngineCaps::GDB),
                file_manager1: engine_caps.contains(EngineCaps::FILE_MANAGER1),
                ..MenuCaps::default()
            };
            let stored = MenuTargets::Process(targets.clone());
            page.update(cx, |page, _| {
                page.menu_targets = Some(stored);
                page.menu_rows = Some(rows);
            });
            process_row_menu(menu, &targets, &caps, window, cx)
        }
        MenuChoice::None => no_menu(&page, menu, window, cx),
    }
}

/// No menu for these rows (a group row, UI-SYS-023). The table opens a popup for every
/// right-click, so it is closed again before the next frame and the table gets the keys back.
fn no_menu(page: &Entity<ServicesPage>, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
    let focus = page.read(cx).focus.clone();
    window.defer(cx, move |window, cx| {
        close_context_menu(window, cx);
        focus.focus(window, cx);
    });
    menu
}

/// The table's copy text for the rows captured when the row menu opened, in view order. Rows
/// that left the view since then are skipped.
struct MenuRows<'a> {
    table: &'a TlTreeTable<ServicesDelegate>,
    rows: SmallVec<[u64; 8]>,
}

impl SysCopySource for MenuRows<'_> {
    type RowId = u64;

    fn copy_rows(&self) -> SmallVec<[u64; 8]> {
        let delegate = &self.table.delegate;
        let mut rows: SmallVec<[(usize, u64); 8]> = self
            .rows
            .iter()
            .filter_map(|&id| Some((delegate.row_index(id)?, id)))
            .collect();
        rows.sort_unstable_by_key(|&(ix, _)| ix);
        rows.into_iter().map(|(_, id)| id).collect()
    }

    fn write_block(&self, row: u64, out: &mut String) {
        self.table.write_block(row, out);
    }

    fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]> {
        SysCopySource::visible_columns(self.table)
    }

    fn write_cell(&self, row: u64, col: ColumnId, out: &mut String) {
        self.table.write_cell(row, col, out);
    }

    fn copy_texts(&self) -> &CopyTexts {
        self.table.copy_texts()
    }
}

impl Render for ServicesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strip = self.render_strip(window, cx);
        div()
            .size_full()
            .on_action(cx.listener(Self::on_start))
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::on_restart))
            .on_action(cx.listener(Self::on_reload))
            .on_action(cx.listener(Self::on_reset_failed))
            .on_action(cx.listener(Self::on_set_startup))
            .on_action(cx.listener(Self::on_file_properties))
            .on_action(cx.listener(Self::on_open_file_location))
            .on_action(cx.listener(Self::on_open_unit_file))
            .on_action(cx.listener(Self::on_jump_to_process))
            .on_action(cx.listener(Self::on_view_logs))
            .on_action(cx.listener(Self::on_open_properties))
            .on_action(cx.listener(Self::on_end_task))
            .on_action(cx.listener(Self::on_end_process))
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_row))
            .on_action(cx.listener(Self::on_copy_details))
            .capture_action(cx.listener(Self::on_copy_rows_as_table))
            .on_action(cx.listener(Self::on_collapse_all))
            .on_action(cx.listener(Self::on_toggle_state))
            .on_action(cx.listener(Self::on_toggle_manager))
            .on_action(cx.listener(Self::on_toggle_unit_type))
            .on_action(cx.listener(Self::on_reset_unit_types))
            .on_action(cx.listener(Self::on_remove_filters))
            .on_action(cx.listener(Self::on_clear_all_filters))
            .on_action(cx.listener(Self::on_open_unit_types))
            .child(
                self.frame
                    .render(Some(strip), Box::new(ServicesClearAllFilters), window, cx),
            )
    }
}

impl ShellPage for ServicesPage {
    fn status_text(&self, out: &mut String) {
        ServicesPage::status_text(self, out);
    }

    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        ServicesPage::set_active(self, active, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        ServicesPage::refresh(self, cx);
    }
}
