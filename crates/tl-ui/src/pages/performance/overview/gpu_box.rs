//! The Overview page's GPU box (UI-PRF-017, D-022, UI-PRF-029): one section per shown GPU, at most
//! two, each a utilization chart, one row per engine (label, meter, percent and a sparkline), a
//! VRAM meter row and a temperature and power row, then a "+n more" row for the GPUs left over.
//!
//! The page hands in the GPUs of the sidebar's visible cards. The box formats its text only when
//! the inputs' hash changes and notifies only then. Engine rows and their sparkline charts are
//! built only when the shown engine list changes (crw.47). Each section is as high as its content
//! with a chart of at least 48 px (TMOG), and the spare height goes to the charts. The second
//! section shows only when both fit. When one section's rows do not fit, it shows only the
//! busiest engine and a "+n more engines" row. Narrow rows drop the meter and keep the sparkline
//! (`meter_fits`). A resize re-renders the box only when one of those changes. Pointer movement
//! never notifies.
//!
//! UI-PRF-019 gives the box three modes. `chart_engines` is the layout above, with an inner
//! splitter under the first section's chart. `engine_charts` shows one chart per shown engine in
//! a grid of cells at least 160 × 70 px, then the VRAM row. `chart` is the utilization chart and
//! the VRAM row only.

use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement,
    Styled as _, Subscription, Window, div, px, size,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::Snapshot;
use tl_model::system::{GpuTotals, Metric};
use tl_model::units;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::super::gpu::{GpuVendor, gpu_totals, shown_engines};
use super::cpu_box::value_text;
use super::inner_split::{InnerSplit, InnerSplitChanged, SplitAxis};
use super::layout::BoxMode;
use super::mem_box::MeterRow;
use super::parts::{
    ChartSettings, MIN_CHART_H, SPARK_GRID, box_chart, box_frame, chart_section_versions,
    class_probe, meter_fits, spark_chart, spark_row,
};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::TlSettings;
use crate::text::style::{label_style, numeric_style, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const GPU_BOX_ID: &str = "perf-overview-box-gpu";

/// The box's layout modes (UI-PRF-019). `chart_engines` is the sections of UI-PRF-017.
///
/// The `chart_engines` split is the first section's chart share of the body. Its default is a
/// nominal 0.5: while the share is that value, the chart takes the spare height as before.
pub const MODES: &[BoxMode] = &[
    BoxMode {
        id: "chart_engines",
        label: "perf-overview-mode-chart-engines",
        split: Some(0.5),
    },
    BoxMode {
        id: "engine_charts",
        label: "perf-overview-mode-engine-charts",
        split: None,
    },
    BoxMode {
        id: "chart",
        label: "perf-overview-mode-chart",
        split: None,
    },
];

/// The debug selector of the splitter under the first chart in `chart_engines`.
pub const GPU_SPLITTER_ID: &str = "perf-overview-gpu-splitter";
/// The debug selector of its accent line.
pub const GPU_SPLITTER_LINE_ID: &str = "perf-overview-gpu-splitter-line";

/// The smallest engine chart cell of `engine_charts`, logical px.
pub const CELL_MIN_W: f32 = 160.;
pub const CELL_MIN_H: f32 = 70.;

/// Which parts the box shows (UI-PRF-019).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GpuMode {
    ChartEngines,
    EngineCharts,
    Chart,
}

impl GpuMode {
    /// The mode `id`; an unknown id is the default.
    fn from_id(id: &str) -> Self {
        match id {
            "engine_charts" => Self::EngineCharts,
            "chart" => Self::Chart,
            _ => Self::ChartEngines,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::ChartEngines => MODES[0].id,
            Self::EngineCharts => MODES[1].id,
            Self::Chart => MODES[2].id,
        }
    }
}

/// The page saves the share the box ended a drag or a double-click on.
impl EventEmitter<InnerSplitChanged> for GpuBox {}

/// GPUs with a section; the rest are counted in the "+n more" line.
pub const MAX_GPUS: usize = 2;

const KEY_VRAM: &str = "perf-overview-gpu-vram";
/// The one row of a shared encode/decode engine.
const KEY_ENC_DEC: &str = "perf-overview-gpu-enc-dec";
const KEY_MORE: &str = "perf-overview-more";
const KEY_MORE_ENGINES: &str = "perf-overview-gpu-more-engines";
const KEY_RANGE: &str = "perf-gpu-row-utilization";

/// The joint between the temperature and the power.
const SEP: &str = " · ";

/// One GPU's section.
pub struct GpuSection {
    pub key: DeviceKey,
    pub idx: DeviceIdx,
    pub heading: SharedString,
    pub chart: Entity<ChartView>,
    /// Utilization.
    pub value: SharedString,
    /// One row per shown engine, in the vendor's order.
    pub engines: SmallVec<[EngineRow; 4]>,
    /// "+n more engines" for every engine but the busiest, shown when the rows do not fit.
    pub engines_more: SharedString,
    pub vram: MeterRow,
    /// Temperature and power, each only when known.
    pub line: SharedString,
}

