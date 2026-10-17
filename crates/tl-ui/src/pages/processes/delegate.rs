//! `ProcessDelegate`, the `TreeTableDelegate` of the process table (E22-A1, UI-PRC-001,
//! UI-PRC-002, UI-PRC-005, UI-PRC-010, ADR-009, PROC-013, PROC-018, PROC-022, PROC-033, D-008).
//!
//! The engine sorts, filters, groups and flattens the rows (PROC-013). The delegate maps each
//! finished `TableRow` to a row kind, flags, a Name cell and metric cells, reports the visible
//! process keys as demand, and forwards sort, expand, column and activation gestures.
//!
//! Rendering cost: every per-row answer reads the installed view through precomputed tables.
//! `row_index`, the sibling pass and the Name cell texts are rebuilt only when the view changes,
//! so a frame allocates nothing and translates nothing once the caches are warm. Words come from
//! `tr` at construction and on a locale change only.
//!
//! NEW and EXITED tints (PROC-043, UI-PRC-011) are computed per row from the frame's clock and
//! never rebuild the row tables. Animation frames are requested only while a visible tint fades;
//! before that, one timer wakes the table at the fade start.
//!
//! The row tooltip (PROC-039) is built only for the hovered row, when the table asks for it.
//! So are the cell tooltips (UI-PRC-004): each is formatted once per hovered row and snapshot.
//! The Name-cell window button (PROC-053) is built only for hovered and selected rows, from the
//! window lists kept per table version.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::hash::BuildHasher as _;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, AsyncApp, Context, Hsla,
    SharedString, Task, WeakEntity, Window, px,
};
use rustc_hash::{FxBuildHasher, FxHashMap};
use smallvec::SmallVec;
use tl_model::demand::{ColumnSet, Demand};
use tl_model::devices::{DeviceIdentity, DeviceKind};
use tl_model::health::{EngineCaps, SourceId};
use tl_model::ids::{ColumnId, GroupKey, NodeId, ProcessKey, PseudoKind};
use tl_model::process::{GroupRow, GroupRowKind, ProcFlags, ProcState};
use tl_model::settings::{NetworkUnit, Settings, UiState};
use tl_model::snapshot::{Snapshot, WindowEntry};
use tl_model::table::{SortSpec, TableRow, UnitRef};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_model::validity::{FieldValidity, Validity};

use super::actions::CapturedTargets;
use super::cells::{
    CellCtx, CellTipWords, CellWords, DEFAULT_VISIBLE, GroupMembers, PROCESS_COLUMNS, PaintState,
    PowerSensor, ProcColumn, RowRef, SUBTREE_PREFIX, TipCtx, cell_suffix, cell_tooltip, dims_zero,
    fill_cell, heat_background, process_column, row_tint, subtree_column, subtree_total,
    tint_deadline,
};
use super::header_totals::header_total;
use super::menu::{MenuCaps, group_header_menu, row_menu, window_popup};
use super::name_cell::{
    NameCellData, NameIcon, NameWords, StateGlyph, name_cell, pseudo_icon, window_button,
};
use super::table::{GroupHeader, ProcessQuery};
use super::tooltip::{RowTipState, RowTipWords, RowTooltip};
use super::{ProcessesPage, capture_row};
use crate::anim::AnimationDriver;
use crate::app::EngineGlobal;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_PROCESS_CELL, TlTooltip};
use crate::components::validity::{GlyphCtx, tooltip_text};
use crate::demand::GlobalDemand;
use crate::i18n::{tr, tr_args};
use crate::settings_global::TlSettings;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use crate::table::tree_table::{
    DEMAND_MAX_IDS, HeaderLines, NameCell, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};
use crate::text::metrics::TextMetricsCache;
use crate::theme::{TlTheme, current, metrics};

/// The highest column phase this build offers. Later beads raise it.
pub const SHIPPED_PHASE: u8 = 1;

/// `sysconf(_SC_CLK_TCK)`. Linux fixes USER_HZ for `/proc` at 100 on every architecture
/// tasklens builds for, and tl-ui has no libc binding to ask.
const CLK_TCK: u32 = 100;

/// Fluent keys of the three group header labels (D-011).
const HEADER_KEYS: [&str; 3] = [
    "processes-header-apps",
    "processes-header-background",
    "processes-header-system",
];

/// Smallest default width of a column measured from its label, logical px.
const MEASURED_MIN_WIDTH: f32 = 60.0;
/// Room added to a measured label for the cell padding.
const LABEL_PAD: f32 = 16.0;

/// The process table's row identity. `NodeId` is not `Copy` (a unit holds an `Arc<str>`), so a
/// unit is named by a number from [`UnitIds`]. Every other node keeps its full key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProcRowId {
    Group(GroupKey),
    Process(ProcessKey),
    KernelThreads,
    Pseudo(PseudoKind),
    Unit(u32),
}

impl ProcRowId {
    /// The id of every node but a unit, which needs [`UnitIds`].
    pub fn fixed(node: &NodeId) -> Option<Self> {
        match node {
            NodeId::Group(key) => Some(ProcRowId::Group(*key)),
            NodeId::Process(key) => Some(ProcRowId::Process(*key)),
            NodeId::KernelThreads => Some(ProcRowId::KernelThreads),
            NodeId::Pseudo(kind) => Some(ProcRowId::Pseudo(*kind)),
            NodeId::Unit(_) => None,
        }
    }
}

/// Unit numbers for [`ProcRowId::Unit`]. Each new unit gets the next number, and a number is
/// never reused, so two units never share an id. Distinct units are a few hundred at most.
/// Keyed by the whole `UnitRef`: `foo.service` may exist in both managers.
#[derive(Default)]
pub struct UnitIds {
    ids: FxHashMap<UnitRef, u32>,
    next: u32,
}

impl UnitIds {
    /// The id of `node`, numbering a unit seen for the first time.
    pub fn intern(&mut self, node: &NodeId) -> ProcRowId {
        match node {
            NodeId::Unit(unit) => {
                let next = &mut self.next;
                let n = *self.ids.entry(unit.clone()).or_insert_with(|| {
                    *next += 1;
                    *next - 1
                });
                ProcRowId::Unit(n)
            }
            _ => ProcRowId::fixed(node).unwrap_or(ProcRowId::KernelThreads),
        }
    }

    /// The id of `node`, or `None` for a unit never seen.
    pub fn get(&self, node: &NodeId) -> Option<ProcRowId> {
        match node {
            NodeId::Unit(unit) => self.ids.get(unit).map(|&n| ProcRowId::Unit(n)),
            _ => ProcRowId::fixed(node),
        }
    }
}

/// The validity tooltips that carry no live value, built once. Stale ("Last updated {age} ago")
/// and Failed ("Could not read: {error}") need per-cell text and come from [`live_tip`].
#[derive(Clone)]
pub struct ValidityTips {
    warming: SharedString,
    restricted: SharedString,
    unsupported: SharedString,
    missing: SharedString,
}

impl Default for ValidityTips {
    fn default() -> Self {
        let tip = |v| tooltip_text(v, &GlyphCtx::default()).unwrap_or_default();
        Self {
            warming: tip(Validity::WarmingUp),
            restricted: tip(Validity::Restricted),
            unsupported: tip(Validity::Unsupported),
            missing: tip(Validity::Missing),
        }
    }
}

