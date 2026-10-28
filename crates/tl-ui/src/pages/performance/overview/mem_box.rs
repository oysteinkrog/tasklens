//! The Overview page's Memory box (UI-PRF-017, D-022, UI-PRF-026, crw.54): five rows that share the
//! body height (In use, Cached, Available, Swap, Commit). A row is its label, value, meter,
//! percent and a sparkline.
//!
//! UI-PRF-019 gives the box three modes. `rows` puts the text and meter part and the sparkline
//! column side by side with an inner splitter between them. `chart_rows` has a used chart over
//! the rows, with a horizontal splitter. `chart` is the composition chart alone. `charts` (D-029)
//! is one filled chart per row, each under a header with its name, value and percent, placed by
//! the Arrange choice.
//!
//! The box is its own view and reads `SYSTEM` only, so a tick with only series changes does not
//! touch it. It formats its text only when the memory totals' hash changes. Each sparkline is a
//! cached child view that re-renders for its own series; its spec is rebuilt only when its scale
//! total changes. Pointer movement never notifies.

use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement, Styled as _,
    Subscription, Window, div, px, relative, size,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::Snapshot;
use tl_model::system::MemoryTotals;
use tl_model::units;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::cpu_box::value_text;
use super::fit::{fit_text, whole_rows};
use super::inner_split::{InnerSplit, InnerSplitChanged, PART_MIN, SplitAxis};
use super::layout::{Arrange, ArrangeChoice, BoxMode};
use super::parts::{
    ChartSettings, GridFit, Meter, SPARK_GRID, SPARK_W, arrange_grid, box_chart, box_frame,
    chart_section_versions, class_probe, edit_chrome, grid_cells, spark_chart,
};
use super::picked::{box_mask, box_snapshot};
use super::range::RangeText;
use super::timeline::OverviewPause;
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, LegendStyle, ValueAlign,
    ValueFormat, YScale, format_value,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_sm};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const MEM_BOX_ID: &str = "perf-overview-box-memory";

/// The box's layout modes (UI-PRF-019). `rows` is the five rows of UI-PRF-017.
///
/// The `rows` split is the text part's share. Its default is a nominal 0.7: while the share is
/// that value, the sparkline column keeps `SPARK_W` whatever the box width (see
/// [`InnerSplit::with_default_second_px`]).
pub const MODES: &[BoxMode] = &[
    BoxMode {
        id: "rows",
        label: "perf-overview-mode-rows",
        split: Some(0.7),
    },
    BoxMode {
        id: "chart_rows",
        label: "perf-overview-mode-chart-rows",
        split: Some(0.5),
    },
    BoxMode {
        id: "chart",
        label: "perf-overview-mode-chart",
        split: None,
    },
    BoxMode {
        id: "charts",
        label: "perf-overview-mode-charts",
        split: None,
    },
];

/// The debug selector of the splitter between the text part and the sparkline column.
pub const MEM_SPLITTER_ID: &str = "perf-overview-memory-splitter";
/// The debug selector of its accent line.
pub const MEM_SPLITTER_LINE_ID: &str = "perf-overview-memory-splitter-line";
/// The debug selector of the splitter between the chart and the rows in `chart_rows`.
pub const MEM_CHART_SPLITTER_ID: &str = "perf-overview-memory-chart-splitter";
/// The debug selector of its accent line.
pub const MEM_CHART_SPLITTER_LINE_ID: &str = "perf-overview-memory-chart-splitter-line";
/// The debug selector of the used chart in `chart_rows`.
pub const MEM_USED_CHART_ID: &str = "perf-overview-memory-used-chart";
/// The debug selector of the composition chart in `chart`.
pub const MEM_COMP_CHART_ID: &str = "perf-overview-memory-comp-chart";

/// The narrowest text part of `rows`, logical px (UI-PRF-019).
const TEXT_PART_MIN_W: f32 = 128.;
/// The narrowest sparkline column, logical px. The default is `SPARK_W`, below `PART_MIN`.
const SPARK_COLUMN_MIN_W: f32 = 48.;

/// The smallest `charts` cell, logical px before `ui_scale` (D-029).
pub const CELL_MIN_W: f32 = 160.;
pub const CELL_MIN_H: f32 = 60.;

/// The width a `charts` grid keeps for its scrollbar when it scrolls, logical px.
const GRID_BAR_W: f32 = 16.;

/// The debug selector of the `charts` grid viewport.
pub const CHARTS_GRID_ID: &str = "perf-overview-memory-charts-grid";

/// The modes with an Arrange choice (UI-PRF-019, D-027, D-029): the two parts of `chart_rows`,
/// the chart on top (Automatic and Stack vertically) or on the left (Side by side), and the
/// cells of `charts`.
pub const ARRANGE: &[(&str, ArrangeChoice)] = &[
    ("chart_rows", ArrangeChoice::Saved(Arrange::Auto)),
    ("charts", ArrangeChoice::Saved(Arrange::Auto)),
];

