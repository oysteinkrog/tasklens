//! The Overview page's Disk box (UI-PRF-017, D-022, UI-PRF-031): the read and write chart of all
//! disks, then one row per shown disk with its rates and busy meter.
//!
//! The page hands in the disks of the sidebar's visible cards. The box formats its text only
//! when the inputs' hash changes and notifies only then. It shows as many rows as fit the box
//! height under a chart of at least 48 px (TMOG), at most 8, then "+n more"; a resize re-renders
//! the box only when that count changes. Pointer movement never notifies.

use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Size, StyleRefinement, Styled as _, Subscription, Window, div, px, size,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::cpu_box::value_text;
use super::parts::{
    ChartSettings, MIN_CHART_H, Meter, box_chart, box_frame, chart_section_versions, class_probe,
    opens_device,
};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const DISK_BOX_ID: &str = "perf-overview-box-disk";

/// Disks with a row; the rest are counted in the "+n more" line.
pub const MAX_DISK_ROWS: usize = 8;

const KEY_TITLE: &str = "perf-overview-box-disks";
const KEY_MORE: &str = "perf-overview-more";
const KEY_READ: &str = "perf-disk-legend-read";
const KEY_WRITE: &str = "perf-disk-legend-write";
const KEY_RANGE: &str = "perf-disk-range-throughput";

/// The rate columns, logical px before `ui_scale`.
const RATE_W: f32 = 76.;
/// The busy meter column, logical px before `ui_scale`.
const BUSY_W: f32 = 48.;

/// The aggregate disk device's idx in `catalog`, `DeviceIdx(u16::MAX)` when it has none.
fn aggregate(catalog: &DeviceCatalog) -> DeviceIdx {
    catalog
        .find(&DeviceIdentity::Aggregate(AggregateKind::Disks))
        .map_or(DeviceIdx(u16::MAX), |e| e.idx)
}

/// One disk's row.
#[derive(Clone, Debug, PartialEq)]
pub struct DiskRow {
    pub key: DeviceKey,
    pub idx: DeviceIdx,
    pub name: SharedString,
    pub read: SharedString,
    pub write: SharedString,
    /// Active time as a fraction, `None` when not known.
    pub busy: Option<f32>,
}

