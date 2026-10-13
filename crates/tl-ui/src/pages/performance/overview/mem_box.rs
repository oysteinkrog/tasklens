//! The Overview page's Memory box (UI-PRF-017, D-022, UI-PRF-026): the used-memory chart and four
//! meters (Used, Available, Cached, Swap).
//!
//! The box is its own view and reads `SYSTEM` only, so a tick with only series changes does not
//! touch it. It formats its text only when the memory totals' hash changes. The chart spec is
//! rebuilt only when the total changes. Pointer movement never notifies.

use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, Context, Entity, Hsla, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Window, div, px,
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
use super::parts::{
    ChartSettings, Meter, SPLIT_W, box_chart, box_frame, chart_section_versions, is_wide,
    split_body_fixed_side, width_probe,
};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_sm};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const MEM_BOX_ID: &str = "perf-overview-box-memory";

const KEY_TITLE: &str = "perf-card-heading-memory";
const KEY_RANGE: &str = "perf-mem-range-memory";
const KEY_ROWS: [&str; 4] = [
    "perf-overview-mem-used",
    "perf-overview-mem-available",
    "perf-overview-mem-cached",
    "perf-overview-mem-swap",
];
const KEY_NO_SWAP: &str = "perf-overview-mem-no-swap";

/// The meter label column, logical px before `ui_scale`.
const ROW_LABEL_W: f32 = 72.;
/// The meter value column, logical px before `ui_scale`.
const ROW_VALUE_W: f32 = 80.;

/// One meter row: its label, its value text, its fill and the fill colour.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterRow {
    pub label: SharedString,
    pub value: SharedString,
    pub fraction: Option<f32>,
    pub color: Hsla,
}

/// The chart: used memory with fill, 0..total.
fn chart_spec(memory: DeviceIdx, total: u64, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
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
        y: YScale::FixedTotal(total as f32),
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

/// No heading and no value: the box title row carries the numbers.
fn chart_labels(range_word: SharedString) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Hidden,
        range_word,
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// `part / whole` as a fraction, `None` when `whole` is 0.
fn fraction(part: u64, whole: u64) -> Option<f32> {
    (whole > 0).then(|| (part as f64 / whole as f64) as f32)
}

/// The colours of the four rows.
fn row_colors(theme: &TlTheme) -> [Hsla; 4] {
    let line = theme.graph.mem_line;
    let with = |a: f32| Hsla {
        a: line.a * a,
        ..line
    };
    [line, with(0.6), with(0.35), theme.graph.kernel_line]
}

/// The Memory box.
pub struct MemBox {
    hub: Entity<SnapshotHub>,
    memory: DeviceIdx,
    chart: Entity<ChartView>,
    rows: [MeterRow; 4],
    /// The total the chart spec was built for.
    total: u64,
    title: SharedString,
    value: SharedString,
    /// Hash of the inputs `value` and `rows` were formatted from.
    text_bits: u64,
    /// The box width of the last paint, and whether the last render put the meters beside the
    /// chart.
    width: Rc<Cell<Pixels>>,
    wide: bool,
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
        let total = snap.system.memory.total;
        let spec = chart_spec(memory, total, &settings, &theme);
        let labels = chart_labels(tr(cx, KEY_RANGE));
        let chart = box_chart(spec, labels, hub, cx);
        let colors = row_colors(&theme);
        let rows = std::array::from_fn(|i| MeterRow {
            label: tr(cx, KEY_ROWS[i]),
            value: SharedString::default(),
            fraction: None,
            color: colors[i],
        });
        let mut this = Self {
            hub: hub.clone(),
            memory,
            chart,
            rows,
            total,
            title: tr(cx, KEY_TITLE),
            value: SharedString::default(),
            text_bits: 0,
            width: Rc::new(Cell::new(px(0.))),
            wide: true,
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

    pub fn rows(&self) -> &[MeterRow; 4] {
        &self.rows
    }

    pub fn chart(&self) -> &Entity<ChartView> {
        &self.chart
    }

    /// The title and value text.
    pub fn text(&self) -> (&SharedString, &SharedString) {
        (&self.title, &self.value)
    }

    /// "Memory\t50.0%", then "Used\t32.00 GiB" per row.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\t');
        out.push_str(&self.value);
        out.push('\n');
        for row in &self.rows {
            out.push_str(&row.label);
            out.push('\t');
            out.push_str(&row.value);
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
            self.rebuild_spec(cx);
        }
        if ev.mask.contains(ComponentMask::SYSTEM) && self.sync_text(&snap.system.memory, false, cx)
        {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The value and the rows when the totals changed, or always with `force`; a new chart spec
    /// when the total changed. True when the text was formatted again.
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
        ] {
            h.write_u64(v);
        }
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        if mem.total != self.total {
            self.total = mem.total;
            self.rebuild_spec(cx);
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
            (mem.available, mem.total),
            (mem.cached, mem.total),
            (swap_used, mem.swap_total),
        ];
        for (i, (part, whole)) in parts.into_iter().enumerate() {
            let row = &mut self.rows[i];
            if valid && i == 3 && whole == 0 {
                row.value = tr(cx, KEY_NO_SWAP);
                row.fraction = None;
                continue;
            }
            row.value = value_text(mem.validity, |out| units::bytes(out, part));
            row.fraction = if valid { fraction(part, whole) } else { None };
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
        self.rebuild_spec(cx);
    }

    /// A new theme or UI scale: new chart and meter colours.
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
        self.rebuild_spec(cx);
        cx.notify();
    }

    /// A new language: new title, labels and range word.
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
        let labels = chart_labels(tr(cx, KEY_RANGE));
        self.chart.update(cx, |v, cx| v.set_labels(labels, cx));
        // "No swap" is translated too.
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_text(&snap.system.memory, true, cx);
        cx.notify();
    }

    fn rebuild_spec(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let spec = chart_spec(self.memory, self.total, &self.settings, &theme);
        self.chart.update(cx, |v, cx| v.set_spec(spec, cx));
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

impl Render for MemBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.wide = is_wide(self.width.get(), m.ui_scale);
        let chart: AnyElement = div()
            .relative()
            .size_full()
            .child(
                self.chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element();
        let meters = div()
            .flex()
            .flex_col()
            .justify_center()
            .gap(m.sp[2])
            .size_full()
            .children(self.rows.iter().map(|r| meter_row(r, &m, &theme)))
            .into_any_element();
        let meters_h = px(KEY_ROWS.len() as f32 * LABEL_ROW * m.ui_scale
            + (KEY_ROWS.len() - 1) as f32 * f32::from(m.sp[2]));
        let body = split_body_fixed_side(chart, meters, self.wide, meters_h, &m).into_any_element();
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
        .child(width_probe(
            Rc::clone(&self.width),
            self.wide,
            px(SPLIT_W * m.ui_scale),
            cx.entity_id(),
        ))
    }
}