/// Which parts the box shows (UI-PRF-019).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MemMode {
    Rows,
    ChartRows,
    Chart,
    Charts,
}

impl MemMode {
    /// The mode `id`; an unknown id is the default.
    fn from_id(id: &str) -> Self {
        match id {
            "chart_rows" => Self::ChartRows,
            "chart" => Self::Chart,
            "charts" => Self::Charts,
            _ => Self::Rows,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Rows => MODES[0].id,
            Self::ChartRows => MODES[1].id,
            Self::Chart => MODES[2].id,
            Self::Charts => MODES[3].id,
        }
    }
}

/// Maps the body size to the layout class the probe compares with the render's.
type Classify = Box<dyn Fn(Size<Pixels>) -> u32>;

/// The page saves the share the box ended a drag or a double-click on.
impl EventEmitter<InnerSplitChanged> for MemBox {}

const KEY_TITLE: &str = "perf-card-heading-memory";
/// The rows in order: In use, Cached, Available, Swap, Commit.
const ROWS: usize = 5;
const KEY_ROWS: [&str; ROWS] = [
    "perf-overview-mem-used",
    "perf-overview-mem-cached",
    "perf-overview-mem-available",
    "perf-overview-mem-swap",
    "perf-overview-mem-commit",
];
/// The debug selector of row `i`'s sparkline column.
pub fn spark_id(i: usize) -> String {
    format!("{MEM_BOX_ID}-spark-{i}")
}

/// The debug selector of row `i`.
pub fn row_id(i: usize) -> String {
    format!("{MEM_BOX_ID}-row-{i}")
}

/// The debug selector of row `i`'s `charts` cell.
pub fn cell_id(i: usize) -> String {
    format!("{MEM_BOX_ID}-cell-{i}")
}

/// The debug selector of the chart area of row `i`'s `charts` cell, below its header.
pub fn cell_chart_id(i: usize) -> String {
    format!("{MEM_BOX_ID}-cell-chart-{i}")
}

/// The debug selector of a part of row `i`'s `charts` cell header: "name", "value" or "percent".
pub fn cell_head_id(i: usize, part: &str) -> String {
    format!("{MEM_BOX_ID}-cell-head-{part}-{i}")
}

/// The debug selector of row `i`'s value text.
pub fn value_id(i: usize) -> String {
    format!("{MEM_BOX_ID}-value-{i}")
}

/// The debug selector of row `i`'s meter.
pub fn meter_id(i: usize) -> String {
    format!("{MEM_BOX_ID}-meter-{i}")
}

/// The debug selector of the box body.
pub const MEM_BODY_ID: &str = "perf-overview-box-memory-body";

const KEY_NO_SWAP: &str = "perf-overview-mem-no-swap";
const SWAP: usize = 3;
const COMMIT: usize = 4;

/// The percent column ("244.3%"), logical px before `ui_scale`.
const ROW_PCT_W: f32 = 48.;

/// The narrowest meter a wide row keeps, logical px before `ui_scale`.
const ROW_MIN_METER_W: f32 = 32.;

/// Whether a text part `width` wide (0 before the first paint) has room for the one-line row:
/// label, value, a meter of at least `ROW_MIN_METER_W` and percent, with their 3 gaps. Narrower
/// parts get the two-line row (`text_row`).
fn row_wide(width: Pixels, m: &UiMetrics) -> bool {
    let fixed = (ROW_LABEL_W + ROW_VALUE_W + ROW_MIN_METER_W + ROW_PCT_W) * m.ui_scale;
    width == px(0.) || f32::from(width) >= fixed + 3. * f32::from(m.sp[4])
}

/// The meter label column, logical px before `ui_scale`.
pub(super) const ROW_LABEL_W: f32 = 72.;
/// The meter value column, logical px before `ui_scale`.
pub(super) const ROW_VALUE_W: f32 = 80.;

/// One meter row: its label, its value text, its fill and the fill colour.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterRow {
    pub label: SharedString,
    pub value: SharedString,
    pub fraction: Option<f32>,
    pub color: Hsla,
}

/// The series of each row, in row order.
const ROW_METRICS: [MetricId; ROWS] = [
    MetricId::MEM_USED,
    MetricId::MEM_CACHED,
    MetricId::MEM_AVAILABLE,
    MetricId::SWAP_USED,
    MetricId::MEM_COMMIT,
];

/// The scale totals of the sparklines: memory, swap and the commit limit, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Totals {
    total: u64,
    swap: u64,
    commit_limit: u64,
}

impl Totals {
    fn of(mem: &MemoryTotals) -> Self {
        Self {
            total: mem.total,
            swap: mem.swap_total,
            commit_limit: mem.commit_limit,
        }
    }