impl ValidityTips {
    /// The tooltip of a cell in `v`, a reference count copy. `None` for Valid, Stale, Failed.
    pub fn get(&self, v: Validity) -> Option<SharedString> {
        match v {
            Validity::WarmingUp => Some(self.warming.clone()),
            Validity::Restricted => Some(self.restricted.clone()),
            Validity::Unsupported => Some(self.unsupported.clone()),
            Validity::Missing => Some(self.missing.clone()),
            Validity::Valid | Validity::Stale | Validity::Failed => None,
        }
    }
}

/// The Stale or Failed tooltip of a process cell at `now`: the age of the process sample, or
/// the Processes source's last error. `None` for every other validity. It allocates, so the
/// delegate calls it only when the table sets `CellOut::want_tooltip`.
pub fn live_tip(v: Validity, snapshot: &Snapshot, now: TimeNs) -> Option<SharedString> {
    let ctx = match v {
        Validity::Stale => GlyphCtx {
            age_ns: now.saturating_sub(snapshot.processes.header.observed_end),
            ..GlyphCtx::default()
        },
        Validity::Failed => GlyphCtx {
            last_error: snapshot.health.get(SourceId::Processes).last_error.clone(),
            ..GlyphCtx::default()
        },
        _ => return None,
    };
    tooltip_text(v, &ctx)
}

/// Process-row cell filling and painting, shared by every table that shows process rows
/// (Processes, Services child rows).
pub struct ProcessRowCells;

impl ProcessRowCells {
    /// Fills `out` for one cell (E22-003): the value through `fill_cell`, the heat fill in
    /// `out.background`, a dimmed zero in `out.dim` (PROC-041), the "Σ " of a subtree total in
    /// `out.prefix` (UI-PRC-002), the " +N" or " (deleted)" in `out.suffix` (UI-PRC-004) and the
    /// validity tooltip in `out.tooltip`. The table attaches the tooltip on the hovered row only.
    /// Allocates nothing unless `out.want_text` is set.
    pub fn fill(
        row: RowRef,
        col: ColumnId,
        ctx: &CellCtx,
        st: PaintState,
        theme: &TlTheme,
        tips: &ValidityTips,
        out: &mut CellOut,
    ) {
        let spec = process_column(col);
        fill_cell(row, spec, ctx, out);
        out.background = heat_background(out, spec, ctx, st, theme);
        out.dim = dims_zero(out, spec, st);
        out.prefix = (st.subtree && subtree_column(col)).then(|| {
            (
                SharedString::new_static(SUBTREE_PREFIX),
                theme.text_secondary,
            )
        });
        out.suffix = cell_suffix(col, out, theme);
        out.tooltip = tips.get(out.validity);
    }
}

/// The cell tooltips of the hovered row, each formatted on its first ask (UI-PRC-004).
#[derive(Default)]
struct CellTips {
    /// `(snapshot sequence, table version, row index)` the tooltips were built for.
    built_for: Option<(u64, u64, usize)>,
    tips: SmallVec<[(ColumnId, Option<SharedString>); 16]>,
    scratch: String,
}

/// A cached Name cell text: `"<name> (N)"` for app, unit, Kernel threads and group header
/// rows, keyed by the name's pointer and the count so an unchanged row is not formatted again.
struct NameEntry {
    name: Arc<str>,
    count: u32,
    primary: SharedString,
    count_run: Range<usize>,
    /// App rows: the caption of the member window with the lowest `stack_index`, and the
    /// shared string the cell draws. Compared by pointer, so an unchanged caption is not
    /// shaped again.
    title: Option<(Arc<str>, SharedString)>,
}

/// The process table delegate. The page installs each snapshot with [`Self::install`] and calls
/// `TlTreeTable::on_new_version` when it returns true.
pub struct ProcessDelegate {
    query: ProcessQuery,
    /// The snapshot the rows came from (or the paused one, E22-024).
    snapshot: Arc<Snapshot>,
    /// `(view.header.version, rows pointer, windows.header.version)` of the last rebuild.
    built_for: Option<(u64, usize, u64)>,
    /// Bumps on every rebuild.
    version: u64,
    row_index: FxHashMap<ProcRowId, u32>,
    /// The id of each row, by index.
    ids: Vec<ProcRowId>,
    units: UnitIds,
    /// Per row: last sibling, ancestor-continues bits (E22-006, [`sibling_pass`]).
    sibling: Vec<(bool, u64)>,
    /// The view is grouped: every depth-0 row is a group header (`RowFlags::IN_GROUP`).
    grouped: bool,
    layout: ColumnLayout,
    /// Every column this build can offer, `gpu_0` to `gpu_15` included.
    specs: FxHashMap<ColumnId, ColumnSpec>,
    /// The columns offered now: phase-shipped columns and the present GPU adapters' slots.
    offered: Vec<ColumnId>,
    /// `DeviceCatalog::version` the GPU slots were built for.
    devices_version: Option<u64>,
    name_cache: FxHashMap<ProcRowId, NameEntry>,
    /// Each app group's windows from its member processes: indices into
    /// `snapshot.windows.windows`, topmost first. Rebuilt with the rows (PROC-053).
    app_windows: FxHashMap<GroupKey, SmallVec<[u32; 4]>>,
    /// "Windows (N)" per window count N, formatted on first use.
    windows_tips: SmallVec<[(usize, SharedString); 4]>,
    members: GroupMembers,
    cell_words: CellWords,
    name_words: NameWords,
    /// "Apps", "Background processes", "System processes".
    header_words: [Arc<str>; 3],
    /// "No processes match the filter" (UI-PRC-024).
    no_match: SharedString,
    headers: [ProcRowId; 3],
    tips: ValidityTips,
    settings: Settings,
    theme: Arc<TlTheme>,
    /// The window the table is in, for the demand part (hidden detection, ADR-008) and the
    /// "Process exited" toast.
    window: Option<AnyWindowHandle>,
    /// The Processes tab is the shown tab. Only then is the demand part sent (ADR-008).
    active: bool,
    /// The process keys of the last visible range, at most `DEMAND_MAX_IDS`.
    visible_keys: Arc<[ProcessKey]>,
    /// The clock the row tints of this frame read, set in `before_rows`.
    tint_now: TimeNs,
    /// `motion.animations` and not reduced motion, set in `before_rows`.
    tint_animations: bool,
    /// The timer that wakes the table when a visible tint starts to fade (or ends, without
    /// animations), with its deadline. The deadline is 0 once the timer fired.
    tint_wake: Option<(TimeNs, Task<()>)>,
    tip_words: RowTipWords,
    /// The hovered row's tooltip lines, shared with `tip_view`.
    tip: Rc<RefCell<RowTipState>>,
    /// The one row tooltip view, created on the first frame.
    tip_view: Option<AnyView>,
    cell_tip_words: CellTipWords,
    cell_tips: RefCell<CellTips>,
    /// The page that owns the table, where the row menu stores its captured targets.
    page: Option<WeakEntity<ProcessesPage>>,
    /// A fixed clock for tests.
    #[cfg(any(test, feature = "test-hooks"))]
    fixed_now: Option<TimeNs>,
}

