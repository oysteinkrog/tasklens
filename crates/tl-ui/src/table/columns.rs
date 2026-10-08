//! Column model shared by every table: per-column specs and the per-table layout (ADR-009,
//! ADR-011, PROC-022, PROC-023, UI-PRC-005, UI-PRC-007).
//!
//! Column state is kept by column id, never by index. The layout is generic over the id type
//! (`TableColumn`): `ColumnId` for Processes and History, `SysColumnId` for the system tables.
//! Widths are stored in logical px at 100% and scaled by `UiMetrics.ui_scale` only in `rebuild`,
//! which precomputes the x offsets that render and hit testing read without allocating.

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{App, Pixels, SharedString, Window, gpui, px};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use smallvec::{SmallVec, smallvec};
use tl_model::ids::ColumnId;
use tl_model::settings::ColumnState;
use tl_model::table::{SortSpec, SysColumnId, SysSortSpec, SysTable};

use crate::table::tree_table::ColumnResizeMode;
use crate::text::metrics::TextMetricsCache;
use crate::theme::metrics::UiMetrics;

/// The five tables of canonical table (a) `[columns.*]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(crate = "gpui::private::schemars")]
pub enum TableId {
    Processes,
    History,
    Services,
    Startup,
    Connections,
}

impl TableId {
    /// The `state.toml` key under `[columns.*]` and `[sort.*]`.
    pub fn state_key(self) -> &'static str {
        match self {
            Self::Processes => "processes",
            Self::History => "history",
            Self::Services => "services",
            Self::Startup => "startup",
            Self::Connections => "connections",
        }
    }
}

/// A column id a `ColumnLayout` can hold: `ColumnId` or `SysColumnId`.
pub trait TableColumn: Copy + Eq + Hash + Debug + 'static {
    /// The `state.toml` string, as `as_str` returns it.
    fn key(self) -> &'static str;
    /// The id for `key` in `table`'s id set. `None` for an unknown key or a table of the other
    /// id type.
    fn parse(table: TableId, key: &str) -> Option<Self>;
    /// True for the column that stays first and visible: Name on the process tables. The system
    /// tables have none.
    fn pinned_first(self) -> bool;
    /// The column's own minimum width, logical px at 100%. [`min_width`] uses it when it is
    /// larger than the kind's minimum. `None` (the default) leaves the kind's minimum.
    fn min_width(self) -> Option<f32> {
        None
    }
}

impl TableColumn for ColumnId {
    fn key(self) -> &'static str {
        self.as_str()
    }

    fn parse(table: TableId, key: &str) -> Option<Self> {
        match table {
            TableId::Processes | TableId::History => ColumnId::from_str(key),
            TableId::Services | TableId::Startup | TableId::Connections => None,
        }
    }

    fn pinned_first(self) -> bool {
        self == ColumnId::Name
    }
}

impl TableColumn for SysColumnId {
    fn key(self) -> &'static str {
        self.as_str()
    }

    fn parse(table: TableId, key: &str) -> Option<Self> {
        let sys = match table {
            TableId::Services => SysTable::Services,
            TableId::Startup => SysTable::Startup,
            TableId::Connections => SysTable::Connections,
            TableId::Processes | TableId::History => return None,
        };
        SysColumnId::from_str(sys, key)
    }

    fn pinned_first(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnAlign {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnKind {
    Text,
    Path,
    Numeric,
}

/// `NotOnThisSystem` shows the picker note "n/a on this system".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Availability {
    Available,
    NotOnThisSystem,
}

/// One column as the header and the picker see it. The owner of each table supplies the specs.
#[derive(Clone, Debug)]
pub struct ColumnSpec<C: TableColumn = ColumnId> {
    pub id: C,
    /// Header line 1, e.g. "Memory".
    pub label: SharedString,
    /// Picker name, e.g. "Working set private". Equals `label` when there is no alias (R-065).
    pub full_name: SharedString,
    /// Picker heading, e.g. "Memory".
    pub group: SharedString,
    /// Text and Path: Left. Numeric: Right.
    pub align: ColumnAlign,
    pub kind: ColumnKind,
    /// False for PSS, USS, Cycles, Cycles delta, Context switches, Context switches delta.
    pub sortable: bool,
    /// Numeric width template, e.g. "100.0%" or "999 MiB" (canonical table (d)).
    pub template: Option<&'static str>,
    /// Logical px at 100%.
    pub default_width: Pixels,
    /// Header tooltip (UI-PRC-005).
    pub tooltip: Option<SharedString>,
    pub availability: Availability,
    /// False only for Name.
    pub hideable: bool,
}

/// Why a layout change was refused. The caller shows the toast.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnError {
    NameCannotBeHidden,
    /// Toast: "At least one column must stay visible".
    LastVisible,
    NameMustStayFirst,
    Unknown,
}