    /// The scale of row `i`: 0..total, 0..swap total, or 0..max(limit, peak) for Commit.
    fn scale(&self, i: usize) -> YScale {
        match i {
            SWAP => YScale::FixedTotal(self.swap.max(1) as f32),
            COMMIT => YScale::Commit(self.commit_limit as f32),
            _ => YScale::FixedTotal(self.total.max(1) as f32),
        }
    }
}

/// The sparkline of row `i`: its series, filled, in the row's colour.
fn spark_spec(
    memory: DeviceIdx,
    i: usize,
    totals: &Totals,
    colors: &[Hsla; ROWS],
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    let (fill, line) = if i == 0 {
        (theme.graph.mem_fill, theme.graph.mem_line)
    } else {
        let line = colors[i];
        (
            Hsla {
                a: line.a * 0.3,
                ..line
            },
            line,
        )
    };
    ChartSpec {
        series: smallvec![ChartSeries {
            id: SeriesId {
                device: memory,
                metric: ROW_METRICS[i],
            },
            fill: Some(fill),
            line,
            ..ChartSeries::default()
        }],
        y: totals.scale(i),
        range_ns: s.range_ns,
        grid: SPARK_GRID,
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// The chart of row `i` in `charts`: its series, filled, in the row's colour, 0..its scale.
fn cell_spec(
    memory: DeviceIdx,
    i: usize,
    totals: &Totals,
    colors: &[Hsla; ROWS],
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    ChartSpec {
        grid: chart_grid(s),
        ..spark_spec(memory, i, totals, colors, s, theme)
    }
}

/// The used chart of `chart_rows`: `MEM_USED` over the box range, filled, 0..total.
fn used_spec(memory: DeviceIdx, totals: &Totals, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: SeriesId {
                device: memory,
                metric: MetricId::MEM_USED,
            },
            fill: Some(theme.graph.mem_fill),
            line: theme.graph.mem_line,
            ..ChartSeries::default()
        }],
        y: totals.scale(0),
        range_ns: s.range_ns,
        grid: chart_grid(s),
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// The composition chart of `chart`: In use, Cached and Available stacked, filled, 0..total.
fn comp_spec(
    memory: DeviceIdx,
    totals: &Totals,
    colors: &[Hsla; ROWS],
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    let band = |i: usize| {
        let line = if i == 0 {
            theme.graph.mem_line
        } else {
            colors[i]
        };
        ChartSeries {
            id: SeriesId {
                device: memory,
                metric: ROW_METRICS[i],
            },
            fill: Some(Hsla {
                a: line.a * if i == 0 { 0.6 } else { 0.4 },
                ..line
            }),
            line,
            ..ChartSeries::default()
        }
    };
    ChartSpec {
        series: smallvec![band(0), band(1), band(2)],
        y: totals.scale(0),
        range_ns: s.range_ns,
        grid: chart_grid(s),
        smooth: s.smooth,
        stack: true,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

fn chart_grid(s: &ChartSettings) -> GridStyle {
    GridStyle {
        lines: true,
        baseline: false,
        outline: true,
        rounded: s.rounded,
        vertical_at: None,
    }
}

/// The labels of a box chart: no readout, no range word (the box frame draws the time axis).
fn chart_labels() -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The labels of the composition chart: `chart_labels` plus a legend of its three bands, in the
/// stacking order (UI-PRF-019). The chart paints it when it is at least 160 × 100 px.
fn comp_labels(cx: &App) -> ChartLabels {
    ChartLabels {
        legend: KEY_ROWS[..3]
            .iter()
            .map(|key| (tr(cx, key), LegendStyle::Solid))
            .collect(),
        ..chart_labels()
    }
}

/// `part / whole` as a fraction, `None` when `whole` is 0.
fn fraction(part: u64, whole: u64) -> Option<f32> {
    (whole > 0).then(|| (part as f64 / whole as f64) as f32)
}

/// The colours of the five rows.
fn row_colors(theme: &TlTheme) -> [Hsla; ROWS] {
    let line = theme.graph.mem_line;
    let with = |a: f32| Hsla {
        a: line.a * a,
        ..line
    };
    [
        line,
        with(0.6),
        with(0.35),
        theme.graph.kernel_line,
        with(0.8),
    ]
}

/// The Memory box.
pub struct MemBox {
    /// The range figures and time axis start the page gives (UI-PRF-055).
    range_text: RangeText,
    hub: Entity<SnapshotHub>,
    memory: DeviceIdx,
    /// One sparkline per row.
    sparks: [Entity<ChartView>; ROWS],
    /// The used chart of `chart_rows` and the composition chart of `chart`.
    used_chart: Entity<ChartView>,
    comp_chart: Entity<ChartView>,
    /// One chart per row for `charts`.
    cell_charts: [Entity<ChartView>; ROWS],
    mode: MemMode,
    /// How `chart_rows` places its two parts and `charts` its cells (D-027).
    arrange: Arrange,
    /// The gap between the text part and the sparkline column (`rows`).
    spark_split: InnerSplit,
    /// The gap between the chart and the rows (`chart_rows`).
    chart_split: InnerSplit,
    rows: [MeterRow; ROWS],
    /// The percent column of each row; empty when unknown.
    percents: [SharedString; ROWS],
    /// The totals the sparkline specs were built for.
    totals: Totals,
    title: SharedString,
    value: SharedString,
    /// Hash of the inputs `value`, `rows` and `percents` were formatted from.
    text_bits: u64,
    /// The body size the last paint measured, for `class_probe`.
    body: Rc<Cell<gpui_kit::Size<Pixels>>>,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    _subs: [Subscription; 5],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl MemBox {
    pub fn new(hub: &Entity<SnapshotHub>, memory: DeviceIdx, cx: &mut Context<Self>) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let snap = box_snapshot(hub, cx);
        let totals = Totals::of(&snap.system.memory);
        let colors = row_colors(&theme);
        let sparks = std::array::from_fn(|i| {
            let spec = spark_spec(memory, i, &totals, &colors, &settings, &theme);
            spark_chart(spec, ValueFormat::Bytes, hub, cx)
        });
        let used_chart = box_chart(
            used_spec(memory, &totals, &settings, &theme),
            chart_labels(),
            hub,
            cx,
        );
        let comp_chart = box_chart(
            comp_spec(memory, &totals, &colors, &settings, &theme),
            comp_labels(cx),
            hub,
            cx,
        );
        let cell_charts = std::array::from_fn(|i| {
            let spec = cell_spec(memory, i, &totals, &colors, &settings, &theme);
            box_chart(spec, chart_labels(), hub, cx)
        });
        let rows = std::array::from_fn(|i| MeterRow {
            label: tr(cx, KEY_ROWS[i]),
            value: SharedString::default(),
            fraction: None,
            color: colors[i],
        });
        let mut this = Self {
            range_text: RangeText::default(),
            hub: hub.clone(),
            memory,
            sparks,
            used_chart,
            comp_chart,
            cell_charts,
            mode: MemMode::Rows,
            arrange: Arrange::Auto,
            spark_split: InnerSplit::new(
                MEM_SPLITTER_ID,
                MEM_SPLITTER_LINE_ID,
                MODES[0].split.unwrap_or_default(),
            )
            .with_mins(TEXT_PART_MIN_W, SPARK_COLUMN_MIN_W)
            .with_default_second_px(SPARK_W),
            chart_split: InnerSplit::new(
                MEM_CHART_SPLITTER_ID,
                MEM_CHART_SPLITTER_LINE_ID,
                MODES[1].split.unwrap_or_default(),
            )
            .with_mins(PART_MIN, PART_MIN),
            rows,
            percents: Default::default(),
            totals,
            title: tr(cx, KEY_TITLE),
            value: SharedString::default(),
            text_bits: 0,
            body: Rc::default(),
            settings,
            settings_versions: chart_section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::on_pause),
                cx.observe_global::<TlSettings>(Self::sync_settings),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_text(&snap.system.memory, true, cx);
        this
    }

    pub fn rows(&self) -> &[MeterRow; ROWS] {
        &self.rows
    }

    /// The percent column of each row ("" when unknown).
    pub fn percents(&self) -> &[SharedString; ROWS] {
        &self.percents
    }

    /// One sparkline per row, in row order.
    pub fn sparks(&self) -> &[Entity<ChartView>; ROWS] {
        &self.sparks
    }

    /// The shown mode's id.
    pub fn mode(&self) -> &'static str {
        self.mode.id()
    }

    /// Shows mode `id`; an unknown id is the default mode. Only this box renders again.
    pub fn set_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let mode = MemMode::from_id(id);
        if mode != self.mode {
            self.mode = mode;
            cx.notify();
        }
    }

    /// The shown mode's split as the box lays it out, after the clamps: the text part's share in
    /// `rows`, the chart's in `chart_rows`, 0 in `chart`.
    pub fn split(&self, m: &UiMetrics) -> f32 {
        match self.mode {
            MemMode::Rows => self.spark_split.share(SplitAxis::Row, m.sp[5], m.ui_scale),
            MemMode::ChartRows => self
                .chart_split
                .share(self.chart_axis(), m.sp[5], m.ui_scale),
            MemMode::Chart | MemMode::Charts => 0.,
        }
    }

    /// How `chart_rows` places its two parts and `charts` its cells.
    pub fn arrange(&self) -> Arrange {
        self.arrange
    }

    /// Places the two parts of `chart_rows` or the cells of `charts` as `arrange` says. Only this
    /// box renders again, and only when it shows one of those modes.
    pub fn set_arrange(&mut self, arrange: Arrange, cx: &mut Context<Self>) {
        if arrange != self.arrange {
            self.arrange = arrange;
            if matches!(self.mode, MemMode::ChartRows | MemMode::Charts) {
                cx.notify();
            }
        }
    }

    /// The axis of the `chart_rows` parts: stacked with a horizontal gap, or side by side with a
    /// vertical gap.
    fn chart_axis(&self) -> SplitAxis {
        match self.arrange {
            Arrange::Horizontal => SplitAxis::Row,
            Arrange::Auto | Arrange::Vertical => SplitAxis::Column,
        }
    }

    /// Takes the saved share for the shown mode. Nothing changes while the gap is held.
    pub fn set_split(&mut self, share: f32, cx: &mut Context<Self>) {
        let changed = match self.mode {
            MemMode::Rows => self.spark_split.set_share(share),
            MemMode::ChartRows => self.chart_split.set_share(share),
            MemMode::Chart | MemMode::Charts => false,
        };
        if changed {
            cx.notify();
        }
    }

    fn spark_inner_split(&mut self) -> &mut InnerSplit {
        &mut self.spark_split
    }

    fn chart_inner_split(&mut self) -> &mut InnerSplit {
        &mut self.chart_split
    }

    /// The used chart of `chart_rows`.
    pub fn used_chart(&self) -> &Entity<ChartView> {
        &self.used_chart
    }

    /// The composition chart of `chart`.
    pub fn comp_chart(&self) -> &Entity<ChartView> {
        &self.comp_chart
    }

    /// One chart per row, in row order, for `charts`.
    pub fn cell_charts(&self) -> &[Entity<ChartView>; ROWS] {
        &self.cell_charts
    }

    /// The title and value text.
    pub fn text(&self) -> (&SharedString, &SharedString) {
        (&self.title, &self.value)
    }

    /// "Memory\t50.0%", then "In use\t32.00 GiB\t50.0%" per row.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\t');
        out.push_str(&self.value);
        out.push('\n');
        for (row, pct) in self.rows.iter().zip(&self.percents) {
            out.push_str(&row.label);
            out.push('\t');
            out.push_str(&row.value);
            if !pct.is_empty() {
                out.push('\t');
                out.push_str(pct);
            }
            out.push('\n');
        }
    }

    /// Takes the range figures and time axis start the page worked out (UI-PRF-055). Notifies
    /// only when they changed.
    pub fn set_range_text(&mut self, text: RangeText, cx: &mut Context<Self>) {
        if self.range_text != text {
            self.range_text = text;
            cx.notify();
        }
    }

    /// The range figures and time axis start the box shows.
    pub fn range_text(&self) -> &RangeText {
        &self.range_text
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let mask = box_mask(ev.mask, cx);
        if !mask.intersects(ComponentMask::SYSTEM | ComponentMask::DEVICES) {
            return;
        }
        let snap: Arc<Snapshot> = box_snapshot(&hub, cx);
        if mask.contains(ComponentMask::DEVICES)
            && let Some(entry) = snap.devices.find(&DeviceIdentity::Memory)
            && entry.idx != self.memory
        {
            self.memory = entry.idx;
            self.rebuild_specs(cx);
        }
        if mask.contains(ComponentMask::SYSTEM) && self.sync_text(&snap.system.memory, false, cx) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The page was held at a time or made live: the value and rows read the other snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let snap = box_snapshot(&self.hub, cx);
        self.sync_text(&snap.system.memory, true, cx);
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// The value, rows and percents when the totals changed, or always with `force`; new
    /// sparkline specs when a scale total changed. True when the text was formatted again.
    fn sync_text(&mut self, mem: &MemoryTotals, force: bool, cx: &mut Context<Self>) -> bool {
        let mut h = FxHasher::default();
        mem.validity.hash(&mut h);
        for v in [
            mem.total,
            mem.used,
            mem.available,
            mem.cached,
            mem.swap_total,
            mem.swap_free,
            mem.committed,
            mem.commit_limit,
        ] {
            h.write_u64(v);
        }
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        let totals = Totals::of(mem);
        if totals != self.totals {
            self.totals = totals;
            self.rebuild_specs(cx);
        }
        let valid = matches!(mem.validity, Validity::Valid | Validity::Stale);
        let unit = NetworkUnit::default();
        self.value = value_text(mem.validity, |out| {
            format_value(
                ValueFormat::PercentOf { total: mem.total },
                mem.used as f32,
                unit,
                out,
            );
        });
        let swap_used = mem.swap_total.saturating_sub(mem.swap_free);
        let parts = [
            (mem.used, mem.total),
            (mem.cached, mem.total),
            (mem.available, mem.total),
            (swap_used, mem.swap_total),
            (mem.committed, mem.commit_limit),
        ];
        for (i, (part, whole)) in parts.into_iter().enumerate() {
            let row = &mut self.rows[i];
            let pct = &mut self.percents[i];
            if valid && i == SWAP && whole == 0 {
                row.value = tr(cx, KEY_NO_SWAP);
                row.fraction = None;
                *pct = SharedString::default();
                continue;
            }
            row.value = value_text(mem.validity, |out| units::bytes(out, part));
            let ratio = fraction(part, whole).filter(|_| valid);
            // The meter stops at full; Commit can pass 100% and its percent says so.
            row.fraction = ratio.map(|r| r.min(1.));
            *pct = match ratio {
                Some(r) => {
                    let mut out = String::new();
                    units::percent(&mut out, r * 100.);
                    out.into()
                }
                None => SharedString::default(),
            };
        }
        true
    }

    /// Applies the chart settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = chart_section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = ChartSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_specs(cx);
    }

    /// A new theme or UI scale: new sparkline and meter colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        let colors = row_colors(current(cx));
        for (row, color) in self.rows.iter_mut().zip(colors) {
            row.color = color;
        }
        self.rebuild_specs(cx);
        cx.notify();
    }

    /// A new language: new title and labels.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        for (row, key) in self.rows.iter_mut().zip(KEY_ROWS) {
            row.label = tr(cx, key);
        }
        let labels = comp_labels(cx);
        self.comp_chart.update(cx, |v, cx| v.set_labels(labels, cx));
        // "No swap" is translated too.
        let snap = box_snapshot(&self.hub, cx);
        self.sync_text(&snap.system.memory, true, cx);
        cx.notify();
    }

    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let colors = row_colors(&theme);
        for (i, chart) in self.sparks.iter().enumerate() {
            let spec = spark_spec(
                self.memory,
                i,
                &self.totals,
                &colors,
                &self.settings,
                &theme,
            );
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        let used = used_spec(self.memory, &self.totals, &self.settings, &theme);
        self.used_chart.update(cx, |v, cx| v.set_spec(used, cx));
        let comp = comp_spec(self.memory, &self.totals, &colors, &self.settings, &theme);
        self.comp_chart.update(cx, |v, cx| v.set_spec(comp, cx));
        for (i, chart) in self.cell_charts.iter().enumerate() {
            let spec = cell_spec(
                self.memory,
                i,
                &self.totals,
                &colors,
                &self.settings,
                &theme,
            );
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }
}