impl ProcessDelegate {
    /// Translates the words and column labels once and builds the layout from
    /// `[columns.processes]`. `snapshot` is the first one shown; it is installed when its view
    /// matches the query.
    pub fn new(
        snapshot: Arc<Snapshot>,
        settings: &Settings,
        state: &UiState,
        window: &Window,
        cx: &App,
    ) -> Self {
        let specs = build_specs(settings, window, cx);
        // `GroupHeader::node` is always a `NodeId::Group`.
        let headers = GroupHeader::ALL
            .map(|h| ProcRowId::fixed(&h.node()).unwrap_or(ProcRowId::KernelThreads));
        let mut this = Self {
            query: ProcessQuery::new(settings, state),
            snapshot: Arc::clone(&snapshot),
            built_for: None,
            version: 0,
            row_index: FxHashMap::default(),
            ids: Vec::new(),
            units: UnitIds::default(),
            sibling: Vec::new(),
            grouped: false,
            layout: ColumnLayout::defaults(TableId::Processes, &[], &[], &[]),
            specs,
            offered: Vec::new(),
            devices_version: None,
            name_cache: FxHashMap::default(),
            app_windows: FxHashMap::default(),
            windows_tips: SmallVec::new(),
            members: GroupMembers::default(),
            cell_words: CellWords::new(cx),
            name_words: NameWords::new(cx),
            header_words: HEADER_KEYS.map(|k| Arc::from(tr(cx, k).as_ref())),
            no_match: tr(cx, "processes-empty-no-match"),
            headers,
            tips: ValidityTips::default(),
            settings: settings.clone(),
            theme: current(cx).clone(),
            window: None,
            active: false,
            visible_keys: Arc::from([]),
            tint_now: now_boot_ns(),
            tint_animations: settings.motion.animations && !cx.reduce_motion(),
            tint_wake: None,
            tip_words: RowTipWords::new(cx),
            tip: Rc::default(),
            tip_view: None,
            cell_tip_words: CellTipWords::new(cx),
            cell_tips: RefCell::default(),
            page: None,
            #[cfg(any(test, feature = "test-hooks"))]
            fixed_now: None,
        };
        this.sync_devices(&snapshot, Some(&state.columns.processes));
        this.layout.rebuild(metrics(cx).ui_scale);
        this.install(&snapshot);
        this
    }

    pub fn query(&self) -> &ProcessQuery {
        &self.query
    }

    pub fn query_mut(&mut self) -> &mut ProcessQuery {
        &mut self.query
    }

    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    /// The columns offered now, in registry order.
    pub fn offered_columns(&self) -> &[ColumnId] {
        &self.offered
    }

    /// The default `[columns.processes]` layout for "Reset to default", over the offered columns.
    pub fn default_layout(&self) -> ColumnLayout {
        let specs = self.offered_specs();
        let defaults: SmallVec<[ColumnId; 9]> = DEFAULT_VISIBLE
            .iter()
            .filter_map(|s| ColumnId::from_str(s))
            .collect();
        let mut layout = ColumnLayout::defaults(TableId::Processes, &specs, &defaults, &defaults);
        layout.rebuild(self.layout.ui_scale());
        layout
    }

    /// The window the table is in. Demand parts carry it so the aggregator drops them while the
    /// main window is hidden.
    pub fn set_window(&mut self, window: Option<AnyWindowHandle>) {
        self.window = window;
    }

    /// The page the row menu hands its captured targets to.
    pub fn set_page(&mut self, page: WeakEntity<ProcessesPage>) {
        self.page = Some(page);
    }