/// Order, visibility and widths of one table's columns, with precomputed x offsets.
#[derive(Clone, Debug)]
pub struct ColumnLayout<C: TableColumn = ColumnId> {
    pub table: TableId,
    /// Every known column. Hidden ones keep their place (UI-PRC-007).
    pub order: Vec<C>,
    pub visible: FxHashSet<C>,
    /// Logical px at 100%, remembered for hidden columns too.
    pub widths: FxHashMap<C, Pixels>,
    /// x offset of each visible column in order, scaled.
    offsets: Arc<[Pixels]>,
    visible_order: Arc<[C]>,
    total_width: Pixels,
    ui_scale: f32,
    /// Bumps on every change; offsets are rebuilt then.
    version: u64,
}

/// Equal when the stored state and the scale match. `version` is ignored, and the derived
/// fields follow from the rest.
impl<C: TableColumn> PartialEq for ColumnLayout<C> {
    fn eq(&self, other: &Self) -> bool {
        self.table == other.table
            && self.order == other.order
            && self.visible == other.visible
            && self.widths == other.widths
            && self.ui_scale == other.ui_scale
    }
}

/// The layout of the Processes and History tables.
pub type ProcessColumnLayout = ColumnLayout<ColumnId>;
/// The layout of the Services, Startup and Connections tables.
pub type SysColumnLayout = ColumnLayout<SysColumnId>;

impl<C: TableColumn> ColumnLayout<C> {
    /// Builds the layout from `[columns.<table>]`. Ids not in `specs` are dropped with a log line
    /// (ADR-011). Spec ids missing from `order` are appended hidden, in spec order. Name is forced
    /// first and visible. Offsets are built at scale 1.0; call `rebuild` with the real scale.
    pub fn from_state(table: TableId, state: &ColumnState, specs: &[ColumnSpec<C>]) -> Self {
        let known = |s: &str| -> Option<C> {
            let id = C::parse(table, s).filter(|id| specs.iter().any(|sp| sp.id == *id));
            if id.is_none() {
                tracing::info!(
                    target: "tasklens::ui",
                    table = table.state_key(),
                    column = s,
                    "unknown column id dropped"
                );
            }
            id
        };

        let mut order = Vec::with_capacity(specs.len());
        let mut seen = FxHashSet::default();
        for id in state.order.iter().filter_map(|s| known(s)) {
            if seen.insert(id) {
                order.push(id);
            }
        }
        let visible: FxHashSet<C> = state
            .visible
            .iter()
            .filter_map(|s| known(s))
            .filter(|id| seen.contains(id))
            .collect();
        for spec in specs {
            if seen.insert(spec.id) {
                order.push(spec.id);
            }
        }

        let mut widths: FxHashMap<C, Pixels> = specs
            .iter()
            .map(|spec| (spec.id, spec.default_width))
            .collect();
        for (s, &w) in &state.widths {
            if let Some(id) = known(s) {
                widths.insert(id, px(w as f32));
            }
        }

        Self::build(table, order, visible, widths)
    }

    /// The `[columns.<table>]` value for this layout. Widths round to whole logical px.
    pub fn to_state(&self) -> ColumnState {
        ColumnState {
            order: self.order.iter().map(|id| id.key().to_owned()).collect(),
            visible: self
                .order
                .iter()
                .filter(|id| self.visible.contains(id))
                .map(|id| id.key().to_owned())
                .collect(),
            widths: self
                .order
                .iter()
                .filter_map(|id| {
                    let w = self.widths.get(id)?;
                    Some((id.key().to_owned(), f32::from(*w).round().max(0.0) as u32))
                })
                .collect(),
        }
    }

    /// The owner's default layout. Ids not in `specs` are ignored; spec ids missing from
    /// `default_order` are appended hidden. Widths come from `ColumnSpec::default_width`.
    pub fn defaults(
        table: TableId,
        specs: &[ColumnSpec<C>],
        default_order: &[C],
        default_visible: &[C],
    ) -> Self {
        let in_specs = |id: &C| specs.iter().any(|sp| sp.id == *id);
        let mut order = Vec::with_capacity(specs.len());
        let mut seen = FxHashSet::default();
        for &id in default_order.iter().filter(|id| in_specs(id)) {
            if seen.insert(id) {
                order.push(id);
            }
        }
        for spec in specs {
            if seen.insert(spec.id) {
                order.push(spec.id);
            }
        }
        let visible = default_visible
            .iter()
            .copied()
            .filter(|id| in_specs(id))
            .collect();
        let widths = specs
            .iter()
            .map(|spec| (spec.id, spec.default_width))
            .collect();
        Self::build(table, order, visible, widths)
    }

    fn build(
        table: TableId,
        mut order: Vec<C>,
        mut visible: FxHashSet<C>,
        widths: FxHashMap<C, Pixels>,
    ) -> Self {
        if let Some(pos) = order.iter().position(|id| id.pinned_first()) {
            order[..=pos].rotate_right(1);
            visible.insert(order[0]);
        }
        let mut layout = Self {
            table,
            order,
            visible,
            widths,
            offsets: Arc::from([]),
            visible_order: Arc::from([]),
            total_width: Pixels::ZERO,
            ui_scale: 1.0,
            version: 0,
        };
        layout.rebuild(1.0);
        layout
    }

    /// Visible ids in order.
    pub fn visible_columns(&self) -> &Arc<[C]> {
        &self.visible_order
    }

