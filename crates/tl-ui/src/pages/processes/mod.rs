//! Processes tab: ProcessesPage, the process table delegate, its cells, menus, confirmations and toasts (docs/ui/processes.md).
//!
//! `ProcessesPage` owns the process table (ADR-009). It subscribes to the hub in `new`, hands
//! each snapshot whose drawn parts changed to the delegate and notifies the table only when the
//! view is the one last asked for. The table is a cached view, so a page render does not redraw
//! it, and nothing here notifies inside `render`.

pub mod actions;
pub mod cells;
pub mod confirm;
pub mod delegate;
pub mod filter;
pub mod header_totals;
pub mod history;
pub mod jump;
pub mod menu;
pub mod name_cell;
pub mod pick;
pub mod run;
pub mod status;
pub mod table;
pub mod toasts;
pub mod toolbar;
pub mod tooltip;

use std::sync::Arc;

use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, UniformListScrollHandle, Window, div,
};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionTarget, Signal};
use tl_model::client::EngineClient;
use tl_model::command::{Command, KWinCommand};
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{ProcessSnapshot, ServicesSnapshot, Snapshot, WindowEntry};
use tl_model::table::{Manager, UnitRef};
use tl_model::validity::Validity;
use tracing::warn;

use self::actions::{
    ActivateWindow, CapturedTarget, CapturedTargets, CopyAllMetrics, CopyDetails, CreateDump,
    EndProcess, EndProcessTree, EndTask, FileProperties, FreezeApp, GoToParent, KillApp,
    MAX_TARGETS, OpenFileLocation, OpenVirusTotal, RestartUnit, ResumeProcess, SetIoPriorityClass,
    SetPriority, ShowWindow, SuspendProcess, ThawApp, ToggleGroupRow,
};
use self::cells::{DELETED_SUFFIX, copy_all_metrics, copy_cell, copy_details, process_column};
use self::confirm::{ConfirmKind, ProtectedAction};
use self::delegate::ProcessDelegate;
use self::filter::FilterBinding;
use self::history::HistoryPage;
use self::run::{RunOptions, run_process_action_with};
use crate::components::toast::ToastKind;
use crate::demand::GlobalDemand;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr};
use crate::pages::minigraph::actions::{AddAppGraph, AddMinigraph, AddNameGraph};
use crate::pages::minigraph::panel::MinigraphPanel;
use crate::settings_global::{Section, TlSettings};
use crate::table::actions::{CollapseAll, CopyCell, OpenProperties};
use crate::table::tree_table::{RowKind, TlTreeTable, TreeTableDelegate as _, TreeTableEvent};
use crate::theme::{UiMetrics, bump_style_version, current, metrics};
use crate::util::clipboard_write;
use crate::window::main::{GlobalShellEvents, PauseChanged, ShellPage};
use crate::window::registry::{CapClass, GlobalRegistry, WindowKey};
use crate::window::status::{StatusAction, StatusChanged};
use crate::windows::actions::{OpenCustomPriority, SendSignal, SetAffinity, SignalOpenRealtime};
use crate::windows::signal::{OtherUserTarget, SignalRefusal, TargetNames};
use status::{StatusCounts, write_actions, write_status};

/// Key context of the page div. The table inside it has `Table`.
pub const PAGE_CONTEXT: &str = "ProcessTable";

/// The ProcessTable keys (PROC-049, table (b), R-045): Delete and Ctrl+E run End Task,
/// Shift+Delete and Ctrl+X run End Process, Ctrl+C runs Copy details in place of the table's
/// Copy row (PROC-070). They dispatch the same handlers as the row menu,
/// with the same confirmations and refusals. Bound before any menu is built, so the menu shows
/// them as hints. The later binding of an action wins the hint, so Delete and Shift+Delete
/// come last.
pub fn bind_keys_processes(cx: &mut App) {
    let ctx = Some(PAGE_CONTEXT);
    cx.bind_keys([
        // The table's own `Table` context is deeper and binds Ctrl+C to CopyRow. This binding
        // matches at that depth and, bound later, wins.
        KeyBinding::new("ctrl-c", CopyDetails, Some("ProcessTable > Table")),
        KeyBinding::new("ctrl-e", EndTask, ctx),
        KeyBinding::new("delete", EndTask, ctx),
        KeyBinding::new("ctrl-x", EndProcess, ctx),
        KeyBinding::new("shift-delete", EndProcess, ctx),
    ]);
}

/// The snapshot parts the table draws. Settings changes come through `TlSettings`.
const DRAWN: ComponentMask = ComponentMask::PROCESSES
    .union(ComponentMask::TABLE)
    .union(ComponentMask::WINDOWS)
    .union(ComponentMask::DEVICES);

/// The settings sections the page follows, in this order: `processes`, `appearance`, `units`,
/// `tables`.
const SECTIONS: [Section; 4] = [
    Section::Processes,
    Section::Appearance,
    Section::Units,
    Section::Tables,
];

/// The entity behind the Processes tab. Built on the first visit and kept (ADR-009).
pub struct ProcessesPage {
    table: Entity<TlTreeTable<ProcessDelegate>>,
    /// Created in `new`, never in render.
    _hub_sub: Subscription,
    /// The table body's focus handle.
    focus: FocusHandle,
    client: Arc<dyn EngineClient>,
    /// Settings, metrics and strings observers, the table's copy events, Ctrl-pause and the
    /// table observer for the status chip.
    _subs: SmallVec<[Subscription; 8]>,
    /// `TlSettings` versions of [`SECTIONS`] last applied.
    section_versions: [u64; 4],
    style_version: u64,
    strings_version: u64,
    /// The targets the row menu captured when it opened (E22-A2). An End handler takes them,
    /// else it captures the selection itself.
    menu_targets: Option<CapturedTargets>,
    /// The jump target's pending jump (E22-A3).
    jump: jump::JumpState,
    /// The shell filter box's events for this tab (UI-PRC-016).
    filter: FilterBinding,
    /// Ctrl-pause (UI-PRC-017): the snapshot the table froze on. Hub changes are ignored while
    /// it is set; the hub keeps the latest, so at most one extra snapshot is held (ADR-005).
    paused: Option<Arc<Snapshot>>,
    /// The minigraph cards over the table (E26-004); `None` without a demand aggregator.
    minigraphs: Option<Entity<MinigraphPanel>>,
    /// What the status chip text was last built from (UI-PRC-018).
    status_counts: StatusCounts,
    /// The status chip text, rebuilt only when `status_counts` or the strings change.
    status: String,
    /// The clickable chip parts: the restricted link and the Administrator mode badge.
    status_actions: SmallVec<[StatusAction; 2]>,
    /// History mode (UI-PRC-030): the page shown in place of the live table. Created when
    /// `history_mode` turns on, dropped when it turns off. The live table is kept.
    history: Option<Entity<HistoryPage>>,
    /// The history table's focus handle, while `history` is set.
    history_focus: Option<FocusHandle>,
    /// Whether the Processes tab is the shown one.
    active: bool,
    /// `TlSettings::state_version` last applied to the mode.
    state_version: u64,
}

