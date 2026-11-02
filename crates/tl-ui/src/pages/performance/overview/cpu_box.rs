//! The Overview page's CPU box (UI-PRF-017, D-022, UI-PRF-023): the total chart and one row per
//! logical CPU (crw.53): name, sparkline, meter, percent and clock. The rows are at least 18 px
//! high, flow top to bottom into up to 4 columns and grow to fill the height (D-040), then
//! scroll at 18 px (D-029). The CPU page's core board
//! draws the same rows (crw.104).
//!
//! The core rows show a moving average of each core's `CPU_TOTAL` (UI-PRF-018, D-024), and the
//! core sparklines plot 1-second means. The total chart and the title value stay raw.
//!
//! The box is its own view. It formats its text only when the inputs' hash changes and notifies
//! only when the title text changed, so an idle tick renders nothing. The total chart and the
//! core sparklines are cached child views that re-render for their own series. Pointer movement
//! never notifies.
//!
//! D-041, ADR-019 phase 2: the modes are presets of the box's widget layout. A preset is a flow
//! layout (`cpu_flow.rs`): the total chart, the cores group, and the Core clocks chart and the
//! power draw strip when they are on. Each part is a widget of the box's `WidgetContainer` that
//! draws its 1.2.0 part at the place the rules above give it: the charts as `MetricWidget`s
//! with `FlowLook::Chart`, the core rows and the `core_charts` cells as the cores group's
//! `GroupWidget` with a `GroupFlowLook`. The box keeps the charts, the core rows' model and the
//! sparklines and hands them to the widgets, so nothing is built twice. When a core's 1% or
//! 0.1 GHz bucket changes, only the cores group renders again. Each render makes a
//! `cpu_flow::CpuPlan`, from which the container's placer works out every widget's rectangle.
//! A grid layout (the user's own widgets, after Apply) is drawn by the box's [`WidgetEditor`]
//! and its `WidgetContainer`; so is the box while Edit Widgets is on.

use std::cell::{Cell, Ref, RefCell};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EntityId, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative, size,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{CPU_BOX_CONTAINER, ContainerLayout};
use tl_model::series::Sample;
use tl_model::settings::{CoreSmoothing, NetworkUnit};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::super::core_clocks::CoreClocks;
use super::super::cpu::{PowerState, SecondaryAvail, logical_devices};
use super::cpu_flow::{self, CpuFlowKeys, CpuLook, CpuPlan, PlanCell, PlanPower};
use super::inner_split::{InnerSplit, InnerSplitChanged, PART_MIN, SplitAxis};
use super::layout::{
    Arrange, ArrangeChoice, BoxMode, POWER_SPLIT_DEFAULT, POWER_SPLIT_RANGE, PowerPlacement,
};
use super::parts::{
    ChartSettings, CoreBarsLayout, CoreBarsModel, SPARK_GRID, SPLIT_W, arrange_grid, box_chart,
    box_frame, chart_section_versions, class_probe, core_bars_layout, core_board, edit_chrome,
    grid_cells, is_wide, spark_chart, width_probe,
};
use super::picked::{box_mask, box_snapshot};
use super::range::RangeText;
use super::timeline::{self, OverviewPause};
use crate::chart::hover::HoverLink;
use crate::chart::view::ChangedRuns;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, LegendStyle, ValueAlign,
    ValueFormat, YScale, format_value,
};
use crate::components::validity::copy_text;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::pages::performance::widget::container::WidgetContainer;
use crate::pages::performance::widget::edit::{WidgetEditEvent, WidgetEditor};
use crate::pages::performance::widget::group_widget::GroupFlowLook;
use crate::pages::performance::widget::metric_widget::FlowLook;
use crate::pages::performance::widget::source::SourceStore;
use crate::selector::TlSelector;
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The debug selector of the gap between the chart and the core rows.
pub const CPU_SPLITTER_ID: &str = "perf-overview-cpu-splitter";

/// The debug selector of its accent line.
pub const CPU_SPLITTER_LINE_ID: &str = "perf-overview-cpu-splitter-line";

/// The debug selector of the gap above the power draw strip, and of its accent line.
pub const POWER_SPLITTER_ID: &str = "perf-overview-cpu-power-splitter";
pub const POWER_SPLITTER_LINE_ID: &str = "perf-overview-cpu-power-splitter-line";

/// The debug selector of the total chart's part.
pub const CPU_CHART_ID: &str = "perf-overview-cpu-chart";

/// The debug selector of the power draw strip.
pub const POWER_STRIP_ID: &str = "perf-overview-cpu-power-strip";

/// The debug selector of the Core clocks chart's part (tasklens-o55.57).
pub const CLOCKS_ID: &str = "perf-overview-cpu-clocks";

/// The smallest power draw strip (UI-PRF-019 "Power draw strip"), logical px.
pub const POWER_STRIP_MIN: f32 = 64.;

/// The share of the parts above the strip a drag or a double-click ended on, rounded to 3
/// decimals. The page saves the strip's share, one minus this, as `power_split`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PowerSplitChanged(pub f32);

impl From<f32> for PowerSplitChanged {
    fn from(share: f32) -> Self {
        Self(share)
    }
}

impl EventEmitter<PowerSplitChanged> for CpuBox {}

/// The narrowest column of core rows (UI-PRF-018 "CPU box splitter"), logical px.
pub const CORE_COLUMN_MIN_W: f32 = 188.;

/// The box's layout modes (UI-PRF-019). `chart_cores` has the total chart and the core rows,
/// the chart taking 40% of a wide body by default.
pub const MODES: &[BoxMode] = &[
    BoxMode {
        id: "chart_cores",
        label: "perf-overview-mode-chart-cores",
        split: Some(0.4),
    },
    BoxMode {
        id: "chart",
        label: "perf-overview-mode-chart",
        split: None,
    },
    BoxMode {
        id: "cores",
        label: "perf-overview-mode-cores",
        split: None,
    },
    BoxMode {
        id: "core_charts",
        label: "perf-overview-mode-core-charts",
        split: None,
    },
];

/// The modes with an Arrange choice (UI-PRF-019, D-027): the grid of `core_charts` and the two
/// parts of `chart_cores`, automatic by default.
pub const ARRANGE: &[(&str, ArrangeChoice)] = &[
    ("chart_cores", ArrangeChoice::Saved(Arrange::Auto)),
    ("core_charts", ArrangeChoice::Saved(Arrange::Auto)),
];

/// The smallest cell of the `core_charts` grid, logical px before `ui_scale` (UI-PRF-019).
pub const CORE_CHART_MIN: (f32, f32) = (120., 60.);

/// The gap between `core_charts` cells, logical px before `ui_scale`.
const CORE_CHART_GAP: f32 = 8.;

/// The width a one-column `core_charts` grid keeps for its scrollbar when it scrolls, logical
/// px before `ui_scale`.
const CORE_GRID_BAR_W: f32 = 16.;

/// The debug selector of the `core_charts` grid.
pub const CORE_CHARTS_ID: &str = "perf-overview-core-charts";

/// The debug selector of core `i`'s cell in the `core_charts` grid.
pub fn core_chart_id(i: usize) -> String {
    format!("perf-overview-core-chart-{i}")
}

/// Which parts the box shows (UI-PRF-019).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CpuMode {
    ChartCores,
    Chart,
    Cores,
    CoreCharts,
}

impl CpuMode {
    /// The mode `id`; an unknown id is the default.
    fn from_id(id: &str) -> Self {
        match id {
            "chart" => Self::Chart,
            "cores" => Self::Cores,
            "core_charts" => Self::CoreCharts,
            _ => Self::ChartCores,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::ChartCores => MODES[0].id,
            Self::Chart => MODES[1].id,
            Self::Cores => MODES[2].id,
            Self::CoreCharts => MODES[3].id,
        }
    }
}