/// The debug selector of a meter row's label.
pub const METER_ROW_LABEL_ID: &str = "perf-meter-row-label";

/// One meter row: label, meter, value.
pub fn meter_row(row: &MeterRow, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let s = m.ui_scale;
    // The text style is assigned first: assigning it later would drop the nowrap, the ellipsis
    // and the colour set on the element.
    let mut label = div();
    *label.text_style() = t_sm(m);
    let label = label
        .debug_selector(|| METER_ROW_LABEL_ID.into())
        .flex_none()
        .w(px(ROW_LABEL_W * s))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(row.label.clone());
    let mut value = div();
    *value.text_style() = numeric_style(m, m.text.sm);
    // Shrinks to its column before the meter row overflows; the value then drops decimals.
    let value = value
        .min_w(px(ROW_VALUE_W * s))
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(fit_text(row.value.clone()));
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * s))
        .child(label)
        .child(div().flex_1().min_w_0().child(Meter {
            fraction: row.fraction,
            color: row.color,
        }))
        .child(value)
        .into_any_element()
}

/// The height of a text row, logical px before `ui_scale`: one line, or two when not `wide`.
fn row_min_h(wide: bool, s: f32) -> gpui_kit::Pixels {
    px(if wide { LABEL_ROW } else { 2. * LABEL_ROW } * s)
}