impl EventEmitter<StatusChanged> for ProcessesPage {}

impl ProcessesPage {
    /// Builds the table over the hub's current snapshot, sends the first table query
    /// (generation 1) and subscribes to the hub. Needs `GlobalHub`.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<GlobalHub>().0.clone();
        let (snapshot, client) = {
            let hub = hub.read(cx);
            (Arc::clone(hub.current()), Arc::clone(hub.client()))
        };
        let (settings, state) = if cx.has_global::<TlSettings>() {
            (
                TlSettings::settings(cx).clone(),
                TlSettings::state(cx).clone(),
            )
        } else {
            (Settings::default(), UiState::default())
        };
        let mut delegate = ProcessDelegate::new(snapshot, &settings, &state, window, cx);
        delegate.set_window(Some(window.window_handle()));
        delegate.set_page(cx.weak_entity());
        delegate.query_mut().bump_and_send(&*client);
        delegate.drop_stale_view();
        let defaults = delegate.default_layout();
        let table = cx.new(|cx| {
            let mut t = TlTreeTable::new(delegate, window, cx);
            t.set_default_layout(defaults);
            t.set_resize_mode(settings.tables.column_resize);
            t.set_column_separators(settings.tables.column_separators);
            t
        });
        let focus = table.read(cx).focus_handle().clone();
        let hub_sub = cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
            this.on_snapshot(ev.mask, cx);
        });
        let mut subs = SmallVec::new();
        subs.push(
            cx.observe_global_in::<TlSettings>(window, |this, window, cx| {
                this.on_settings(cx);
                this.sync_history(window, cx);
            }),
        );
        subs.push(cx.observe_global::<UiMetrics>(|this, cx| this.on_metrics(cx)));
        subs.push(cx.observe_global::<Strings>(|this, cx| this.on_strings(cx)));
        // Ctrl+Shift+C is the table's own TSV copy (ADR-015); the page shows its toast.
        subs.push(cx.subscribe(&table, |this, _, ev: &TreeTableEvent, cx| {
            match ev {
                TreeTableEvent::CopiedRows { .. } => {
                    toast(ToastKind::Success, tr(cx, "processes-toast-copied"), cx);
                }
                // Rows held back during the press are installed now.
                TreeTableEvent::PressReleased => this.on_snapshot(DRAWN, cx),
                _ => {}
            }
        }));
        // Snapshot installs and selection changes both notify the table.
        subs.push(cx.observe(&table, |this, _, cx| this.refresh_status(false, cx)));
        // The shell owns the Ctrl rule and the chip's paused tag (E20-012).
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|e| e.0.clone()) {
            subs.push(cx.subscribe(&events, |this, _, ev: &PauseChanged, cx| {
                this.set_paused(ev.0, cx);
            }));
        }
        let section_versions = cx
            .try_global::<TlSettings>()
            .map_or([0; 4], |tl| SECTIONS.map(|s| tl.version(s)));
        jump::register_show_hidden(cx);
        let minigraphs = cx
            .try_global::<GlobalDemand>()
            .map(|d| d.0.clone())
            .map(|demand| {
                cx.new(|cx| {
                    let mut panel = MinigraphPanel::new(&hub, &demand, cx);
                    panel.set_own_space(true);
                    panel
                })
            });
        let history_mode = state.processes.history_mode;
        let state_version = cx
            .try_global::<TlSettings>()
            .map_or(0, |t| t.state_version());
        let mut history = None;
        let mut history_focus = None;
        if history_mode {
            let page = cx.new(|cx| HistoryPage::new(window, cx));
            history_focus = Some(page.read(cx).table().read(cx).focus_handle().clone());
            history = Some(page);
        }
        Self {
            table,
            _hub_sub: hub_sub,
            focus,
            client,
            _subs: subs,
            section_versions,
            style_version: cx.try_global::<UiMetrics>().map_or(0, |m| m.style_version),
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
            menu_targets: None,
            jump: jump::JumpState::default(),
            filter: FilterBinding::new(cx),
            paused: None,
            minigraphs,
            status_counts: StatusCounts::default(),
            status: String::new(),
            status_actions: SmallVec::new(),
            history,
            history_focus,
            active: false,
            state_version,
        }
    }

    /// Mounts or drops the `HistoryPage` when `history_mode` changed. The live table entity is
    /// never rebuilt. Focus follows only while this tab is shown.
    fn sync_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tl) = cx.try_global::<TlSettings>() else {
            return;
        };
        let version = tl.state_version();
        if version == self.state_version {
            return;
        }
        self.state_version = version;
        let want = TlSettings::state(cx).processes.history_mode;
        if want == self.history.is_some() {
            return;
        }
        if want {
            let page = cx.new(|cx| HistoryPage::new(window, cx));
            let focus = page.read(cx).table().read(cx).focus_handle().clone();
            if self.paused.is_some() {
                page.update(cx, |p, cx| p.set_paused(true, cx));
            }
            if self.active {
                window.focus(&focus, cx);
            }
            self.history = Some(page);
            self.history_focus = Some(focus);
        } else {
            self.history = None;
            self.history_focus = None;
            if self.active {
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    /// The status chip text (UI-PRC-018). Empty until the first view arrives.
    pub fn status_text(&self, out: &mut String) {
        out.push_str(&self.status);
    }

    /// The clickable chip parts (E20-008).
    pub fn status_actions(&self) -> SmallVec<[StatusAction; 2]> {
        self.status_actions.clone()
    }

    /// Rebuilds the chip text when a count, the filter state or the selection size changed, or
    /// always with `force` (new strings), and emits `StatusChanged` when the text changed. Counts
    /// come from a view built for the current query; until it arrives the previous counts stay.
    fn refresh_status(&mut self, force: bool, cx: &mut Context<Self>) {
        let t = self.table.read(cx);
        let d = &t.delegate;
        let selected = t
            .selection
            .ids()
            .filter(|&id| d.row_index(id).is_some())
            .count();
        let Some(view) = d.query().shown() else {
            return;
        };
        let helper = cx.global::<GlobalHub>().0.read(cx).current().health.helper;
        let counts = if view.query_generation == d.query().generation() {
            StatusCounts {
                helper,
                ..StatusCounts::of(view, !d.query().filter().is_empty(), selected)
            }
        } else {
            StatusCounts {
                selected: u32::try_from(selected).unwrap_or(u32::MAX),
                helper,
                ..self.status_counts
            }
        };
        if !force && counts == self.status_counts && !self.status.is_empty() {
            return;
        }
        self.status_counts = counts;
        let mut text = String::with_capacity(self.status.len());
        write_status(&mut text, counts, cx);
        let mut actions = SmallVec::new();
        write_actions(&mut actions, counts, cx);
        if text != self.status || actions != self.status_actions {
            self.status = text;
            self.status_actions = actions;
            cx.emit(StatusChanged);
        }
    }

    /// True while Ctrl-pause holds the table on one snapshot (UI-PRC-017).
    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    /// Ctrl-pause start: the table keeps drawing the snapshot it shows and skips anchoring.
    /// End: the latest snapshot and view are installed, the table renders once and anchors once.
    fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if paused == self.paused.is_some() {
            return;
        }
        if let Some(history) = &self.history {
            history.update(cx, |h, cx| h.set_paused(paused, cx));
        }
        if paused {
            let shown = Arc::clone(self.table.read(cx).delegate.snapshot());
            self.paused = Some(shown);
            self.table.update(cx, |t, _| t.paused = true);
        } else {
            self.paused = None;
            self.table.update(cx, |t, _| t.paused = false);
            self.on_snapshot(DRAWN, cx);
        }
    }

    /// The History mode page, while History mode is on.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn history_page(&self) -> Option<&Entity<HistoryPage>> {
        self.history.as_ref()
    }

    pub fn table(&self) -> &Entity<TlTreeTable<ProcessDelegate>> {
        &self.table
    }

    /// The handle the table's `uniform_list` tracks. Scrolling through it moves the table (the
    /// E01 `table-2000` bench scene drives it).
    pub fn scroll_handle(&self, cx: &App) -> UniformListScrollHandle {
        self.table.read(cx).scroll_handle()
    }

    pub fn focus_table(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// One hub change. Only a change to a drawn part reaches the delegate, and the table is
    /// notified only when the view was built for the current query. While paused only an unsent
    /// query is retried. The same holds while a row is pressed (UI-PRC-008).
    fn on_snapshot(&mut self, mask: ComponentMask, cx: &mut Context<Self>) {
        if mask.intersects(ComponentMask::HEALTH) {
            self.refresh_status(false, cx);
        }
        let paused = self.paused.is_some();
        let hub = cx.global::<GlobalHub>().0.clone();
        let snapshot = Arc::clone(hub.read(cx).current());
        let client = &*self.client;
        self.table.update(cx, |t, cx| {
            let d = &mut t.delegate;
            d.query_mut().resend_if_unsent(client);
            if paused
                || t.pressed
                || !mask.intersects(DRAWN)
                || snapshot.table.query_generation != d.query().generation()
            {
                return;
            }
            if d.install(&snapshot) {
                t.on_new_version(cx);
            } else {
                cx.notify();
            }
        });
        if !paused && !self.table.read(cx).pressed {
            self.resolve_jump(cx);
        }
    }

    /// `processes.mode` sends a query; `appearance`, `units` and `tables` start a new style
    /// version, which clears the cell text caches.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        let Some(tl) = cx.try_global::<TlSettings>() else {
            return;
        };
        let versions = SECTIONS.map(|s| tl.version(s));
        if versions == self.section_versions {
            return;
        }
        let old = std::mem::replace(&mut self.section_versions, versions);
        let processes = versions[0] != old[0];
        let restyle = versions[1..] != old[1..];
        let settings = TlSettings::settings(cx).clone();
        let client = &*self.client;
        self.table.update(cx, |t, cx| {
            t.delegate.set_settings(&settings);
            t.set_resize_mode(settings.tables.column_resize);
            t.set_column_separators(settings.tables.column_separators);
            if processes {
                t.delegate.query_mut().sync_settings(&settings, client);
            }
            if restyle {
                bump_style_version(cx);
            }
            cx.notify();
        });
    }

    /// A theme switch bumps the style version; the heat colours follow the new theme.
    fn on_metrics(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        let theme = current(cx).clone();
        self.table.update(cx, |t, cx| {
            t.delegate.set_theme(theme);
            cx.notify();
        });
    }

    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.refresh_status(true, cx);
        self.table.update(cx, |t, cx| {
            t.delegate.refresh_strings(cx);
            t.on_new_version(cx);
        });
    }

    /// Ctrl+L: collapses every row but the three group headers (PROC-020).
    fn on_collapse_all(&mut self, _: &CollapseAll, _: &mut Window, cx: &mut Context<Self>) {
        let client = &*self.client;
        self.table.update(cx, |t, _| {
            let q = t.delegate.query_mut();
            q.collapse_all();
            q.bump_and_send(client);
        });
    }

    /// The group header menu's Expand or Collapse: toggles that header, not the focus row.
    fn on_toggle_group_row(&mut self, a: &ToggleGroupRow, _: &mut Window, cx: &mut Context<Self>) {
        let node = a.node.clone();
        self.table
            .update(cx, |t, cx| t.delegate.toggle_node(node, cx));
    }

    /// The selection set in view order, group headers excluded, at most [`MAX_TARGETS`] items
    /// (`truncated` set beyond that), read from the shown (or paused) snapshot (PROC-001).
    pub fn capture_targets(&self, clicked_col: Option<ColumnId>, cx: &App) -> CapturedTargets {
        let table = self.table.read(cx);
        let d = &table.delegate;
        let mut rows: Vec<usize> = table
            .selection
            .ids()
            .filter_map(|id| d.row_index(id))
            .collect();
        rows.sort_unstable();
        let snapshot = d.snapshot();
        let targets = rows
            .into_iter()
            .filter_map(|ix| capture_row(snapshot, ix, d.row_kind(ix)));
        CapturedTargets::new(targets, clicked_col)
    }

    /// Called by the row menu when it opens, so its items act on the rows it was opened for
    /// even when the rows move before the click (E22-A2).
    pub fn set_menu_targets(&mut self, targets: CapturedTargets) {
        self.menu_targets = Some(targets);
    }

    /// Drops the stored menu targets once the menu has closed, so a later keyboard command
    /// reads the current selection instead of the rows of a dismissed menu.
    pub fn clear_menu_targets(&mut self) {
        self.menu_targets = None;
    }

    fn take_targets(&mut self, cx: &App) -> CapturedTargets {
        self.menu_targets
            .take()
            .unwrap_or_else(|| self.capture_targets(None, cx))
    }

    /// Opens one Properties window for each captured process, app or unit row, in selection
    /// order (D-008, UI-PRC-013 item 19). Group headers and pseudo rows do nothing.
    fn on_open_properties(&mut self, _: &OpenProperties, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_targets(cx);
        let unit_groups: FxHashMap<NodeId, tl_model::ids::GroupKey> = self
            .table
            .read(cx)
            .delegate
            .snapshot()
            .table
            .groups
            .iter()
            .filter(|group| group.kind == tl_model::process::GroupRowKind::Unit)
            .map(|group| (group.node.clone(), group.key))
            .collect();
        let mut process_cap_notified = false;
        let mut app_cap_notified = false;
        for target in targets.items {
            match (target.kind, target.node) {
                (RowKind::Process | RowKind::KernelThread, NodeId::Process(key)) => {
                    if should_attempt_properties(
                        &WindowKey::Properties(key),
                        CapClass::ProcessProps,
                        &mut process_cap_notified,
                        cx,
                    ) {
                        crate::windows::props::open_process_properties(key, cx);
                    }
                }
                (RowKind::App, NodeId::Group(group)) => {
                    if should_attempt_properties(
                        &WindowKey::AppProperties(group),
                        CapClass::AppProps,
                        &mut app_cap_notified,
                        cx,
                    ) {
                        crate::windows::app_props::open_app_properties(group, cx);
                    }
                }
                (RowKind::Unit, node @ NodeId::Unit(_)) => {
                    if let Some(group) = unit_groups.get(&node)
                        && should_attempt_properties(
                            &WindowKey::AppProperties(*group),
                            CapClass::AppProps,
                            &mut app_cap_notified,
                            cx,
                        )
                    {
                        crate::windows::app_props::open_app_properties(*group, cx);
                    }
                }
                _ => {}
            }
        }
    }

    /// A process command (E22-017): the shared entry point runs it on the menu-open targets,
    /// else on the selection, with the snapshot the table shows.
    fn on_process_action<A: Action>(&mut self, a: &A, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.take_targets(cx);
        self.run_action(a, targets, window, cx);
    }

    /// Runs `action` through [`run::run_process_action_with`] on `targets`.
    fn run_action(
        &mut self,
        action: &dyn Action,
        targets: CapturedTargets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = RunOptions {
            snapshot: Some(Arc::clone(self.table.read(cx).delegate.snapshot())),
            on_sent: None,
        };
        run_process_action_with(action, targets, options, window, cx);
    }

    fn on_copy_cell(&mut self, a: &CopyCell, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(CopyKind::Cell(a.col), cx);
    }

    fn on_copy_details(&mut self, _: &CopyDetails, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(CopyKind::Details, cx);
    }

    fn on_copy_all_metrics(&mut self, _: &CopyAllMetrics, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(CopyKind::AllMetrics, cx);
    }

    /// A window popup item (PROC-053, PROC-054): sends `KWin(Activate)` for the window and shows
    /// "Process window shown." The bridge does not answer an activation (ADR-005), so the toast
    /// follows the send. A window gone from the live windows snapshot gets "No window found".
    fn on_activate_window(&mut self, a: &ActivateWindow, _: &mut Window, cx: &mut Context<Self>) {
        let internal_id = cx.try_global::<GlobalHub>().and_then(|hub| {
            hub.0
                .read(cx)
                .current()
                .windows
                .windows
                .iter()
                .find(|w| *w.internal_id == *a.internal_id)
                .map(|w| Arc::clone(&w.internal_id))
        });
        let Some(internal_id) = internal_id else {
            warning_toast(tr(cx, "prc-055-no-window-found"), cx);
            return;
        };
        if let Err(err) = self
            .client
            .send(Command::KWin(KWinCommand::Activate { internal_id }))
        {
            warn!(target: "tasklens::ui", ?err, "KWin activate not sent");
            return;
        }
        toast(
            ToastKind::Success,
            tr(cx, "processes-toast-window-shown"),
            cx,
        );
    }

    /// Copy, Copy details and Copy all metrics (PROC-070, ADR-015): the captured targets in
    /// view order, exited rows too, onto the clipboard, then a Success toast (UI-PRC-015).
    /// Command lines leave only through Copy all metrics and Copy on the Command line cell.
    fn copy(&mut self, what: CopyKind, cx: &mut Context<Self>) {
        let nodes: Vec<NodeId> = self
            .take_targets(cx)
            .items
            .into_iter()
            .map(|t| t.node)
            .collect();
        let (text, rows) = self.table.read(cx).delegate.with_rows(&nodes, |rows, ctx| {
            let text = match what {
                CopyKind::Cell(col) => copy_cell(rows, process_column(col), ctx),
                CopyKind::Details => copy_details(rows, ctx),
                CopyKind::AllMetrics => copy_all_metrics(rows, ctx),
            };
            (text, rows.len())
        });
        if rows == 0 {
            return;
        }
        if clipboard_write(cx, text).is_err() {
            return;
        }
        let key = match (what, rows > 1) {
            (CopyKind::Cell(_), false) => "processes-toast-copied",
            (CopyKind::Cell(_), true) => "processes-toast-values-copied",
            (CopyKind::Details, _) => "processes-toast-details-copied",
            (CopyKind::AllMetrics, false) => "processes-toast-metrics-copied",
            (CopyKind::AllMetrics, true) => "processes-toast-many-metrics-copied",
        };
        toast(ToastKind::Success, tr(cx, key), cx);
    }
}