    /// x offset of each entry of `visible_columns`, scaled.
    pub fn offsets(&self) -> &Arc<[Pixels]> {
        &self.offsets
    }

    /// Sum of the visible widths, scaled.
    pub fn total_width(&self) -> Pixels {
        self.total_width
    }

    /// Width of `id` scaled by the last `rebuild` scale. Zero for an id not in the layout.
    pub fn width(&self, id: C) -> Pixels {
        self.widths
            .get(&id)
            .map_or(Pixels::ZERO, |w| *w * self.ui_scale)
    }

    /// Shows or hides `id`. Showing restores its remembered width and its old slot in `order`.
    pub fn set_visible(&mut self, id: C, on: bool) -> Result<(), ColumnError> {
        if !self.order.contains(&id) {
            return Err(ColumnError::Unknown);
        }
        if on {
            if self.visible.insert(id) {
                self.rebuild(self.ui_scale);
            }
            return Ok(());
        }
        if id.pinned_first() {
            return Err(ColumnError::NameCannotBeHidden);
        }
        if !self.visible.contains(&id) {
            return Ok(());
        }
        if self.visible.len() == 1 {
            return Err(ColumnError::LastVisible);
        }
        self.visible.remove(&id);
        self.rebuild(self.ui_scale);
        Ok(())
    }

    /// Sets the width of `id` from scaled px, as `width` returns them. Stored at 100%.
    pub fn set_width(&mut self, id: C, w: Pixels) {
        if let Some(slot) = self.widths.get_mut(&id) {
            *slot = px(f32::from(w) / self.ui_scale);
            self.rebuild(self.ui_scale);
        }
    }

    /// Moves `id` before `before`, or to the end on `None`. Nothing may move Name or go before it.
    pub fn move_column(&mut self, id: C, before: Option<C>) -> Result<(), ColumnError> {
        if id.pinned_first() || before.is_some_and(C::pinned_first) {
            return Err(ColumnError::NameMustStayFirst);
        }
        let from = self
            .order
            .iter()
            .position(|c| *c == id)
            .ok_or(ColumnError::Unknown)?;
        if let Some(b) = before {
            if !self.order.contains(&b) {
                return Err(ColumnError::Unknown);
            }
            if b == id {
                return Ok(());
            }
        }
        self.order.remove(from);
        let to = before
            .and_then(|b| self.order.iter().position(|c| *c == b))
            .unwrap_or(self.order.len());
        self.order.insert(to, id);
        self.rebuild(self.ui_scale);
        Ok(())
    }

