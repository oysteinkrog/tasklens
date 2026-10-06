//! Column model shared by every table: per-column specs and the per-table layout (ADR-009,
//! ADR-011, PROC-022, PROC-023, UI-PRC-005, UI-PRC-007).
//!
//! Column state is kept by `ColumnId`, never by index. Widths are stored in logical px at 100%
//! and scaled by `UiMetrics.ui_scale` only in `rebuild`, which precomputes the x offsets that
//! render and hit testing read without allocating.

use std::sync::Arc;

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{App, Pixels, SharedString, Window, gpui, px};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use tl_model::ids::ColumnId;
use tl_model::settings::ColumnState;
use tl_model::table::SortSpec;

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
pub struct ColumnSpec {
    pub id: ColumnId,
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
pub struct ColumnLayout {
    pub table: TableId,
    /// Every known column. Hidden ones keep their place (UI-PRC-007).
    pub order: Vec<ColumnId>,
    pub visible: FxHashSet<ColumnId>,
    /// Logical px at 100%, remembered for hidden columns too.
    pub widths: FxHashMap<ColumnId, Pixels>,
    /// x offset of each visible column in order, scaled.
    offsets: Arc<[Pixels]>,
    visible_order: Arc<[ColumnId]>,
    total_width: Pixels,
    ui_scale: f32,
    /// Bumps on every change; offsets are rebuilt then.
    version: u64,
}

/// Equal when the stored state and the scale match. `version` is ignored, and the derived
/// fields follow from the rest.
impl PartialEq for ColumnLayout {
    fn eq(&self, other: &Self) -> bool {
        self.table == other.table
            && self.order == other.order
            && self.visible == other.visible
            && self.widths == other.widths
            && self.ui_scale == other.ui_scale
    }
}

impl ColumnLayout {
    /// Builds the layout from `[columns.<table>]`. Ids not in `specs` are dropped with a log line
    /// (ADR-011). Spec ids missing from `order` are appended hidden, in spec order. Name is forced
    /// first and visible. Offsets are built at scale 1.0; call `rebuild` with the real scale.
    pub fn from_state(table: TableId, state: &ColumnState, specs: &[ColumnSpec]) -> Self {
        let known = |s: &str| -> Option<ColumnId> {
            let id = ColumnId::from_str(s).filter(|id| specs.iter().any(|sp| sp.id == *id));
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
        let visible: FxHashSet<ColumnId> = state
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

        let mut widths: FxHashMap<ColumnId, Pixels> = specs
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
        let ids = |f: &dyn Fn(&ColumnId) -> bool| -> Vec<String> {
            self.order
                .iter()
                .filter(|id| f(id))
                .map(|id| id.as_str().to_owned())
                .collect()
        };
        ColumnState {
            order: ids(&|_| true),
            visible: ids(&|id| self.visible.contains(id)),
            widths: self
                .order
                .iter()
                .filter_map(|id| {
                    let w = self.widths.get(id)?;
                    Some((
                        id.as_str().to_owned(),
                        f32::from(*w).round().max(0.0) as u32,
                    ))
                })
                .collect(),
        }
    }

    /// The owner's default layout. Ids not in `specs` are ignored; spec ids missing from
    /// `default_order` are appended hidden. Widths come from `ColumnSpec::default_width`.
    pub fn defaults(
        table: TableId,
        specs: &[ColumnSpec],
        default_order: &[ColumnId],
        default_visible: &[ColumnId],
    ) -> Self {
        let in_specs = |id: &ColumnId| specs.iter().any(|sp| sp.id == *id);
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
        mut order: Vec<ColumnId>,
        mut visible: FxHashSet<ColumnId>,
        widths: FxHashMap<ColumnId, Pixels>,
    ) -> Self {
        if let Some(pos) = order.iter().position(|id| *id == ColumnId::Name) {
            order[..=pos].rotate_right(1);
            visible.insert(ColumnId::Name);
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
    pub fn visible_columns(&self) -> &Arc<[ColumnId]> {
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
    pub fn width(&self, id: ColumnId) -> Pixels {
        self.widths
            .get(&id)
            .map_or(Pixels::ZERO, |w| *w * self.ui_scale)
    }

    /// Shows or hides `id`. Showing restores its remembered width and its old slot in `order`.
    pub fn set_visible(&mut self, id: ColumnId, on: bool) -> Result<(), ColumnError> {
        if !self.order.contains(&id) {
            return Err(ColumnError::Unknown);
        }
        if on {
            if self.visible.insert(id) {
                self.rebuild(self.ui_scale);
            }
            return Ok(());
        }
        if id == ColumnId::Name {
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
    pub fn set_width(&mut self, id: ColumnId, w: Pixels) {
        if let Some(slot) = self.widths.get_mut(&id) {
            *slot = px(f32::from(w) / self.ui_scale);
            self.rebuild(self.ui_scale);
        }
    }

    /// Moves `id` before `before`, or to the end on `None`. Nothing may move Name or go before it.
    pub fn move_column(
        &mut self,
        id: ColumnId,
        before: Option<ColumnId>,
    ) -> Result<(), ColumnError> {
        if id == ColumnId::Name || before == Some(ColumnId::Name) {
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
        let visible_order: Arc<[ColumnId]> = self
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

    /// The visible column under `x` (scaled, from the left edge of the first column).
    pub fn column_at_x(&self, x: Pixels) -> Option<ColumnId> {
        if x < Pixels::ZERO || x >= self.total_width {
            return None;
        }
        let i = self.offsets.partition_point(|o| *o <= x);
        self.visible_order.get(i.checked_sub(1)?).copied()
    }

    /// The visible column whose right edge lies within `half_band` of `x`, nearest first. This
    /// is the resize hit band, 4 px each side (UI-PRC-005).
    pub fn boundary_near_x(&self, x: Pixels, half_band: Pixels) -> Option<ColumnId> {
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

/// Minimum width of a column, scaled (PROC-023, UI-PRC-005). Numeric: template width + 16
/// padding, + 12 more on the sort column for the arrow. Text and Path: 60.
///
/// The template is measured in the table font at `t_base` with `tnum`, so it follows scale
/// changes. `TextMetricsCache` measures through the window's text system, hence `window`.
pub fn min_width(
    spec: &ColumnSpec,
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
        _ => px(60.0 * m.ui_scale),
    }
}

/// The sort after a header click (UI-PRC-005, PROC-047). Same column: flip direction. New
/// column: Numeric starts descending, Text and Path ascending. A free function because
/// `SortSpec` lives in tl-model.
pub fn next_sort(current: &SortSpec, col: ColumnId, kind: ColumnKind) -> SortSpec {
    let ascending = if current.column == col {
        !current.ascending
    } else {
        kind != ColumnKind::Numeric
    };
    SortSpec {
        column: col,
        ascending,
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
}