/// Which copy command runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CopyKind {
    /// The column under the pointer.
    Cell(ColumnId),
    Details,
    AllMetrics,
}

/// The Suspend or Resume item of the row menu (UI-PRC-013 item 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuspendLabel {
    Suspend,
    Resume,
}

/// `Resume` when every target is stopped (State T), else `Suspend`. With several targets the
/// menu shows "Resume Selected Tasks" only when every one is stopped.
pub fn suspend_or_resume(t: &CapturedTargets) -> SuspendLabel {
    if !t.items.is_empty() && t.items.iter().all(|t| t.stopped) {
        SuspendLabel::Resume
    } else {
        SuspendLabel::Suspend
    }
}

/// The confirmation action names, translated once per strings version (R-132).
struct ConfirmWords {
    end_task: SharedString,
    end_process: SharedString,
    suspend: SharedString,
    resume: SharedString,
    set_priority: SharedString,
    set_io_priority: SharedString,
}

impl ConfirmWords {
    fn new(cx: &App) -> Self {
        Self {
            end_task: tr(cx, "processes-confirm-action-end-task"),
            end_process: tr(cx, "processes-confirm-action-end-process"),
            suspend: tr(cx, "processes-confirm-action-suspend"),
            resume: tr(cx, "processes-confirm-action-resume"),
            set_priority: tr(cx, "processes-confirm-action-set-priority"),
            set_io_priority: tr(cx, "processes-confirm-action-set-io-priority"),
        }
    }
}