    /// Recomputes offsets, visible order and total width at `ui_scale`, and bumps `version`.
    pub fn rebuild(&mut self, ui_scale: f32) {
        self.ui_scale = ui_scale;
        let visible_order: Arc<[C]> = self
            .order
            .iter()
            .copied()
            .filter(|id| self.visible.contains(id))
            .collect();
        let mut x = Pixels::ZERO;
        let offsets: Arc<[Pixels]> = visible_order
            .iter()
            .map(|id| {
                let at = x;
                x += self.width(*id);
                at
            })
            .collect();
        self.visible_order = visible_order;
        self.offsets = offsets;
        self.total_width = x;
        self.version += 1;
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The scale of the last `rebuild`.
    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    /// The visible column under `x` (scaled, from the left edge of the first column).
    pub fn column_at_x(&self, x: Pixels) -> Option<C> {
        if x < Pixels::ZERO || x >= self.total_width {
            return None;
        }
        let i = self.offsets.partition_point(|o| *o <= x);
        self.visible_order.get(i.checked_sub(1)?).copied()
    }

    /// The visible column whose right edge lies within `half_band` of `x`, nearest first. This
    /// is the resize hit band, 4 px each side (UI-PRC-005).
    pub fn boundary_near_x(&self, x: Pixels, half_band: Pixels) -> Option<C> {
        let n = self.visible_order.len();
        let right = |i: usize| -> Pixels {
            if i + 1 < n {
                self.offsets[i + 1]
            } else {
                self.total_width
            }
        };
        // Right edges are offsets[1..] then total_width. Find the first edge at or past x; the
        // edge before it may be nearer.
        let i = self
            .offsets
            .get(1..)
            .map_or(0, |r| r.partition_point(|o| *o < x));
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter(|&i| i < n)
            .map(|i| (i, (right(i) - x).abs()))
            .filter(|(_, d)| *d <= half_band)
            .min_by(|a, b| a.1.cmp(&b.1))
            .map(|(i, _)| self.visible_order[i])
    }
}

/// The Name column's minimum width, logical px at 100% (R-190).
pub const NAME_MIN_WIDTH: f32 = 120.0;

/// Minimum width of a column, scaled (PROC-023, UI-PRC-005). Numeric: template width + 16
/// padding, + 12 more on the sort column for the arrow. Name (the pinned first column): 120,
/// the R-190 depth-0 app text start 78 plus 42 of text. Other Text and Path columns: 60. A
/// column's own [`TableColumn::min_width`] raises it.
///
/// The template is measured in the table font at `t_base` with `tnum`, so it follows scale
/// changes. `TextMetricsCache` measures through the window's text system, hence `window`.
pub fn min_width<C: TableColumn>(
    spec: &ColumnSpec<C>,
    is_sort_column: bool,
    metrics: &mut TextMetricsCache,
    m: &UiMetrics,
    window: &Window,
    cx: &App,
) -> Pixels {
    let kind_min = kind_min_width(spec, is_sort_column, metrics, m, window, cx);
    match spec.id.min_width() {
        Some(own) => kind_min.max(px(own * m.ui_scale)),
        None => kind_min,
    }
}

fn kind_min_width<C: TableColumn>(
    spec: &ColumnSpec<C>,
    is_sort_column: bool,
    metrics: &mut TextMetricsCache,
    m: &UiMetrics,
    window: &Window,
    cx: &App,
) -> Pixels {
    match (spec.kind, spec.template) {
        (ColumnKind::Numeric, Some(template)) => {
            let text = metrics.template_width(template, m.text.base, window, cx);
            let arrow = if is_sort_column {
                m.sp[5]
            } else {
                Pixels::ZERO
            };
            text + m.sp[6] + arrow
        }
        _ if spec.id.pinned_first() => px(NAME_MIN_WIDTH * m.ui_scale),
        _ => px(60.0 * m.ui_scale),
    }
}

/// Drag of the right boundary of `col` to `new_width` (UI-PRC-005, R-075). `new_width`,
/// `viewport` and `mins` are logical px at 100%, as `widths` stores them; a column missing from
/// `mins` has no minimum. Offsets are rebuilt once at the layout's current scale.
///
/// The dragged width is clamped to its minimum. `Proportional`: the visible columns right of
/// `col` share `viewport` minus the widths up to and including `col`, in proportion to their
/// current widths, each at least its minimum; when the minimums do not fit, they sit at their
/// minimums and the table overflows. `LastColumn`: the last visible column takes the difference,
/// clamped to its minimum. When `col` is the last visible column, only it changes.
pub fn resize<C: TableColumn>(
    layout: &mut ColumnLayout<C>,
    col: C,
    new_width: Pixels,
    mode: ColumnResizeMode,
    viewport: Pixels,
    mins: &FxHashMap<C, Pixels>,
) {
    let visible = layout.visible_order.clone();
    let Some(pos) = visible.iter().position(|c| *c == col) else {
        return;
    };
    let min_of = |c: &C| mins.get(c).copied().unwrap_or(Pixels::ZERO);
    let width_of =
        |layout: &ColumnLayout<C>, c: &C| layout.widths.get(c).copied().unwrap_or(Pixels::ZERO);
    let old = width_of(layout, &col);
    let new = new_width.max(min_of(&col));
    layout.widths.insert(col, new);
    let right = &visible[pos + 1..];
    if let Some(last) = right.last() {
        match mode {
            ColumnResizeMode::Proportional => {
                let left = visible[..=pos]
                    .iter()
                    .fold(Pixels::ZERO, |sum, c| sum + width_of(layout, c));
                share_proportionally(layout, right, viewport - left, &min_of);
            }
            ColumnResizeMode::LastColumn => {
                let w = (width_of(layout, last) + old - new).max(min_of(last));
                layout.widths.insert(*last, w);
            }
        }
    }
    layout.rebuild(layout.ui_scale);
}

/// Gives `cols` the width `room` in proportion to their current widths. A column whose share
/// is below its minimum is fixed at the minimum and the rest is shared again among the others.
fn share_proportionally<C: TableColumn>(
    layout: &mut ColumnLayout<C>,
    cols: &[C],
    room: Pixels,
    min_of: &impl Fn(&C) -> Pixels,
) {
    let current: SmallVec<[(C, f32); 24]> = cols
        .iter()
        .map(|c| (*c, layout.widths.get(c).map_or(0.0, |w| w.as_f32())))
        .collect();
    let mut fixed: SmallVec<[bool; 24]> = smallvec![false; current.len()];
    let mut room = room.as_f32();
    loop {
        let free: f32 = current
            .iter()
            .zip(&fixed)
            .filter(|(_, f)| !**f)
            .map(|((_, w), _)| *w)
            .sum();
        let n_free = fixed.iter().filter(|f| !**f).count();
        if n_free == 0 {
            return;
        }
        let shares: SmallVec<[f32; 24]> = current
            .iter()
            .map(|(_, w)| {
                if free > 0.0 {
                    room * w / free
                } else {
                    room / n_free as f32
                }
            })
            .collect();
        let mut clamped = false;
        for (i, (c, _)) in current.iter().enumerate() {
            let min = min_of(c).as_f32();
            if !fixed[i] && shares[i] < min {
                fixed[i] = true;
                clamped = true;
                room -= min;
                layout.widths.insert(*c, px(min));
            }
        }
        if !clamped {
            for (i, (c, _)) in current.iter().enumerate() {
                if !fixed[i] {
                    layout.widths.insert(*c, px(shares[i]));
                }
            }
            return;
        }
    }
}

/// The fitted width of one column, scaled (UI-PRC-006): the largest of its minimum, the widest
/// measured text + 16 and the header label + 16 + 12 (room for the sort arrow).
pub fn fit_width(min: Pixels, widest_text: Pixels, label: Pixels, m: &UiMetrics) -> Pixels {
    min.max(widest_text + m.sp[6])
        .max(label + m.sp[6] + m.sp[5])
}

/// The sort after a header click (UI-PRC-005, PROC-047). Same column: flip direction. New
/// column: Numeric starts descending, Text and Path ascending. A free function because
/// `SortSpec` lives in tl-model.
pub fn next_sort(current: &SortSpec, col: ColumnId, kind: ColumnKind) -> SortSpec {
    SortSpec {
        column: col,
        ascending: next_ascending(current.column, current.ascending, col, kind),
    }
}

/// `next_sort` for a system table.
pub fn next_sys_sort(current: &SysSortSpec, col: SysColumnId, kind: ColumnKind) -> SysSortSpec {
    SysSortSpec {
        column: col,
        ascending: next_ascending(current.column, current.ascending, col, kind),
    }
}

fn next_ascending<C: TableColumn>(current: C, ascending: bool, col: C, kind: ColumnKind) -> bool {
    if current == col {
        !ascending
    } else {
        kind != ColumnKind::Numeric
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gpui_kit::{TestAppContext, TestDispatcher};
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::text::fonts::load_bundled_fonts;
    use crate::theme::metrics::compute;

    fn spec(id: ColumnId, kind: ColumnKind, width: f32) -> ColumnSpec {
        let label = SharedString::new(id.as_str());
        ColumnSpec {
            id,
            label: label.clone(),
            full_name: label.clone(),
            group: label,
            align: if kind == ColumnKind::Numeric {
                ColumnAlign::Right
            } else {
                ColumnAlign::Left
            },
            kind,
            sortable: true,
            template: (kind == ColumnKind::Numeric).then_some("100.0%"),
            default_width: px(width),
            tooltip: None,
            availability: Availability::Available,
            hideable: id != ColumnId::Name,
        }
    }

    fn specs() -> Vec<ColumnSpec> {
        vec![
            spec(ColumnId::Name, ColumnKind::Text, 248.0),
            spec(ColumnId::User, ColumnKind::Text, 92.0),
            spec(ColumnId::Pid, ColumnKind::Numeric, 96.0),
            spec(ColumnId::Cpu, ColumnKind::Numeric, 63.0),
            spec(ColumnId::Memory, ColumnKind::Numeric, 92.0),
        ]
    }

    fn state(order: &[&str], visible: &[&str], widths: &[(&str, u32)]) -> ColumnState {
        ColumnState {
            order: order.iter().map(|s| (*s).to_owned()).collect(),
            visible: visible.iter().map(|s| (*s).to_owned()).collect(),
            widths: widths.iter().map(|(s, w)| ((*s).to_owned(), *w)).collect(),
        }
    }

    fn default_layout() -> ColumnLayout {
        let all = ["name", "user", "pid", "cpu", "memory"];
        ColumnLayout::from_state(TableId::Processes, &state(&all, &all, &[]), &specs())
    }

    /// Collects the formatted fields of every event, for log assertions.
    #[derive(Default)]
    struct Capture(Mutex<Vec<String>>);

    struct Fields<'a>(&'a mut String);

    impl tracing::field::Visit for Fields<'_> {
        fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
            self.0.push_str(&format!("{}={v:?} ", f.name()));
        }
    }

    impl tracing::Subscriber for &'static Capture {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut line = format!("{} ", event.metadata().target());
            event.record(&mut Fields(&mut line));
            self.0.lock().unwrap().push(line);
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn unknown_id_dropped_and_logged() {
        let capture: &'static Capture = Box::leak(Box::default());
        let layout = tracing::subscriber::with_default(capture, || {
            ColumnLayout::from_state(
                TableId::Processes,
                &state(&["name", "pid", "bogus"], &["name", "pid"], &[]),
                &specs(),
            )
        });
        let lines = capture.0.lock().unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("tasklens::ui ") && l.contains("\"bogus\"")),
            "no log line names bogus: {lines:?}"
        );
        assert_eq!(
            layout.order,
            [
                ColumnId::Name,
                ColumnId::Pid,
                ColumnId::User,
                ColumnId::Cpu,
                ColumnId::Memory
            ]
        );
        assert_eq!(
            &*layout.visible_columns().clone(),
            [ColumnId::Name, ColumnId::Pid]
        );
        assert_eq!(layout.width(ColumnId::Cpu), px(63.0));
    }

    #[test]
    fn name_forced_first_and_visible() {
        let layout = ColumnLayout::from_state(
            TableId::Processes,
            &state(&["user", "pid", "name"], &["user", "pid"], &[]),
            &specs(),
        );
        assert_eq!(
            layout.order[..3],
            [ColumnId::Name, ColumnId::User, ColumnId::Pid]
        );
        assert_eq!(layout.visible_columns()[0], ColumnId::Name);

        let mut layout = default_layout();
        let before = layout.clone();
        let version = layout.version();
        assert_eq!(
            layout.set_visible(ColumnId::Name, false),
            Err(ColumnError::NameCannotBeHidden)
        );
        assert_eq!(layout, before);
        assert_eq!(layout.version(), version);
    }

    #[test]
    fn reshown_column_keeps_slot_and_width() {
        let mut layout = default_layout();
        layout.set_width(ColumnId::Cpu, px(70.0));
        assert_eq!(layout.visible_columns()[3], ColumnId::Cpu);
        layout.set_visible(ColumnId::Cpu, false).unwrap();
        assert!(!layout.visible_columns().contains(&ColumnId::Cpu));
        layout.set_visible(ColumnId::Cpu, true).unwrap();
        assert_eq!(layout.visible_columns()[3], ColumnId::Cpu);
        assert_eq!(layout.width(ColumnId::Cpu), px(70.0));

        for id in [
            ColumnId::User,
            ColumnId::Pid,
            ColumnId::Cpu,
            ColumnId::Memory,
        ] {
            layout.set_visible(id, false).unwrap();
        }
        assert_eq!(&*layout.visible_columns().clone(), [ColumnId::Name]);
        assert_eq!(layout.set_visible(ColumnId::User, true), Ok(()));
        assert_eq!(
            layout.set_visible(ColumnId::Gpu, true),
            Err(ColumnError::Unknown)
        );
    }

    #[test]
    fn last_visible_refused() {
        // Without Name in the specs the Name rule cannot protect the last column.
        let specs = vec![
            spec(ColumnId::User, ColumnKind::Text, 92.0),
            spec(ColumnId::Pid, ColumnKind::Numeric, 96.0),
        ];
        let mut layout = ColumnLayout::defaults(
            TableId::Services,
            &specs,
            &[ColumnId::User],
            &[ColumnId::User],
        );
        assert_eq!(
            layout.set_visible(ColumnId::User, false),
            Err(ColumnError::LastVisible)
        );
    }

    #[test]
    fn offsets_scaled_and_rebuilt_on_change() {
        let mut layout = ColumnLayout::from_state(
            TableId::Processes,
            &state(
                &["name", "user", "pid"],
                &["name", "user", "pid"],
                &[("name", 248), ("user", 92), ("pid", 96)],
            ),
            &specs(),
        );
        let v0 = layout.version();
        layout.rebuild(1.5);
        assert!(layout.version() > v0);
        assert_eq!(&*layout.offsets().clone(), [px(0.0), px(372.0), px(510.0)]);
        assert_eq!(layout.total_width(), px(654.0));
        assert_eq!(layout.width(ColumnId::Name), px(372.0));

        let v1 = layout.version();
        layout.set_width(ColumnId::Name, px(300.0));
        assert!(layout.version() > v1);
        assert_eq!(layout.widths[&ColumnId::Name], px(200.0));
        assert_eq!(layout.offsets()[1], px(300.0));

        assert_eq!(layout.column_at_x(px(0.0)), Some(ColumnId::Name));
        assert_eq!(layout.column_at_x(px(299.0)), Some(ColumnId::Name));
        assert_eq!(layout.column_at_x(px(300.0)), Some(ColumnId::User));
        assert_eq!(layout.column_at_x(px(1000.0)), None);
        assert_eq!(
            layout.boundary_near_x(px(297.0), px(4.0)),
            Some(ColumnId::Name)
        );
        assert_eq!(
            layout.boundary_near_x(px(303.0), px(4.0)),
            Some(ColumnId::Name)
        );
        assert_eq!(layout.boundary_near_x(px(310.0), px(4.0)), None);
        assert_eq!(
            layout.boundary_near_x(px(580.0), px(4.0)),
            Some(ColumnId::Pid)
        );
    }

    /// A test context backed by the real Linux text system, with the bundled fonts loaded.
    fn cosmic_cx() -> TestAppContext {
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
        cx.update(|cx| {
            let report = load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        });
        cx
    }

    #[test]
    fn min_width_from_template() {
        let mut cx = cosmic_cx();
        let vcx = cx.add_empty_window();
        vcx.update(|window, cx| {
            let mut cache = TextMetricsCache::default();
            let m = compute(&AppearanceSettings::default(), 0);
            let cpu = spec(ColumnId::Cpu, ColumnKind::Numeric, 63.0);
            let text = cache.template_width("100.0%", m.text.base, window, cx);
            assert!(text > Pixels::ZERO);
            assert_eq!(
                min_width(&cpu, true, &mut cache, &m, window, cx),
                text + px(16.0) + px(12.0)
            );
            assert_eq!(
                min_width(&cpu, false, &mut cache, &m, window, cx),
                text + px(16.0)
            );

            let user = spec(ColumnId::User, ColumnKind::Text, 92.0);
            assert_eq!(min_width(&user, true, &mut cache, &m, window, cx), px(60.0));
            let big = compute(
                &AppearanceSettings {
                    ui_scale_pct: 150,
                    ..AppearanceSettings::default()
                },
                0,
            );
            assert_eq!(
                min_width(&user, false, &mut cache, &big, window, cx),
                px(90.0)
            );
        });
    }

    /// Name, User, PID visible at the given widths, PID last.
    fn three(name: u32, user: u32, pid: u32) -> ColumnLayout {
        let all = ["name", "user", "pid"];
        ColumnLayout::from_state(
            TableId::Processes,
            &state(&all, &all, &[("name", name), ("user", user), ("pid", pid)]),
            &specs(),
        )
    }

    fn mins(entries: &[(ColumnId, f32)]) -> FxHashMap<ColumnId, Pixels> {
        entries.iter().map(|&(c, w)| (c, px(w))).collect()
    }

    #[test]
    fn resize_clamps_to_template_minimum() {
        let mins = mins(&[
            (ColumnId::Name, 60.0),
            (ColumnId::User, 60.0),
            (ColumnId::Pid, 63.0),
        ]);
        for mode in [ColumnResizeMode::Proportional, ColumnResizeMode::LastColumn] {
            let mut layout = three(200, 100, 100);
            resize(&mut layout, ColumnId::Pid, px(20.0), mode, px(400.0), &mins);
            assert_eq!(layout.widths[&ColumnId::Pid], px(63.0), "{mode:?}");

            let mut layout = three(200, 100, 100);
            resize(
                &mut layout,
                ColumnId::User,
                px(30.0),
                mode,
                px(400.0),
                &mins,
            );
            assert_eq!(layout.widths[&ColumnId::User], px(60.0), "{mode:?}");
        }
    }

    #[test]
    fn resize_proportional_vs_last_column() {
        let none = FxHashMap::default();
        let mut layout = three(200, 100, 100);
        let v = layout.version();
        resize(
            &mut layout,
            ColumnId::Name,
            px(240.0),
            ColumnResizeMode::Proportional,
            px(400.0),
            &none,
        );
        assert_eq!(layout.widths[&ColumnId::Name], px(240.0));
        assert_eq!(layout.widths[&ColumnId::User], px(80.0));
        assert_eq!(layout.widths[&ColumnId::Pid], px(80.0));
        assert_eq!(layout.version(), v + 1, "one rebuild per resize");
        assert_eq!(layout.offsets()[2], px(320.0));

        let mut layout = three(200, 100, 100);
        resize(
            &mut layout,
            ColumnId::Name,
            px(240.0),
            ColumnResizeMode::LastColumn,
            px(400.0),
            &none,
        );
        assert_eq!(layout.widths[&ColumnId::User], px(100.0));
        assert_eq!(layout.widths[&ColumnId::Pid], px(60.0));

        // Proportional shares keep the ratio of the current widths and respect minimums.
        let mut layout = three(200, 150, 50);
        let m = mins(&[(ColumnId::Pid, 63.0)]);
        resize(
            &mut layout,
            ColumnId::Name,
            px(240.0),
            ColumnResizeMode::Proportional,
            px(400.0),
            &m,
        );
        assert_eq!(layout.widths[&ColumnId::Pid], px(63.0));
        assert_eq!(layout.widths[&ColumnId::User], px(97.0));

        // Minimums that do not fit: the columns sit at their minimums and the table overflows.
        let mut layout = three(200, 100, 100);
        let m = mins(&[(ColumnId::User, 60.0), (ColumnId::Pid, 63.0)]);
        resize(
            &mut layout,
            ColumnId::Name,
            px(380.0),
            ColumnResizeMode::Proportional,
            px(400.0),
            &m,
        );
        assert_eq!(layout.widths[&ColumnId::User], px(60.0));
        assert_eq!(layout.widths[&ColumnId::Pid], px(63.0));
        assert_eq!(layout.total_width(), px(503.0));

        // LastColumn clamps the last column to its minimum.
        let mut layout = three(200, 100, 100);
        resize(
            &mut layout,
            ColumnId::Name,
            px(300.0),
            ColumnResizeMode::LastColumn,
            px(400.0),
            &m,
        );
        assert_eq!(layout.widths[&ColumnId::Pid], px(63.0));
        assert_eq!(layout.widths[&ColumnId::User], px(100.0));
    }

    #[test]
    fn resize_last_visible_column_only_changes_itself() {
        let none = FxHashMap::default();
        for mode in [ColumnResizeMode::Proportional, ColumnResizeMode::LastColumn] {
            let mut layout = three(200, 100, 100);
            resize(
                &mut layout,
                ColumnId::Pid,
                px(150.0),
                mode,
                px(400.0),
                &none,
            );
            assert_eq!(layout.widths[&ColumnId::Name], px(200.0), "{mode:?}");
            assert_eq!(layout.widths[&ColumnId::User], px(100.0), "{mode:?}");
            assert_eq!(layout.widths[&ColumnId::Pid], px(150.0), "{mode:?}");
            assert_eq!(layout.total_width(), px(450.0), "{mode:?}");
        }
        // A hidden column after the last visible one does not count.
        let mut layout = default_layout();
        layout.set_visible(ColumnId::Memory, false).unwrap();
        let before = layout.widths[&ColumnId::Memory];
        resize(
            &mut layout,
            ColumnId::Cpu,
            px(100.0),
            ColumnResizeMode::LastColumn,
            px(800.0),
            &none,
        );
        assert_eq!(layout.widths[&ColumnId::Cpu], px(100.0));
        assert_eq!(layout.widths[&ColumnId::Memory], before);
        assert_eq!(layout.widths[&ColumnId::Pid], px(96.0));
    }

    #[test]
    fn fit_width_rule() {
        let m = compute(&AppearanceSettings::default(), 0);
        assert_eq!(fit_width(px(60.0), px(150.0), px(40.0), &m), px(166.0));
        assert_eq!(fit_width(px(60.0), px(10.0), px(40.0), &m), px(68.0));
        assert_eq!(fit_width(px(90.0), px(10.0), px(10.0), &m), px(90.0));
    }

    #[test]
    fn next_sort_numeric_desc_text_asc() {
        let pid_asc = SortSpec {
            column: ColumnId::Pid,
            ascending: true,
        };
        let flipped = next_sort(&pid_asc, ColumnId::Pid, ColumnKind::Numeric);
        assert_eq!((flipped.column, flipped.ascending), (ColumnId::Pid, false));
        let cpu = next_sort(&pid_asc, ColumnId::Cpu, ColumnKind::Numeric);
        assert_eq!((cpu.column, cpu.ascending), (ColumnId::Cpu, false));
        let user = next_sort(&pid_asc, ColumnId::User, ColumnKind::Text);
        assert_eq!((user.column, user.ascending), (ColumnId::User, true));
        let path = next_sort(&pid_asc, ColumnId::ImagePath, ColumnKind::Path);
        assert!(path.ascending);
    }

    #[test]
    fn state_round_trip() {
        let mut layout = default_layout();
        layout.set_visible(ColumnId::Cpu, false).unwrap();
        layout.set_width(ColumnId::Cpu, px(70.0));
        layout
            .move_column(ColumnId::Memory, Some(ColumnId::User))
            .unwrap();
        let back = ColumnLayout::from_state(TableId::Processes, &layout.to_state(), &specs());
        assert_eq!(back, layout);
        assert_eq!(back.visible_columns(), layout.visible_columns());
        assert_eq!(back.offsets(), layout.offsets());
    }

    #[test]
    fn move_refuses_before_name() {
        let mut layout = default_layout();
        let before = layout.clone();
        assert_eq!(
            layout.move_column(ColumnId::Pid, Some(ColumnId::Name)),
            Err(ColumnError::NameMustStayFirst)
        );
        assert_eq!(
            layout.move_column(ColumnId::Name, None),
            Err(ColumnError::NameMustStayFirst)
        );
        assert_eq!(layout, before);

        layout.move_column(ColumnId::Pid, None).unwrap();
        assert_eq!(layout.order.last(), Some(&ColumnId::Pid));
        layout
            .move_column(ColumnId::Pid, Some(ColumnId::User))
            .unwrap();
        assert_eq!(layout.order[1], ColumnId::Pid);
        assert_eq!(
            layout.move_column(ColumnId::Gpu, None),
            Err(ColumnError::Unknown)
        );
    }

    #[test]
    fn services_layout_state_round_trip() {
        use tl_model::table::ServiceColumn;

        let all = SysColumnId::all(SysTable::Services);
        assert_eq!(all.len(), 19);
        let specs: Vec<ColumnSpec<SysColumnId>> = all
            .iter()
            .map(|&id| {
                let label = SharedString::new(id.as_str());
                ColumnSpec {
                    id,
                    label: label.clone(),
                    full_name: label.clone(),
                    group: label,
                    align: ColumnAlign::Left,
                    kind: ColumnKind::Text,
                    sortable: true,
                    template: None,
                    default_width: px(100.0),
                    tooltip: None,
                    availability: Availability::Available,
                    hideable: true,
                }
            })
            .collect();
        let name = SysColumnId::Services(ServiceColumn::Name);
        let status = SysColumnId::Services(ServiceColumn::Status);
        let pid = SysColumnId::Services(ServiceColumn::Pid);
        let mut layout: SysColumnLayout =
            ColumnLayout::defaults(TableId::Services, &specs, all, &[name, status, pid]);
        layout.set_width(status, px(140.0));
        layout.set_width(pid, px(70.0));
        layout.move_column(pid, Some(name)).unwrap();
        layout.set_visible(status, false).unwrap();

        let state = layout.to_state();
        assert_eq!(state.order.len(), 19);
        assert_eq!(state.order[..3], ["pid", "name", "display_name"]);
        assert_eq!(state.visible, ["pid", "name"]);
        assert_eq!(state.widths["status"], 140);

        let back = ColumnLayout::from_state(TableId::Services, &state, &specs);
        assert_eq!(back, layout);
        assert_eq!(back.width(status), px(140.0));
        assert_eq!(&*back.visible_columns().clone(), [pid, name]);
        assert_eq!(back.offsets(), layout.offsets());

        // Process ids do not parse in a system table.
        let wrong = ColumnLayout::<SysColumnId>::from_state(TableId::Processes, &state, &specs);
        assert!(wrong.visible.is_empty());
    }
}