/// The `core_charts` grid for `n` cores in a body `w` px wide: as many columns of at least
/// 120 px as fit, and the rows they need. Returns `(columns, rows)`.
pub fn core_chart_dims(n: usize, w: Pixels, scale: f32) -> (usize, usize) {
    let n = n.max(1);
    let gap = CORE_CHART_GAP * scale;
    let fit = ((f32::from(w).max(0.) + gap) / (CORE_CHART_MIN.0 * scale + gap)).floor() as usize;
    let cols = fit.clamp(1, n);
    (cols, n.div_ceil(cols))
}

/// The gutter between the total chart and the core rows: 16 px (UI-PRF-059).
pub fn core_gutter(m: &UiMetrics) -> Pixels {
    m.sp[6]
}

/// The page saves the chart share the box ended a drag or a double-click on.
impl EventEmitter<InnerSplitChanged> for CpuBox {}

/// The box's debug selector.
pub const CPU_BOX_ID: &str = "perf-overview-box-cpu";

/// The debug selector of the text an empty grid layout shows (UI-PRF-061).
pub const EMPTY_ID: &str = "perf-overview-cpu-widgets-empty";

const KEY_EMPTY: &str = "perf-widget-empty";

const KEY_TITLE: &str = "perf-card-heading-cpu";
pub(crate) const KEY_CORE: &str = "perf-overview-core";
const KEY_RANGE: &str = "perf-cpu-heading-utilization";
const KEY_LEGEND_TOTAL: &str = "perf-legend-total";
const KEY_LEGEND_KERNEL: &str = "perf-legend-kernel";
const KEY_POWER: &str = "perf-cpu-secondary-power";

/// The span one core sparkline point averages (UI-PRF-018).
pub(crate) const SPARK_MEAN_NS: u64 = 1_000_000_000;

/// One core's exponential moving average of `CPU_TOTAL`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CoreAverage {
    /// Time of the newest sample taken in.
    t_ns: u64,
    /// The average; `None` before the first valid sample and after an invalid one.
    pub(crate) value: Option<f32>,
}

impl CoreAverage {
    /// Takes in the samples newer than `t_ns`: `s += (x − s) · (1 − e^(−Δt/τ))`. An invalid
    /// sample clears the average, and the next valid sample starts it again.
    pub(crate) fn advance(&mut self, samples: &[Sample], tau_s: f32) {
        let from = samples.partition_point(|s| s.t_ns <= self.t_ns);
        for s in &samples[from..] {
            self.value = if !s.is_valid() {
                None
            } else {
                match self.value {
                    Some(v) if self.t_ns > 0 => {
                        let dt_s = (s.t_ns - self.t_ns) as f32 * 1e-9;
                        let k = 1.0 - (-dt_s / tau_s).exp();
                        Some(v + (s.value - v) * k)
                    }
                    _ => Some(s.value),
                }
            };
            self.t_ns = s.t_ns;
        }
    }
}

/// The joint between the clock and the temperature.
const SEP: &str = " · ";