/// The process keys of `rows`, in order.
fn process_keys(rows: &[CapturedTarget]) -> Vec<ProcessKey> {
    rows.iter()
        .filter_map(|t| match t.node {
            NodeId::Process(key) => Some(key),
            _ => None,
        })
        .collect()
}

/// The non-destructive OtherUser confirmation for the first of `rows` owned by another user.
fn other_user(rows: &[CapturedTarget], action: &SharedString) -> Option<ConfirmKind> {
    let t = rows
        .iter()
        .find(|t| t.flags.contains(ProcFlags::OTHER_USER))?;
    Some(ConfirmKind::OtherUser {
        action: action.clone(),
        name: t.name.clone(),
        pid: t.pid.unwrap_or_default(),
        user: t.user.clone(),
        destructive: false,
    })
}

/// Which app or unit row command runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GroupOp {
    Kill,
    Freeze,
    Thaw,
}

/// The targets of Kill App, Freeze App and Thaw App on one app or unit row (R-092, PROC-064).
/// An app row gives the cgroup of every app.slice sample with the row's key, and nothing when
/// there is none: a member's unit is never the fallback, so no unit action reaches a session
/// scope. A unit row gives its unit.
///
/// An app.slice scope that also holds a live process outside the row's members (a terminal
/// scope that launched the app, say) is left out: freezing or killing it would hit other
/// programs (tasklens-pdu.61).
pub(crate) fn group_targets(s: &Snapshot, t: &CapturedTarget) -> SmallVec<[ActionTarget; 1]> {
    group_plan(s, t).targets
}