/// One engine row: label, meter and percent in `meter`, and a sparkline of the engine's series.
pub struct EngineRow {
    /// The engine's `performance.hidden_gpu_engines` key ("enc" for a shared encode/decode row).
    pub key: &'static str,
    /// The label's `en-US.ftl` key.
    name: &'static str,
    pub metric: MetricId,
    pub meter: MeterRow,
    pub chart: Entity<ChartView>,
}

/// An engine row as the snapshot and the settings ask for it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct EngineWant {
    key: &'static str,
    name: &'static str,
    metric: MetricId,
}

/// The engine rows of GPU `idx`: every reported engine that is not hidden, in the vendor's order.
/// A shared encode/decode engine (`enc_dec_shared`) is one row at the place of the first of the
/// two; it is hidden only when both "enc" and "dec" are.
fn engine_wants(snap: &Snapshot, idx: DeviceIdx, hidden: &[String]) -> SmallVec<[EngineWant; 4]> {
    let totals = gpu_totals(snap, idx);
    let shared = totals.is_some_and(|t| t.enc_dec_shared);
    let mut merged = false;
    let mut out = SmallVec::new();
    for e in shown_engines(GpuVendor::of(snap, idx), totals, hidden) {
        let want = if shared && matches!(e.key, "enc" | "dec") {
            if merged {
                continue;
            }
            merged = true;
            EngineWant {
                key: "enc",
                name: KEY_ENC_DEC,
                metric: MetricId::GPU_ENC,
            }
        } else {
            EngineWant {
                key: e.key,
                name: e.name,
                metric: e.metric,
            }
        };
        out.push(want);
    }
    out
}

/// The current value of an engine's series in `t`.
fn engine_metric(t: &GpuTotals, metric: MetricId) -> Metric<f32> {
    if metric == MetricId::GPU_UTIL {
        t.util_pct
    } else if metric == MetricId::GPU_ENC {
        t.enc_pct
    } else if metric == MetricId::GPU_DEC {
        t.dec_pct
    } else {
        Metric::with(Validity::Unsupported)
    }
}

/// An engine row's sparkline: filled, 0..100, over the box's time range.
fn engine_spec(gpu: DeviceIdx, metric: MetricId, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: SeriesId {
                device: gpu,
                metric,
            },
            fill: Some(theme.graph.gpu_fill),
            line: theme.graph.gpu_line,
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

/// The utilization chart: filled, 0..100.
fn chart_spec(gpu: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: SeriesId {
                device: gpu,
                metric: MetricId::GPU_UTIL,
            },
            fill: Some(theme.graph.gpu_fill),
            line: theme.graph.gpu_line,
            ..ChartSeries::default()
        }],
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

/// No heading and no value: the section rows carry the numbers.
fn chart_labels(range_word: SharedString) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Hidden,
        range_word,
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

fn hash_metric<T: Copy + Default>(h: &mut FxHasher, m: &Metric<T>, bits: impl Fn(T) -> u64) {
    h.write_u64(bits(m.value));
    m.validity.hash(h);
}

fn shown(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

/// The vertical space of the parts of a section, in px for `scale`: (row, gap, section gap).
fn spacing(m: &UiMetrics) -> (f32, f32, f32) {
    (
        LABEL_ROW * m.ui_scale,
        f32::from(m.sp[2]),
        f32::from(m.sp[5]),
    )
}

/// A section's body rows under its chart: the engine rows, and the rest (VRAM and, when known,
/// the temperature and power row).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct SectionRows {
    engines: usize,
    rest: usize,
}

/// `chart` shows the chart and the VRAM row only.
fn section_rows(s: &GpuSection, mode: GpuMode) -> SectionRows {
    if mode == GpuMode::Chart {
        return SectionRows {
            engines: 0,
            rest: 1,
        };
    }
    SectionRows {
        engines: s.engines.len(),
        rest: 1 + usize::from(!s.line.is_empty()),
    }
}

/// What `engine_charts` shows: how many sections and grid columns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ChartsFit {
    sections: usize,
    cols: usize,
}

impl ChartsFit {
    fn class(self) -> u32 {
        self.sections as u32 | (self.cols as u32) << 2
    }
}

/// The grid columns of a body `width` wide: as many cells of at least `CELL_MIN_W` as fit.
fn grid_cols(width: Pixels, m: &UiMetrics) -> usize {
    let (_, gap, _) = spacing(m);
    let w = f32::from(width);
    if w <= 0. {
        return 1;
    }
    (((w + gap) / (CELL_MIN_W * m.ui_scale + gap)).floor() as usize).max(1)
}