    /// Called by the page on tab activation. Active sends the demand part, which turns on the
    /// process sweep and the 1 Hz CPU and memory totals; inactive withdraws it.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<TlTreeTable<Self>>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else if let Some(agg) = cx.try_global::<GlobalDemand>().map(|g| g.0.clone()) {
            let source = cx.entity_id();
            agg.update(cx, |a, cx| a.remove(source, cx));
        }
    }

    /// This surface's demand part: the process table with its shown columns, and the visible
    /// process keys for the detail reads.
    pub fn demand_part(&self) -> Demand {
        let mut columns = ColumnSet::EMPTY;
        for col in self.layout.visible_columns().iter() {
            columns.insert(*col);
        }
        Demand {
            any_surface_visible: true,
            process_table: true,
            process_columns: columns,
            detail_keys: Arc::clone(&self.visible_keys),
            ..Demand::default()
        }
    }

    /// Sends the demand part while the tab is active.
    fn send_demand(&self, cx: &mut Context<TlTreeTable<Self>>) {
        if !self.active {
            return;
        }
        let Some(agg) = cx.try_global::<GlobalDemand>().map(|g| g.0.clone()) else {
            return;
        };
        let (source, window, part) = (cx.entity_id(), self.window, self.demand_part());
        agg.update(cx, |a, cx| a.set(source, window, part, cx));
    }

    /// Follows a settings change: units, heat, mode. Cell texts that depend on units change
    /// through the table's style version.
    pub fn set_settings(&mut self, settings: &Settings) {
        let net_bytes = settings.units.network == NetworkUnit::Bytes;
        for spec in self.specs.values_mut() {
            spec.template = process_column(spec.id).template(net_bytes);
        }
        self.settings = settings.clone();
    }

    /// Follows a theme change, for the heat colour.
    pub fn set_theme(&mut self, theme: Arc<TlTheme>) {
        self.theme = theme;
    }

    /// Translates every word again after a locale change. Name cell texts are rebuilt on the
    /// next install.
    pub fn refresh_strings(&mut self, cx: &App) {
        self.cell_words = CellWords::new(cx);
        self.name_words = NameWords::new(cx);
        self.header_words = HEADER_KEYS.map(|k| Arc::from(tr(cx, k).as_ref()));
        self.no_match = tr(cx, "processes-empty-no-match");
        self.tip_words = RowTipWords::new(cx);
        self.tip.borrow_mut().clear();
        self.cell_tip_words = CellTipWords::new(cx);
        self.cell_tips.borrow_mut().built_for = None;
        self.windows_tips.clear();
        for spec in self.specs.values_mut() {
            let col = process_column(spec.id);
            spec.label = tr(cx, col.label);
            spec.full_name = col.alias.map_or_else(|| spec.label.clone(), |k| tr(cx, k));
            spec.group = tr(cx, col.group.label_key());
            if let Some(key) = col.header_tooltip {
                spec.tooltip = Some(tr(cx, key));
            }
        }
        self.name_cache.clear();
        self.built_for = None;
        let snapshot = Arc::clone(&self.snapshot);
        self.install(&snapshot);
    }

    /// Shows `snapshot` when its table view was built for the current query generation. True
    /// when the rows on screen changed, so the page calls `TlTreeTable::on_new_version`.
    pub fn install(&mut self, snapshot: &Arc<Snapshot>) -> bool {
        if !self.query.accept(&snapshot.table) {
            return false;
        }
        if !Arc::ptr_eq(&self.snapshot, snapshot) {
            self.snapshot = Arc::clone(snapshot);
        }
        let changed_columns = self.sync_devices(snapshot, None);
        let p = &self.snapshot.processes;
        self.members
            .refresh(&self.snapshot.table, &p.records, &p.extras);
        let view = &self.snapshot.table;
        let key = (
            view.header.version,
            view.rows.as_ptr() as usize,
            self.snapshot.windows.header.version,
        );
        if self.built_for == Some(key) {
            return changed_columns;
        }
        self.built_for = Some(key);
        self.version += 1;
        self.rebuild_rows();
        true
    }

    /// Drops the installed view when it was not built for the current query generation: the
    /// view the hub held before the page sent its first query (pdu.55). The table then shows no
    /// rows until a view for the query arrives.
    pub fn drop_stale_view(&mut self) {
        let generation = self.query.generation();
        if self
            .query
            .shown()
            .is_none_or(|v| v.query_generation == generation)
        {
            return;
        }
        self.query.forget_shown();
        self.row_index.clear();
        self.ids.clear();
        self.sibling.clear();
        self.grouped = false;
        self.built_for = None;
        self.version += 1;
    }

    /// `row_index`, the sibling pass and the Name cell texts for the installed view. O(rows).
    fn rebuild_rows(&mut self) {
        let snapshot = Arc::clone(&self.snapshot);
        let view = &snapshot.table;
        let rows = &view.rows;

        self.row_index.clear();
        self.row_index.reserve(rows.len());
        self.ids.clear();
        for (ix, row) in rows.iter().enumerate() {
            let id = self.units.intern(&row.node);
            self.ids.push(id);
            self.row_index.insert(id, ix as u32);
        }

        sibling_pass(rows.iter().map(|r| r.depth), &mut self.sibling);
        self.grouped = rows
            .first()
            .is_some_and(|r| matches!(r.node, NodeId::Group(_)));

        group_windows(&snapshot, &mut self.app_windows);

        for (ix, row) in rows.iter().enumerate() {
            let (name, count) = match (row.group, self.kind_of(row)) {
                (_, RowKind::GroupHeader) => {
                    let Some(h) = self.header_slot(ix) else {
                        continue;
                    };
                    (&self.header_words[h], row.descendants)
                }
                (Some(g), RowKind::App | RowKind::Unit | RowKind::KernelThread) => {
                    let g = &view.groups[g as usize];
                    (&g.name, g.members)
                }
                _ => continue,
            };
            let id = self.ids[ix];
            let top = match (self.kind_of(row), row.group) {
                (RowKind::App, Some(g)) => self
                    .app_windows
                    .get(&view.groups[g as usize].key)
                    .and_then(|w| w.first().copied()),
                _ => None,
            };
            let title = top.map(|w| &snapshot.windows.windows[w as usize].caption);
            let fresh = self.name_cache.get(&id).is_some_and(|e| {
                Arc::ptr_eq(&e.name, name)
                    && e.count == count
                    && match (&e.title, title) {
                        (None, None) => true,
                        (Some((a, _)), Some(b)) => Arc::ptr_eq(a, b),
                        _ => false,
                    }
            });
            if !fresh {
                let mut primary = String::with_capacity(name.len() + 8);
                primary.push_str(name);
                let start = primary.len() + 1;
                let _ = write!(primary, " ({count})");
                let entry = NameEntry {
                    name: Arc::clone(name),
                    count,
                    count_run: start..primary.len(),
                    primary: primary.into(),
                    title: title.map(|t| (Arc::clone(t), SharedString::from(t.as_ref()))),
                };
                self.name_cache.insert(id, entry);
            }
        }
        let index = &self.row_index;
        self.name_cache.retain(|id, _| index.contains_key(id));
    }

    /// The KWin bridge is up: the windows snapshot is Valid or Stale. Without it no window
    /// button is drawn (PROC-053).
    fn bridge_up(&self) -> bool {
        matches!(
            self.snapshot.windows.header.validity,
            Validity::Valid | Validity::Stale
        )
    }

    /// The top-level windows row `ix` owns, as indices into `snapshot.windows.windows`, topmost
    /// first: a process row's own PID, an app row's member PIDs. Empty for every other row.
    fn row_windows(&self, ix: usize) -> SmallVec<[u32; 4]> {
        let row = self.row(ix);
        match self.kind_of(row) {
            RowKind::App => self
                .group(row)
                .and_then(|g| self.app_windows.get(&g.key))
                .cloned()
                .unwrap_or_default(),
            RowKind::Process => {
                let Some(r) = row
                    .record
                    .and_then(|i| self.snapshot.processes.records.get(i as usize))
                else {
                    return SmallVec::new();
                };
                let all = &self.snapshot.windows.windows;
                let start = all.partition_point(|w| w.pid < r.key.pid);
                let end = all.partition_point(|w| w.pid <= r.key.pid);
                let mut out: SmallVec<[u32; 4]> = (start as u32..end as u32).collect();
                out.sort_by_key(|&w| all[w as usize].stack_index);
                out
            }
            _ => SmallVec::new(),
        }
    }

    /// The windows behind row `ix`'s window button, or `None` when the row draws no button: no
    /// KWin bridge, or no window of its own.
    fn button_windows(&self, ix: usize) -> Option<SmallVec<[u32; 4]>> {
        if !self.bridge_up() {
            return None;
        }
        let windows = self.row_windows(ix);
        (!windows.is_empty()).then_some(windows)
    }

    /// The tooltip of row `ix`'s window button, "Windows (N)", or `None` when the row draws no
    /// button (PROC-053). The table draws the button on hovered and selected rows only.
    pub fn window_button_tip(&mut self, ix: usize, cx: &App) -> Option<SharedString> {
        let n = self.button_windows(ix)?.len();
        Some(self.windows_tip(n, cx))
    }

    /// "Windows (N)", formatted once per N.
    fn windows_tip(&mut self, n: usize, cx: &App) -> SharedString {
        if let Some((_, tip)) = self.windows_tips.iter().find(|(c, _)| *c == n) {
            return tip.clone();
        }
        let tip = tr_args(
            cx,
            "processes-name-windows",
            &[("count", FluentValue::from(n))],
        );
        if self.windows_tips.len() == self.windows_tips.inline_size() {
            self.windows_tips.clear();
        }
        self.windows_tips.push((n, tip.clone()));
        tip
    }

    /// Rebuilds the GPU N slots when the device catalog changed (PROC-033). True when the
    /// offered set changed. `state` is the stored layout at construction, else the live one.
    fn sync_devices(
        &mut self,
        snapshot: &Snapshot,
        state: Option<&tl_model::settings::ColumnState>,
    ) -> bool {
        let devices = &snapshot.devices;
        if self.devices_version == Some(devices.version) && state.is_none() {
            return false;
        }
        self.devices_version = Some(devices.version);
        let mut adapters: SmallVec<[(&Arc<str>, &Arc<str>); 4]> = devices
            .present_of(DeviceKind::Gpu)
            .filter_map(|e| match &e.identity {
                DeviceIdentity::Gpu { pci_bus_id, .. } => Some((pci_bus_id, &e.display_name)),
                _ => None,
            })
            .collect();
        adapters.sort_by(|a, b| a.0.cmp(b.0));
        for (n, (pci, name)) in adapters.iter().take(16).enumerate() {
            if let Some(spec) = self.specs.get_mut(&gpu_column(n)) {
                spec.tooltip = Some(format!("{name} ({pci})").into());
            }
        }
        let offered: Vec<ColumnId> = PROCESS_COLUMNS
            .iter()
            .filter(|c| match gpu_slot(c.id) {
                Some(n) => n < adapters.len(),
                None => c.phase <= SHIPPED_PHASE,
            })
            .map(|c| c.id)
            .collect();
        if offered == self.offered && state.is_none() {
            return false;
        }
        self.offered = offered;
        let stored = match state {
            Some(s) => s.clone(),
            None => self.layout.to_state(),
        };
        let scale = self.layout.ui_scale();
        self.layout = ColumnLayout::from_state(TableId::Processes, &stored, &self.offered_specs());
        self.layout.rebuild(scale);
        true
    }

    fn offered_specs(&self) -> Vec<ColumnSpec> {
        self.offered
            .iter()
            .filter_map(|id| self.specs.get(id).cloned())
            .collect()
    }

    fn row(&self, ix: usize) -> &TableRow {
        &self.snapshot.table.rows[ix]
    }

    fn group(&self, row: &TableRow) -> Option<&GroupRow> {
        row.group
            .and_then(|g| self.snapshot.table.groups.get(g as usize))
    }

    /// Which of the three group headers row `ix` is.
    fn header_slot(&self, ix: usize) -> Option<usize> {
        let id = self.ids[ix];
        self.headers.iter().position(|h| *h == id)
    }

    /// The row id of `node`, or `None` for a unit this delegate never showed.
    pub fn id_of(&self, node: &NodeId) -> Option<ProcRowId> {
        self.units.get(node)
    }

    fn kind_of(&self, row: &TableRow) -> RowKind {
        if let Some(g) = self.group(row) {
            return match g.kind {
                GroupRowKind::App => RowKind::App,
                GroupRowKind::Unit => RowKind::Unit,
                GroupRowKind::KernelThreads => RowKind::KernelThread,
                GroupRowKind::Pseudo => RowKind::Pseudo,
            };
        }
        match row
            .record
            .and_then(|i| self.snapshot.processes.records.get(i as usize))
        {
            Some(r) if r.flags.contains(ProcFlags::KERNEL_THREAD) => RowKind::KernelThread,
            Some(_) => RowKind::Process,
            None => match row.node {
                NodeId::Pseudo(_) => RowKind::Pseudo,
                NodeId::KernelThreads => RowKind::KernelThread,
                NodeId::Unit(_) => RowKind::Unit,
                _ => RowKind::GroupHeader,
            },
        }
    }

    /// The record or group row behind row `ix`. `None` for a group header.
    fn row_ref(&self, ix: usize) -> Option<RowRef<'_>> {
        let row = self.row(ix);
        let kind = self.kind_of(row);
        if let Some(g) = self.group(row) {
            return Some(RowRef::Group(g, kind));
        }
        row.record
            .and_then(|i| self.snapshot.processes.records.get(i as usize))
            .map(|r| RowRef::Record(r, row, kind))
    }

    fn cell_ctx(&self) -> CellCtx<'_> {
        let s = &*self.snapshot;
        let system = &s.system;
        CellCtx {
            view: &s.table,
            records: &s.processes.records,
            extras: &s.processes.extras,
            settings: &self.settings,
            words: &self.cell_words,
            members: &self.members,
            ncpu: system.cpu_topology.online.count().min(u32::from(u16::MAX)) as u16,
            online: &system.cpu_topology.online,
            mem_total: system.memory.total,
            vram_total: system.gpus.iter().map(|(_, g)| g.mem_total.value).sum(),
            boot_time_s: system.boot_time_s,
            clk_tck: CLK_TCK,
        }
    }

    /// Runs `f` on the rows of `nodes` the shown view holds, in the order given, with the cell
    /// context. Group headers and nodes not in the view are left out. For the copy commands
    /// (PROC-070).
    pub fn with_rows<R>(
        &self,
        nodes: &[NodeId],
        f: impl FnOnce(&[RowRef<'_>], &CellCtx<'_>) -> R,
    ) -> R {
        let rows: Vec<RowRef<'_>> = nodes
            .iter()
            .filter_map(|node| self.id_of(node))
            .filter_map(|id| self.row_index.get(&id))
            .filter_map(|&ix| self.row_ref(ix as usize))
            .collect();
        f(&rows, &self.cell_ctx())
    }

    /// The UI-PRC-004 tooltip of cell (`ix`, `col`), from the hovered row's cache.
    fn cell_tip(
        &self,
        ix: usize,
        row: RowRef,
        col: ColumnId,
        ctx: &CellCtx,
    ) -> Option<SharedString> {
        let mut cache = self.cell_tips.borrow_mut();
        let key = (self.snapshot.sequence, self.version, ix);
        if cache.built_for != Some(key) {
            cache.built_for = Some(key);
            cache.tips.clear();
        }
        if let Some((_, tip)) = cache.tips.iter().find(|(c, _)| *c == col) {
            return tip.clone();
        }
        let s = &*self.snapshot;
        let tip_ctx = TipCtx {
            words: &self.cell_tip_words,
            extras_p2: &s.processes.extras_p2,
            zram: s.system.memory.zram,
            power: PowerSensor::from_validity(s.system.cpu.package_power_w.validity),
        };
        let CellTips { tips, scratch, .. } = &mut *cache;
        scratch.clear();
        let tip = cell_tooltip(row, process_column(col), ctx, &tip_ctx, scratch)
            .then(|| SharedString::from(scratch.as_str().to_owned()));
        tips.push((col, tip.clone()));
        tip
    }

    fn paint_state(&self, exited: bool, subtree: bool) -> PaintState {
        let a = &self.settings.appearance;
        PaintState {
            selected: false,
            exited,
            heat_on: a.heat_coloring,
            heat_intensity_pct: a.heat_intensity_pct,
            dim_zero: self.settings.tables.dim_zero_values,
            subtree,
        }
    }

    fn client_send(&mut self, cx: &App) {
        if let Some(engine) = cx.try_global::<EngineGlobal>() {
            let client = Arc::clone(&engine.0);
            self.query.bump_and_send(&*client);
        }
    }

    fn persist(&self, cx: &mut App) {
        if cx.has_global::<TlSettings>() {
            self.query.persist(cx);
        }
    }

    /// The flags of row `ix`'s process record and when they first appeared. `None` for a row
    /// without a record. Without extras the time is unknown and reads as long ago.
    fn record_flags(&self, ix: usize) -> Option<(ProcFlags, TimeNs)> {
        let i = self.row(ix).record? as usize;
        let p = &self.snapshot.processes;
        let r = p.records.get(i)?;
        Some((r.flags, p.extras.get(i).map_or(0, |x| x.flag_since_ns)))
    }

    fn exited(&self, ix: usize) -> bool {
        self.record_flags(ix)
            .is_some_and(|(flags, _)| flags.contains(ProcFlags::EXITED))
    }

    fn clock(&self) -> TimeNs {
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(now) = self.fixed_now {
            return now;
        }
        now_boot_ns()
    }

    /// Fixes the clock the row tints read; `None` goes back to `CLOCK_BOOTTIME`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_fixed_now(&mut self, now: Option<TimeNs>) {
        self.fixed_now = now;
    }

    /// Wakes the table at `at` unless an earlier wake is pending.
    fn schedule_tint_wake(&mut self, at: TimeNs, cx: &mut Context<TlTreeTable<Self>>) {
        if self
            .tint_wake
            .as_ref()
            .is_some_and(|(due, _)| *due != 0 && *due <= at)
        {
            return;
        }
        let wait = Duration::from_nanos(at.saturating_sub(self.tint_now));
        let task = cx.spawn(
            async move |this: WeakEntity<TlTreeTable<Self>>, cx: &mut AsyncApp| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |t, cx| {
                    if let Some(wake) = &mut t.delegate.tint_wake {
                        wake.0 = 0;
                    }
                    cx.notify();
                });
            },
        );
        self.tint_wake = Some((at, task));
    }

    /// The Warning toast "Process exited" for Enter on an exited row (UI-PRC-011).
    fn toast_exited(&self, cx: &mut App) {
        let Some(window) = self.window else {
            return;
        };
        let text = tr(cx, "processes-toast-process-exited");
        // The table's action handler holds the window; push once it is released.
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx));
                }
            });
        });
    }

    fn toggle(&mut self, id: ProcRowId, cx: &mut App) {
        let Some(&ix) = self.row_index.get(&id) else {
            return;
        };
        let node = self.row(ix as usize).node.clone();
        self.toggle_node(node, cx);
    }

    /// Expands or collapses `node`, whichever row has the focus (the group header menu).
    pub fn toggle_node(&mut self, node: NodeId, cx: &mut App) {
        self.query.toggle(node);
        self.client_send(cx);
        self.persist(cx);
    }
}