/// The targets of one app or unit row command and how many app.slice scopes were left out.
pub(crate) struct GroupPlan {
    pub(crate) targets: SmallVec<[ActionTarget; 1]>,
    /// App.slice samples with the row's key that hold other programs.
    pub(crate) skipped: u32,
    /// All app.slice samples with the row's key.
    pub(crate) total: u32,
}

/// [`group_targets`] with the skipped count, for the Kill App confirmation text.
pub(crate) fn group_plan(s: &Snapshot, t: &CapturedTarget) -> GroupPlan {
    let none = GroupPlan {
        targets: SmallVec::new(),
        skipped: 0,
        total: 0,
    };
    match (&t.node, t.kind) {
        (NodeId::Unit(unit), RowKind::Unit) => GroupPlan {
            targets: SmallVec::from_buf([ActionTarget::Unit(unit.clone())]),
            ..none
        },
        (node, RowKind::App) => {
            let Some(group) = s.table.groups.iter().find(|g| &g.node == node) else {
                return none;
            };
            let p = &s.processes;
            let own: FxHashSet<usize> = members(p, node).into_iter().collect();
            // The cgroups of live processes that are not members of the row.
            let others: Vec<&str> = (0..p.records.len())
                .filter(|i| !own.contains(i) && !p.records[*i].flags.contains(ProcFlags::EXITED))
                .filter_map(|i| p.records[i].meta.cgroup.as_deref())
                .collect();
            let shared = |scope: &str| {
                others.iter().any(|c| {
                    *c == scope
                        || c.strip_prefix(scope)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            };
            let samples = p.apps.iter().filter(|a| a.group == group.key);
            let mut plan = none;
            for a in samples {
                plan.total += 1;
                if shared(&a.cgroup.rel_path) {
                    plan.skipped += 1;
                } else {
                    plan.targets.push(ActionTarget::Cgroup(a.cgroup.clone()));
                }
            }
            plan
        }
        _ => none,
    }
}

/// Whether the app or unit row `node` reads as frozen (`cgroup.events` has `frozen 1`).
pub(crate) fn group_frozen(s: &Snapshot, node: &NodeId) -> bool {
    s.table
        .groups
        .iter()
        .find(|g| &g.node == node)
        .is_some_and(|g| g.frozen)
}

/// Which End command runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum End {
    /// SIGTERM.
    Task,
    /// SIGKILL.
    Process,
    /// SIGSTOP, then SIGKILL, on a process and its descendants (ADR-006).
    Tree,
}

impl End {
    fn kind(self) -> ActionKind {
        match self {
            End::Task => ActionKind::Signal(Signal::Term),
            End::Process => ActionKind::Signal(Signal::Kill),
            End::Tree => ActionKind::KillTree,
        }
    }
}

/// What one End command sends and whether it asks first.
struct EndPlan {
    /// The rows that gave keys, in view order.
    rows: Vec<CapturedTarget>,
    /// Without duplicates, at most [`MAX_TARGETS`].
    keys: Vec<ProcessKey>,
    /// `None`: send at once (D-009).
    confirm: Option<ConfirmKind>,
    /// End Process Tree: the first record of the subtree that is tasklens itself or PID 1.
    /// The page then shows its refusal toast and sends nothing.
    refused: Option<CapturedTarget>,
}

impl EndPlan {
    /// `None` when no row gives a key: End Process skips app and unit rows until Kill App
    /// (E22-035), End Process Tree takes the first process row.
    fn new(
        end: End,
        rows: Vec<CapturedTarget>,
        p: &ProcessSnapshot,
        words: &ConfirmWords,
    ) -> Option<Self> {
        let forest = Forest::new(&p.records);
        // (key, index into `used`).
        let mut keys: Vec<(ProcessKey, usize)> = Vec::new();
        let mut used: Vec<CapturedTarget> = Vec::new();
        // End Process Tree: the subtree's records, root first.
        let mut subtree: Vec<usize> = Vec::new();
        for t in rows {
            let n = used.len();
            match (end, t.kind, &t.node) {
                (End::Tree, RowKind::Process, NodeId::Process(key)) => {
                    match forest.index_of(*key) {
                        Some(root) => {
                            subtree = forest.subtree(root);
                            keys.extend(subtree.iter().map(|&i| (p.records[i].key, n)));
                        }
                        None => keys.push((*key, n)),
                    }
                    used.push(t);
                    break;
                }
                (End::Task | End::Process, RowKind::Process, NodeId::Process(key)) => {
                    keys.push((*key, n));
                    used.push(t);
                }
                (End::Task, RowKind::App | RowKind::Unit, node) => {
                    let primaries = primaries(p, node);
                    if primaries.is_empty() {
                        continue;
                    }
                    keys.extend(
                        primaries
                            .into_iter()
                            .map(|i| (p.records[forest.unwrap_bwrap(i)].key, n)),
                    );
                    used.push(t);
                }
                _ => {}
            }
        }
        let mut seen = FxHashSet::default();
        keys.retain(|(key, _)| seen.insert(*key));
        keys.truncate(MAX_TARGETS);
        if keys.is_empty() {
            return None;
        }

        let refused = subtree
            .iter()
            .map(|&i| &p.records[i])
            .find(|r| r.flags.contains(ProcFlags::IS_SELF) || r.key.pid == 1)
            .map(record_target);

        let count = keys.len() as u32;
        let protected = used
            .iter()
            .find(|t| t.flags.contains(ProcFlags::PROTECTED))
            .map(|t| t.name.clone())
            .or_else(|| {
                subtree
                    .iter()
                    .map(|&i| &p.records[i])
                    .find(|r| r.flags.contains(ProcFlags::PROTECTED))
                    .map(|r| SharedString::from(&r.meta.name))
            });
        let confirm = if let Some(name) = protected {
            Some(ConfirmKind::Protected {
                action: if end == End::Process {
                    ProtectedAction::Kill
                } else {
                    ProtectedAction::End
                },
                name,
                tree_count: (end == End::Tree).then_some(count),
            })
        } else if end == End::Tree {
            Some(ConfirmKind::EndTree { count })
        } else if used.len() >= 2 {
            Some(ConfirmKind::EndMany {
                count,
                kill: end == End::Process,
            })
        } else {
            keys.iter()
                .find(|(_, n)| used[*n].flags.contains(ProcFlags::OTHER_USER))
                .map(|(key, n)| {
                    let t = &used[*n];
                    ConfirmKind::OtherUser {
                        action: if end == End::Task {
                            words.end_task.clone()
                        } else {
                            words.end_process.clone()
                        },
                        name: t.name.clone(),
                        pid: t.pid.unwrap_or(key.pid),
                        user: t.user.clone(),
                        destructive: true,
                    }
                })
        };
        Some(Self {
            rows: used,
            keys: keys.into_iter().map(|(key, _)| key).collect(),
            confirm,
            refused,
        })
    }
}

/// Flags an app or unit row takes from its primary processes.
const GROUP_FLAGS: ProcFlags = ProcFlags::PROTECTED
    .union(ProcFlags::OTHER_USER)
    .union(ProcFlags::IS_SELF);

/// The target of row `ix`. `None` for a group header.
pub(crate) fn capture_row(s: &Snapshot, ix: usize, kind: RowKind) -> Option<CapturedTarget> {
    if kind == RowKind::GroupHeader {
        return None;
    }
    let row = s.table.rows.get(ix)?;
    let p = &s.processes;
    if let Some(i) = row.record.map(|i| i as usize)
        && let Some(r) = p.records.get(i)
    {
        return Some(CapturedTarget {
            node: row.node.clone(),
            kind,
            exe_deleted: exe_deleted(&r.meta) || p.extras.get(i).is_some_and(|e| e.exe_deleted),
            unit: main_pid_unit(&s.services, r.key.pid),
            // Read by the engine for the visible rows only (R-144).
            io_priority: p
                .extras_p2
                .get(i)
                .filter(|x| x.key == Some(r.key))
                .and_then(|x| x.io_priority),
            ..record_target(r)
        });
    }
    let group = row.group.and_then(|g| s.table.groups.get(g as usize));
    let primaries = primaries(p, &row.node);
    let first = primaries.first().map(|&i| &p.records[i]);
    let mut flags = primaries.iter().fold(ProcFlags::empty(), |f, &i| {
        f | (p.records[i].flags & GROUP_FLAGS)
    });
    if matches!(kind, RowKind::App | RowKind::Unit) && first.is_none() {
        flags |= ProcFlags::EXITED;
    }
    let user = group
        .and_then(|g| g.user.as_ref())
        .or(first.map(|r| &r.meta.user))
        .map_or_else(SharedString::default, SharedString::from);
    Some(CapturedTarget {
        node: row.node.clone(),
        kind,
        name: group.map_or_else(SharedString::default, |g| SharedString::from(&g.name)),
        pid: None,
        uid: first.map_or(0, |r| r.uid),
        user,
        flags,
        member_count: group.map_or(0, |g| g.members),
        stopped: false,
        ppid: 0,
        nice: None,
        io_priority: None,
        exe_deleted: false,
        unit: match &row.node {
            NodeId::Unit(u) => Some(u.clone()),
            _ => None,
        },
    })
}

/// The first unit whose MainPID is `pid`. Pid 0 means no MainPID and never matches.
fn main_pid_unit(s: &ServicesSnapshot, pid: u32) -> Option<UnitRef> {
    if pid == 0 {
        return None;
    }
    s.units
        .iter()
        .find(|u| u.main_pid == pid)
        .map(|u| u.unit.clone())
}

/// The target of one process record, as a process row. `exe_deleted` is false and
/// `io_priority` is `None`: they are in the extras, which only the row menu reads.
pub(crate) fn record_target(r: &ProcessRecord) -> CapturedTarget {
    CapturedTarget {
        node: NodeId::Process(r.key),
        kind: RowKind::Process,
        name: SharedString::from(&r.meta.name),
        pid: Some(r.key.pid),
        uid: r.uid,
        user: SharedString::from(&r.meta.user),
        flags: r.flags,
        member_count: 1,
        stopped: r.state == ProcState::Stopped,
        ppid: r.ppid,
        nice: Some(r.nice),
        io_priority: None,
        exe_deleted: false,
        unit: None,
    }
}

/// The user unit a `plasmashell` process row restarts (PROC-072). `None` for every other row
/// and for a plasmashell of another user.
pub(crate) fn plasma_restart_unit(t: &CapturedTarget) -> Option<UnitRef> {
    (t.kind == RowKind::Process
        && &*t.name == "plasmashell"
        && !t.flags.contains(ProcFlags::OTHER_USER))
    .then(|| UnitRef {
        manager: Manager::User,
        name: Arc::from(PLASMA_UNIT),
    })
}

const PLASMA_UNIT: &str = "plasma-plasmashell.service";

fn is_plasma_unit(u: &UnitRef) -> bool {
    u.manager == Manager::User && &*u.name == PLASMA_UNIT
}

/// The topmost window (lowest `stack_index`) of the captured row: a process row's own PID, an
/// app or unit row's members. `None` without a live windows snapshot.
pub(crate) fn topmost_window<'a>(s: &'a Snapshot, t: &CapturedTarget) -> Option<&'a WindowEntry> {
    if !matches!(s.windows.header.validity, Validity::Valid | Validity::Stale) {
        return None;
    }
    let owns: Box<dyn Fn(u32) -> bool> = match (&t.node, t.pid) {
        (NodeId::Process(key), _) => {
            let pid = key.pid;
            Box::new(move |p| p == pid)
        }
        (node, _) => {
            let pids: Vec<u32> = members(&s.processes, node)
                .into_iter()
                .map(|i| s.processes.records[i].key.pid)
                .collect();
            Box::new(move |p| pids.contains(&p))
        }
    };
    s.windows
        .windows
        .iter()
        .filter(|w| owns(w.pid))
        .min_by_key(|w| w.stack_index)
}

