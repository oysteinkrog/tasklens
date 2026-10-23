//! The Overview page's CPU box (UI-PRF-017, D-022, UI-PRF-023): the total chart and one row per
//! logical CPU (crw.53): name, sparkline, meter, percent and GHz.
//!
//! The core rows show a moving average of each core's `CPU_TOTAL` (UI-PRF-018, D-024), and the
//! core sparklines plot 1-second means. The total chart and the title value stay raw.
//!
//! The box is its own view. It formats its text only when the inputs' hash changes and notifies
//! only when the text, a core's 1% bucket or a core's 0.1 GHz bucket changed, so an idle tick
//! renders nothing. The total chart and the core sparklines are cached child views that re-render
//! for their own series. Pointer movement never notifies.

use std::cell::{Cell, Ref, RefCell};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Subscription, Window, div, px, relative, size,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::Sample;
use tl_model::settings::{CoreSmoothing, NetworkUnit};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::super::cpu::{PowerState, SecondaryAvail, logical_devices};
use super::inner_split::{InnerSplit, InnerSplitChanged, PART_MIN, SplitAxis};
use super::layout::{BoxMode, POWER_SPLIT_DEFAULT, POWER_SPLIT_RANGE};
use super::parts::{
    ChartSettings, CoreBars, CoreBarsLayout, CoreBarsModel, SPARK_GRID, SPLIT_W, box_chart,
    box_frame, chart_section_versions, class_probe, core_bars_layout, core_sparks, is_wide,
    spark_chart, split_body_share, width_probe,
};
use crate::chart::view::ChangedRuns;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::components::validity::copy_text;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
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

/// The debug selector of the power draw strip.
pub const POWER_STRIP_ID: &str = "perf-overview-cpu-power-strip";

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

/// The smallest cell of the `core_charts` grid, logical px before `ui_scale` (UI-PRF-019).
pub const CORE_CHART_MIN: (f32, f32) = (120., 60.);

/// The gap between `core_charts` cells, logical px before `ui_scale`.
const CORE_CHART_GAP: f32 = 8.;

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

/// The page saves the chart share the box ended a drag or a double-click on.
impl EventEmitter<InnerSplitChanged> for CpuBox {}

/// The box's debug selector.
pub const CPU_BOX_ID: &str = "perf-overview-box-cpu";

const KEY_TITLE: &str = "perf-card-heading-cpu";
const KEY_CORE: &str = "perf-overview-core";
const KEY_RANGE: &str = "perf-cpu-heading-utilization";
const KEY_POWER: &str = "perf-cpu-secondary-power";

/// The span one core sparkline point averages (UI-PRF-018).
const SPARK_MEAN_NS: u64 = 1_000_000_000;

/// One core's exponential moving average of `CPU_TOTAL`.
#[derive(Clone, Copy, Debug, Default)]
struct CoreAverage {
    /// Time of the newest sample taken in.
    t_ns: u64,
    /// The average; `None` before the first valid sample and after an invalid one.
    value: Option<f32>,
}