/// The height of an `engine_charts` section: a grid of `engines` cells in `cols` columns, then
/// the VRAM row, `lines` more rows, and a heading row when `heading`.
fn charts_section_h(
    engines: usize,
    cols: usize,
    lines: usize,
    heading: bool,
    m: &UiMetrics,
) -> f32 {
    let (row, gap, _) = spacing(m);
    let grid_rows = engines.div_ceil(cols.max(1));
    let head = if heading { row + gap } else { 0. };
    let grid = if grid_rows == 0 {
        0.
    } else {
        grid_rows as f32 * (CELL_MIN_H * m.ui_scale + gap)
    };
    head + grid + (1 + lines) as f32 * (gap + row) - if grid_rows == 0 { gap } else { 0. }
}

/// What `engine_charts` fits in `body`: both sections when both fit in full, else the first with
/// "+n more" for the others.
fn charts_fit(body: Size<Pixels>, engines: &[usize], total: usize, m: &UiMetrics) -> ChartsFit {
    let cols = grid_cols(body.width, m);
    let count = engines.len();
    if body.height == px(0.) || count == 0 {
        return ChartsFit {
            sections: count,
            cols,
        };
    }
    let (_, _, section_gap) = spacing(m);
    let more = |shown: usize| usize::from(total > shown);
    if count >= 2 {
        let two = charts_section_h(engines[0], cols, 0, false, m)
            + section_gap
            + charts_section_h(engines[1], cols, more(2), true, m);
        if f32::from(body.height) >= two {
            return ChartsFit { sections: 2, cols };
        }
    }
    ChartsFit { sections: 1, cols }
}

/// What a body shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fit {
    /// How many sections.
    sections: usize,
    /// The first section shows only its busiest engine and a "+n more engines" row.
    collapsed: bool,
    /// The engine rows draw their meters (see `meter_fits`).
    meters: bool,
}

impl Fit {
    /// The layout class for `class_probe`.
    fn class(self) -> u32 {
        self.sections as u32 | u32::from(self.collapsed) << 2 | u32::from(self.meters) << 3
    }
}

/// The engine rows a section draws: all of them, or the busiest and "+n more engines" when
/// `collapsed` and there are more than two.
fn engine_lines(engines: usize, collapsed: bool) -> usize {
    if collapsed { engines.min(2) } else { engines }
}

/// The content height of a section, in px: its heading row (`heading`), a `MIN_CHART_H` chart
/// and `lines` body rows.
fn section_h(lines: usize, heading: bool, m: &UiMetrics) -> f32 {
    let (row, gap, _) = spacing(m);
    let head = if heading { row + gap } else { 0. };
    head + MIN_CHART_H * m.ui_scale + lines as f32 * (gap + row)
}

/// What fits a body of `body` (everything before the first paint). `rows` has each section's
/// body rows and `total` counts every GPU; those without a section get a "+n more" row under the
/// last section. Both sections show only when both fit in full. Otherwise one section shows, and
/// when its rows do not fit, only its busiest engine and "+n more engines".
fn fit(body: Size<Pixels>, rows: &[SectionRows], total: usize, m: &UiMetrics) -> Fit {
    let meters = meter_fits(body.width, m);
    let count = rows.len();
    if body.height == px(0.) || count == 0 {
        return Fit {
            sections: count,
            collapsed: false,
            meters,
        };
    }
    let (_, _, section_gap) = spacing(m);
    let more = |shown: usize| usize::from(total > shown);
    let h = f32::from(body.height);
    let lines = |r: SectionRows, collapsed: bool| engine_lines(r.engines, collapsed) + r.rest;
    if count >= 2 {
        let two = section_h(lines(rows[0], false), false, m)
            + section_gap
            + section_h(lines(rows[1], false) + more(2), true, m);
        if h >= two {
            return Fit {
                sections: 2,
                collapsed: false,
                meters,
            };
        }
    }
    let full = section_h(lines(rows[0], false) + more(1), false, m);
    Fit {
        sections: 1,
        collapsed: h < full && rows[0].engines > 2,
        meters,
    }
}

/// The debug selector of section `i`'s body rows.
pub fn rows_id(i: usize) -> String {
    format!("{GPU_BOX_ID}-rows-{i}")
}

/// The debug selector of section `i`'s engine row `key`.
pub fn engine_row_id(i: usize, key: &str) -> String {
    format!("{GPU_BOX_ID}-engine-{i}-{key}")
}

/// The debug selector of section `i`'s `engine_charts` cell `key`.
pub fn engine_cell_id(i: usize, key: &str) -> String {
    format!("{GPU_BOX_ID}-cell-{i}-{key}")
}

/// The debug selector of section `i`'s utilization chart.
pub fn chart_id(i: usize) -> String {
    format!("{GPU_BOX_ID}-chart-{i}")
}