/// The total chart: total with fill, kernel as a line when `kernel`, 0..100.
fn total_spec(cpu: DeviceIdx, s: &ChartSettings, theme: &TlTheme, hover: bool) -> ChartSpec {
    let mut series = smallvec![ChartSeries {
        id: SeriesId {
            device: cpu,
            metric: MetricId::CPU_TOTAL,
        },
        fill: Some(theme.graph.cpu_fill),
        line: theme.graph.cpu_line,
        ..ChartSeries::default()
    }];
    if s.kernel {
        series.push(ChartSeries {
            id: SeriesId {
                device: cpu,
                metric: MetricId::CPU_KERNEL,
            },
            fill: None,
            line: theme.graph.kernel_line,
            ..ChartSeries::default()
        });
    }
    ChartSpec {
        series,
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
            vertical_at: None,
        },
        smooth: s.smooth,
        hover,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// A core sparkline: `CPU_TOTAL` of logical CPU `device`, filled, 0..100.
pub(crate) fn core_spec(device: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: SeriesId {
                device,
                metric: MetricId::CPU_TOTAL,
            },
            fill: Some(theme.graph.cpu_fill),
            line: theme.graph.cpu_line,
            mean_ns: SPARK_MEAN_NS,
            ..ChartSeries::default()
        }],
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        range_ns: s.range_ns,
        grid: SPARK_GRID,
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// The power draw strip: package watts filled, core watts dashed, the same series, colours and
/// scale as the Power draw chart of the CPU page.
fn power_spec(cpu: DeviceIdx, s: &ChartSettings, theme: &TlTheme, hover: bool) -> ChartSpec {
    let series = |metric, fill: bool, dashed| ChartSeries {
        id: SeriesId {
            device: cpu,
            metric,
        },
        fill: fill.then_some(theme.graph.cpu_fill),
        line: theme.graph.cpu_line,
        dashed,
        ..ChartSeries::default()
    };
    ChartSpec {
        series: smallvec![
            series(MetricId::CPU_POWER_PKG, true, false),
            series(MetricId::CPU_POWER_CORE, false, true),
        ],
        y: YScale::StickyUp { hw_max: None },
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
            vertical_at: None,
        },
        smooth: s.smooth,
        hover,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// The Core clocks chart's range, grid and hover; [`CoreClocks`] adds the lines and the scale.
fn clocks_base(s: &ChartSettings) -> ChartSpec {
    ChartSpec {
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
            vertical_at: None,
        },
        smooth: s.smooth,
        hover: true,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// "Power draw" and the current package watts at the right.
fn power_labels(heading: SharedString) -> ChartLabels {
    ChartLabels {
        heading: Some(heading),
        value_fmt: ValueFormat::Watts,
        value_align: ValueAlign::Right,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// No heading and no value: the box title row carries the numbers.
fn total_labels(range_word: SharedString, kernel: bool, cx: &App) -> ChartLabels {
    let mut legend = smallvec![(tr(cx, KEY_LEGEND_TOTAL), LegendStyle::Solid)];
    if kernel {
        legend.push((tr(cx, KEY_LEGEND_KERNEL), LegendStyle::Dashed));
    }
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Hidden,
        range_word,
        legend,
        show_axis_max: false,
    }
}

/// The newest valid sample of `id`.
pub(crate) fn newest(snap: &Snapshot, id: SeriesId) -> Option<f32> {
    snap.series
        .get(id)?
        .samples
        .iter()
        .rev()
        .find(|s| s.is_valid())
        .map(|s| s.value)
}

/// The text for a value of `validity`: `format(out)` for Valid and Stale, else the validity
/// glyph.
pub(super) fn value_text(validity: Validity, format: impl FnOnce(&mut String)) -> SharedString {
    match validity {
        Validity::Valid | Validity::Stale => {
            let mut out = String::new();
            format(&mut out);
            out.into()
        }
        v => copy_text(v, &SharedString::default()),
    }
}

/// The CPU box.
pub struct CpuBox {
    /// The range figures the page gives (UI-PRF-055).
    range_text: RangeText,
    hub: Entity<SnapshotHub>,
    cpu: DeviceIdx,
    total: Entity<ChartView>,
    bars: Rc<RefCell<CoreBarsModel>>,
    /// One sparkline per entry of `logical`, shared with the cores group's flow look.
    sparks: Rc<[Entity<ChartView>]>,
    /// Logical CPU id to device; `DeviceIdx(u16::MAX)` for an id with no catalog entry.
    logical: Vec<DeviceIdx>,
    /// `CPU_TOTAL` of each entry of `logical`.
    logical_ids: Vec<SeriesId>,
    /// `CPU_FREQ` of each entry of `logical`.
    freq_ids: Vec<SeriesId>,
    /// `performance.overview_core_smoothing`, and the average of each entry of `logical`.
    smoothing: CoreSmoothing,
    averages: Vec<CoreAverage>,
    /// The page is held at a time (UI-PRF-057): the rows show the history's values there,
    /// unsmoothed.
    paused: bool,
    /// Whether the last render put the core rows beside the chart: the box is wide and the
    /// arrangement is automatic, or the arrangement is Side by side.
    side: bool,
    /// The core grid's size at the last paint, and the layout the last render used.
    grid_size: Rc<Cell<Size<Pixels>>>,
    grid: Rc<Cell<CoreBarsLayout>>,
    catalog_version: u64,
    title: SharedString,
    value: SharedString,
    line: SharedString,
    /// Hash of the inputs `value` and `line` were formatted from.
    text_bits: u64,
    /// The box width of the last paint, and whether the last render put the bars beside the
    /// chart.
    width: Rc<Cell<Pixels>>,
    wide: bool,
    mode: CpuMode,
    /// How `core_charts` places its cells and `chart_cores` its two parts (D-027).
    arrange: Arrange,
    /// The gap between the chart and the core rows (UI-PRF-018 "CPU box splitter").
    split: InnerSplit,
    /// Whether the power draw strip is switched on, and whether RAPL can be read.
    power: bool,
    power_state: PowerState,
    /// The strip's chart, built the first time the strip is switched on.
    power_chart: Option<Entity<ChartView>>,
    /// The gap above the strip. Its share is the parts' share of the body, or the graph's share
    /// of its column when the strip sits under the graph.
    power_split: InnerSplit<PowerSplitChanged>,
    /// Where the strip sits (tasklens-o55.55).
    power_placement: PowerPlacement,
    /// The total chart, the Core clocks chart and the strip share one hover cursor. The total
    /// chart has hover while another chart sits under it in its column, the strip while it sits
    /// under the graph ([`Self::power_under_graph`]). The Core clocks chart always has hover.
    hover_link: HoverLink,
    hover_on: bool,
    power_hover_on: bool,
    /// Whether the Core clocks chart is switched on (the saved `clocks`, tasklens-o55.57), and
    /// its chart, built the first time it is switched on.
    clocks: bool,
    core_clocks: Option<CoreClocks>,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    /// The flow layout of the shown preset (ADR-019): which parts the box shows.
    flow: ContainerLayout,
    /// The saved grid layout, when the user has one: the editor draws it.
    saved_grid: Option<ContainerLayout>,
    /// Edit Widgets is on: the editor draws the body.
    editing: bool,
    editor: Entity<WidgetEditor>,
    container: Entity<WidgetContainer>,
    /// The plan of the last flow render, which the container's placer reads.
    plan: PlanCell,
    _subs: [Subscription; 5],
    _editor_sub: Subscription,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

/// The saved grid layout of the CPU box, if any.
fn saved_grid(cx: &App) -> Option<ContainerLayout> {
    if !cx.has_global::<TlSettings>() {
        return None;
    }
    TlSettings::settings(cx)
        .performance
        .layouts
        .known(CPU_BOX_CONTAINER)
        .filter(|l| l.preset.is_none())
        .cloned()
}

/// The keys of the default mode, with nothing switched on.
fn default_keys() -> CpuFlowKeys<'static> {
    CpuFlowKeys {
        mode: MODES[0].id,
        arrange: Some(Arrange::Auto),
        split: None,
        power: false,
        power_split: None,
        power_placement: None,
        clocks: false,
    }
}

impl CpuBox {
    pub fn new(hub: &Entity<SnapshotHub>, cpu: DeviceIdx, cx: &mut Context<Self>) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let spec = total_spec(cpu, &settings, &theme, false);
        let labels = total_labels(tr(cx, KEY_RANGE), settings.kernel, cx);
        let total = box_chart(spec, labels, hub, cx);
        let hover_link = HoverLink::new();
        total.update(cx, |v, cx| v.set_hover_link(hover_link.clone(), cx));
        let snap = box_snapshot(hub, cx);
        let grid_size = size(px(0.), px(0.));
        let flow = cpu_flow::flow_layout(default_keys());
        let saved = saved_grid(cx);
        let plan = PlanCell::default();
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::new(hub)));
            let shown = saved.clone().unwrap_or_else(|| flow.clone());
            let plan = plan.clone();
            cx.new(|cx| {
                let mut c = WidgetContainer::new(CPU_BOX_CONTAINER, shown, hub, store, cx);
                c.set_flow_placer(Some(cpu_flow::placer(plan)), cx);
                // The box draws the widgets of its flow layouts in their 1.2.0 parts.
                c.set_hosted(true, cx);
                c
            })
        };
        let editor = cx.new(|cx| WidgetEditor::new(container.clone(), hub, cx));
        let editor_sub = cx.subscribe(&editor, Self::on_widget_edit);
        let mut this = Self {
            range_text: RangeText::default(),
            hub: hub.clone(),
            cpu,
            total,
            bars: Rc::default(),
            sparks: Rc::from([]),
            logical: Vec::new(),
            logical_ids: Vec::new(),
            freq_ids: Vec::new(),
            smoothing: TlSettings::settings(cx).performance.overview_core_smoothing,
            averages: Vec::new(),
            paused: timeline::paused(cx).is_some(),
            grid_size: Rc::new(Cell::new(grid_size)),
            grid: Rc::new(Cell::new(core_bars_layout(
                0,
                grid_size.width,
                grid_size.height,
                1.,
            ))),
            catalog_version: u64::MAX,
            title: tr(cx, KEY_TITLE),
            value: SharedString::default(),
            line: SharedString::default(),
            text_bits: 0,
            width: Rc::new(Cell::new(px(0.))),
            wide: true,
            side: true,
            mode: CpuMode::ChartCores,
            arrange: Arrange::Auto,
            split: InnerSplit::new(
                CPU_SPLITTER_ID,
                CPU_SPLITTER_LINE_ID,
                MODES[0].split.unwrap_or_default(),
            )
            .with_mins(PART_MIN, CORE_COLUMN_MIN_W),
            power: false,
            power_state: PowerState::NoZone,
            power_chart: None,
            power_split: InnerSplit::new(
                POWER_SPLITTER_ID,
                POWER_SPLITTER_LINE_ID,
                1. - POWER_SPLIT_DEFAULT,
            )
            .with_range(1. - POWER_SPLIT_RANGE.1, 1. - POWER_SPLIT_RANGE.0)
            .with_mins(PART_MIN, POWER_STRIP_MIN),
            power_placement: PowerPlacement::FullWidth,
            hover_link,
            hover_on: false,
            power_hover_on: false,
            clocks: false,
            core_clocks: None,
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
            flow,
            saved_grid: saved,
            editing: false,
            editor,
            container,
            plan,
            _editor_sub: editor_sub,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_devices(&snap, cx);
        this.sync_text(&snap, true);
        this.sync_power_state(&snap);
        // A hovered core row highlights its core in the Core clocks chart.
        let weak = cx.weak_entity();
        this.bars.borrow_mut().set_hover_sink(move |cpu, cx| {
            let _ = weak.update(cx, |b, cx| {
                if let Some(clocks) = &b.core_clocks {
                    clocks.highlight_cpu(cpu, cx);
                }
            });
        });
        this
    }

    pub fn total_chart(&self) -> &Entity<ChartView> {
        &self.total
    }

    pub fn bars(&self) -> Ref<'_, CoreBarsModel> {
        self.bars.borrow()
    }

    /// One sparkline per logical CPU.
    pub fn core_sparks(&self) -> &[Entity<ChartView>] {
        &self.sparks
    }

    /// The core grid layout of the last render.
    pub fn core_layout(&self) -> CoreBarsLayout {
        self.grid.get()
    }

    /// The editor of the box's widgets (Edit Widgets, UI-PRF-061).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The container of the box's widgets.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The flow layout of the shown preset. Its preset keeps the mode, the arrangement, the
    /// strip's placement and the two switches; the splits are the box's own, so a drag does not
    /// lay the widgets out again.
    pub fn flow(&self) -> &ContainerLayout {
        &self.flow
    }

    /// The saved grid layout the box shows, if any.
    pub fn grid(&self) -> Option<&ContainerLayout> {
        self.saved_grid.as_ref()
    }

    /// The plan of the last flow render: the parts and their widgets the placer places.
    pub fn flow_plan(&self) -> Option<CpuPlan> {
        self.plan.plan()
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn_widgets(&self) -> SmallVec<[u32; 4]> {
        self.plan.drawn()
    }

    /// Whether the box draws the parts of its flow layout itself (no grid layout, not editing).
    fn host_draws(&self) -> bool {
        !self.editing && self.saved_grid.is_none()
    }

    /// Gets the container ready for Edit Widgets: with a flow layout it takes the preset's
    /// widgets that the last render drew, placed where they were drawn, so the editor pins
    /// them there. A Core clocks chart or a power draw strip the mode does not show is left out,
    /// as it took no space. With a grid layout the container already holds it.
    pub fn prepare_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing || self.saved_grid.is_some() {
            return;
        }
        let drawn = self.plan.drawn();
        let mut layout = self.flow.clone();
        layout.widgets.retain(|w| drawn.contains(&w.id));
        self.container.update(cx, |c, cx| {
            c.set_layout(layout, cx);
            c.place_again(cx);
        });
    }

    /// Edit Widgets started or ended. When it ends, the box shows the saved layout again.
    fn on_widget_edit(
        &mut self,
        _: Entity<WidgetEditor>,
        ev: &WidgetEditEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            WidgetEditEvent::Started => self.editing = true,
            WidgetEditEvent::Ended { .. } => {
                self.editing = false;
                self.saved_grid = saved_grid(cx);
                self.show_saved(cx);
            }
        }
        self.notify(cx);
    }

    /// The container holds the saved grid layout, or the flow layout the box draws.
    fn show_saved(&mut self, cx: &mut Context<Self>) {
        let shown = self.saved_grid.clone().unwrap_or_else(|| self.flow.clone());
        self.container.update(cx, |c, cx| c.set_layout(shown, cx));
    }

    /// The saved grid layout came, went or changed: outside Edit Widgets the box shows it.
    fn sync_grid(&mut self, cx: &mut Context<Self>) {
        let grid = saved_grid(cx);
        if grid == self.saved_grid {
            return;
        }
        self.saved_grid = grid;
        if self.editing {
            return;
        }
        self.show_saved(cx);
        self.notify(cx);
    }

    /// The flow layout of the shown mode, arrangement and switches. Returns whether it changed.
    fn sync_flow(&mut self, cx: &mut Context<Self>) -> bool {
        let has_arrange = ARRANGE.iter().any(|(id, _)| *id == self.mode.id());
        let flow = cpu_flow::flow_layout(CpuFlowKeys {
            mode: self.mode.id(),
            arrange: has_arrange.then_some(self.arrange),
            split: None,
            power: self.power,
            power_split: None,
            power_placement: (self.power_placement != PowerPlacement::FullWidth)
                .then_some(self.power_placement),
            clocks: self.clocks,
        });
        if flow == self.flow {
            return false;
        }
        self.flow = flow;
        if !self.editing && self.saved_grid.is_none() {
            self.show_saved(cx);
        }
        true
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A core row's text or bucket changed: the cores group draws the rows, so only it renders
    /// again. When the box draws no cores group (a grid layout keeps its own sources, and the
    /// `chart` mode shows no rows) nothing renders; before the first flow render places the
    /// group, the box renders.
    fn notify_cores(&mut self, cx: &mut Context<Self>) {
        if !self.host_draws() || self.mode == CpuMode::Chart {
            return;
        }
        let Some(id) = cpu_flow::cores_of(&self.flow) else {
            return;
        };
        let group = self.container.read(cx).group(id).cloned();
        match group {
            Some(g) => g.update(cx, |g, cx| g.flow_changed(cx)),
            None => self.notify(cx),
        }
    }

    /// The title, value and line text.
    pub fn text(&self) -> (&SharedString, &SharedString, &SharedString) {
        (&self.title, &self.value, &self.line)
    }

    /// Whether the last render put the bars beside the chart (box at least 560 px wide, or the
    /// arrangement is Side by side).
    pub fn side_by_side(&self) -> bool {
        self.side
    }

    /// How `core_charts` places its cells and `chart_cores` its parts.
    pub fn arrange(&self) -> Arrange {
        self.arrange
    }

    /// Places the `core_charts` cells and the `chart_cores` parts as `arrange` says. Only this
    /// box renders again, and only in a mode the choice applies to.
    pub fn set_arrange(&mut self, arrange: Arrange, cx: &mut Context<Self>) {
        if arrange != self.arrange {
            self.arrange = arrange;
            self.sync_flow(cx);
            if matches!(self.mode, CpuMode::ChartCores | CpuMode::CoreCharts) {
                cx.notify();
            }
        }
    }

    /// The chart's share of the body as the box lays it out, after the clamps: its width side by
    /// side, its height stacked.
    pub fn split(&self, m: &UiMetrics) -> f32 {
        self.split
            .share(self.split_axis(), core_gutter(m), m.ui_scale)
    }

    /// The axis of the `chart_cores` parts: side by side with a vertical gap, or stacked with a
    /// horizontal gap.
    fn split_axis(&self) -> SplitAxis {
        if self.side {
            SplitAxis::Row
        } else {
            SplitAxis::Column
        }
    }

    /// Takes the saved share. Nothing changes while the gap is held.
    pub fn set_split(&mut self, share: f32, cx: &mut Context<Self>) {
        if self.split.set_share(share) {
            cx.notify();
        }
    }

    /// The shown mode's id.
    pub fn mode(&self) -> &'static str {
        self.mode.id()
    }

    /// Shows mode `id`; an unknown id is the default mode. Only this box renders again.
    pub fn set_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let mode = CpuMode::from_id(id);
        if mode != self.mode {
            self.mode = mode;
            self.sync_flow(cx);
            self.sync_hover(cx);
            cx.notify();
        }
    }

    fn inner_split(&mut self) -> &mut InnerSplit {
        &mut self.split
    }

    fn power_inner_split(&mut self) -> &mut InnerSplit<PowerSplitChanged> {
        &mut self.power_split
    }

    /// Whether the CPU power draw can be read.
    pub fn power_state(&self) -> PowerState {
        self.power_state
    }

    /// Whether the strip is switched on (the saved `power`), readable or not.
    pub fn power_on(&self) -> bool {
        self.power
    }

    /// Whether the box draws the strip: switched on and RAPL readable. No zone draws nothing,
    /// and an unreadable zone has nothing to plot.
    pub fn power_shown(&self) -> bool {
        self.power && self.power_state == PowerState::Readable
    }

    /// Where the strip sits.
    pub fn power_placement(&self) -> PowerPlacement {
        self.power_placement
    }

    /// Whether the box draws the strip under the total chart, beside the core rows: the strip
    /// is shown, placed under the graph, and the mode has the chart and the core rows. Other
    /// modes draw it full width.
    pub fn power_under_graph(&self) -> bool {
        self.power_shown()
            && self.power_placement == PowerPlacement::UnderGraph
            && self.mode == CpuMode::ChartCores
    }

    /// Whether the box draws the Core clocks chart under the total chart: switched on, in a
    /// mode with the total chart.
    pub fn clocks_shown(&self) -> bool {
        self.clocks && matches!(self.mode, CpuMode::ChartCores | CpuMode::Chart)
    }

    /// Whether the Core clocks chart is switched on (the saved `clocks`).
    pub fn clocks_on(&self) -> bool {
        self.clocks
    }

    /// The Core clocks chart, once it has been switched on.
    pub fn core_clocks(&self) -> Option<&CoreClocks> {
        self.core_clocks.as_ref()
    }

    /// Shows or hides the Core clocks chart under the total chart (tasklens-o55.57).
    pub fn set_clocks(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.clocks {
            return;
        }
        self.clocks = on;
        if on && self.core_clocks.is_none() {
            let labels = super::super::core_clocks::clocks_labels(false, cx);
            let view = box_chart(clocks_base(&self.settings), labels, &self.hub, cx);
            view.update(cx, |v, cx| v.set_hover_link(self.hover_link.clone(), cx));
            let snap = box_snapshot(&self.hub, cx);
            let base = clocks_base(&self.settings);
            self.core_clocks = Some(CoreClocks::new(view, base, false, &snap, cx));
        }
        self.sync_flow(cx);
        self.sync_hover(cx);
        cx.notify();
    }

    /// Gives the charts of the graph column hover with one shared cursor: the total chart while
    /// the strip or the Core clocks chart sits under it, the strip while it sits under the
    /// graph. Full width keeps the strip without hover.
    fn sync_hover(&mut self, cx: &mut Context<Self>) {
        let on = self.power_under_graph() || self.clocks_shown();
        let power_on = self.power_under_graph();
        if (on, power_on) == (self.hover_on, self.power_hover_on) {
            return;
        }
        self.hover_on = on;
        self.power_hover_on = power_on;
        let charts = [
            (Some(&self.total), on),
            (self.power_chart.as_ref(), power_on),
        ];
        for (chart, hover) in charts {
            let Some(chart) = chart else {
                continue;
            };
            chart.update(cx, |v, cx| {
                let spec = ChartSpec {
                    hover,
                    ..v.spec.clone()
                };
                v.set_spec(spec, cx);
            });
        }
    }

    /// The strip's chart, once it has been switched on.
    pub fn power_chart(&self) -> Option<&Entity<ChartView>> {
        self.power_chart.as_ref()
    }

    /// The strip's share of the body as the box lays it out, after the clamps.
    pub fn power_share(&self, m: &UiMetrics) -> f32 {
        1. - self
            .power_split
            .share(SplitAxis::Column, m.sp[5], m.ui_scale)
    }

    /// Takes the saved strip: shown or not, its share of the body (`power_split`) and where it
    /// sits. Nothing changes while the gap is held.
    pub fn set_power(
        &mut self,
        on: bool,
        strip_share: f32,
        placement: PowerPlacement,
        cx: &mut Context<Self>,
    ) {
        let mut changed = self.power_split.set_share(1. - strip_share);
        if on != self.power {
            self.power = on;
            changed = true;
        }
        if placement != self.power_placement {
            self.power_placement = placement;
            changed = true;
        }
        if on && self.power_chart.is_none() {
            let theme = current(cx).clone();
            let spec = power_spec(self.cpu, &self.settings, &theme, self.power_hover_on);
            let labels = power_labels(tr(cx, KEY_POWER));
            let chart = box_chart(spec, labels, &self.hub, cx);
            chart.update(cx, |v, cx| v.set_hover_link(self.hover_link.clone(), cx));
            self.power_chart = Some(chart);
        }
        changed |= self.sync_flow(cx);
        self.sync_hover(cx);
        if changed {
            cx.notify();
        }
    }

    /// Reads whether RAPL is readable. True when that changed.
    fn sync_power_state(&mut self, snap: &Snapshot) -> bool {
        let state = SecondaryAvail::read(snap, &snap.device_details.cpu).power;
        std::mem::replace(&mut self.power_state, state) != state
    }

    /// "CPU\t12%", the line when there is one, then "C0\t10%" per logical CPU, with "\t3.9 GHz"
    /// when its clock is known.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\t');
        out.push_str(&self.value);
        out.push('\n');
        if !self.line.is_empty() {
            out.push_str(&self.line);
            out.push('\n');
        }
        self.bars.borrow().copy_text(out);
    }

    /// Takes the range figures the page worked out (UI-PRF-055). Notifies
    /// only when they changed.
    pub fn set_range_text(&mut self, text: RangeText, cx: &mut Context<Self>) {
        if self.range_text != text {
            self.range_text = text;
            cx.notify();
        }
    }

    /// The range figures the box shows.
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
        let devices = mask.contains(ComponentMask::DEVICES);
        let series = mask.contains(ComponentMask::SERIES);
        let system = mask.contains(ComponentMask::SYSTEM);
        let details = mask.contains(ComponentMask::DEVICE_DETAILS);
        let inventory = mask.contains(ComponentMask::INVENTORY);
        if !(devices || series || system || details || inventory) {
            return;
        }
        let snap = box_snapshot(&hub, cx);
        let mut changed = false;
        let mut rows = false;
        if devices {
            changed |= self.sync_devices(&snap, cx);
            rows |= changed;
        }
        if series && !changed {
            let runs = ChangedRuns::new(&ev.changed_series);
            // While paused the changed ids are live ones: the history's series all changed.
            if self.paused
                || self
                    .logical_ids
                    .iter()
                    .chain(&self.freq_ids)
                    .any(|id| runs.contains(*id))
            {
                rows |= self.sync_bars(&snap);
            }
        }
        if system {
            changed |= self.sync_text(&snap, false);
        }
        if (system || details) && self.sync_power_state(&snap) {
            changed = true;
            self.sync_hover(cx);
        }
        if (devices || system || inventory)
            && let Some(clocks) = &mut self.core_clocks
        {
            clocks.sync(&snap, cx);
        }
        if changed {
            self.notify(cx);
        }
        if rows {
            self.notify_cores(cx);
        }
    }

    /// The page was held at a time or made live: the value, line and rows read the other
    /// snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let paused = timeline::paused(cx).is_some();
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        self.averages.fill(CoreAverage::default());
        let snap = box_snapshot(&self.hub, cx);
        self.sync_bars(&snap);
        self.sync_text(&snap, true);
        self.notify(cx);
        self.notify_cores(cx);
    }

    /// Rebuilds the logical CPU list, the row names and the sparklines when the catalog changed.
    /// True when it did.
    fn sync_devices(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        if snap.devices.version == self.catalog_version {
            return false;
        }
        self.catalog_version = snap.devices.version;
        let old = std::mem::take(&mut self.logical);
        logical_devices(&snap.devices, &mut self.logical);
        let id = |device, metric| SeriesId { device, metric };
        self.logical_ids.clear();
        self.logical_ids.extend(
            self.logical
                .iter()
                .map(|&device| id(device, MetricId::CPU_TOTAL)),
        );
        self.freq_ids.clear();
        self.freq_ids.extend(
            self.logical
                .iter()
                .map(|&device| id(device, MetricId::CPU_FREQ)),
        );
        self.averages.clear();
        self.averages
            .resize(self.logical.len(), CoreAverage::default());
        self.sync_sparks(&old, cx);
        self.sync_names(cx);
        self.sync_bars(snap);
        true
    }

    /// One sparkline per logical CPU. Keeps the charts of CPUs whose device did not change.
    fn sync_sparks(&mut self, old: &[DeviceIdx], cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let mut sparks: Vec<Entity<ChartView>> = self.sparks.to_vec();
        sparks.truncate(self.logical.len());
        for (i, &device) in self.logical.iter().enumerate() {
            let spec = core_spec(device, &self.settings, &theme);
            if let Some(chart) = sparks.get(i) {
                if old.get(i) != Some(&device) {
                    chart.update(cx, |v, cx| v.set_spec(spec, cx));
                }
            } else {
                let chart = spark_chart(spec, ValueFormat::Percent, &self.hub, cx);
                sparks.push(chart);
            }
        }
        if sparks[..] != self.sparks[..] {
            self.sparks = sparks.into();
        }
    }

    /// "C0", "C1", … for the logical CPUs.
    fn sync_names(&mut self, cx: &mut Context<Self>) {
        let names = (0..self.logical.len())
            .map(|n| tr_args(cx, KEY_CORE, &[("n", FluentValue::from(n))]))
            .collect::<Vec<_>>();
        let mut bars = self.bars.borrow_mut();
        bars.set_cpus(names.into_iter());
    }

    /// The smoothed `CPU_TOTAL` (the newest valid sample with smoothing off) and the newest valid
    /// `CPU_FREQ` of each logical CPU to the rows; while paused, the newest at the picked time.
    /// True when a row's 1% or 0.1 GHz bucket changed.
    fn sync_bars(&mut self, snap: &Snapshot) -> bool {
        let tau_s = if self.paused {
            0.
        } else {
            self.smoothing.tau_s()
        };
        if tau_s > 0.0 {
            for (avg, id) in self.averages.iter_mut().zip(&self.logical_ids) {
                if let Some(view) = snap.series.get(*id) {
                    avg.advance(&view.samples, tau_s);
                }
            }
        }
        let mut bars = self.bars.borrow_mut();
        let pct = if tau_s > 0.0 {
            bars.set_values(self.averages.iter().map(|avg| avg.value))
        } else {
            bars.set_values(self.logical_ids.iter().map(|id| newest(snap, *id)))
        };
        let ghz = bars.set_freqs(self.freq_ids.iter().map(|id| newest(snap, *id)));
        let mark = bars.set_clock_mark(snap.system.cpu.freq_is_bogomips);
        pct || ghz || mark
    }

    /// The value (utilization) and line (clock, temperature) when their inputs changed, or
    /// always with `force`. True when the text was formatted again.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> bool {
        let cpu = &snap.system.cpu;
        let mut h = FxHasher::default();
        h.write_u32(cpu.util_pct.value.to_bits());
        cpu.util_pct.validity.hash(&mut h);
        h.write_u32(cpu.freq_mhz.value);
        cpu.freq_mhz.validity.hash(&mut h);
        h.write_u32(cpu.temp_c.value.to_bits());
        cpu.temp_c.validity.hash(&mut h);
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        let unit = NetworkUnit::default();
        self.value = value_text(cpu.util_pct.validity, |out| {
            format_value(ValueFormat::Percent, cpu.util_pct.value, unit, out);
        });
        let shown = |v: Validity| matches!(v, Validity::Valid | Validity::Stale);
        let mut line = String::new();
        if shown(cpu.freq_mhz.validity) {
            format_value(ValueFormat::Mhz, cpu.freq_mhz.value as f32, unit, &mut line);
        }
        if shown(cpu.temp_c.validity) {
            if !line.is_empty() {
                line.push_str(SEP);
            }
            format_value(ValueFormat::Celsius, cpu.temp_c.value, unit, &mut line);
        }
        self.line = line.into();
        true
    }

    /// Applies the chart settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let smoothing = TlSettings::settings(cx).performance.overview_core_smoothing;
        if smoothing != self.smoothing {
            self.smoothing = smoothing;
            self.averages.fill(CoreAverage::default());
            let snap = box_snapshot(&self.hub, cx);
            if self.sync_bars(&snap) {
                self.notify_cores(cx);
            }
        }
        self.sync_grid(cx);
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
        self.rebuild_spec(cx);
    }

    /// A new theme or UI scale: new chart colours, and the frame draws in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_spec(cx);
        cx.notify();
    }

    /// A new language: new title, range word and core names.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        let labels = total_labels(tr(cx, KEY_RANGE), self.settings.kernel, cx);
        self.total.update(cx, |v, cx| v.set_labels(labels, cx));
        if let Some(chart) = &self.power_chart {
            let labels = power_labels(tr(cx, KEY_POWER));
            chart.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        if let Some(clocks) = &self.core_clocks {
            clocks.apply(cx);
        }
        let (values, freqs) = {
            let bars = self.bars.borrow();
            (bars.values.clone(), bars.freqs.clone())
        };
        self.sync_names(cx);
        let mut bars = self.bars.borrow_mut();
        bars.set_values(values.into_iter());
        bars.set_freqs(freqs.into_iter());
        drop(bars);
        cx.notify();
        self.notify_cores(cx);
    }

    fn rebuild_spec(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let spec = total_spec(self.cpu, &self.settings, &theme, self.hover_on);
        // The x ticks give the range, so the chart keeps no range caption, as `box_chart` set it
        // up.
        let labels = total_labels(SharedString::default(), self.settings.kernel, cx);
        self.total.update(cx, |v, cx| {
            v.set_spec(spec, cx);
            v.set_labels(labels, cx);
        });
        if let Some(chart) = &self.power_chart {
            let spec = power_spec(self.cpu, &self.settings, &theme, self.power_hover_on);
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        if let Some(clocks) = &mut self.core_clocks {
            clocks.set_base(clocks_base(&self.settings), cx);
        }
        for (chart, &device) in self.sparks.iter().zip(&self.logical) {
            let spec = core_spec(device, &self.settings, &theme);
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }
}

/// The widgets a flow render drew, in drawing order.
type Drawn = SmallVec<[u32; 4]>;

/// "No widgets. Use Edit Widgets to add some.", centred in the body (UI-PRF-061).
fn empty_text(m: &UiMetrics, theme: &TlTheme, cx: &App) -> AnyElement {
    let mut text = div();
    *text.text_style() = t_xs(m);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            text.tl_selector(|| EMPTY_ID.to_owned())
                .text_color(theme.text_secondary)
                .child(tr(cx, KEY_EMPTY)),
        )
        .into_any_element()
}

