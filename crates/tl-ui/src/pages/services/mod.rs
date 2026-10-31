//! Services tab (UI-SYS-010 to UI-SYS-024).
//!
//! `ServicesPage` puts the tab together: the shared system-table frame with the Services
//! delegate, the filter strip, the status text, F5 refresh, the first-visit loading toast and
//! the handlers that route every Services command to the flows, menus, copy and Properties code.
//! Menu items act on the targets captured when the menu opened; keyboard commands read the
//! selection when they run.
//!
//! Child process rows get the process row menu (UI-SYS-016). Its process commands, and Ctrl+E on
//! child rows, run through the shared entry point `run_process_action`, so they get the Processes
//! tab's refusals, confirmations and toasts. Properties opens Process Properties.

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
    Action, AnyElement, AnyWindowHandle, App, Bounds, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, Subscription, Task, WeakEntity, Window, div, point,
};
use smallvec::SmallVec;
use tl_model::actions::ActionTarget;
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::health::{EngineCaps, SourceId};
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::settings::UiState;
use tl_model::snapshot::{
    ComponentMask, ProcessSnapshot, ServiceBackend, ServiceCounts, Snapshot, StartupType,
};
use tl_model::table::{SysColumnId, SysNodeId, UnitRef};
use tl_model::units;

use self::actions::{
    JumpOrigin, SERVICE_TABLE_CONTEXT, ServiceFileProperties, ServiceJumpToProcess,
    ServiceOpenFileLocation, ServiceOpenUnitFile, ServiceReload, ServiceResetFailed,
    ServiceRestart, ServiceSetStartupType, ServiceStart, ServiceStop, ServiceViewLogs,
    ServicesClearAllFilters, ServicesRemoveFilters, ServicesResetUnitTypes, ServicesShowHidden,
    ServicesToggleManagerFilter, ServicesToggleStateFilter, ServicesToggleUnitType,
};
use self::delegate::{ServicesDelegate, row_key};
use self::filter_strip::{
    ServiceFilterState, ServicesOpenUnitTypes, apply_remove_filters, apply_reset_unit_types,
    apply_reveal, apply_toggle_manager, apply_toggle_state, apply_toggle_unit_type, render_strip,
    unit_types_menu,
};
use self::flows::UnitVerb;
use self::menu::{MenuChoice, ServiceMenuTargets, menu_choice, service_menu};
use crate::actions::ClearFilter;
use crate::components::menu::{TlMenu, close_context_menu, open_context_menu_at};
use crate::components::toast::{
    SERVICES_LOADING_TOAST, ToastAction, ToastKey, ToastKind, ToastQueue,
};
use crate::host::HostContext;
use crate::hub::SnapshotChanged;
use crate::i18n::{tr, tr_args};
use crate::pages::minigraph::actions::{AddAppGraph, AddMinigraph, AddNameGraph};
use crate::pages::processes::actions::{
    CapturedTargets, CopyAllMetrics, CopyDetails, CreateDump, EndProcess, EndProcessTree, EndTask,
    FileProperties, GoToParent, OpenFileLocation, OpenVirusTotal, RestartUnit, ResumeProcess,
    SetIoPriorityClass, SetPriority, ShowWindow, SuspendProcess,
};
use crate::pages::processes::menu::{MenuCaps, row_menu as process_row_menu};
use crate::pages::processes::record_target;
use crate::pages::processes::run::run_process_action;
use crate::pages::sys_copy::{CopyTexts, SysCopySource, copy_cell, copy_row, copy_rows_as_table};
use crate::pages::sys_table::{SysActionsEvent, SysBody, SysTableFrame, loading_status_text};
use crate::settings_global::TlSettings;
use crate::table::TreeTableDelegate as _;
use crate::table::actions::{CollapseAll, CopyCell, CopyRow, CopyRowsAsTable, OpenProperties};
use crate::table::tree_table::{TlTreeTable, TreeTableEvent};
use crate::theme::metrics;
use crate::util::clipboard_write;
use crate::window::main::{FilterChanged, PauseChanged, ShellPage};
use crate::window::registry::{CapClass, GlobalRegistry, WindowKey};
use crate::window::status::StatusChanged;
use crate::windows::actions::{OpenCustomPriority, SendSignal, SetAffinity, SignalOpenRealtime};
use crate::windows::props::open_process_properties;
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