/// The GPU box.
pub struct GpuBox {
    hub: Entity<SnapshotHub>,
    sections: SmallVec<[GpuSection; 2]>,
    mode: GpuMode,
    /// The gap under the first chart (`chart_engines`).
    chart_split: InnerSplit,
    /// GPUs past `MAX_GPUS`.
    more: usize,
    /// "+n more" for `more`, and for `more + 1` when the second section does not fit.
    more_text: SharedString,
    more_text_fit: SharedString,
    text_bits: u64,
    /// `performance.hidden_gpu_engines`.
    hidden: Vec<String>,
    /// The body size of the last paint and what the last render showed.
    body: Rc<Cell<Size<Pixels>>>,
    shown: usize,
    collapsed: bool,
    meters: bool,
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

impl GpuBox {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        gpus: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            hub: hub.clone(),
            sections: SmallVec::new(),
            mode: GpuMode::ChartEngines,
            chart_split: InnerSplit::new(
                GPU_SPLITTER_ID,
                GPU_SPLITTER_LINE_ID,
                MODES[0].split.unwrap_or_default(),
            )
            .with_mins(MIN_CHART_H, 2. * LABEL_ROW)
            .with_default_second_px(0.),
            more: 0,
            more_text: SharedString::default(),
            more_text_fit: SharedString::default(),
            text_bits: 0,
            hidden: TlSettings::settings(cx)
                .performance
                .hidden_gpu_engines
                .clone(),
            body: Rc::new(Cell::new(size(px(0.), px(0.)))),
            shown: 0,
            collapsed: false,
            meters: true,
            settings: ChartSettings::read(cx),
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
        this.build_sections(gpus, cx);
        this
    }

    /// New GPUs from the page. Sections whose key is unchanged keep their chart.
    pub fn set_gpus(
        &mut self,
        gpus: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) {
        let same = gpus.len() == self.sections.len() + self.more
            && gpus
                .iter()
                .zip(&self.sections)
                .all(|((k, i, h), s)| *k == s.key && *i == s.idx && *h == s.heading);
        if same {
            return;
        }
        self.build_sections(gpus, cx);
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// The shown mode's id.
    pub fn mode(&self) -> &'static str {
        self.mode.id()
    }

