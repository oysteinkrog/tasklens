//! The Overview page's GPU box (UI-PRF-017, D-022, UI-PRF-029): one section per shown GPU, at most
//! two, each a utilization chart, one row per engine (label, meter, percent and a sparkline), a
//! VRAM meter row and a temperature and power row.
//!
//! The page hands in the GPUs of the sidebar's visible cards. The box formats its text only when
//! the inputs' hash changes and notifies only then. Engine rows and their sparkline charts are
//! built only when the shown engine list changes (crw.47). The second section shows only when
//! both fit the box height with charts of at least 48 px (TMOG); a resize re-renders the box only
//! when that changes. Pointer movement never notifies.

use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Size, StyleRefinement, Styled as _, Subscription, Window, div, px, size,
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
use super::mem_box::MeterRow;
use super::parts::{
    ChartSettings, MIN_CHART_H, SPARK_GRID, box_chart, box_frame, chart_section_versions,
    class_probe, spark_chart, spark_row,
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

/// GPUs with a section; the rest are counted in the "+n more" line.
pub const MAX_GPUS: usize = 2;

const KEY_VRAM: &str = "perf-overview-gpu-vram";
/// The one row of a shared encode/decode engine.
const KEY_ENC_DEC: &str = "perf-overview-gpu-enc-dec";
const KEY_MORE: &str = "perf-overview-more";
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

/// The body rows under a section's chart: the engines, VRAM and, when known, temperature and
/// power.
fn body_rows(s: &GpuSection) -> usize {
    s.engines.len() + 1 + usize::from(!s.line.is_empty())
}

/// How many sections fit a body of `body` (0 before the first paint shows them all). `rows` has
/// each section's body rows. The first section is a chart and its rows (its heading is the box
/// title row); later ones add a heading row. The "+n more" row counts when GPUs are left over.
fn sections_fit(body: Size<Pixels>, rows: &[usize], hidden: usize, m: &UiMetrics) -> usize {
    let count = rows.len();
    if body.height == px(0.) || count < 2 {
        return count;
    }
    let (row, gap, section_gap) = spacing(m);
    let chart = MIN_CHART_H * m.ui_scale;
    let section = |n: usize| chart + n as f32 * (gap + row);
    let first = section(rows[0]);
    let next = row + gap + section(rows[1]);
    let more = if hidden > 0 { section_gap + row } else { 0. };
    if f32::from(body.height) >= first + section_gap + next + more {
        2
    } else {
        1
    }
}

/// The GPU box.
pub struct GpuBox {
    hub: Entity<SnapshotHub>,
    sections: SmallVec<[GpuSection; 2]>,
    /// GPUs past `MAX_GPUS`.
    more: usize,
    /// "+n more" for `more`, and for `more + 1` when the second section does not fit.
    more_text: SharedString,
    more_text_fit: SharedString,
    text_bits: u64,
    /// `performance.hidden_gpu_engines`.
    hidden: Vec<String>,
    /// The body size of the last paint, and how many sections the last render showed.
    body: Rc<Cell<Size<Pixels>>>,
    shown: usize,
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

impl Render for GpuBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = m.ui_scale;
        let row_h = px(LABEL_ROW * s);
        let count = self.sections.len();
        let more = self.more;
        let rows: SmallVec<[usize; MAX_GPUS]> = self.sections.iter().map(body_rows).collect();
        self.shown = sections_fit(self.body.get(), &rows, more, &m);

        let mut body = div().relative().flex().flex_col().size_full().gap(m.sp[5]);
        for (i, section) in self.sections.iter().take(self.shown).enumerate() {
            let mut col = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(MIN_CHART_H * s))
                .w_full()
                .gap(m.sp[2]);
            if i > 0 {
                let mut heading = div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(theme.text_primary)
                    .child(section.heading.clone());
                *heading.text_style() = label_style(&m, m.text.sm, FontWeight::MEDIUM);
                let mut value = div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(theme.text_primary)
                    .child(section.value.clone());
                *value.text_style() = numeric_style(&m, m.text.sm);
                col = col.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_none()
                        .items_center()
                        .justify_between()
                        .h(row_h)
                        .child(heading)
                        .child(value),
                );
            }
            col = col
                .child(
                    div()
                        .flex_1()
                        .min_h(px(MIN_CHART_H * s))
                        .w_full()
                        .child(chart_element(section)),
                )
                .children(
                    section
                        .engines
                        .iter()
                        .map(|e| spark_row(&e.meter, Some(&e.chart), &m, &theme)),
                )
                .child(spark_row(&section.vram, None, &m, &theme));
            if !section.line.is_empty() {
                let mut line = div()
                    .flex_none()
                    .h(row_h)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(theme.text_secondary)
                    .child(section.line.clone());
                *line.text_style() = t_xs(&m);
                col = col.child(line);
            }
            body = body.child(col);
        }
        let more_text = if self.shown < count {
            Some(self.more_text_fit.clone())
        } else {
            (more > 0).then(|| self.more_text.clone())
        };
        if let Some(text) = more_text {
            let mut line = div()
                .flex_none()
                .h(row_h)
                .text_color(theme.text_secondary)
                .child(text);
            *line.text_style() = t_xs(&m);
            body = body.child(line);
        }
        let shown = self.shown as u32;
        let probe_m = m;
        body = body.child(class_probe(
            Rc::clone(&self.body),
            shown,
            move |b| sections_fit(b, &rows, more, &probe_m) as u32,
            cx.entity_id(),
        ));

        let first = self.sections.first();
        let pick = |f: fn(&GpuSection) -> &SharedString| first.map(f).cloned().unwrap_or_default();
        box_frame(
            GPU_BOX_ID,
            first.map(|s| s.key.clone()),
            pick(|s| &s.heading),
            pick(|s| &s.value),
            SharedString::default(),
            body.into_any_element(),
            &m,
            &theme,
        )
        .size_full()
    }
}