/// A Go to service jump waiting for the view of the last query sent.
struct PendingUnitJump {
    unit: UnitRef,
    /// Show already cleared the text filter: a missing row is not found.
    after_show: bool,
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
    /// The jump waiting for its view.
    jump: Option<PendingUnitJump>,
    /// The jump the "not found" toast's Show button continues.
    jump_hidden: Option<PendingUnitJump>,
    focus: FocusHandle,
    window: AnyWindowHandle,
    /// Where the strip was drawn last frame, for the Unit types menu anchor.
    strip_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The host this page shows. Set when it opens; never changes.
    host: HostContext,
    _subs: Vec<Subscription>,
}

impl EventEmitter<StatusChanged> for ServicesPage {}

impl ServicesPage {
    pub fn new(host: &HostContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (state, strip) = match cx.try_global::<TlSettings>() {
            Some(_) => (
                TlSettings::state(cx).clone(),
                ServiceFilterState::from_settings(&TlSettings::settings(cx).services),
            ),
            None => (UiState::default(), ServiceFilterState::default()),
        };
        let mut delegate = ServicesDelegate::new(host, &state);
        delegate.set_page(cx.weak_entity());
        delegate.set_window(Some(window.window_handle()));
        let sort = delegate.sort();
        // The group rows draw open from the saved state; the engine lists their units only for
        // groups in the query's expanded set (UI-SYS-004).
        let expanded: Arc<[SysNodeId]> = delegate.expanded().iter().cloned().collect();
        let mut frame = SysTableFrame::new(
            host,
            Tab::Services,
            SERVICE_TABLE_CONTEXT,
            &[SourceId::SystemdSystem, SourceId::SystemdUser],
            delegate,
            sort,
            Some(strip.to_filter()),
            window,
            cx,
        );
        frame.query.seed_expanded(expanded);
        let focus = frame.table.read(cx).focus_handle().clone();
        let table = frame.table.clone();
        let mut subs = Vec::with_capacity(6);
        if let Some(hub) = host.hub() {
            subs.push(cx.subscribe(hub, |this, hub, ev: &SnapshotChanged, cx| {
                let snap = Arc::clone(hub.read(cx).current());
                this.on_snapshot(&snap, ev.mask, cx);
            }));
        }
        if let Some(events) = host.events() {
            subs.push(cx.subscribe(events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Services {
                    this.frame.on_filter_changed(&ev.text, cx);
                }
            }));
            subs.push(cx.subscribe(events, |this, _, ev: &PauseChanged, cx| {
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
        register_show_hidden(cx);
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
            jump: None,
            jump_hidden: None,
            focus,
            window: window.window_handle(),
            strip_bounds: Rc::default(),
            host: host.clone(),
            _subs: subs,
        }
    }

    /// The host's engine client, absent only in tests that install none.
    fn engine(&self) -> Option<Arc<dyn EngineClient>> {
        self.host.client().cloned()
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
        let version = self
            .host
            .hub()
            .map_or(0, |h| h.read(cx).current().services.header.version);
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

    /// Go to service (UI-SYS-021, SYS-010): called after the main window made Services the
    /// active tab. Every origin is another tab, so "Switched to Services tab" always shows.
    ///
    /// Removes only the strip filters that hide the unit, expands its manager group with one
    /// query and waits for that view. The view decides whether the text filter hides the row
    /// (the engine matches more columns than id, description and path), so the page asks it
    /// instead of predicting. Toasts are deferred: the caller may be inside the window's update.
    pub fn select_unit(
        &mut self,
        unit: UnitRef,
        _origin: JumpOrigin,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.jump = None;
        self.jump_hidden = None;
        let hub = self.host.hub().expect("Go to service needs the host's hub");
        let snapshot = Arc::clone(hub.read(cx).current());
        let Some(record) = snapshot.services.units.iter().find(|u| u.unit == unit) else {
            let text = tr(cx, "services-sys021-service-not-found");
            self.defer_toast(ToastKind::Warning, text, None, cx);
            return;
        };
        let text = tr(cx, "services-sys021-switched-to-services-tab");
        self.defer_toast(ToastKind::Info, text, None, cx);
        if let Some(client) = self.engine() {
            apply_reveal(&mut self.strip, record, &mut self.frame.query, &*client, cx);
            let expanded = self.frame.table.update(cx, |table, cx| {
                table.delegate.expand_group(unit.manager, cx)
            });
            self.frame.query.set_expanded(expanded, &*client);
        }
        self.jump = Some(PendingUnitJump {
            unit,
            after_show: false,
        });
        cx.notify();
    }

    /// Runs after each services snapshot: once the view of the last query sent is installed,
    /// the jump selects the unit or explains why it cannot.
    fn resolve_jump(&mut self, cx: &mut Context<Self>) {
        if self.jump.is_none() {
            return;
        }
        let installed = self
            .frame
            .table
            .read(cx)
            .delegate
            .services()
            .query_generation;
        if installed != self.frame.query.last_sent() {
            return;
        }
        let Some(job) = self.jump.take() else {
            return;
        };
        let id = row_key(&SysNodeId::Unit(job.unit.clone()));
        if self
            .frame
            .table
            .update(cx, |table, cx| table.jump_to(id, cx))
        {
            let focus = self.focus.clone();
            let window = self.window;
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| window.focus(&focus, cx));
            });
            let text = tr(cx, "services-sys021-switched-to-service");
            self.defer_toast(ToastKind::Info, text, None, cx);
            return;
        }
        let text = tr(cx, "services-sys021-service-not-found");
        if !job.after_show && !self.filter_text().is_empty() {
            let show = ToastAction {
                label: tr(cx, "services-sys021-show"),
                action: ServicesShowHidden.boxed_clone(),
            };
            self.defer_toast(ToastKind::Warning, text, Some(show), cx);
            self.jump_hidden = Some(job);
        } else {
            self.defer_toast(ToastKind::Warning, text, None, cx);
        }
    }