/// The live member records of an app or unit row, in record order. Empty for every other
/// node.
fn members(p: &ProcessSnapshot, node: &NodeId) -> Vec<usize> {
    let live = |i: usize| !p.records[i].flags.contains(ProcFlags::EXITED);
    match node {
        NodeId::Group(key) => (0..p.records.len())
            .filter(|&i| p.records[i].group == *key && live(i))
            .collect(),
        NodeId::Unit(unit) => (0..p.records.len())
            .filter(|&i| p.extras.get(i).and_then(|x| x.unit.as_ref()) == Some(unit) && live(i))
            .collect(),
        _ => Vec::new(),
    }
}

/// The processes a dialog opened from the row menu acts on, and its subtitle subject (UI-PRC-021).
/// One app or unit row gives its current member processes, at most [`MAX_TARGETS`]; several rows
/// give their processes, each once.
fn dialog_targets(
    rows: &[CapturedTarget],
    p: &ProcessSnapshot,
) -> Option<(Vec<ProcessKey>, crate::windows::affinity::AffinitySubject)> {
    use crate::windows::affinity::AffinitySubject;
    let (keys, subject): (Vec<ProcessKey>, _) = match rows {
        [t] => {
            let keys: Vec<ProcessKey> = match (t.kind, &t.node) {
                (RowKind::Process | RowKind::KernelThread, NodeId::Process(key)) => vec![*key],
                (RowKind::App | RowKind::Unit, node) => members(p, node)
                    .into_iter()
                    .take(MAX_TARGETS)
                    .map(|i| p.records[i].key)
                    .collect(),
                _ => Vec::new(),
            };
            let first = keys.first()?;
            let name = t.name.clone();
            let subject = match t.kind {
                RowKind::App | RowKind::Unit => AffinitySubject::App {
                    name,
                    n: keys.len(),
                },
                _ => AffinitySubject::Process {
                    name,
                    pid: t.pid.unwrap_or(first.pid),
                },
            };
            (keys, subject)
        }
        // Several rows (E23-005): their processes, each once.
        rows => {
            let mut keys: Vec<ProcessKey> = Vec::new();
            for t in rows {
                if let (RowKind::Process | RowKind::KernelThread, NodeId::Process(key)) =
                    (t.kind, &t.node)
                    && !keys.contains(key)
                    && keys.len() < MAX_TARGETS
                {
                    keys.push(*key);
                }
            }
            let subject = match (&keys[..], rows.first()) {
                ([key], Some(t)) => AffinitySubject::Process {
                    name: t.name.clone(),
                    pid: key.pid,
                },
                _ => AffinitySubject::Processes(keys.len()),
            };
            (keys, subject)
        }
    };
    (!keys.is_empty()).then_some((keys, subject))
}