fn cached_full<V: Render>(view: Entity<V>) -> AnyElement {
    view.cached(StyleRefinement::default().size_full())
        .into_any_element()
}

impl CpuBox {
    /// `chart` filling its part, drawn by widget `id` as `FlowLook::Chart`; the chart itself
    /// when the layout has no such widget.
    fn chart_widget(
        &self,
        id: Option<u32>,
        chart: &Entity<ChartView>,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let widget = id.and_then(|id| {
            let look = FlowLook::Chart(chart.clone());
            let w = self
                .container
                .update(cx, |c, cx| c.flow_widget(id, look, cx))?;
            drawn.push(id);
            Some(w)
        });
        match widget {
            Some(w) => cached_full(w),
            None => cached_full(chart.clone()),
        }
    }

    /// The cores group drawing `look`, when the layout has it.
    fn cores_widget(
        &self,
        look: GroupFlowLook,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = cpu_flow::cores_of(&self.flow)?;
        let group = self
            .container
            .update(cx, |c, cx| c.flow_group(id, look, cx))?;
        drawn.push(id);
        Some(cached_full(group))
    }

    /// The total chart, filling its part.
    fn chart_part(&self, drawn: &mut Drawn, cx: &mut Context<Self>) -> AnyElement {
        let id = cpu_flow::total_of(&self.flow);
        let chart = self.chart_widget(id, &self.total, drawn, cx);
        div()
            .tl_selector(|| CPU_CHART_ID.into())
            .relative()
            .size_full()
            .child(chart)
            .into_any_element()
    }