/// One memory row's text and meter part: label, value, meter and percent. A `wide` row puts them
/// on one line. A narrow row has two lines, the label and value over the meter and percent.
fn text_row(
    row: &MeterRow,
    percent: &SharedString,
    index: usize,
    wide: bool,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let s = m.ui_scale;
    let mut label = div()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(row.label.clone());
    *label.text_style() = t_sm(m);
    let mut value = div()
        .debug_selector(move || value_id(index))
        .min_w_0()
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(fit_text(row.value.clone()));
    *value.text_style() = numeric_style(m, m.text.sm);
    let mut pct = div()
        .flex_none()
        .w(px(ROW_PCT_W * s))
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(fit_text(percent.clone()));
    *pct.text_style() = numeric_style(m, m.text.sm);
    let meter = div()
        .debug_selector(move || meter_id(index))
        .flex_1()
        .min_w_0()
        .child(Meter {
            fraction: row.fraction,
            color: row.color,
        });
    let line = || div().flex().flex_row().items_center().gap(m.sp[4]);
    let row_el = div()
        .debug_selector(move || row_id(index))
        .flex()
        .flex_row()
        .flex_1()
        .items_center()
        .min_h(row_min_h(wide, s))
        .gap(m.sp[4]);
    if !wide {
        let text = div()
            .flex()
            .flex_col()
            .justify_center()
            .flex_1()
            .min_w_0()
            .child(
                line()
                    .h(px(LABEL_ROW * s))
                    .child(label.flex_1().min_w_0())
                    .child(value),
            )
            .child(line().h(px(LABEL_ROW * s)).child(meter).child(pct));
        return row_el.child(text).into_any_element();
    }
    row_el
        .child(label.flex_none().w(px(ROW_LABEL_W * s)))
        .child(value.flex_none().w(px(ROW_VALUE_W * s)))
        .child(meter)
        .child(pct)
        .into_any_element()
}