/// Process action targets and display/protection metadata for Send Signal. One app or unit row
/// resolves to its primary processes, including E22's bwrap unwrapping. Several rows keep view
/// order, remove duplicate process keys and stop at [`MAX_TARGETS`].
fn signal_targets(
    rows: &[CapturedTarget],
    p: &ProcessSnapshot,
) -> Option<(SmallVec<[ActionTarget; 1]>, TargetNames)> {
    let forest = Forest::new(&p.records);
    let mut seen = FxHashSet::default();
    let mut resolved: Vec<CapturedTarget> = Vec::new();
    for row in rows {
        match (row.kind, &row.node) {
            (RowKind::Process | RowKind::KernelThread, NodeId::Process(key)) => {
                if seen.insert(*key) {
                    resolved.push(row.clone());
                }
            }
            (RowKind::App | RowKind::Unit, node) => {
                for i in primaries(p, node) {
                    let i = forest.unwrap_bwrap(i);
                    let record = &p.records[i];
                    if seen.insert(record.key) {
                        resolved.push(record_target(record));
                    }
                    if resolved.len() == MAX_TARGETS {
                        break;
                    }
                }
            }
            _ => {}
        }
        if resolved.len() == MAX_TARGETS {
            break;
        }
    }
    if resolved.is_empty() {
        return None;
    }

    let mut names = match rows {
        [row] if matches!(row.kind, RowKind::App | RowKind::Unit) => {
            TargetNames::app(row.name.clone(), resolved.len() as u32)
        }
        [row] => TargetNames::process(
            row.name.clone(),
            row.pid
                .unwrap_or_else(|| resolved[0].pid.unwrap_or_default()),
        ),
        _ => TargetNames::processes(resolved.len() as u32),
    };
    if rows.iter().any(|row| row.kind == RowKind::KernelThread) {
        names = names.with_refusal(SignalRefusal::KernelThread);
    }
    if let Some(row) = rows
        .iter()
        .chain(resolved.iter())
        .find(|row| row.flags.contains(ProcFlags::PROTECTED))
    {
        names = names.with_protected(row.name.clone());
    }
    if let Some(row) = resolved
        .iter()
        .find(|row| row.flags.contains(ProcFlags::OTHER_USER))
    {
        names = names.with_other_user(OtherUserTarget {
            name: row.name.clone(),
            pid: row.pid.unwrap_or_default(),
            user: row.user.clone(),
        });
    }
    let targets = resolved
        .into_iter()
        .filter_map(|row| match row.node {
            NodeId::Process(key) => Some(ActionTarget::Process(key)),
            _ => None,
        })
        .collect();
    Some((targets, names))
}

/// Test hook for the row-menu adapter. Production callers use the two page action handlers.
#[cfg(feature = "test-hooks")]
pub fn signal_targets_for_test(
    rows: &[CapturedTarget],
    p: &ProcessSnapshot,
) -> Option<(SmallVec<[ActionTarget; 1]>, TargetNames)> {
    signal_targets(rows, p)
}

/// The live member records of an app or unit row whose parent is not a member, lowest PID
/// first. Empty for every other node.
fn primaries(p: &ProcessSnapshot, node: &NodeId) -> Vec<usize> {
    let members = members(p, node);
    let pids: FxHashSet<u32> = members.iter().map(|&i| p.records[i].key.pid).collect();
    let mut out: Vec<usize> = members
        .into_iter()
        .filter(|&i| !pids.contains(&p.records[i].ppid))
        .collect();
    out.sort_unstable_by_key(|&i| p.records[i].key.pid);
    out
}

