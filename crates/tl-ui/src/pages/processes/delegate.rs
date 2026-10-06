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

use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{AnyView, AnyWindowHandle, App, Context, Div, Pixels, SharedString, Window, px};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceIdentity, DeviceKind};
use tl_model::ids::{ColumnId, GroupKey, NodeId, ProcessKey, PseudoKind};
use tl_model::process::{GroupRow, GroupRowKind, ProcFlags, ProcState};
use tl_model::settings::{NetworkUnit, Settings, UiState};
use tl_model::snapshot::Snapshot;
use tl_model::table::{SortSpec, TableRow, UnitRef};
use tl_model::validity::{FieldValidity, Validity};

use super::cells::{
    CellCtx, CellWords, DEFAULT_VISIBLE, GroupMembers, PROCESS_COLUMNS, PaintState, ProcColumn,
    RowRef, fill_cell, heat_background, process_column, render_metric_cell,
};
use super::header_totals::header_total;
use super::name_cell::{NameCellData, NameIcon, NameWords, StateGlyph, name_cell};
use super::table::{GroupHeader, ProcessQuery};
use crate::app::EngineGlobal;
use crate::components::menu::TlMenu;
use crate::components::validity::{GlyphCtx, tooltip_text};
use crate::demand::GlobalDemand;
use crate::i18n::tr;
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
/// and Failed ("Could not read: {error}") need per-cell text and get none here.
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

/// Process-row cell filling and painting, shared by every table that shows process rows
/// (Processes, Services child rows).
pub struct ProcessRowCells;

impl ProcessRowCells {
    /// Fills `out` for one cell (E22-003): the value through `fill_cell`, the heat fill in
    /// `out.background` and the validity tooltip in `out.tooltip`. The table attaches the
    /// tooltip on the hovered row only. Allocates nothing unless `out.want_text` is set.
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
        out.tooltip = tips.get(out.validity);
    }

    /// One filled cell other than Name, `width` wide. `text` is the cached display text of
    /// `out`. On a `STALE` row the table dims the whole row, so a Stale cell is not dimmed a
    /// second time here.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        out: &CellOut,
        text: SharedString,
        col: ColumnId,
        width: Pixels,
        st: PaintState,
        row_stale: bool,
        theme: &TlTheme,
        cx: &App,
    ) -> Div {
        let spec = process_column(col);
        if row_stale && out.validity == Validity::Stale {
            let undimmed = CellOut {
                bits: out.bits,
                validity: Validity::Valid,
                background: None,
                ..CellOut::default()
            };
            return render_metric_cell(&undimmed, text, spec, width, st, theme, cx);
        }
        render_metric_cell(out, text, spec, width, st, theme, cx)
    }
}

/// A cached Name cell text: `"<name> (N)"` for app, unit, Kernel threads and group header
/// rows, keyed by the name's pointer and the count so an unchanged row is not formatted again.
struct NameEntry {
    name: Arc<str>,
    count: u32,
    primary: SharedString,
    count_run: Range<usize>,
}

/// The process table delegate. The page installs each snapshot with [`Self::install`] and calls
/// `TlTreeTable::on_new_version` when it returns true.
pub struct ProcessDelegate {
    query: ProcessQuery,
    /// The snapshot the rows came from (or the paused one, E22-024).
    snapshot: Arc<Snapshot>,
    /// `(view.header.version, rows pointer)` of the last rebuild.
    built_for: Option<(u64, usize)>,
    /// Bumps on every rebuild.
    version: u64,
    row_index: FxHashMap<ProcRowId, u32>,
    /// The id of each row, by index.
    ids: Vec<ProcRowId>,
    units: UnitIds,
    /// Per row: last sibling, ancestor-continues bits (E22-006).
    sibling: Vec<(bool, u64)>,
    layout: ColumnLayout,
    /// Every column this build can offer, `gpu_0` to `gpu_15` included.
    specs: FxHashMap<ColumnId, ColumnSpec>,
    /// The columns offered now: phase-shipped columns and the present GPU adapters' slots.
    offered: Vec<ColumnId>,
    /// `DeviceCatalog::version` the GPU slots were built for.
    devices_version: Option<u64>,
    name_cache: FxHashMap<ProcRowId, NameEntry>,
    members: GroupMembers,
    cell_words: CellWords,
    name_words: NameWords,
    /// "Apps", "Background processes", "System processes".
    header_words: [Arc<str>; 3],
    headers: [ProcRowId; 3],
    tips: ValidityTips,
    settings: Settings,
    theme: Arc<TlTheme>,
    /// The window the table is in, for the demand part (hidden detection, ADR-008).
    window: Option<AnyWindowHandle>,
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
            layout: ColumnLayout::defaults(TableId::Processes, &[], &[], &[]),
            specs,
            offered: Vec::new(),
            devices_version: None,
            name_cache: FxHashMap::default(),
            members: GroupMembers::default(),
            cell_words: CellWords::new(cx),
            name_words: NameWords::new(cx),
            header_words: HEADER_KEYS.map(|k| Arc::from(tr(cx, k).as_ref())),
            headers,
            tips: ValidityTips::default(),
            settings: settings.clone(),
            theme: current(cx).clone(),
            window: None,
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
        let key = (view.header.version, view.rows.as_ptr() as usize);
        if self.built_for == Some(key) {
            return changed_columns;
        }
        self.built_for = Some(key);
        self.version += 1;
        self.rebuild_rows();
        true
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