/// The tree bits of every row from the rows' depths in display order, in one reverse pass
/// (E22-006, PROC-017). `out[ix]` is `(last_sibling, ancestor_continues)`: the row is the last
/// sibling when no later row at its depth comes before a shallower row, and bit k is set when
/// the ancestor at depth k + 1 is not the last sibling, so the guide line at level k continues
/// through the row (k < depth - 1, k < 64). O(rows), run once per table version.
pub fn sibling_pass(
    depths: impl DoubleEndedIterator<Item = u8> + ExactSizeIterator,
    out: &mut Vec<(bool, u64)>,
) {
    out.clear();
    out.resize(depths.len(), (true, 0));
    // Walking backwards, bit d of `seen` is set when a later row at depth d follows with no
    // shallower row in between: the open ancestor at depth d has a later sibling.
    let mut seen = [0u64; 4];
    for (ix, d) in depths.enumerate().rev() {
        let d = usize::from(d);
        let (w, b) = (d / 64, d % 64);
        let last = (seen[w] >> b) & 1 == 0;
        // Bit k of `continues` is bit k + 1 of `seen`.
        let continues = (seen[0] >> 1) | (seen[1] << 63);
        let bits = match d {
            0 | 1 => 0,
            2..=64 => continues & ((1u64 << (d - 1)) - 1),
            _ => continues,
        };
        out[ix] = (last, bits);
        // This row is now the latest at depth d, and the deeper rows after it are not its
        // siblings' subtrees any more.
        seen[w] = (seen[w] | (1 << b)) & (u64::MAX >> (63 - b));
        seen[w + 1..].iter_mut().for_each(|x| *x = 0);
    }
}

