//! The Overview page's Memory box (UI-PRF-017, D-022, UI-PRF-026, crw.54): five rows that share the
//! body height (In use, Cached, Available, Swap, Commit). A row is its label, value, meter,
//! percent and a sparkline.
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
    AnyElement, Context, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, SharedString, StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use rustc_hash::FxHasher;
use smallvec::smallvec;
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::Snapshot;
use tl_model::system::MemoryTotals;
use tl_model::units;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::cpu_box::value_text;
use super::parts::{
    ChartSettings, Meter, SPARK_GRID, SPARK_W, box_frame, chart_section_versions, class_probe,
    spark_chart,
};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{ChartBg, ChartSeries, ChartSpec, ChartView, ValueFormat, YScale, format_value};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_sm};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const MEM_BOX_ID: &str = "perf-overview-box-memory";

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

/// The narrowest text column of a narrow row, logical px before `ui_scale`: a 72 px meter (as wide
/// as the one-line label column), the gap and the percent. The sparkline takes the rest, at most
/// `SPARK_W`.
const ROW_NARROW_TEXT_W: f32 = ROW_LABEL_W + ROW_PCT_W + 8.;

/// Whether a body `width` wide (0 before the first paint) has room for the one-line row: label,
/// value, a meter of at least `ROW_MIN_METER_W`, percent and the sparkline, with their 4 gaps.
/// Narrower bodies get the two-line row (`memory_row`).
fn row_wide(width: Pixels, m: &UiMetrics) -> bool {
    let fixed = (ROW_LABEL_W + ROW_VALUE_W + ROW_MIN_METER_W + ROW_PCT_W + SPARK_W) * m.ui_scale;
    width == px(0.) || f32::from(width) >= fixed + 4. * f32::from(m.sp[4])
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
    hub: Entity<SnapshotHub>,
    memory: DeviceIdx,
    /// One sparkline per row.
    sparks: [Entity<ChartView>; ROWS],
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
    _subs: [Subscription; 4],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl MemBox {
    pub fn new(hub: &Entity<SnapshotHub>, memory: DeviceIdx, cx: &mut Context<Self>) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let totals = Totals::of(&snap.system.memory);
        let colors = row_colors(&theme);
        let sparks = std::array::from_fn(|i| {
            let spec = spark_spec(memory, i, &totals, &colors, &settings, &theme);
            spark_chart(spec, ValueFormat::Bytes, hub, cx)
        });
        let rows = std::array::from_fn(|i| MeterRow {
            label: tr(cx, KEY_ROWS[i]),
            value: SharedString::default(),
            fraction: None,
            color: colors[i],
        });
        let mut this = Self {
            hub: hub.clone(),
            memory,
            sparks,
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

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        if !ev
            .mask
            .intersects(ComponentMask::SYSTEM | ComponentMask::DEVICES)
        {
            return;
        }
        let snap: Arc<Snapshot> = Arc::clone(hub.read(cx).current());
        if ev.mask.contains(ComponentMask::DEVICES)
            && let Some(entry) = snap.devices.find(&DeviceIdentity::Memory)
            && entry.idx != self.memory
        {
            self.memory = entry.idx;
            self.rebuild_specs(cx);
        }
        if ev.mask.contains(ComponentMask::SYSTEM) && self.sync_text(&snap.system.memory, false, cx)
        {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
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
        // "No swap" is translated too.
        let snap = Arc::clone(self.hub.read(cx).current());
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
    }
}

/// One meter row: label, meter, value.
pub(super) fn meter_row(row: &MeterRow, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let s = m.ui_scale;
    let mut label = div()
        .flex_none()
        .w(px(ROW_LABEL_W * s))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(row.label.clone());
    *label.text_style() = t_sm(m);
    let mut value = div()
        .flex_none()
        .w(px(ROW_VALUE_W * s))
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(row.value.clone());
    *value.text_style() = numeric_style(m, m.text.sm);
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

/// One memory row: label, value, meter, percent and a sparkline that fills the row height.
/// A `wide` row puts them on one line. A narrow row has two lines of text, the label and value
/// over the meter and percent, with the sparkline beside them.
fn memory_row(
    row: &MeterRow,
    percent: &SharedString,
    chart: &Entity<ChartView>,
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
        .flex_none()
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(row.value.clone());
    *value.text_style() = numeric_style(m, m.text.sm);
    let mut pct = div()
        .flex_none()
        .w(px(ROW_PCT_W * s))
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(percent.clone());
    *pct.text_style() = numeric_style(m, m.text.sm);
    let meter = div()
        .debug_selector(move || meter_id(index))
        .flex_1()
        .min_w_0()
        .child(Meter {
            fraction: row.fraction,
            color: row.color,
        });
    let spark = div()
        .debug_selector(move || spark_id(index))
        .relative()
        .h_full()
        .child(chart.clone().cached(StyleRefinement::default().size_full()));
    let line = || div().flex().flex_row().items_center().gap(m.sp[4]);
    let row_el = div()
        .debug_selector(move || row_id(index))
        .flex()
        .flex_row()
        .flex_1()
        .items_center()
        .gap(m.sp[4]);
    if !wide {
        let text = div()
            .flex()
            .flex_col()
            .justify_center()
            .flex_1()
            .min_w(px(ROW_NARROW_TEXT_W * s))
            .child(
                line()
                    .h(px(LABEL_ROW * s))
                    .child(label.flex_1().min_w_0())
                    .child(value),
            )
            .child(line().h(px(LABEL_ROW * s)).child(meter).child(pct));
        return row_el
            .min_h(px(2. * LABEL_ROW * s))
            .child(text)
            .child(spark.flex_1().min_w_0().max_w(px(SPARK_W * s)))
            .into_any_element();
    }
    row_el
        .min_h(px(LABEL_ROW * s))
        .child(label.flex_none().w(px(ROW_LABEL_W * s)))
        .child(value.w(px(ROW_VALUE_W * s)))
        .child(meter)
        .child(pct)
        .child(spark.flex_none().w(px(SPARK_W * s)))
        .into_any_element()
}

impl Render for MemBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let probe_m = m;
        let wide = row_wide(self.body.get().width, &m);
        let body = div()
            .debug_selector(|| MEM_BODY_ID.into())
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[2])
            .children(
                self.rows
                    .iter()
                    .zip(&self.percents)
                    .zip(&self.sparks)
                    .enumerate()
                    .map(|(i, ((r, p), c))| memory_row(r, p, c, i, wide, &m, &theme)),
            )
            .child(class_probe(
                Rc::clone(&self.body),
                u32::from(wide),
                move |b| u32::from(row_wide(b.width, &probe_m)),
                cx.entity_id(),
            ))
            .into_any_element();
        box_frame(
            MEM_BOX_ID,
            Some(DeviceKey(DeviceKey::MEMORY.into())),
            self.title.clone(),
            self.value.clone(),
            SharedString::default(),
            body,
            &m,
            &theme,
        )
        .relative()
        .size_full()
    }
}