    /// The total chart with the Core clocks chart under it when that is shown, on one time
    /// axis: the two take equal shares of the column.
    fn graph_part(&self, m: &UiMetrics, drawn: &mut Drawn, cx: &mut Context<Self>) -> AnyElement {
        let chart = self.chart_part(drawn, cx);
        let Some(view) = self
            .core_clocks
            .as_ref()
            .filter(|_| self.clocks_shown())
            .map(|c| c.view().clone())
        else {
            return chart;
        };
        let clocks = self.chart_widget(cpu_flow::clocks_of(&self.flow), &view, drawn, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[5])
            .child(div().relative().flex_1().min_h_0().w_full().child(chart))
            .child(
                div()
                    .tl_selector(|| CLOCKS_ID.into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(clocks),
            )
            .into_any_element()
    }

    /// The core rows with their sparklines, drawn by the cores group.
    fn rows_part(
        &mut self,
        m: &UiMetrics,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = GroupFlowLook::CoreRows {
            bars: Rc::clone(&self.bars),
            sparks: Rc::clone(&self.sparks),
            grid_size: Rc::clone(&self.grid_size),
            layout: Rc::clone(&self.grid),
        };
        if let Some(el) = self.cores_widget(look, drawn, cx) {
            return el;
        }
        let (rows, grid) = core_board(&self.bars, &self.sparks, &self.grid_size, m, cx.entity_id());
        self.grid.set(grid);
        rows
    }

    /// One filled chart per logical CPU, drawn by the cores group.
    fn core_charts_part(
        &self,
        m: &UiMetrics,
        theme: &TlTheme,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = GroupFlowLook::CoreCells {
            bars: Rc::clone(&self.bars),
            sparks: Rc::clone(&self.sparks),
            grid_size: Rc::clone(&self.grid_size),
            arrange: self.arrange,
        };
        self.cores_widget(look, drawn, cx).unwrap_or_else(|| {
            core_charts_element(
                &self.bars,
                &self.sparks,
                &self.grid_size,
                self.arrange,
                m,
                theme,
                cx.entity_id(),
            )
        })
    }

    /// The power draw strip, drawn by its widget.
    fn strip(
        &self,
        chart: &Entity<ChartView>,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.chart_widget(cpu_flow::power_of(&self.flow), chart, drawn, cx)
    }
}

/// The `core_charts` grid: one filled chart per logical CPU (`sparks`), each with "C{n}" and
/// the smoothed percent from `bars`. With `arrange` automatic the grid keeps cells at
/// 120 × 60 px at least and scrolls when the rows need more; otherwise one column or one row.
/// `grid_size` is the grid's size at the last paint, and `view` renders again when a resize
/// moves the grid into another class.
pub(crate) fn core_charts_element(
    bars: &Rc<RefCell<CoreBarsModel>>,
    sparks: &[Entity<ChartView>],
    grid_size: &Rc<Cell<Size<Pixels>>>,
    arrange: Arrange,
    m: &UiMetrics,
    theme: &TlTheme,
    view: EntityId,
) -> AnyElement {
    let n = sparks.len();
    let s = m.ui_scale;
    let gap = px(CORE_CHART_GAP * s);
    if arrange != Arrange::Auto {
        return core_charts_arranged(bars, sparks, grid_size, arrange, m, theme, view);
    }
    let (cols, rows) = core_chart_dims(n, grid_size.get().width, s);
    let bars = bars.borrow();
    let mut grid = div()
        .id(CORE_CHARTS_ID)
        .tl_selector(|| CORE_CHARTS_ID.into())
        .size_full()
        .flex()
        .flex_col()
        .gap(gap)
        .overflow_y_scroll();
    for r in 0..rows {
        let mut row = div()
            .flex()
            .flex_row()
            .gap(gap)
            .flex_1()
            .min_h(px(CORE_CHART_MIN.1 * s))
            .w_full();
        for c in 0..cols {
            let i = r * cols + c;
            let cell = div().relative().flex_1().min_w_0().h_full();
            let Some(chart) = sparks.get(i) else {
                row = row.child(cell);
                continue;
            };
            row = row.child(core_cell(cell, i, chart, &bars, m, theme));
        }
        grid = grid.child(row);
    }
    drop(bars);
    div()
        .relative()
        .size_full()
        .child(grid)
        .child(class_probe(
            Rc::clone(grid_size),
            (cols | (rows << 10)) as u32,
            move |size| {
                let (cols, rows) = core_chart_dims(n, size.width, s);
                (cols | (rows << 10)) as u32
            },
            view,
        ))
        .into_any_element()
}

/// The chart of core `i` as `cell`: the name at the top left, the smoothed percent at the top
/// right.
fn core_cell(
    cell: Div,
    i: usize,
    chart: &Entity<ChartView>,
    bars: &CoreBarsModel,
    m: &UiMetrics,
    theme: &TlTheme,
) -> Div {
    let s = m.ui_scale;
    let mut name = div();
    *name.text_style() = t_xs(m);
    let mut pct = div();
    *pct.text_style() = numeric_style(m, m.text.xs);
    cell.tl_selector(move || core_chart_id(i))
        .child(chart.clone().cached(StyleRefinement::default().size_full()))
        .child(
            name.absolute()
                .top(px(2. * s))
                .left(px(4. * s))
                .text_color(theme.text_secondary)
                .child(SharedString::from(bars.name(i).to_owned())),
        )
        .child(
            pct.absolute()
                .top(px(2. * s))
                .right(px(4. * s))
                .text_color(theme.text_primary)
                .child(SharedString::from(bars.pct_label(i).to_owned())),
        )
}

/// `core_charts` in one column or one row: every cell an equal share of the grid, at least
/// the minimum cell. Cells that do not fit scroll under a scrollbar.
fn core_charts_arranged(
    bars: &Rc<RefCell<CoreBarsModel>>,
    sparks: &[Entity<ChartView>],
    grid_size: &Rc<Cell<Size<Pixels>>>,
    arrange: Arrange,
    m: &UiMetrics,
    theme: &TlTheme,
    view: EntityId,
) -> AnyElement {
    let n = sparks.len();
    let s = m.ui_scale;
    let gap = px(CORE_CHART_GAP * s);
    let min = size(px(CORE_CHART_MIN.0 * s), px(CORE_CHART_MIN.1 * s));
    let bar = px(CORE_GRID_BAR_W * s);
    let fit = arrange_grid(arrange, n, grid_size.get(), min, gap, bar);
    let bars = bars.borrow();
    let cells = grid_cells(
        &fit,
        sparks.iter().enumerate().map(|(i, chart)| {
            let cell = div()
                .relative()
                .flex_none()
                .w(fit.cell.width)
                .h(fit.cell.height);
            core_cell(cell, i, chart, &bars, m, theme).into_any_element()
        }),
        gap,
        CORE_CHARTS_ID,
    );
    drop(bars);
    div()
        .relative()
        .size_full()
        .child(
            div()
                .tl_selector(|| CORE_CHARTS_ID.into())
                .size_full()
                .overflow_hidden()
                .child(cells),
        )
        .child(class_probe(
            Rc::clone(grid_size),
            fit.class(),
            move |size| arrange_grid(arrange, n, size, min, gap, bar).class(),
            view,
        ))
        .into_any_element()
}

impl CpuBox {
    /// `parts` over the power draw strip with a horizontal gap between them, the strip at the
    /// bottom and full width: of the body, or of the chart's column when it sits under the
    /// graph.
    fn with_power_strip(
        &mut self,
        parts: AnyElement,
        strip: AnyElement,
        m: &UiMetrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share = self
            .power_split
            .share(SplitAxis::Column, m.sp[5], m.ui_scale);
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[5])
            // The parts give way when the body is smaller than their share.
            .child(
                div()
                    .relative()
                    .flex_shrink(1.)
                    .min_h_0()
                    .h(relative(share))
                    .w_full()
                    .overflow_hidden()
                    .child(parts),
            )
            .child(
                div()
                    .tl_selector(|| POWER_STRIP_ID.into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(strip),
            )
            .child(self.power_split.probe(Self::power_inner_split, cx))
            .child(self.power_split.gap(
                SplitAxis::Column,
                share,
                m.sp[5],
                Self::power_inner_split,
                cx,
            ))
            .into_any_element()
    }