/// Each app group's windows from its member processes, topmost (lowest `stack_index`) first,
/// as indices into `snapshot.windows.windows` (PROC-008, PROC-053). Empty when the KWin bridge
/// is not there (`Unsupported`).
fn group_windows(snapshot: &Snapshot, out: &mut FxHashMap<GroupKey, SmallVec<[u32; 4]>>) {
    out.clear();
    let windows = &snapshot.windows;
    if windows.header.validity == Validity::Unsupported || windows.windows.is_empty() {
        return;
    }
    let pids: FxHashMap<u32, GroupKey> = snapshot
        .processes
        .records
        .iter()
        .map(|r| (r.key.pid, r.group))
        .collect();
    for (i, w) in windows.windows.iter().enumerate() {
        if let Some(&group) = pids.get(&w.pid) {
            out.entry(group).or_default().push(i as u32);
        }
    }
    for list in out.values_mut() {
        list.sort_by_key(|&i| windows.windows[i as usize].stack_index);
    }
}

fn restricted(v: &FieldValidity) -> bool {
    v.0.contains(&Validity::Restricted)
}

/// `gpu_0` + n. `ColumnId::ALL` is in number order from 1, and `gpu_0` to `gpu_15` are
/// numbered in a row.
fn gpu_column(n: usize) -> ColumnId {
    ColumnId::ALL[ColumnId::Gpu0 as usize - 1 + n]
}

/// The adapter slot of a `gpu_0` to `gpu_15` column.
fn gpu_slot(id: ColumnId) -> Option<usize> {
    let n = (id as usize).checked_sub(ColumnId::Gpu0 as usize)?;
    (id as usize <= ColumnId::Gpu15 as usize).then_some(n)
}

/// One spec per column this build can offer: shipped phases and every GPU N slot.
fn build_specs(settings: &Settings, window: &Window, cx: &App) -> FxHashMap<ColumnId, ColumnSpec> {
    let m = metrics(cx);
    let net_bytes = settings.units.network == NetworkUnit::Bytes;
    let mut measure = TextMetricsCache::default();
    PROCESS_COLUMNS
        .iter()
        .filter(|c| c.phase <= SHIPPED_PHASE || gpu_slot(c.id).is_some())
        .map(|c| (c.id, spec(c, net_bytes, &mut measure, m, window, cx)))
        .collect()
}

fn spec(
    c: &ProcColumn,
    net_bytes: bool,
    measure: &mut TextMetricsCache,
    m: &crate::theme::UiMetrics,
    window: &Window,
    cx: &App,
) -> ColumnSpec {
    let label = tr(cx, c.label);
    let default_width = c.default_width.unwrap_or_else(|| {
        let w = measure.template_width(&label, m.text.base, window, cx);
        (f32::from(w) / m.ui_scale.max(0.01) + LABEL_PAD).max(MEASURED_MIN_WIDTH)
    });
    let align = c.align();
    ColumnSpec {
        id: c.id,
        full_name: c.alias.map_or_else(|| label.clone(), |k| tr(cx, k)),
        label,
        group: tr(cx, c.group.label_key()),
        align,
        kind: match align {
            ColumnAlign::Left => ColumnKind::Text,
            ColumnAlign::Right => ColumnKind::Numeric,
        },
        sortable: c.sortable,
        template: c.template(net_bytes),
        default_width: px(default_width),
        tooltip: c.header_tooltip.map(|k| tr(cx, k)),
        availability: Availability::Available,
        hideable: c.id != ColumnId::Name,
    }
}

impl TreeTableDelegate for ProcessDelegate {
    type RowId = ProcRowId;
    type Column = ColumnId;
    type Sort = SortSpec;

    /// The rows of the installed view. `snapshot` can hold a view that was never installed (one
    /// the hub had before the page sent its query), so its row count is not used.
    fn row_count(&self) -> usize {
        self.ids.len()
    }

    fn row_id(&self, ix: usize) -> ProcRowId {
        self.ids[ix]
    }

    fn row_index(&self, id: ProcRowId) -> Option<usize> {
        self.row_index.get(&id).map(|&ix| ix as usize)
    }

    fn row_kind(&self, ix: usize) -> RowKind {
        self.kind_of(self.row(ix))
    }

    fn row_depth(&self, ix: usize) -> u8 {
        self.row(ix).depth
    }