/// Row `index`'s sparkline cell. It has the height of its text row, so the two columns line up.
fn spark_cell(chart: &Entity<ChartView>, index: usize, wide: bool, m: &UiMetrics) -> AnyElement {
    div()
        .debug_selector(move || spark_id(index))
        .relative()
        .flex_1()
        .min_h(row_min_h(wide, m.ui_scale))
        .child(chart.clone().cached(StyleRefinement::default().size_full()))
        .into_any_element()
}

/// The smallest `charts` cell, in px.
fn cell_min(m: &UiMetrics) -> Size<Pixels> {
    size(px(CELL_MIN_W * m.ui_scale), px(CELL_MIN_H * m.ui_scale))
}

/// The `charts` grid for the five cells in `body`, placed as `arrange` says (`arrange_grid`).
/// Before the first paint the body is empty and the fit is one column or one row.
fn charts_fit(arrange: Arrange, body: Size<Pixels>, m: &UiMetrics) -> GridFit {
    arrange_grid(
        arrange,
        ROWS,
        body,
        cell_min(m),
        m.sp[2],
        px(GRID_BAR_W * m.ui_scale),
    )
}

/// A `charts` cell of `cell` px: the header with the row's name, value and percent over its chart.
fn chart_cell(
    i: usize,
    row: &MeterRow,
    percent: &SharedString,
    chart: &Entity<ChartView>,
    cell: Size<Pixels>,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    // The text style is set first: setting it replaces the nowrap, ellipsis and colour set
    // before it, and a narrow cell would wrap a long name.
    let mut name = div();
    *name.text_style() = t_sm(m);
    let name = name
        .debug_selector(move || cell_head_id(i, "name"))
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(row.label.clone());
    let mut value = div();
    *value.text_style() = numeric_style(m, m.text.sm);
    let value = value
        .debug_selector(move || cell_head_id(i, "value"))
        .flex()
        .min_w_0()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(fit_text(row.value.clone()));
    let mut pct = div();
    *pct.text_style() = numeric_style(m, m.text.sm);
    let pct = pct
        .debug_selector(move || cell_head_id(i, "percent"))
        .flex()
        .min_w_0()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(fit_text(percent.clone()));
    div()
        .debug_selector(move || cell_id(i))
        .flex()
        .flex_col()
        .flex_none()
        .w(cell.width)
        .h(cell.height)
        .overflow_hidden()
        .gap(m.sp[2])
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(LABEL_ROW * m.ui_scale))
                .child(name)
                .child(value)
                .child(pct),
        )
        .child(
            div()
                .debug_selector(move || cell_chart_id(i))
                .relative()
                .flex_1()
                .min_h_0()
                .child(chart.clone().cached(StyleRefinement::default().size_full())),
        )
        .into_any_element()
}