    /// Shows mode `id`; an unknown id is the default mode. Only this box renders again.
    pub fn set_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let mode = GpuMode::from_id(id);
        if mode != self.mode {
            self.mode = mode;
            cx.notify();
        }
    }

    /// The first chart's share of the body as laid out in `chart_engines`, after the clamps; 0 in
    /// the other modes.
    pub fn split(&self, m: &UiMetrics) -> f32 {
        match self.mode {
            GpuMode::ChartEngines => self
                .chart_split
                .share(SplitAxis::Column, m.sp[5], m.ui_scale),
            _ => 0.,
        }
    }

    /// Takes the saved share for `chart_engines`. Nothing changes while the gap is held.
    pub fn set_split(&mut self, share: f32, cx: &mut Context<Self>) {
        if self.mode == GpuMode::ChartEngines && self.chart_split.set_share(share) {
            cx.notify();
        }
    }

    fn chart_inner_split(&mut self) -> &mut InnerSplit {
        &mut self.chart_split
    }

    pub fn sections(&self) -> &[GpuSection] {
        &self.sections
    }

    pub fn more(&self) -> usize {
        self.more
    }

    /// The "+n more" text for `more()`, empty when there is none.
    pub fn more_text(&self) -> &SharedString {
        &self.more_text
    }

    /// How many sections the last render showed.
    pub fn shown(&self) -> usize {
        self.shown
    }

    /// Whether the last render showed only the first section's busiest engine and "+n more
    /// engines".
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    /// Whether the last render drew the engine rows' meters.
    pub fn meters(&self) -> bool {
        self.meters
    }

    /// The "+n more" row of the last render, empty when every GPU is shown.
    pub fn shown_more_text(&self) -> SharedString {
        if self.shown < self.sections.len() {
            self.more_text_fit.clone()
        } else {
            self.more_text.clone()
        }
    }

    /// "GPU 0\t12%", then one line per engine, the VRAM and the line, per section; then
    /// "+n more".
    pub fn copy_text(&self, out: &mut String) {
        for s in &self.sections {
            out.push_str(&s.heading);
            out.push('\t');
            out.push_str(&s.value);
            out.push('\n');
            for e in &s.engines {
                out.push_str(&e.meter.label);
                out.push('\t');
                out.push_str(&e.meter.value);
                out.push('\n');
            }
            out.push_str(&s.vram.label);
            out.push('\t');
            out.push_str(&s.vram.value);
            out.push('\n');
            if !s.line.is_empty() {
                out.push_str(&s.line);
                out.push('\n');
            }
        }
        if self.more > 0 {
            out.push_str(&self.more_text);
            out.push('\n');
        }
    }

    fn build_sections(
        &mut self,
        gpus: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) {
        let theme = current(cx).clone();
        let mut old = std::mem::take(&mut self.sections);
        for (key, idx, heading) in gpus.iter().take(MAX_GPUS) {
            let section = match old.iter().position(|s| s.key == *key) {
                Some(i) => {
                    let mut s = old.remove(i);
                    if s.idx != *idx {
                        s.idx = *idx;
                        let spec = chart_spec(*idx, &self.settings, &theme);
                        s.chart.update(cx, |v, cx| v.set_spec(spec, cx));
                        for e in &s.engines {
                            let spec = engine_spec(*idx, e.metric, &self.settings, &theme);
                            e.chart.update(cx, |v, cx| v.set_spec(spec, cx));
                        }
                    }
                    s.heading = heading.clone();
                    s
                }
                None => {
                    let spec = chart_spec(*idx, &self.settings, &theme);
                    let labels = chart_labels(tr(cx, KEY_RANGE));
                    let hub = self.hub.clone();
                    GpuSection {
                        key: key.clone(),
                        idx: *idx,
                        heading: heading.clone(),
                        chart: box_chart(spec, labels, &hub, cx),
                        value: SharedString::default(),
                        engines: SmallVec::new(),
                        engines_more: SharedString::default(),
                        vram: MeterRow {
                            label: tr(cx, KEY_VRAM),
                            value: SharedString::default(),
                            fraction: None,
                            color: theme.graph.gpu_line,
                        },
                        line: SharedString::default(),
                    }
                }
            };
            self.sections.push(section);
        }
        self.more = gpus.len().saturating_sub(MAX_GPUS);
        self.sync_more_text(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_engines(&snap, cx);
        self.sync_text(&snap, true);
    }

    /// Rebuilds the engine rows of every section whose shown engine list changed. A row whose
    /// key is unchanged keeps its chart; text changes never build a chart. True when a list
    /// changed.
    fn sync_engines(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let theme = current(cx).clone();
        let mut changed = false;
        for i in 0..self.sections.len() {
            let idx = self.sections[i].idx;
            let wants = engine_wants(snap, idx, &self.hidden);
            let same = {
                let rows = &self.sections[i].engines;
                rows.len() == wants.len()
                    && rows
                        .iter()
                        .zip(&wants)
                        .all(|(r, w)| r.key == w.key && r.name == w.name && r.metric == w.metric)
            };
            if same {
                continue;
            }
            changed = true;
            let mut old = std::mem::take(&mut self.sections[i].engines);
            let mut rows = SmallVec::new();
            for w in wants {
                let row = match old.iter().position(|r| r.key == w.key) {
                    Some(j) => {
                        let mut r = old.remove(j);
                        if r.name != w.name {
                            r.name = w.name;
                            r.meter.label = tr(cx, w.name);
                        }
                        if r.metric != w.metric {
                            r.metric = w.metric;
                            let spec = engine_spec(idx, w.metric, &self.settings, &theme);
                            r.chart.update(cx, |v, cx| v.set_spec(spec, cx));
                        }
                        r
                    }
                    None => {
                        let spec = engine_spec(idx, w.metric, &self.settings, &theme);
                        EngineRow {
                            key: w.key,
                            name: w.name,
                            metric: w.metric,
                            meter: MeterRow {
                                label: tr(cx, w.name),
                                value: SharedString::default(),
                                fraction: None,
                                color: theme.graph.gpu_line,
                            },
                            chart: spark_chart(spec, ValueFormat::Percent, &self.hub, cx),
                        }
                    }
                };
                rows.push(row);
            }
            self.sections[i].engines_more = more_engines(rows.len(), cx);
            self.sections[i].engines = rows;
        }
        changed
    }

    fn sync_more_text(&mut self, cx: &mut Context<Self>) {
        let more = |n: usize| tr_args(cx, KEY_MORE, &[("n", FluentValue::from(n))]);
        self.more_text = if self.more > 0 {
            more(self.more)
        } else {
            SharedString::default()
        };
        self.more_text_fit = more(self.more + 1);
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        if !ev
            .mask
            .intersects(ComponentMask::SYSTEM | ComponentMask::DEVICES | ComponentMask::INVENTORY)
        {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        let rows = self.sync_engines(&snap, cx);
        if self.sync_text(&snap, rows) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// Every section's value, VRAM row and line when their inputs changed, or always with
    /// `force`. True when the text was formatted again.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> bool {
        let totals = |idx: DeviceIdx| {
            snap.system
                .gpus
                .iter()
                .find(|(i, _)| *i == idx)
                .map(|(_, t)| t)
        };
        let mut h = FxHasher::default();
        for s in &self.sections {
            s.idx.0.hash(&mut h);
            let Some(t) = totals(s.idx) else {
                h.write_u8(0);
                continue;
            };
            hash_metric(&mut h, &t.util_pct, |v| u64::from(v.to_bits()));
            hash_metric(&mut h, &t.enc_pct, |v| u64::from(v.to_bits()));
            hash_metric(&mut h, &t.dec_pct, |v| u64::from(v.to_bits()));
            t.enc_dec_shared.hash(&mut h);
            hash_metric(&mut h, &t.mem_used, |v| v);
            hash_metric(&mut h, &t.mem_total, |v| v);
            hash_metric(&mut h, &t.temp_c, |v| u64::from(v.to_bits()));
            hash_metric(&mut h, &t.power_w, |v| u64::from(v.to_bits()));
        }
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        for s in &mut self.sections {
            format_section(s, totals(s.idx));
        }
        true
    }

    /// Applies the chart settings and the hidden engines when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = chart_section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let hidden = &TlSettings::settings(cx).performance.hidden_gpu_engines;
        if *hidden != self.hidden {
            self.hidden = hidden.clone();
            let snap = Arc::clone(self.hub.read(cx).current());
            if self.sync_engines(&snap, cx) {
                self.sync_text(&snap, true);
                #[cfg(any(test, feature = "test-hooks"))]
                self.notify_count.set(self.notify_count.get() + 1);
                cx.notify();
            }
        }
        let new = ChartSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_specs(cx);
    }

    /// A new theme or UI scale: new chart and meter colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        let line = current(cx).graph.gpu_line;
        for s in &mut self.sections {
            s.vram.color = line;
            for e in &mut s.engines {
                e.meter.color = line;
            }
        }
        self.rebuild_specs(cx);
        cx.notify();
    }

    /// A new language: new labels and "+n more".
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        let (vram, range) = (tr(cx, KEY_VRAM), tr(cx, KEY_RANGE));
        for s in &mut self.sections {
            s.vram.label = vram.clone();
            for e in &mut s.engines {
                e.meter.label = tr(cx, e.name);
            }
            s.engines_more = more_engines(s.engines.len(), cx);
            let labels = chart_labels(range.clone());
            s.chart.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        self.sync_more_text(cx);
        cx.notify();
    }

    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        for s in &self.sections {
            let spec = chart_spec(s.idx, &self.settings, &theme);
            s.chart.update(cx, |v, cx| v.set_spec(spec, cx));
            for e in &s.engines {
                let spec = engine_spec(s.idx, e.metric, &self.settings, &theme);
                e.chart.update(cx, |v, cx| v.set_spec(spec, cx));
            }
        }
    }
}