    /// The parts of the flow layout at their 1.2.0 places (UI-PRF-017, UI-PRF-019, D-040),
    /// each drawn by its widget. `drawn` takes the widgets drawn.
    fn flow_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *m;
        let body: AnyElement = match self.mode {
            CpuMode::Chart => self.graph_part(&m, drawn, cx),
            // The last meter keeps a gap above the "Now" label under the box.
            CpuMode::Cores => {
                let rows = self.rows_part(&m, drawn, cx);
                div().size_full().pb(m.sp[2]).child(rows).into_any_element()
            }
            CpuMode::CoreCharts => self.core_charts_part(&m, theme, drawn, cx),
            CpuMode::ChartCores => {
                let chart = self.graph_part(&m, drawn, cx);
                // Under the graph the strip shares the chart's column and the core rows keep
                // the body's full height (tasklens-o55.55).
                let chart = match self
                    .power_chart
                    .clone()
                    .filter(|_| self.power_under_graph())
                {
                    Some(power) => {
                        let strip = self.strip(&power, drawn, cx);
                        self.with_power_strip(chart, strip, &m, cx)
                    }
                    None => chart,
                };
                let rows = self.rows_part(&m, drawn, cx);
                // Side by side the core rows keep their 188 px column; stacked they keep the
                // usual part minimum.
                let rows_min = if self.side {
                    CORE_COLUMN_MIN_W
                } else {
                    PART_MIN
                };
                // Stacked, a chart part with the strip or the Core clocks chart under it keeps
                // room for each.
                let gap = f32::from(m.sp[5]) / m.ui_scale;
                let mut chart_min = PART_MIN;
                if !self.side && self.power_under_graph() {
                    chart_min += POWER_STRIP_MIN + gap;
                }
                if !self.side && self.clocks_shown() {
                    chart_min += PART_MIN + gap;
                }
                self.split.set_mins(chart_min, rows_min);
                let share = self.split(&m);
                let frame = div().flex().size_full().gap(core_gutter(&m));
                // The chart part gives way when the body is smaller than its share, so the rows
                // never push it past the bottom of the body.
                let (frame, chart_part, rows_part) = if self.side {
                    (
                        frame.flex_row(),
                        div().flex_shrink(1.).min_w_0().w(relative(share)).h_full(),
                        div().flex_1().min_w_0().h_full(),
                    )
                } else {
                    (
                        frame.flex_col(),
                        div().flex_shrink(1.).min_h_0().h(relative(share)).w_full(),
                        div().flex_1().min_h_0().w_full().pb(m.sp[2]),
                    )
                };
                div()
                    .relative()
                    .size_full()
                    .child(
                        frame
                            .child(chart_part.child(chart))
                            .child(rows_part.child(rows)),
                    )
                    .child(self.split.probe(Self::inner_split, cx))
                    .child(self.split.gap(
                        self.split_axis(),
                        share,
                        core_gutter(&m),
                        Self::inner_split,
                        cx,
                    ))
                    .into_any_element()
            }
        };
        let full_width = self.power_shown() && !self.power_under_graph();
        match self.power_chart.clone().filter(|_| full_width) {
            Some(chart) => {
                let strip = self.strip(&chart, drawn, cx);
                self.with_power_strip(body, strip, &m, cx)
            }
            None => body,
        }
    }