impl MemBox {
    /// `charts`: one chart per row that fills the body in one column, one row or a grid. Cells
    /// that do not fit scroll under a scrollbar.
    fn charts_body(&mut self, m: &UiMetrics, theme: &TlTheme) -> (AnyElement, GridFit) {
        let fit = charts_fit(self.arrange, self.body.get(), m);
        let cells = grid_cells(
            &fit,
            (0..ROWS).map(|i| {
                chart_cell(
                    i,
                    &self.rows[i],
                    &self.percents[i],
                    &self.cell_charts[i],
                    fit.cell,
                    m,
                    theme,
                )
            }),
            m.sp[2],
            CHARTS_GRID_ID,
        );
        let body = div()
            .debug_selector(|| CHARTS_GRID_ID.into())
            .relative()
            .size_full()
            .overflow_hidden()
            .child(cells)
            .into_any_element();
        (body, fit)
    }

    /// The five text rows in a column that shares the body height. A row that does not fit whole
    /// is left out, top first (UI-PRF-017).
    fn text_rows(&self, wide: bool, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
        whole_rows()
            .size_full()
            .gap(m.sp[2])
            .children(
                self.rows
                    .iter()
                    .zip(&self.percents)
                    .enumerate()
                    .map(|(i, (r, p))| text_row(r, p, i, wide, m, theme)),
            )
            .into_any_element()
    }