    fn row_expandable(&self, ix: usize) -> Option<bool> {
        let row = self.row(ix);
        row.has_children.then_some(row.expanded)
    }

    fn row_flags(&self, ix: usize) -> RowFlags {
        let row = self.row(ix);
        let mut bits = 0;
        if let Some(r) = row
            .record
            .and_then(|i| self.snapshot.processes.records.get(i as usize))
        {
            if r.flags.contains(ProcFlags::NEW) {
                bits |= RowFlags::NEW;
            }
            if r.flags.contains(ProcFlags::EXITED) {
                bits |= RowFlags::EXITED;
            }
            if r.state == ProcState::Stopped {
                bits |= RowFlags::SUSPENDED;
            }
            if restricted(&r.validity) {
                bits |= RowFlags::RESTRICTED;
            }
        } else if let Some(g) = self.group(row) {
            if restricted(&g.validity) {
                bits |= RowFlags::RESTRICTED;
            }
            if g.frozen {
                bits |= RowFlags::FROZEN;
            }
        }
        if self.snapshot.table.header.validity == Validity::Stale {
            bits |= RowFlags::STALE;
        }
        let (last, ancestors) = self.sibling.get(ix).copied().unwrap_or((true, 0));
        if last {
            bits |= RowFlags::LAST_SIBLING;
        }
        if self.grouped && row.depth > 0 {
            bits |= RowFlags::IN_GROUP;
        }
        RowFlags {
            bits,
            ancestor_continues: ancestors,
        }
    }

    fn table_version(&self) -> u64 {
        self.version
    }

    fn guide_lines(&self) -> bool {
        self.settings.processes.tree_guide_lines
    }

    fn columns(&self) -> &ColumnLayout {
        &self.layout
    }

    fn column_spec(&self, id: ColumnId) -> &ColumnSpec {
        self.specs
            .get(&id)
            .unwrap_or_else(|| &self.specs[&ColumnId::Name])
    }