/// "+n more engines" for a section with `engines` engine rows: all but the busiest.
fn more_engines(engines: usize, cx: &App) -> SharedString {
    let n = engines.saturating_sub(1);
    tr_args(cx, KEY_MORE_ENGINES, &[("n", FluentValue::from(n))])
}

/// One section's value ("12%"), engine percents and fills, VRAM ("3.20 GiB / 16.00 GiB" and its
/// fill) and line ("62 °C · 120 W"). A GPU with no totals in the snapshot shows the Missing glyph.
fn format_section(s: &mut GpuSection, t: Option<&GpuTotals>) {
    let Some(t) = t else {
        let missing = value_text(Validity::Missing, |_| {});
        for e in &mut s.engines {
            e.meter.value = missing.clone();
            e.meter.fraction = None;
        }
        s.value = missing.clone();
        s.vram.value = missing;
        s.vram.fraction = None;
        s.line = SharedString::default();
        return;
    };
    let unit = NetworkUnit::default();
    s.value = value_text(t.util_pct.validity, |out| {
        format_value(ValueFormat::Percent, t.util_pct.value, unit, out);
    });
    for e in &mut s.engines {
        let v = engine_metric(t, e.metric);
        e.meter.value = value_text(v.validity, |out| {
            format_value(ValueFormat::Percent, v.value, unit, out);
        });
        e.meter.fraction = shown(v.validity).then(|| v.value / 100.);
    }
    let vram_known = shown(t.mem_used.validity) && shown(t.mem_total.validity);
    s.vram.value = if vram_known {
        let mut out = String::new();
        units::bytes_of(&mut out, t.mem_used.value, t.mem_total.value);
        out.into()
    } else {
        let v = if shown(t.mem_used.validity) {
            t.mem_total.validity
        } else {
            t.mem_used.validity
        };
        value_text(v, |_| {})
    };
    s.vram.fraction = (vram_known && t.mem_total.value > 0)
        .then(|| (t.mem_used.value as f64 / t.mem_total.value as f64) as f32);
    let mut line = String::new();
    if shown(t.temp_c.validity) {
        format_value(ValueFormat::Celsius, t.temp_c.value, unit, &mut line);
    }
    if shown(t.power_w.validity) {
        if !line.is_empty() {
            line.push_str(SEP);
        }
        format_value(ValueFormat::Watts, t.power_w.value, unit, &mut line);
    }
    s.line = line.into();
}

/// The section's chart.
fn chart_element(s: &GpuSection) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(
            s.chart
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
        .into_any_element()
}