    /// The Show button: clears the text filter (the shell box and the query), then waits for the
    /// view and selects the unit.
    fn show_hidden(&mut self, cx: &mut Context<Self>) {
        let Some(mut job) = self.jump_hidden.take() else {
            return;
        };
        // Deferred: the button's click is still inside the window's update.
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                window.dispatch_action(ClearFilter.boxed_clone(), cx);
            });
        });
        if let Some(client) = self.engine() {
            self.frame.query.set_filter("", &*client);
        }
        job.after_show = true;
        self.jump = Some(job);
    }

    /// A toast in the page's window, pushed after the current update ends.
    fn defer_toast(
        &self,
        kind: ToastKind,
        text: SharedString,
        action: Option<ToastAction>,
        cx: &mut Context<Self>,
    ) {
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(kind, text, None, action, cx));
                }
            });
        });
    }

    /// Forgets the targets captured when the row menu opened.
    pub(super) fn clear_menu_targets(&mut self) {
        self.menu_targets = None;
        self.menu_rows = None;
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, mask: ComponentMask, cx: &mut Context<Self>) {
        if !mask.contains(ComponentMask::SERVICES) {
            if mask.contains(ComponentMask::PROCESSES) {
                self.on_processes(snap, cx);
            }
            return;
        }
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
        self.resolve_jump(cx);
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

    /// A process update with no services update. Rows rebuild only on a new services version.
    /// While every unit row is collapsed nothing changes and nothing renders; with a unit
    /// expanded only the table redraws, so its child rows show the new process records.
    fn on_processes(&mut self, snap: &Arc<Snapshot>, cx: &mut Context<Self>) {
        if let Some(client) = self.engine() {
            self.frame.query.on_snapshot(&*client);
        }
        if self.frame.is_paused() {
            return;
        }
        self.frame.table.update(cx, |table, cx| {
            if table.delegate.install_processes(snap) {
                table.on_new_version(cx);
            }
        });
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
            && let Some(client) = self.engine()
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

    /// The menu-open child process targets, or the child rows of the current selection.
    fn take_process_targets(&mut self, cx: &App) -> CapturedTargets {
        if let Some(MenuTargets::Process(targets)) = self.menu_targets.take() {
            return targets;
        }
        let nodes = self.selected_nodes(cx);
        child_targets(self.frame.table.read(cx).delegate.processes(), &nodes)
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
        let targets = self.take_process_targets(cx);
        run_process_action(&self.host, &EndTask, targets, window, cx);
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
        let Some(snap) = self.host.hub().map(|h| Arc::clone(h.read(cx).current())) else {
            return;
        };
        flows::run_jump_to_process(record, &snap, window, cx);
    }

    fn on_view_logs(&mut self, _: &ServiceViewLogs, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_unit_targets(cx);
        if let Some(unit) = targets.units.first() {
            ServicePropertiesWindow::open(&self.host, unit.clone(), ServicePropsTab::Logs, cx);
        }
    }

    /// Enter, Alt+Enter, Ctrl+I, double-click and the menu's Properties: one Service Properties
    /// window per selected unit (at most 10) and one Process Properties window per selected
    /// child row. The table toggles a group row itself. Process Properties owns its cap of 8
    /// and its toasts; one cap toast shows per command.
    fn on_open_properties(&mut self, _: &OpenProperties, _: &mut Window, cx: &mut Context<Self>) {
        let (units, keys): (Vec<UnitRef>, SmallVec<[ProcessKey; 1]>) =
            match self.menu_targets.take() {
                Some(MenuTargets::Unit(targets)) => {
                    (targets.units.iter().cloned().collect(), SmallVec::new())
                }
                Some(MenuTargets::Process(targets)) => (Vec::new(), targets.process_keys()),
                None => {
                    let nodes = self.selected_nodes(cx);
                    let services = Arc::clone(self.frame.table.read(cx).delegate.services());
                    let targets = ServiceMenuTargets::capture(
                        &services,
                        &nodes,
                        None,
                        self.frame.actions.read(cx),
                    );
                    let keys = nodes
                        .iter()
                        .filter_map(|node| match node {
                            SysNodeId::UnitProcess(_, key) => Some(*key),
                            _ => None,
                        })
                        .collect();
                    (targets.units.iter().cloned().collect(), keys)
                }
            };
        for unit in units.into_iter().take(MAX_PROPERTIES_WINDOWS) {
            ServicePropertiesWindow::open(&self.host, unit, ServicePropsTab::General, cx);
        }
        let mut cap_notified = false;
        for key in keys {
            if should_attempt_properties(
                &WindowKey::Properties(key).on_host(self.host.remote_name()),
                CapClass::ProcessProps,
                &mut cap_notified,
                cx,
            ) {
                open_process_properties(&self.host, key, cx);
            }
        }
    }

    // ---- Child process rows ----

    /// A process row command on child rows: the menu-open targets, else the selected child rows.
    fn on_process_action<A: Action>(&mut self, a: &A, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_process_targets(cx);
        run_process_action(&self.host, a, targets, window, cx);
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

    /// The process menu's "Copy all metrics" on child rows (PROC-070): the Processes tab's text
    /// and toasts, for the menu-open targets, else the selected child rows.
    fn on_copy_all_metrics(
        &mut self,
        _: &CopyAllMetrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_rows = None;
        let keys: Vec<ProcessKey> = self
            .take_process_targets(cx)
            .items
            .iter()
            .filter_map(|t| match t.node {
                NodeId::Process(key) => Some(key),
                _ => None,
            })
            .collect();
        let (text, rows) = self.frame.table.read(cx).delegate.copy_all_metrics(&keys);
        if rows == 0 || clipboard_write(cx, text).is_err() {
            return;
        }
        let key = if rows > 1 {
            "processes-toast-many-metrics-copied"
        } else {
            "processes-toast-metrics-copied"
        };
        let text = tr(cx, key);
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
        }
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
        let Some(client) = self.engine() else {
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
        let Some(client) = self.engine() else {
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
        let Some(client) = self.engine() else {
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
        let Some(client) = self.engine() else {
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
        let Some(client) = self.engine() else {
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

/// Installs the `ServicesShowHidden` handler for `page`. The toast button dispatches from the
/// toast layer, outside the page's element tree, so the handler is app-level.
fn register_show_hidden(cx: &mut Context<ServicesPage>) {
    let page = cx.weak_entity();
    // `App::on_action`: `Context::on_action` is the element-level form.
    let app: &mut App = cx;
    app.on_action(move |_: &ServicesShowHidden, cx| {
        let _ = page.update(cx, |p, cx| p.show_hidden(cx));
    });
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
            let host = page.read(cx).host.clone();
            service_menu(menu, &targets, delegate.backend(), &host, window, cx)
        }
        MenuChoice::Process => {
            let targets = child_targets(delegate.processes(), nodes);
            if targets.items.is_empty() {
                return no_menu(&page, menu, window, cx);
            }
            let engine_caps = page
                .read(cx)
                .host
                .hub()
                .map(|h| h.read(cx).current().health.caps)
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
            let host = page.read(cx).host.clone();
            process_row_menu(&host, menu, &targets, &caps, window, cx)
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
            .on_action(cx.listener(Self::on_process_action::<EndTask>))
            .on_action(cx.listener(Self::on_process_action::<EndProcess>))
            .on_action(cx.listener(Self::on_process_action::<EndProcessTree>))
            .on_action(cx.listener(Self::on_process_action::<SuspendProcess>))
            .on_action(cx.listener(Self::on_process_action::<ResumeProcess>))
            .on_action(cx.listener(Self::on_process_action::<SetPriority>))
            .on_action(cx.listener(Self::on_process_action::<SetIoPriorityClass>))
            .on_action(cx.listener(Self::on_process_action::<OpenCustomPriority>))
            .on_action(cx.listener(Self::on_process_action::<SetAffinity>))
            .on_action(cx.listener(Self::on_process_action::<SendSignal>))
            .on_action(cx.listener(Self::on_process_action::<SignalOpenRealtime>))
            .on_action(cx.listener(Self::on_process_action::<GoToParent>))
            .on_action(cx.listener(Self::on_process_action::<ShowWindow>))
            .on_action(cx.listener(Self::on_process_action::<OpenFileLocation>))
            .on_action(cx.listener(Self::on_process_action::<FileProperties>))
            .on_action(cx.listener(Self::on_process_action::<OpenVirusTotal>))
            .on_action(cx.listener(Self::on_process_action::<CreateDump>))
            .on_action(cx.listener(Self::on_process_action::<RestartUnit>))
            .on_action(cx.listener(Self::on_process_action::<AddMinigraph>))
            .on_action(cx.listener(Self::on_process_action::<AddNameGraph>))
            .on_action(cx.listener(Self::on_process_action::<AddAppGraph>))
            .on_action(cx.listener(Self::on_copy_all_metrics))
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

/// Lets the first over-cap open call show its toast, then suppresses the same cap toast for the
/// rest of this action. An already-open key still gets focused when the class is full.
fn should_attempt_properties(
    key: &WindowKey,
    class: CapClass,
    cap_notified: &mut bool,
    cx: &App,
) -> bool {
    let Some(registry) = cx.try_global::<GlobalRegistry>() else {
        return true;
    };
    let registry = registry.0.read(cx);
    if registry.get(key).is_some() || registry.count(class) < class.cap() {
        return true;
    }
    if *cap_notified {
        false
    } else {
        *cap_notified = true;
        true
    }
}