/// The IO chart: read filled, write dashed, auto-scaled IEC rates.
fn chart_spec(device: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    let g = &theme.graph;
    let id = |metric| SeriesId { device, metric };
    ChartSpec {
        series: smallvec![
            ChartSeries {
                id: id(MetricId::DISK_READ),
                fill: Some(g.disk_fill),
                line: g.disk_line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: id(MetricId::DISK_WRITE),
                fill: None,
                line: g.disk_line,
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y: YScale::AutoRate { iec: true },
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

/// No heading and no value: the rows carry the numbers.
fn chart_labels(range_word: SharedString) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::RateIec,
        value_align: ValueAlign::Hidden,
        range_word,
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// How many rows fit a body of `body` under the chart and the column header, at most `rows`;
/// one row goes to "+n more" when not all of `total` fit. All `rows` before the first paint.
fn rows_fit(body: Size<Pixels>, rows: usize, total: usize, m: &UiMetrics) -> usize {
    if body.height == px(0.) {
        return rows;
    }
    let row = LABEL_ROW * m.ui_scale;
    let space = f32::from(body.height) - MIN_CHART_H * m.ui_scale - f32::from(m.sp[5]) - row;
    let fit = (space.max(0.) / row).floor() as usize;
    if fit >= total {
        rows.min(total)
    } else {
        fit.saturating_sub(1).min(rows)
    }
}

/// The Disk box.
pub struct DiskBox {
    hub: Entity<SnapshotHub>,
    rows: SmallVec<[DiskRow; 8]>,
    /// Disks past `MAX_DISK_ROWS`.
    more: usize,
    /// "+n more" by the number of rows shown, empty when every disk has a row.
    more_texts: SmallVec<[SharedString; 9]>,
    io_chart: Entity<ChartView>,
    /// The aggregate device the chart is on.
    aggregate: DeviceIdx,
    catalog_version: u64,
    title: SharedString,
    read_word: SharedString,
    write_word: SharedString,
    text_bits: u64,
    /// The body size of the last paint, and how many rows the last render showed.
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

impl DiskBox {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        disks: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let aggregate = aggregate(&snap.devices);
        let spec = chart_spec(aggregate, &settings, &theme);
        let labels = chart_labels(tr(cx, KEY_RANGE));
        let io_chart = box_chart(spec, labels, hub, cx);
        let mut this = Self {
            hub: hub.clone(),
            rows: SmallVec::new(),
            more: 0,
            more_texts: SmallVec::new(),
            io_chart,
            aggregate,
            catalog_version: snap.devices.version,
            title: tr(cx, KEY_TITLE),
            read_word: tr(cx, KEY_READ),
            write_word: tr(cx, KEY_WRITE),
            text_bits: 0,
            body: Rc::new(Cell::new(size(px(0.), px(0.)))),
            shown: 0,
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
        this.build_rows(disks, cx);
        this
    }

    /// New disks from the page. Nothing happens when they are the same.
    pub fn set_disks(
        &mut self,
        disks: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) {
        let same = disks.len() == self.rows.len() + self.more
            && disks
                .iter()
                .zip(&self.rows)
                .all(|((k, i, n), r)| *k == r.key && *i == r.idx && *n == r.name);
        if same {
            return;
        }
        self.build_rows(disks, cx);
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    pub fn rows(&self) -> &[DiskRow] {
        &self.rows
    }

    pub fn more(&self) -> usize {
        self.more
    }

    pub fn io_chart(&self) -> &Entity<ChartView> {
        &self.io_chart
    }

    /// How many rows the last render showed.
    pub fn shown(&self) -> usize {
        self.shown
    }

    /// The "+n more" text the last render showed, empty for none.
    pub fn shown_more_text(&self) -> SharedString {
        self.more_texts.get(self.shown).cloned().unwrap_or_default()
    }

    /// "sda\tRead 1.0 MiB/s\tWrite 0 KiB/s" per row, then "+n more".
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\n');
        for r in &self.rows {
            out.push_str(&r.name);
            for (word, rate) in [(&self.read_word, &r.read), (&self.write_word, &r.write)] {
                out.push('\t');
                out.push_str(word);
                out.push(' ');
                out.push_str(rate);
            }
            out.push('\n');
        }
        if let Some(more) = self
            .more_texts
            .get(self.rows.len())
            .filter(|t| !t.is_empty())
        {
            out.push_str(more);
            out.push('\n');
        }
    }

    fn build_rows(
        &mut self,
        disks: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) {
        self.rows.clear();
        self.rows.extend(
            disks
                .iter()
                .take(MAX_DISK_ROWS)
                .map(|(key, idx, name)| DiskRow {
                    key: key.clone(),
                    idx: *idx,
                    name: name.clone(),
                    read: SharedString::default(),
                    write: SharedString::default(),
                    busy: None,
                }),
        );
        self.more = disks.len().saturating_sub(MAX_DISK_ROWS);
        self.sync_more_texts(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_text(&snap, true);
    }

    /// "+n more" for each number of rows shown.
    fn sync_more_texts(&mut self, cx: &mut Context<Self>) {
        let total = self.rows.len() + self.more;
        self.more_texts.clear();
        for shown in 0..=self.rows.len() {
            let n = total - shown;
            self.more_texts.push(if n == 0 {
                SharedString::default()
            } else {
                tr_args(cx, KEY_MORE, &[("n", FluentValue::from(n))])
            });
        }
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let devices = ev.mask.contains(ComponentMask::DEVICES);
        if !(devices || ev.mask.contains(ComponentMask::SYSTEM)) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        if devices && snap.devices.version != self.catalog_version {
            self.catalog_version = snap.devices.version;
            let aggregate = aggregate(&snap.devices);
            if aggregate != self.aggregate {
                self.aggregate = aggregate;
                self.rebuild_spec(cx);
            }
        }
        if self.sync_text(&snap, false) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// Every row's rates and busy meter when their inputs changed, or always with `force`. True
    /// when the text was formatted again.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> bool {
        let totals = |idx: DeviceIdx| {
            snap.system
                .disks
                .iter()
                .find(|(i, _)| *i == idx)
                .map(|(_, t)| t)
        };
        let mut h = FxHasher::default();
        for r in &self.rows {
            r.idx.0.hash(&mut h);
            let Some(t) = totals(r.idx) else {
                h.write_u8(0);
                continue;
            };
            h.write_u64(t.read_bps.value.to_bits());
            t.read_bps.validity.hash(&mut h);
            h.write_u64(t.write_bps.value.to_bits());
            t.write_bps.validity.hash(&mut h);
            h.write_u32(t.active_pct.value.to_bits());
            t.active_pct.validity.hash(&mut h);
        }
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        let unit = NetworkUnit::default();
        for r in &mut self.rows {
            let Some(t) = totals(r.idx) else {
                let missing = value_text(Validity::Missing, |_| {});
                r.read = missing.clone();
                r.write = missing;
                r.busy = None;
                continue;
            };
            let rate = |m: &tl_model::system::Metric<f64>| {
                value_text(m.validity, |out| {
                    format_value(ValueFormat::RateIec, m.value as f32, unit, out);
                })
            };
            r.read = rate(&t.read_bps);
            r.write = rate(&t.write_bps);
            r.busy = matches!(t.active_pct.validity, Validity::Valid | Validity::Stale)
                .then(|| t.active_pct.value / 100.);
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

    /// A new theme or UI scale: new chart colours, and the rows draw in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_spec(cx);
        cx.notify();
    }

    /// A new language: new title, column words and "+n more".
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        self.read_word = tr(cx, KEY_READ);
        self.write_word = tr(cx, KEY_WRITE);
        let labels = chart_labels(tr(cx, KEY_RANGE));
        self.io_chart.update(cx, |v, cx| v.set_labels(labels, cx));
        self.sync_more_texts(cx);
        cx.notify();
    }

    fn rebuild_spec(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let spec = chart_spec(self.aggregate, &self.settings, &theme);
        self.io_chart.update(cx, |v, cx| v.set_spec(spec, cx));
    }
}

/// The debug selector prefix of a disk row, followed by its device key.
pub const DISK_ROW_ID_PREFIX: &str = "perf-overview-disk-row:";

/// A row: name, read, write, busy. With `None` for the meter it is the column header. A click
/// on a row with a `target` opens that disk (crw.44).
fn row_element(
    target: Option<&DeviceKey>,
    name: &SharedString,
    read: &SharedString,
    write: &SharedString,
    busy: Option<Option<f32>>,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let s = m.ui_scale;
    let header = busy.is_none();
    let (color, style) = if header {
        (theme.text_secondary, t_xs(m))
    } else {
        (theme.text_primary, numeric_style(m, m.text.sm))
    };
    let rate = |text: &SharedString| {
        let mut d = div()
            .flex_none()
            .w(px(RATE_W * s))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .text_color(color)
            .child(text.clone());
        *d.text_style() = style.clone();
        d
    };
    let mut label = div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(name.clone());
    *label.text_style() = if header { t_xs(m) } else { t_sm(m) };
    let busy_el = div().flex_none().w(px(BUSY_W * s));
    let busy_el = match busy.flatten() {
        Some(f) => busy_el.child(Meter {
            fraction: Some(f),
            color: theme.graph.disk_line,
        }),
        None if !header => busy_el.child(Meter {
            fraction: None,
            color: theme.graph.disk_line,
        }),
        None => busy_el,
    };
    let mut row = div();
    if let Some(key) = target.map(|k| k.0.clone()) {
        row = row.debug_selector(move || format!("{DISK_ROW_ID_PREFIX}{key}"));
    }
    let row = row
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * s))
        .child(label)
        .child(rate(read))
        .child(rate(write))
        .child(busy_el);
    opens_device(row, target.cloned()).into_any_element()
}

impl Render for DiskBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = m.ui_scale;
        let rows = self.rows.len();
        let total = rows + self.more;
        self.shown = rows_fit(self.body.get(), rows, total, &m);

        let chart = div()
            .relative()
            .flex_1()
            .min_h(px(MIN_CHART_H * s))
            .w_full()
            .child(
                self.io_chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            );
        let mut list = div().flex().flex_col().flex_none().w_full();
        if rows > 0 {
            list = list.child(row_element(
                None,
                &SharedString::default(),
                &self.read_word,
                &self.write_word,
                None,
                &m,
                &theme,
            ));
        }
        for r in self.rows.iter().take(self.shown) {
            list = list.child(row_element(
                Some(&r.key),
                &r.name,
                &r.read,
                &r.write,
                Some(r.busy),
                &m,
                &theme,
            ));
        }
        let more = self.shown_more_text();
        if !more.is_empty() {
            let mut line = div()
                .flex_none()
                .h(px(LABEL_ROW * s))
                .text_color(theme.text_secondary)
                .child(more);
            *line.text_style() = t_xs(&m);
            list = list.child(line);
        }
        let probe_m = m;
        let body = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[5])
            .child(chart)
            .child(list)
            .child(class_probe(
                Rc::clone(&self.body),
                self.shown as u32,
                move |b| rows_fit(b, rows, total, &probe_m) as u32,
                cx.entity_id(),
            ));
        box_frame(
            DISK_BOX_ID,
            self.rows.first().map(|r| r.key.clone()),
            self.title.clone(),
            SharedString::default(),
            SharedString::default(),
            body.into_any_element(),
            &m,
            &theme,
        )
        .size_full()
    }
}