/// A secondary text row: "+n more engines" or "+n more".
fn text_row(text: &SharedString, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let mut line = div()
        .flex_none()
        .h(px(LABEL_ROW * m.ui_scale))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(text.clone());
    *line.text_style() = t_xs(m);
    line.into_any_element()
}

/// The index of the engine with the highest current value; the first on a tie or with none.
fn busiest(engines: &[EngineRow]) -> usize {
    let mut best = 0;
    for (i, e) in engines.iter().enumerate() {
        let v = e.meter.fraction.unwrap_or(-1.);
        if v > engines[best].meter.fraction.unwrap_or(-1.) {
            best = i;
        }
    }
    best
}

/// A later section's heading row: the GPU name and its utilization.
fn section_heading(
    section: &GpuSection,
    row_h: Pixels,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let mut heading = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(section.heading.clone());
    *heading.text_style() = label_style(m, m.text.sm, FontWeight::MEDIUM);
    let mut value = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(section.value.clone());
    *value.text_style() = numeric_style(m, m.text.sm);
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .h(row_h)
        .child(heading)
        .child(value)
        .into_any_element()
}

/// One `engine_charts` cell: the engine name and percent over its chart.
fn engine_cell(i: usize, e: &EngineRow, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let key = e.key;
    let mut name = div()
        .flex_1()
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(e.meter.label.clone());
    *name.text_style() = t_xs(m);
    let mut value = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(e.meter.value.clone());
    *value.text_style() = numeric_style(m, m.text.sm);
    div()
        .debug_selector(move || engine_cell_id(i, key))
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h(px(CELL_MIN_H * m.ui_scale))
        .gap(m.sp[2])
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[2])
                .h(px(LABEL_ROW * m.ui_scale))
                .child(name)
                .child(value),
        )
        .child(
            div().relative().flex_1().min_h_0().child(
                e.chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
        )
        .into_any_element()
}

impl GpuBox {
    /// `chart_engines` and `chart`: the sections of UI-PRF-017, or only their charts and VRAM rows.
    fn list_body(&mut self, m: &UiMetrics, theme: &TlTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = m.ui_scale;
        let row_h = px(LABEL_ROW * s);
        let mode = self.mode;
        let rows: SmallVec<[SectionRows; MAX_GPUS]> = self
            .sections
            .iter()
            .map(|sec| section_rows(sec, mode))
            .collect();
        let total = self.sections.len() + self.more;
        let body_size = self.body.get();
        let shown = fit(body_size, &rows, total, m);
        self.shown = shown.sections;
        self.collapsed = shown.collapsed;
        self.meters = shown.meters;
        let more = self.shown_more_text();
        let with_rows = mode == GpuMode::ChartEngines;

        // The first chart's height under the splitter. The untouched default keeps the flex
        // layout: the split then reads what that layout gives the chart.
        let (_, gap, section_gap) = spacing(m);
        let len = f32::from(body_size.height);
        let lines: SmallVec<[usize; MAX_GPUS]> = (0..shown.sections)
            .map(|i| {
                engine_lines(rows[i].engines, shown.collapsed && i == 0)
                    + rows[i].rest
                    + usize::from(i + 1 == shown.sections && !more.is_empty())
            })
            .collect();
        let mut split_share = None;
        let mut custom_chart_h = None;
        if with_rows && shown.sections > 0 {
            let n = shown.sections as f32;
            let basis: f32 = lines
                .iter()
                .enumerate()
                .map(|(i, l)| section_h(*l, i > 0, m))
                .sum();
            let spare = (len - basis - (n - 1.) * section_gap).max(0.);
            let first_h = section_h(lines[0], false, m) + spare / n;
            let chart_h =
                (first_h - lines[0] as f32 * (gap + f32::from(row_h))).max(MIN_CHART_H * s);
            let second = (len - f32::from(m.sp[5]) - chart_h).max(0.) / s;
            self.chart_split.set_default_second_px(second);
            let share = self.chart_split.share(SplitAxis::Column, m.sp[5], s);
            split_share = Some(share);
            if self.chart_split.saved() != self.chart_split.default_share() && len > 0. {
                custom_chart_h = Some(share * len);
            }
        }

        let mut body = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .gap(m.sp[5]);
        for (i, section) in self.sections.iter().take(shown.sections).enumerate() {
            let collapsed = shown.collapsed && i == 0;
            let last = i + 1 == shown.sections;
            let fixed = custom_chart_h.filter(|_| i == 0);
            let mut col = div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .w_full()
                .gap(m.sp[2]);
            col = match fixed {
                Some(h) => col
                    .flex_grow(0.)
                    .flex_basis(px(h + lines[i] as f32 * (gap + f32::from(row_h)))),
                None => col
                    .flex_grow(1.)
                    .flex_basis(px(section_h(lines[i], i > 0, m))),
            };
            if i > 0 {
                col = col.child(section_heading(section, row_h, m, theme));
            }
            let engine = |e: &EngineRow| {
                let key = e.key;
                div()
                    .debug_selector(move || engine_row_id(i, key))
                    .flex_none()
                    .w_full()
                    .child(spark_row(&e.meter, Some(&e.chart), shown.meters, m, theme))
            };
            let mut section_rows = div()
                .debug_selector(move || rows_id(i))
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .gap(m.sp[2]);
            if with_rows {
                if collapsed && section.engines.len() > 2 {
                    section_rows = section_rows
                        .child(engine(&section.engines[busiest(&section.engines)]))
                        .child(text_row(&section.engines_more, m, theme));
                } else {
                    section_rows = section_rows.children(section.engines.iter().map(engine));
                }
            }
            section_rows =
                section_rows.child(spark_row(&section.vram, None, shown.meters, m, theme));
            if with_rows && !section.line.is_empty() {
                section_rows = section_rows.child(text_row(&section.line, m, theme));
            }
            if last && !more.is_empty() {
                section_rows = section_rows.child(text_row(&more, m, theme));
            }
            let chart = div()
                .debug_selector(move || chart_id(i))
                .min_h(px(MIN_CHART_H * s))
                .w_full()
                .child(chart_element(section));
            let chart = match fixed {
                Some(h) => chart.flex_none().h(px(h)),
                None => chart.flex_1(),
            };
            body = body.child(col.child(chart).child(section_rows));
        }
        let probe_m = *m;
        body = body.child(class_probe(
            Rc::clone(&self.body),
            shown.class(),
            move |b| fit(b, &rows, total, &probe_m).class(),
            cx.entity_id(),
        ));
        if let Some(share) = split_share {
            body = body
                .child(self.chart_split.probe(Self::chart_inner_split, cx))
                .child(self.chart_split.gap(
                    SplitAxis::Column,
                    share,
                    m.sp[5],
                    Self::chart_inner_split,
                    cx,
                ));
        }
        body.into_any_element()
    }