    /// The plan of a flow render (ADR-019 "Presets"): the parts the box drew and the widget of
    /// each, with the splits as the render laid them out.
    fn make_plan(&self, m: &UiMetrics) -> CpuPlan {
        let look = match self.mode {
            CpuMode::ChartCores => CpuLook::ChartCores,
            CpuMode::Chart => CpuLook::Chart,
            CpuMode::Cores => CpuLook::Cores,
            CpuMode::CoreCharts => CpuLook::CoreCharts,
        };
        let graph = matches!(self.mode, CpuMode::ChartCores | CpuMode::Chart);
        let flow = &self.flow;
        CpuPlan {
            look,
            m: *m,
            side: self.side,
            share: if self.mode == CpuMode::ChartCores {
                self.split(m)
            } else {
                0.
            },
            total: cpu_flow::total_of(flow).filter(|_| graph),
            cores: cpu_flow::cores_of(flow).filter(|_| self.mode != CpuMode::Chart),
            clocks: cpu_flow::clocks_of(flow)
                .filter(|_| self.clocks_shown() && self.core_clocks.is_some()),
            power: cpu_flow::power_of(flow)
                .filter(|_| self.power_shown() && self.power_chart.is_some())
                .map(|id| PlanPower {
                    id,
                    parts_share: self
                        .power_split
                        .share(SplitAxis::Column, m.sp[5], m.ui_scale),
                    under_graph: self.power_under_graph(),
                }),
        }
    }
}