/// The parent links of the live records of one snapshot, built once per action (R-192).
struct Forest<'a> {
    records: &'a [ProcessRecord],
    /// Record indexes of each PID's live children, lowest PID first.
    children: FxHashMap<u32, SmallVec<[usize; 4]>>,
}

impl<'a> Forest<'a> {
    fn new(records: &'a [ProcessRecord]) -> Self {
        let mut children: FxHashMap<u32, SmallVec<[usize; 4]>> = FxHashMap::default();
        for (i, r) in records.iter().enumerate() {
            if !r.flags.contains(ProcFlags::EXITED) && r.ppid != r.key.pid {
                children.entry(r.ppid).or_default().push(i);
            }
        }
        for c in children.values_mut() {
            c.sort_unstable_by_key(|&i| records[i].key.pid);
        }
        Self { records, children }
    }

    fn index_of(&self, key: ProcessKey) -> Option<usize> {
        self.records.binary_search_by(|r| r.key.cmp(&key)).ok()
    }

    /// `root` and its descendants, breadth first, at most [`MAX_TARGETS`] record indexes.
    fn subtree(&self, root: usize) -> Vec<usize> {
        let mut out = vec![root];
        let mut seen = FxHashSet::default();
        seen.insert(self.records[root].key.pid);
        let mut next = 0;
        while next < out.len() && out.len() < MAX_TARGETS {
            let pid = self.records[out[next]].key.pid;
            next += 1;
            for &child in self.children.get(&pid).into_iter().flatten() {
                if out.len() == MAX_TARGETS {
                    break;
                }
                if seen.insert(self.records[child].key.pid) {
                    out.push(child);
                }
            }
        }
        out
    }

    /// A bwrap record's first non-bwrap descendant in the breadth-first walk (PROC-010,
    /// R-192); any other record, or a bwrap without one, is itself.
    fn unwrap_bwrap(&self, i: usize) -> usize {
        if !is_bwrap(&self.records[i]) {
            return i;
        }
        self.subtree(i)
            .into_iter()
            .skip(1)
            .find(|&j| !is_bwrap(&self.records[j]))
            .unwrap_or(i)
    }
}

/// The exe basename is `bwrap`; `comm` when the exe link was not readable.
fn is_bwrap(r: &ProcessRecord) -> bool {
    let name = match &r.meta.exe {
        Some(exe) => exe.rsplit('/').next().unwrap_or(exe),
        None => &r.meta.comm,
    };
    name == "bwrap"
}

/// A Warning toast on the main window.
/// The executable was deleted: its path ends in " (deleted)" (PROC-073). The row menu disables
/// Open file location for it.
pub fn exe_deleted(meta: &ProcessMeta) -> bool {
    meta.exe
        .as_deref()
        .is_some_and(|e| e.ends_with(DELETED_SUFFIX))
}

fn warning_toast(text: SharedString, cx: &mut App) {
    toast(ToastKind::Warning, text, cx);
}

/// A toast of `kind` on the main window.
fn toast(kind: ToastKind, text: SharedString, cx: &mut App) {
    let Some(main) = crate::window::main::main_window_view(cx) else {
        warn!(target: "tasklens::ui", "no main window for the Processes toast");
        return;
    };
    let queue = main.read(cx).toasts().clone();
    queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
}

impl Render for ProcessesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "ProcessesPage".into())
            .key_context(PAGE_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_collapse_all))
            .on_action(cx.listener(Self::on_open_properties))
            .on_action(cx.listener(Self::on_toggle_group_row))
            .on_action(cx.listener(Self::on_process_action::<EndTask>))
            .on_action(cx.listener(Self::on_process_action::<EndProcess>))
            .on_action(cx.listener(Self::on_process_action::<EndProcessTree>))
            .on_action(cx.listener(Self::on_process_action::<KillApp>))
            .on_action(cx.listener(Self::on_process_action::<FreezeApp>))
            .on_action(cx.listener(Self::on_process_action::<ThawApp>))
            .on_action(cx.listener(Self::on_process_action::<SuspendProcess>))
            .on_action(cx.listener(Self::on_process_action::<ResumeProcess>))
            .on_action(cx.listener(Self::on_process_action::<SetPriority>))
            .on_action(cx.listener(Self::on_process_action::<SetIoPriorityClass>))
            .on_action(cx.listener(Self::on_process_action::<OpenCustomPriority>))
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_details))
            .on_action(cx.listener(Self::on_copy_all_metrics))
            .on_action(cx.listener(Self::on_process_action::<SetAffinity>))
            .on_action(cx.listener(Self::on_process_action::<SendSignal>))
            .on_action(cx.listener(Self::on_process_action::<SignalOpenRealtime>))
            .on_action(cx.listener(Self::on_process_action::<GoToParent>))
            .on_action(cx.listener(Self::on_process_action::<AddMinigraph>))
            .on_action(cx.listener(Self::on_process_action::<AddNameGraph>))
            .on_action(cx.listener(Self::on_process_action::<AddAppGraph>))
            .on_action(cx.listener(Self::on_process_action::<OpenFileLocation>))
            .on_action(cx.listener(Self::on_process_action::<OpenVirusTotal>))
            .on_action(cx.listener(Self::on_process_action::<CreateDump>))
            .on_action(cx.listener(Self::on_activate_window))
            .on_action(cx.listener(Self::on_process_action::<ShowWindow>))
            .on_action(cx.listener(Self::on_process_action::<RestartUnit>))
            .on_action(cx.listener(Self::on_process_action::<FileProperties>))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .debug_selector(|| "ProcessesTableArea".into())
                    .child(match &self.history {
                        Some(history) => history
                            .clone()
                            .cached(StyleRefinement::default().size_full())
                            .into_any_element(),
                        None => self
                            .table
                            .clone()
                            .cached(StyleRefinement::default().size_full())
                            .into_any_element(),
                    }),
            )
            // The cards get a row of their own under the table, so no row is covered.
            .children(self.minigraphs.clone())
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

impl ShellPage for ProcessesPage {
    fn status_text(&self, out: &mut String) {
        ProcessesPage::status_text(self, out);
    }

    fn status_actions(&self) -> SmallVec<[StatusAction; 2]> {
        ProcessesPage::status_actions(self)
    }

    fn primary_focus(&self) -> FocusHandle {
        self.history_focus
            .clone()
            .unwrap_or_else(|| self.focus.clone())
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.table
            .update(cx, |t, cx| t.delegate.set_active(active, cx));
        if let Some(panel) = &self.minigraphs {
            panel.update(cx, |p, cx| p.set_active(active, cx));
        }
    }
}