    /// `engine_charts`: per section a grid of engine charts, then the VRAM row.
    fn charts_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = m.ui_scale;
        let row_h = px(LABEL_ROW * s);
        let engines: SmallVec<[usize; MAX_GPUS]> =
            self.sections.iter().map(|sec| sec.engines.len()).collect();
        let total = self.sections.len() + self.more;
        let shown = charts_fit(self.body.get(), &engines, total, m);
        self.shown = shown.sections;
        self.collapsed = false;
        self.meters = meter_fits(self.body.get().width, m);
        let more = self.shown_more_text();
        let cols = shown.cols;

        let mut body = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .gap(m.sp[5]);
        for (i, section) in self.sections.iter().take(shown.sections).enumerate() {
            let last = i + 1 == shown.sections;
            let extra = usize::from(last && !more.is_empty());
            let mut col = div()
                .flex()
                .flex_col()
                .flex_grow(1.)
                .flex_shrink_0()
                .flex_basis(px(charts_section_h(engines[i], cols, extra, i > 0, m)))
                .w_full()
                .gap(m.sp[2]);
            if i > 0 {
                col = col.child(section_heading(section, row_h, m, theme));
            }
            let mut grid = div()
                .debug_selector(move || rows_id(i))
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(engines[i].div_ceil(cols) as f32 * CELL_MIN_H * s))
                .w_full()
                .gap(m.sp[2]);
            for chunk in section.engines.chunks(cols) {
                let mut line = div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(CELL_MIN_H * s))
                    .gap(m.sp[2]);
                for e in chunk {
                    line = line.child(engine_cell(i, e, m, theme));
                }
                for _ in chunk.len()..cols {
                    line = line.child(div().flex_1().min_w_0());
                }
                grid = grid.child(line);
            }
            let mut rest = div()
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .gap(m.sp[2])
                .child(spark_row(&section.vram, None, self.meters, m, theme));
            if last && !more.is_empty() {
                rest = rest.child(text_row(&more, m, theme));
            }
            body = body.child(col.child(grid).child(rest));
        }
        let probe_m = *m;
        body = body.child(class_probe(
            Rc::clone(&self.body),
            shown.class(),
            move |b| charts_fit(b, &engines, total, &probe_m).class(),
            cx.entity_id(),
        ));
        body.into_any_element()
    }
}

impl Render for GpuBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let body = match self.mode {
            GpuMode::EngineCharts => self.charts_body(&m, &theme, cx),
            GpuMode::ChartEngines | GpuMode::Chart => self.list_body(&m, &theme, cx),
        };
        let first = self.sections.first();
        let pick = |f: fn(&GpuSection) -> &SharedString| first.map(f).cloned().unwrap_or_default();
        box_frame(
            GPU_BOX_ID,
            first.map(|s| s.key.clone()),
            pick(|s| &s.heading),
            pick(|s| &s.value),
            SharedString::default(),
            body,
            &m,
            &theme,
        )
        .size_full()
    }
}