impl Render for CpuBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.wide = is_wide(self.width.get(), m.ui_scale);
        self.side = match self.arrange {
            Arrange::Auto => self.wide,
            Arrange::Vertical => false,
            Arrange::Horizontal => true,
        };
        let editor = cached_full(self.editor.clone());
        let body = if !self.host_draws() {
            // The user's widgets, or Edit Widgets: the editor draws them.
            let empty = !self.editing
                && self
                    .saved_grid
                    .as_ref()
                    .is_some_and(|g| g.widgets.is_empty());
            div()
                .relative()
                .size_full()
                .child(editor)
                .when(empty, |d| d.child(empty_text(&m, &theme, cx)))
                .into_any_element()
        } else {
            let mut drawn = Drawn::new();
            let body = self.flow_body(&m, &theme, &mut drawn, cx);
            let plan = self.make_plan(&m);
            let ids_changed = self.plan.drawn() != drawn;
            self.plan.set_drawn(drawn);
            if self.plan.set(plan) || ids_changed {
                self.container.update(cx, |c, cx| {
                    c.place_again(cx);
                });
            }
            // The editor sits under the body with no widgets, so its container knows the body's
            // size when Edit Widgets starts.
            div()
                .relative()
                .size_full()
                .child(div().absolute().top_0().left_0().size_full().child(editor))
                .child(body)
                .into_any_element()
        };
        box_frame(
            CPU_BOX_ID,
            Some(DeviceKey(DeviceKey::CPU.into())),
            self.title.clone(),
            self.value.clone(),
            self.line.clone(),
            &self.range_text,
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .relative()
        .size_full()
        .child(width_probe(
            Rc::clone(&self.width),
            self.wide,
            px(SPLIT_W * m.ui_scale),
            cx.entity_id(),
        ))
    }
}