impl CoreAverage {
    /// Takes in the samples newer than `t_ns`: `s += (x − s) · (1 − e^(−Δt/τ))`. An invalid
    /// sample clears the average, and the next valid sample starts it again.
    fn advance(&mut self, samples: &[Sample], tau_s: f32) {
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
fn total_spec(cpu: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
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
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// A core sparkline: `CPU_TOTAL` of logical CPU `device`, filled, 0..100.
fn core_spec(device: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
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
fn power_spec(cpu: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
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
fn total_labels(range_word: SharedString) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Hidden,
        range_word,
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The newest valid sample of `id`.
pub(super) fn newest(snap: &Snapshot, id: SeriesId) -> Option<f32> {
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
    hub: Entity<SnapshotHub>,
    cpu: DeviceIdx,
    total: Entity<ChartView>,
    bars: Rc<RefCell<CoreBarsModel>>,
    /// One sparkline per entry of `logical`.
    sparks: Vec<Entity<ChartView>>,
    /// Logical CPU id to device; `DeviceIdx(u16::MAX)` for an id with no catalog entry.
    logical: Vec<DeviceIdx>,
    /// `CPU_TOTAL` of each entry of `logical`.
    logical_ids: Vec<SeriesId>,
    /// `CPU_FREQ` of each entry of `logical`.
    freq_ids: Vec<SeriesId>,
    /// `performance.overview_core_smoothing`, and the average of each entry of `logical`.
    smoothing: CoreSmoothing,
    averages: Vec<CoreAverage>,
    /// The core grid's size at the last paint, and the layout the last render used.
    grid_size: Rc<Cell<Size<Pixels>>>,
    grid: CoreBarsLayout,
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
    /// The gap between the chart and the core rows (UI-PRF-018 "CPU box splitter").
    split: InnerSplit,
    /// Whether the power draw strip is switched on, and whether RAPL can be read.
    power: bool,
    power_state: PowerState,
    /// The strip's chart, built the first time the strip is switched on.
    power_chart: Option<Entity<ChartView>>,
    /// The gap above the strip. Its share is the parts' share of the body.
    power_split: InnerSplit<PowerSplitChanged>,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    _subs: [Subscription; 4],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl CpuBox {
    pub fn new(hub: &Entity<SnapshotHub>, cpu: DeviceIdx, cx: &mut Context<Self>) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let spec = total_spec(cpu, &settings, &theme);
        let labels = total_labels(tr(cx, KEY_RANGE));
        let total = box_chart(spec, labels, hub, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let grid_size = size(px(0.), px(0.));
        let mut this = Self {
            hub: hub.clone(),
            cpu,
            total,
            bars: Rc::default(),
            sparks: Vec::new(),
            logical: Vec::new(),
            logical_ids: Vec::new(),
            freq_ids: Vec::new(),
            smoothing: TlSettings::settings(cx).performance.overview_core_smoothing,
            averages: Vec::new(),
            grid_size: Rc::new(Cell::new(grid_size)),
            grid: core_bars_layout(0, grid_size.width, grid_size.height, 1.),
            catalog_version: u64::MAX,
            title: tr(cx, KEY_TITLE),
            value: SharedString::default(),
            line: SharedString::default(),
            text_bits: 0,
            width: Rc::new(Cell::new(px(0.))),
            wide: true,
            mode: CpuMode::ChartCores,
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
            settings,
            settings_versions: chart_section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<TlSettings>(Self::sync_settings),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_devices(&snap, cx);
        this.sync_text(&snap, true);
        this.sync_power_state(&snap);
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
        self.grid
    }

    /// The title, value and line text.
    pub fn text(&self) -> (&SharedString, &SharedString, &SharedString) {
        (&self.title, &self.value, &self.line)
    }

    /// Whether the last render put the bars beside the chart (box at least 560 px wide).
    pub fn side_by_side(&self) -> bool {
        self.wide
    }

    /// The chart's share of the body as the box lays it out, after the clamps.
    pub fn split(&self, m: &UiMetrics) -> f32 {
        self.split.share(SplitAxis::Row, m.sp[5], m.ui_scale)
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

    /// Takes the saved strip: shown or not, and its share of the body (`power_split`). Nothing
    /// changes while the gap is held.
    pub fn set_power(&mut self, on: bool, strip_share: f32, cx: &mut Context<Self>) {
        let mut changed = self.power_split.set_share(1. - strip_share);
        if on != self.power {
            self.power = on;
            changed = true;
        }
        if on && self.power_chart.is_none() {
            let theme = current(cx).clone();
            let spec = power_spec(self.cpu, &self.settings, &theme);
            let labels = power_labels(tr(cx, KEY_POWER));
            self.power_chart = Some(box_chart(spec, labels, &self.hub, cx));
        }
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

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let devices = ev.mask.contains(ComponentMask::DEVICES);
        let series = ev.mask.contains(ComponentMask::SERIES);
        let system = ev.mask.contains(ComponentMask::SYSTEM);
        let details = ev.mask.contains(ComponentMask::DEVICE_DETAILS);
        if !(devices || series || system || details) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        let mut changed = false;
        if devices {
            changed |= self.sync_devices(&snap, cx);
        }
        if series && !changed {
            let runs = ChangedRuns::new(&ev.changed_series);
            if self
                .logical_ids
                .iter()
                .chain(&self.freq_ids)
                .any(|id| runs.contains(*id))
            {
                changed |= self.sync_bars(&snap);
            }
        }
        if system {
            changed |= self.sync_text(&snap, false);
        }
        if system || details {
            changed |= self.sync_power_state(&snap);
        }
        if changed {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
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
        self.sparks.truncate(self.logical.len());
        for (i, &device) in self.logical.iter().enumerate() {
            let spec = core_spec(device, &self.settings, &theme);
            if let Some(chart) = self.sparks.get(i) {
                if old.get(i) != Some(&device) {
                    chart.update(cx, |v, cx| v.set_spec(spec, cx));
                }
            } else {
                let chart = spark_chart(spec, ValueFormat::Percent, &self.hub, cx);
                self.sparks.push(chart);
            }
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
    /// `CPU_FREQ` of each logical CPU to the rows. True when a row's 1% or 0.1 GHz bucket
    /// changed.
    fn sync_bars(&mut self, snap: &Snapshot) -> bool {
        let tau_s = self.smoothing.tau_s();
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
        pct || ghz
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
            let snap = Arc::clone(self.hub.read(cx).current());
            if self.sync_bars(&snap) {
                #[cfg(any(test, feature = "test-hooks"))]
                self.notify_count.set(self.notify_count.get() + 1);
                cx.notify();
            }
        }
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
        let labels = total_labels(tr(cx, KEY_RANGE));
        self.total.update(cx, |v, cx| v.set_labels(labels, cx));
        if let Some(chart) = &self.power_chart {
            let labels = power_labels(tr(cx, KEY_POWER));
            chart.update(cx, |v, cx| v.set_labels(labels, cx));
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
    }

    fn rebuild_spec(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let spec = total_spec(self.cpu, &self.settings, &theme);
        self.total.update(cx, |v, cx| v.set_spec(spec, cx));
        if let Some(chart) = &self.power_chart {
            let spec = power_spec(self.cpu, &self.settings, &theme);
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        for (chart, &device) in self.sparks.iter().zip(&self.logical) {
            let spec = core_spec(device, &self.settings, &theme);
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }
}

impl CpuBox {
    /// The total chart, filling its part.
    fn chart_part(&self) -> AnyElement {
        div()
            .relative()
            .size_full()
            .child(
                self.total
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }

    /// The core rows with their sparklines.
    fn rows_part(&mut self, m: &UiMetrics, cx: &mut Context<Self>) -> AnyElement {
        let n = self.logical.len();
        let s = m.ui_scale;
        let grid_size = self.grid_size.get();
        self.grid = core_bars_layout(n, grid_size.width, grid_size.height, s);
        div()
            .relative()
            .size_full()
            .child(CoreBars {
                model: Rc::clone(&self.bars),
            })
            .child(core_sparks(&self.sparks, &self.grid, m))
            .child(class_probe(
                Rc::clone(&self.grid_size),
                self.grid.class(),
                move |size| core_bars_layout(n, size.width, size.height, s).class(),
                cx.entity_id(),
            ))
            .into_any_element()
    }

    /// One filled chart per logical CPU in a grid, each with "C{n}" and the smoothed percent.
    /// The grid keeps cells at 120 × 60 px at least and scrolls when the rows need more.
    fn core_charts_part(
        &self,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let n = self.logical.len();
        let s = m.ui_scale;
        let gap = px(CORE_CHART_GAP * s);
        let (cols, rows) = core_chart_dims(n, self.grid_size.get().width, s);
        let bars = self.bars.borrow();
        let mut grid = div()
            .id(CORE_CHARTS_ID)
            .debug_selector(|| CORE_CHARTS_ID.into())
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
                let Some(chart) = self.sparks.get(i).filter(|_| i < n) else {
                    row = row.child(cell);
                    continue;
                };
                let mut name = div();
                *name.text_style() = t_xs(m);
                let mut pct = div();
                *pct.text_style() = numeric_style(m, m.text.xs);
                let cell = cell
                    .debug_selector(move || core_chart_id(i))
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
                    );
                row = row.child(cell);
            }
            grid = grid.child(row);
        }
        drop(bars);
        div()
            .relative()
            .size_full()
            .child(grid)
            .child(class_probe(
                Rc::clone(&self.grid_size),
                (cols | (rows << 10)) as u32,
                move |size| {
                    let (cols, rows) = core_chart_dims(n, size.width, s);
                    (cols | (rows << 10)) as u32
                },
                cx.entity_id(),
            ))
            .into_any_element()
    }
}

impl CpuBox {
    /// `parts` over the power draw strip with a horizontal gap between them, the strip at the
    /// bottom of the body and full width.
    fn with_power_strip(
        &mut self,
        parts: AnyElement,
        chart: Entity<ChartView>,
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
            .child(
                div()
                    .relative()
                    .flex_none()
                    .h(relative(share))
                    .w_full()
                    .overflow_hidden()
                    .child(parts),
            )
            .child(
                div()
                    .debug_selector(|| POWER_STRIP_ID.into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(chart.cached(StyleRefinement::default().size_full())),
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
}

impl Render for CpuBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.wide = is_wide(self.width.get(), m.ui_scale);
        let body: AnyElement = match self.mode {
            CpuMode::Chart => self.chart_part(),
            CpuMode::Cores => self.rows_part(&m, cx),
            CpuMode::CoreCharts => self.core_charts_part(&m, &theme, cx),
            CpuMode::ChartCores => {
                let chart = self.chart_part();
                let rows = self.rows_part(&m, cx);
                let share = self.split(&m);
                let split = split_body_share(chart, rows, self.wide, share, &m);
                let mut body = div()
                    .relative()
                    .size_full()
                    .child(split)
                    .child(self.split.probe(Self::inner_split, cx));
                if self.wide {
                    body = body.child(self.split.gap(
                        SplitAxis::Row,
                        share,
                        m.sp[5],
                        Self::inner_split,
                        cx,
                    ));
                }
                body.into_any_element()
            }
        };
        let body = match self.power_chart.clone().filter(|_| self.power_shown()) {
            Some(chart) => self.with_power_strip(body, chart, &m, cx),
            None => body,
        };
        box_frame(
            CPU_BOX_ID,
            Some(DeviceKey(DeviceKey::CPU.into())),
            self.title.clone(),
            self.value.clone(),
            self.line.clone(),
            body,
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