    fn cell(&self, ix: usize, col: ColumnId, out: &mut CellOut) {
        let ctx = self.cell_ctx();
        let Some(row_ref) = self.row_ref(ix) else {
            // A group header: every metric cell is blank.
            out.bits = 0;
            out.validity = Validity::Valid;
            return;
        };
        let exited =
            matches!(row_ref, RowRef::Record(r, _, _) if r.flags.contains(ProcFlags::EXITED));
        ProcessRowCells::fill(
            row_ref,
            col,
            &ctx,
            self.paint_state(exited, subtree_total(row_ref, &ctx).is_some()),
            &self.theme,
            &self.tips,
            out,
        );
        if out.want_tooltip && matches!(out.validity, Validity::Stale | Validity::Failed) {
            out.tooltip = live_tip(out.validity, &self.snapshot, now_boot_ns());
        }
        // The column's own text replaces the validity tooltip of a value or an `n/a`.
        if out.want_tooltip
            && matches!(out.validity, Validity::Valid | Validity::Unsupported)
            && let Some(tip) = self.cell_tip(ix, row_ref, col, &ctx)
        {
            out.tooltip = Some(tip);
        }
        if exited {
            // UI-PRC-011: all text of an exited row is secondary.
            out.text_color = Some(self.theme.text_secondary);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let row = self.row(ix);
        let kind = self.kind_of(row);
        let cached = self.name_cache.get(&self.ids[ix]);
        let mut d = NameCellData {
            kind,
            depth: row.depth,
            expandable: row.has_children.then_some(row.expanded),
            icon: NameIcon::None,
            primary: SharedString::default(),
            count_run: None,
            state_glyph: None,
            exited: false,
            secondary: None,
        };
        if let Some(g) = self.group(row) {
            d.icon = match &g.node {
                NodeId::Pseudo(kind) => pseudo_icon(*kind),
                _ => g
                    .icon
                    .as_ref()
                    .map_or(NameIcon::None, |i| NameIcon::File(Arc::clone(i))),
            };
            d.primary = SharedString::from(self.cell_words.group_name(g));
            if g.frozen {
                d.state_glyph = Some(StateGlyph::Frozen);
            }
        } else if let Some(r) = row
            .record
            .and_then(|i| self.snapshot.processes.records.get(i as usize))
        {
            d.icon = r
                .meta
                .icon
                .as_ref()
                .map_or(NameIcon::None, |i| NameIcon::File(Arc::clone(i)));
            d.primary = SharedString::from(&r.meta.name);
            d.exited = r.flags.contains(ProcFlags::EXITED);
            if r.state == ProcState::Stopped {
                d.state_glyph = Some(StateGlyph::Suspended);
            }
        }
        if let Some(e) = cached {
            d.primary = e.primary.clone();
            d.count_run = Some(e.count_run.clone());
            d.secondary = e.title.as_ref().map(|(_, s)| s.clone());
        }
        name_cell(d, &self.name_words)
    }

    /// The window button (PROC-053, UI-PRC-003) on a hovered or selected row that owns a
    /// top-level window, while the KWin bridge is up. A click lists the row's windows of this
    /// snapshot in a popup below the button. Its items dispatch `ActivateWindow` in the table's
    /// context, and Esc returns focus to the table.
    fn name_trailing(
        &mut self,
        ix: usize,
        _hovered: bool,
        _selected: bool,
        focused: bool,
        _window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> Option<AnyElement> {
        let windows = self.button_windows(ix)?;
        let tip = self.windows_tip(windows.len(), cx);
        let snapshot = Arc::clone(&self.snapshot);
        let table = cx.weak_entity();
        let open = move |at, window: &mut Window, cx: &mut App| {
            let Some(focus) = table.upgrade().map(|t| t.read(cx).focus_handle().clone()) else {
                return;
            };
            let all = &snapshot.windows.windows;
            let list: Vec<WindowEntry> = windows
                .iter()
                .filter_map(|&w| all.get(w as usize).cloned())
                .collect();
            let menu = TlMenu::build(window, cx, focus, move |menu, _, cx| {
                window_popup(menu, &list, cx)
            });
            open_context_menu_at(at, window, cx, menu);
        };
        let key = FxBuildHasher.hash_one(self.ids[ix]);
        Some(window_button(key, tip, focused, open, cx))
    }

    fn header_total(&self, col: ColumnId, out: &mut CellOut) -> bool {
        header_total(
            process_column(col),
            &self.snapshot.table.totals,
            &self.settings,
            out,
        )
    }

    fn sort(&self) -> SortSpec {
        self.query.sort()
    }

    fn selectable(&self, ix: usize) -> bool {
        self.row_kind(ix) != RowKind::GroupHeader
    }

    /// The row menu (E22-A2). The targets are captured now and stored on the page, so its
    /// items act on these rows even when the rows move before the click. A group header gets
    /// its own one-item menu and captures nothing.
    fn context_menu(
        &mut self,
        targets: &[ProcRowId],
        clicked_col: Option<ColumnId>,
        menu: TlMenu,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let rows: SmallVec<[usize; 8]> = targets
            .iter()
            .filter_map(|id| self.row_index(*id))
            .collect();
        if let [ix] = rows[..]
            && self.row_kind(ix) == RowKind::GroupHeader
        {
            let expanded = self.row_expandable(ix).unwrap_or(false);
            return group_header_menu(menu, expanded, self.row(ix).node.clone(), cx);
        }
        let captured = CapturedTargets::new(
            rows.iter()
                .filter_map(|&ix| capture_row(&self.snapshot, ix, self.row_kind(ix))),
            clicked_col,
        );
        if captured.items.is_empty() {
            return menu;
        }
        if let Some(page) = self.page.as_ref().and_then(WeakEntity::upgrade) {
            let stored = captured.clone();
            page.update(cx, |p, _| p.set_menu_targets(stored));
        }
        let engine_caps = self.snapshot.health.caps;
        let caps = MenuCaps {
            gdb: engine_caps.contains(EngineCaps::GDB),
            file_manager1: engine_caps.contains(EngineCaps::FILE_MANAGER1),
            ..MenuCaps::default()
        };
        row_menu(menu, &captured, &caps, window, cx)
    }

    fn on_menu_closed(&mut self, cx: &mut Context<TlTreeTable<Self>>) {
        if let Some(page) = self.page.as_ref().and_then(WeakEntity::upgrade) {
            page.update(cx, |p, _| p.clear_menu_targets());
        }
    }

    fn on_toggle_expand(&mut self, id: ProcRowId, cx: &mut Context<TlTreeTable<Self>>) {
        self.toggle(id, cx);
    }

    fn on_sort(&mut self, col: ColumnId, cx: &mut Context<TlTreeTable<Self>>) {
        let spec = process_column(col);
        if !spec.sortable {
            return;
        }
        self.query.set_sort(col, spec.kind);
        self.client_send(cx);
        self.persist(cx);
    }

    fn on_columns_changed(&mut self, layout: &ColumnLayout, cx: &mut Context<TlTreeTable<Self>>) {
        self.layout = layout.clone();
        if cx.has_global::<TlSettings>() {
            let state = layout.to_state();
            TlSettings::update(cx, |d| d.state.columns.processes = state);
        }
        self.send_demand(cx);
    }

    fn on_visible_range(&mut self, ids: &[ProcRowId], cx: &mut Context<TlTreeTable<Self>>) {
        self.visible_keys = ids
            .iter()
            .filter_map(|id| match id {
                ProcRowId::Process(key) => Some(*key),
                _ => None,
            })
            .take(DEMAND_MAX_IDS)
            .collect();
        self.send_demand(cx);
    }

    /// Enter, Alt+Enter or Ctrl+I (D-008). A group header toggles. An exited row is not
    /// actionable and shows the "Process exited" toast (UI-PRC-011). Any other row lets
    /// `OpenProperties` go on to the page, whose handler opens Properties for the focus row.
    fn on_activate(&mut self, id: ProcRowId, cx: &mut Context<TlTreeTable<Self>>) {
        let Some(ix) = self.row_index(id) else {
            return;
        };
        if self.row_kind(ix) == RowKind::GroupHeader {
            self.toggle(id, cx);
        } else if self.exited(ix) {
            self.toast_exited(cx);
        } else {
            cx.propagate();
        }
    }

    /// The NEW or EXITED tint at this frame's clock (PROC-043). `highlight_new_exited = false`
    /// draws none; the row still lingers.
    fn row_tint(&self, ix: usize, theme: &TlTheme) -> Option<Hsla> {
        let (flags, since) = self.record_flags(ix)?;
        let p = &self.settings.processes;
        row_tint(
            flags,
            since,
            self.tint_now,
            p.interval_ms,
            self.tint_animations,
            p.highlight_new_exited,
        )
        .map(|(kind, alpha)| kind.color(alpha, theme))
    }

    /// Reads the clock for this frame's tints. Requests the next animation frame while a
    /// visible tint fades; otherwise sets a timer for the next fade start. Allocates nothing
    /// unless it sets a new timer.
    fn before_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) {
        if self.tip_view.is_none() {
            let tip = Rc::clone(&self.tip);
            self.tip_view = Some(cx.new(|_| RowTooltip::new(tip)).into());
        }
        let now = self.clock();
        self.tint_now = now;
        self.tint_animations = self.settings.motion.animations && !cx.reduce_motion();
        let p = &self.settings.processes;
        if !p.highlight_new_exited {
            return;
        }
        let mut next: Option<TimeNs> = None;
        for ix in range {
            if let Some((flags, since)) = self.record_flags(ix)
                && let Some(at) =
                    tint_deadline(flags, since, now, p.interval_ms, self.tint_animations, true)
            {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        match next {
            Some(at) if at <= now => {
                if let Some(driver) = AnimationDriver::try_global(window, cx) {
                    let id = cx.entity_id();
                    driver.update(cx, |d, cx| d.request(id, window, cx));
                }
            }
            Some(at) => self.schedule_tint_wake(at, cx),
            None => {}
        }
    }

    /// The table calls this for the hovered row only, once its tooltips are ready. The lines
    /// are built on the first call for a row and snapshot. Group headers have no tooltip.
    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        let view = self.tip_view.as_ref()?;
        let row = self.row_ref(ix)?;
        let key = (self.snapshot.sequence, self.version, ix);
        self.tip
            .borrow_mut()
            .update(key, row, &self.snapshot, &self.tip_words)
            .then(|| view.clone())
    }

    /// An empty view under a filter: "No processes match the filter" (UI-PRC-024). The engine
    /// sends no group header when nothing matches at all.
    fn empty_text(&self) -> Option<SharedString> {
        (!self.query.filter().is_empty()).then(|| self.no_match.clone())
    }

    /// The no-match text comes with "Clear filter" (UI-PRC-024).
    fn empty_clear_filter(&self) -> bool {
        !self.query.filter().is_empty()
    }

    fn cell_tip_delay(&self) -> Duration {
        DELAY_PROCESS_CELL
    }

    /// The command line keeps its 80-column line breaks (UI-PRC-004).
    fn cell_tip_content(&self, col: ColumnId, tip: SharedString) -> TlTooltip {
        if col == ColumnId::Cmdline {
            TlTooltip::preformatted(tip)
        } else {
            TlTooltip::text(tip)
        }
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::Two
    }

    fn columns_fixed(&self) -> bool {
        false
    }

    /// R-216: double-click toggles a row with children and never opens Properties.
    fn double_click_expands(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::health::SourceId;
    use tl_model::snapshot::Snapshot;
    use tl_model::time::BootId;
    use tl_model::validity::Validity;

    use super::live_tip;

    const SEC: u64 = 1_000_000_000;

    #[test]
    fn live_tip_stale_age_and_failed_error() {
        let mut s = Snapshot::empty(BootId(0), 0);
        Arc::make_mut(&mut s.processes).header.observed_end = 10 * SEC;
        Arc::make_mut(&mut s.health).sources[SourceId::Processes.index()].last_error =
            Some("permission denied".into());

        let stale = live_tip(Validity::Stale, &s, 22 * SEC).expect("Stale has a tooltip");
        assert_eq!(stale.as_ref(), "Last updated 12s ago");
        let failed = live_tip(Validity::Failed, &s, 22 * SEC).expect("Failed has a tooltip");
        assert_eq!(failed.as_ref(), "Could not read: permission denied");
        for v in [Validity::Valid, Validity::WarmingUp, Validity::Restricted] {
            assert!(live_tip(v, &s, 22 * SEC).is_none(), "{v:?}");
        }
    }
}