    /// `rows`: the text part, the splitter and the sparkline column.
    fn rows_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share = self.split(m);
        let text = self.text_rows(wide, m, theme);
        let sparks = whole_rows().size_full().gap(m.sp[2]).children(
            self.sparks
                .iter()
                .enumerate()
                .map(|(i, c)| spark_cell(c, i, wide, m)),
        );
        div()
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .gap(m.sp[5])
            .child(div().flex_none().w(relative(share)).h_full().child(text))
            .child(div().flex_1().min_w_0().h_full().child(sparks))
            .child(self.spark_split.probe(Self::spark_inner_split, cx))
            .child(self.spark_split.gap(
                SplitAxis::Row,
                share,
                m.sp[5],
                Self::spark_inner_split,
                cx,
            ))
            .into_any_element()
    }

    /// `chart_rows`: the used chart over the five rows, with a horizontal splitter.
    fn chart_rows_body(
        &mut self,
        wide: bool,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share = self.split(m);
        let side = self.arrange == Arrange::Horizontal;
        let chart = div()
            .debug_selector(|| MEM_USED_CHART_ID.into())
            .relative()
            .flex_none()
            .child(
                self.used_chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            );
        let chart = if side {
            chart.w(relative(share)).h_full()
        } else {
            chart.h(relative(share)).w_full()
        };
        let rows = div().flex_1().min_h_0().min_w_0();
        let rows = if side { rows.h_full() } else { rows.w_full() };
        let frame = div().relative().flex().size_full().gap(m.sp[5]);
        let frame = if side {
            frame.flex_row()
        } else {
            frame.flex_col()
        };
        frame
            .child(chart)
            .child(rows.child(self.text_rows(wide, m, theme)))
            .child(self.chart_split.probe(Self::chart_inner_split, cx))
            .child(self.chart_split.gap(
                self.chart_axis(),
                share,
                m.sp[5],
                Self::chart_inner_split,
                cx,
            ))
            .into_any_element()
    }
}

impl Render for MemBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let probe_m = m;
        let body_w = self.body.get().width;
        // The class the probe reports: whether the text rows are one line. `chart` has none.
        let (class, classify): (u32, Classify) = match self.mode {
            MemMode::Rows => {
                let sharer = self.spark_split.sharer(m.sp[5], m.ui_scale);
                let text_w = move |len: f32| sharer(len) * len;
                let wide = row_wide(px(text_w(f32::from(body_w))), &m);
                (
                    u32::from(wide),
                    Box::new(move |b| {
                        u32::from(row_wide(px(text_w(f32::from(b.width))), &probe_m))
                    }),
                )
            }
            MemMode::ChartRows if self.arrange == Arrange::Horizontal => {
                // The rows are the right part: what the chart leaves of the width.
                let sharer = self.chart_split.sharer(m.sp[5], m.ui_scale);
                let gap = f32::from(m.sp[5]);
                let rows_w = move |len: f32| px(((1. - sharer(len)) * len - gap).max(0.));
                (
                    u32::from(row_wide(rows_w(f32::from(body_w)), &m)),
                    Box::new(move |b| u32::from(row_wide(rows_w(f32::from(b.width)), &probe_m))),
                )
            }
            MemMode::ChartRows => (
                u32::from(row_wide(body_w, &m)),
                Box::new(move |b| u32::from(row_wide(b.width, &probe_m))),
            ),
            MemMode::Chart => (0, Box::new(|_| 0)),
            MemMode::Charts => {
                let arrange = self.arrange;
                (
                    charts_fit(arrange, self.body.get(), &m).class(),
                    Box::new(move |b| charts_fit(arrange, b, &probe_m).class()),
                )
            }
        };
        let inner: AnyElement = match self.mode {
            MemMode::Rows => self.rows_body(&m, &theme, class == 1, cx),
            MemMode::ChartRows => self.chart_rows_body(class == 1, &m, &theme, cx),
            MemMode::Charts => self.charts_body(&m, &theme).0,
            MemMode::Chart => div()
                .debug_selector(|| MEM_COMP_CHART_ID.into())
                .relative()
                .size_full()
                .child(
                    self.comp_chart
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
                .into_any_element(),
        };
        let body = div()
            .debug_selector(|| MEM_BODY_ID.into())
            .relative()
            .size_full()
            .child(inner)
            .child(class_probe(
                Rc::clone(&self.body),
                class,
                classify,
                cx.entity_id(),
            ))
            .into_any_element();
        box_frame(
            MEM_BOX_ID,
            Some(DeviceKey(DeviceKey::MEMORY.into())),
            self.title.clone(),
            self.value.clone(),
            SharedString::default(),
            &self.range_text,
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .relative()
        .size_full()
    }
}