        self.sibling.clear();
        self.sibling.resize(rows.len(), (true, 0));
        // Walking backwards, `seen[d]` is true when a later row at depth d follows with no
        // shallower row in between: the open ancestor at depth d has a later sibling.
        let mut seen = [false; 256];
        for (ix, row) in rows.iter().enumerate().rev() {
            let d = usize::from(row.depth);
            let mut bits = 0u64;
            for (k, &on) in seen.iter().enumerate().take(d.min(64)) {
                if on {
                    bits |= 1 << k;
                }
            }
            self.sibling[ix] = (!seen[d], bits);
            seen[d] = true;
            seen[d + 1..].iter_mut().for_each(|s| *s = false);
        }

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
            let fresh = self
                .name_cache
                .get(&id)
                .is_some_and(|e| Arc::ptr_eq(&e.name, name) && e.count == count);
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
                };
                self.name_cache.insert(id, entry);
            }
        }
        let index = &self.row_index;
        self.name_cache.retain(|id, _| index.contains_key(id));
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

    fn paint_state(&self, exited: bool) -> PaintState {
        let a = &self.settings.appearance;
        PaintState {
            selected: false,
            exited,
            heat_on: a.heat_coloring,
            heat_intensity_pct: a.heat_intensity_pct,
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

    fn toggle(&mut self, id: ProcRowId, cx: &mut App) {
        let Some(&ix) = self.row_index.get(&id) else {
            return;
        };
        let node = self.row(ix as usize).node.clone();
        self.query.toggle(node);
        self.client_send(cx);
        self.persist(cx);
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

    fn row_count(&self) -> usize {
        self.snapshot.table.rows.len()
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
        } else if let Some(g) = self.group(row)
            && restricted(&g.validity)
        {
            bits |= RowFlags::RESTRICTED;
        }
        if self.snapshot.table.header.validity == Validity::Stale {
            bits |= RowFlags::STALE;
        }
        let (last, ancestors) = self.sibling.get(ix).copied().unwrap_or((true, 0));
        if last {
            bits |= RowFlags::LAST_SIBLING;
        }
        RowFlags {
            bits,
            ancestor_continues: ancestors,
        }
    }

    fn table_version(&self) -> u64 {
        self.version
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
        let row = self.row(ix);
        let kind = self.kind_of(row);
        let ctx = self.cell_ctx();
        let row_ref = if let Some(g) = self.group(row) {
            RowRef::Group(g, kind)
        } else if let Some(r) = row
            .record
            .and_then(|i| self.snapshot.processes.records.get(i as usize))
        {
            RowRef::Record(r, row, kind)
        } else {
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
            self.paint_state(exited),
            &self.theme,
            &self.tips,
            out,
        );
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
        };
        if let Some(g) = self.group(row) {
            d.icon = g
                .icon
                .as_ref()
                .map_or(NameIcon::None, |i| NameIcon::File(Arc::clone(i)));
            d.primary = SharedString::from(&g.name);
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
        }
        name_cell(d, &self.name_words)
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

    fn context_menu(
        &mut self,
        _: &[ProcRowId],
        _: ColumnId,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
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
    }

    fn on_visible_range(&mut self, ids: &[ProcRowId], cx: &mut Context<TlTreeTable<Self>>) {
        let Some(agg) = cx.try_global::<GlobalDemand>().map(|g| g.0.clone()) else {
            return;
        };
        let keys: Arc<[ProcessKey]> = ids
            .iter()
            .filter_map(|id| match id {
                ProcRowId::Process(key) => Some(*key),
                _ => None,
            })
            .take(DEMAND_MAX_IDS)
            .collect();
        let part = Demand {
            detail_keys: keys,
            ..Demand::default()
        };
        let source = cx.entity_id();
        let window = self.window;
        agg.update(cx, |a, cx| a.set(source, window, part, cx));
    }

    /// Enter, Alt+Enter or Ctrl+I (D-008). A group header toggles. Any other row lets
    /// `OpenProperties` go on to the page, whose handler opens Properties for the focus row.
    fn on_activate(&mut self, id: ProcRowId, cx: &mut Context<TlTreeTable<Self>>) {
        let Some(ix) = self.row_index(id) else {
            return;
        };
        if self.row_kind(ix) == RowKind::GroupHeader {
            self.toggle(id, cx);
        } else {
            cx.propagate();
        }
    }

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn empty_text(&self) -> Option<SharedString> {
        None
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
