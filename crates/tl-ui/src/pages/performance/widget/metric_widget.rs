//! One widget: one source in one or more forms (ADR-019 "Entities and data flow", UI-PRF-061).
//!
//! A [`MetricWidget`] is a GPUI entity keyed by its widget id. It observes the shared
//! [`SourceData`] of its source and takes the name, value, stats and bar from it. It notifies
//! only when a part it shows changed: the value when it shows a value, the stats line when it
//! shows stats, the bar's bucket when it shows a bar, and the name or the unavailable state. A
//! sample on another source never reaches it.
//!
//! "Shows" means the forms its composition keeps at its size: a part a narrow Row drops does not
//! notify it.
//!
//! It owns its `ChartView` while it draws a chart or sparkline and the source is available; the
//! chart is a cached child view whose geometry is rebuilt only when its series version, scale
//! or size changes (ADR-009). A new Y scale from the source reaches the chart even when nothing
//! else the widget shows changed. The widget picks its composition (Cell, Tile or Row,
//! `compose.rs`) from its forms and the size of its last paint. A resize renders it again only
//! when the composition or the forms it keeps change.
//!
//! An unavailable widget shows its name (with the saved device name when the device is gone)
//! and one centred line: "Not connected", "Not reported by this GPU" or "Needs a newer
//! tasklens". It comes back by itself when the source does.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement, Styled as _,
    Subscription, Window, div, px, size,
};
use smallvec::smallvec;
use tl_model::ids::SeriesId;
use tl_model::layout::{FormSet, WidgetOptions, WidgetSetting};

use super::compose::{
    CELL_BAR_PAD, CELL_HEADER_H, CELL_STATS_H, Composed, Composition, ROW_BAR_MIN_W, ROW_GAP,
    ROW_NAME_W, ROW_SPARK_W, ROW_STATS_W, ROW_VALUE_W, TILE_STATS_H, compose,
};
use super::source::{Availability, SourceData};
use crate::chart::{ChartBg, ChartSeries, ChartSpec, ChartView, GridStyle, ValueFormat, YScale};
use crate::hub::SnapshotHub;
use crate::i18n::{Strings, tr};
use crate::pages::performance::overview::fit::fit_text;
use crate::pages::performance::overview::parts::{
    ChartSettings, Meter, SPARK_GRID, box_chart, chart_section_versions, class_probe, spark_labels,
};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_lg, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The joint between the device name and the metric name.
const SEP: &str = " · ";

const KEY_NOT_CONNECTED: &str = "perf-widget-not-connected";
const KEY_NOT_REPORTED: &str = "perf-widget-not-reported";
const KEY_UNKNOWN: &str = "perf-widget-unknown";

/// The class the size probe uses for an unavailable widget, which has no composition.
const UNAVAILABLE_CLASS: u32 = u32::MAX;

/// Bar buckets: the bar notifies only when its fill moves by half a percent or more.
const BAR_BUCKETS: f32 = 200.;

/// The debug selector of widget `id`'s root.
pub fn widget_id(id: u32) -> String {
    format!("perf-widget-{id}")
}

/// The debug selector of a part of widget `id`: "name", "value", "stats", "bar", "chart" or
/// "reason" (the unavailable line).
pub fn part_id(id: u32, part: &str) -> String {
    format!("perf-widget-{id}-{part}")
}

/// The bar's bucket for `fraction`; `None` without a bar.
fn bar_bucket(fraction: Option<f32>) -> Option<u16> {
    fraction.map(|f| (f.clamp(0., 1.) * BAR_BUCKETS).round() as u16)
}

/// What the widget shows, compared to tell whether a source change must render it again.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    availability: Availability,
    /// The widget's forms the device offers. The composition is picked from these.
    forms: FormSet,
    /// The forms the composition keeps at the widget's size. Only these parts are read from
    /// the source.
    kept: FormSet,
    name: SharedString,
    value: SharedString,
    stats: SharedString,
    bar: Option<u16>,
    /// The bar's fill, kept with the bucket that was last shown.
    fraction: Option<f32>,
    reason: SharedString,
    series: Option<SeriesId>,
}

/// What the chart spec is built from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChartKey {
    series: SeriesId,
    y: YScale,
    options: WidgetOptions,
    format: ValueFormat,
}

/// One widget of a container.
pub struct MetricWidget {
    setting: WidgetSetting,
    hub: Entity<SnapshotHub>,
    source: Entity<SourceData>,
    /// Whether the name has the GPU's name in front (the box shows two GPUs).
    device_prefix: bool,
    shown: Shown,
    chart: Option<Entity<ChartView>>,
    chart_key: Option<ChartKey>,
    /// Whether the chart is styled for a Cell (outlined) or a Row (sparkline).
    chart_cell: bool,
    /// The widget's size at its last paint, or the size the container gave.
    size: Rc<Cell<Size<Pixels>>>,
    composed: Composed,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    _source_sub: Subscription,
    _subs: [Subscription; 3],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl MetricWidget {
    /// A widget for `setting` on `source` (from the page's `SourceStore`).
    pub fn new(
        setting: WidgetSetting,
        source: Entity<SourceData>,
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            setting,
            hub: hub.clone(),
            _source_sub: cx.observe(&source, Self::on_source),
            source,
            device_prefix: false,
            shown: Shown {
                availability: Availability::Gone,
                forms: FormSet::empty(),
                kept: FormSet::empty(),
                name: SharedString::default(),
                value: SharedString::default(),
                stats: SharedString::default(),
                bar: None,
                fraction: None,
                reason: SharedString::default(),
                series: None,
            },
            chart: None,
            chart_key: None,
            chart_cell: true,
            size: Rc::new(Cell::new(size(px(0.), px(0.)))),
            composed: Composed {
                composition: Composition::Row,
                forms: FormSet::empty(),
            },
            settings: ChartSettings::read(cx),
            settings_versions: chart_section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
            _subs: [
                cx.observe_global::<TlSettings>(Self::on_settings),
                cx.observe_global::<UiMetrics>(Self::on_style),
                cx.observe_global::<Strings>(Self::on_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.shown = this.read_shown(cx);
        this.sync_chart(cx);
        this
    }

    /// The widget id, unique in its container.
    pub fn id(&self) -> u32 {
        self.setting.id
    }

    pub fn setting(&self) -> &WidgetSetting {
        &self.setting
    }

    pub fn source(&self) -> &Entity<SourceData> {
        &self.source
    }

    /// The chart while the widget shows one.
    pub fn chart(&self) -> Option<&Entity<ChartView>> {
        self.chart.as_ref()
    }

    pub fn availability(&self) -> Availability {
        self.shown.availability
    }

    /// The composition and forms of the last render.
    pub fn composed(&self) -> Composed {
        self.composed
    }

    /// The forms the widget draws: its forms that the device offers.
    pub fn drawn_forms(&self) -> FormSet {
        self.shown.forms
    }

    /// The name, value and stats text and the unavailable line, as shown.
    pub fn texts(&self) -> [&SharedString; 4] {
        [
            &self.shown.name,
            &self.shown.value,
            &self.shown.stats,
            &self.shown.reason,
        ]
    }

    /// The bar's fill as shown, 0 to 1.
    pub fn bar_fraction(&self) -> Option<f32> {
        self.shown.fraction
    }

    /// New settings for the same widget id (a layout change keeps the entity). A new source
    /// replaces the observed one; the chart is kept when its series and scale stay.
    pub fn set_setting(
        &mut self,
        setting: WidgetSetting,
        source: Entity<SourceData>,
        cx: &mut Context<Self>,
    ) {
        if self.setting == setting && self.source == source {
            return;
        }
        self.setting = setting;
        if self.source != source {
            self._source_sub = cx.observe(&source, Self::on_source);
            self.source = source;
        }
        self.shown = self.read_shown(cx);
        self.sync_chart(cx);
        self.notify(cx);
    }

    /// Puts the GPU's name in front of the metric's name, when the box shows two GPUs.
    pub fn set_device_prefix(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.device_prefix == on {
            return;
        }
        self.device_prefix = on;
        self.refresh(cx);
    }

    /// The size the container places the widget at. It renders again only when its
    /// composition or kept forms change; otherwise the next paint just uses the size.
    pub fn set_size(&mut self, area: Size<Pixels>, cx: &mut Context<Self>) {
        if self.size.get() == area {
            return;
        }
        self.size.set(area);
        if self.class_at(area, metrics(cx).ui_scale) != self.class() {
            self.shown = self.read_shown(cx);
            self.sync_chart(cx);
            self.notify(cx);
        }
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// The source changed: notify only when a part the widget shows changed.
    fn on_source(&mut self, _: Entity<SourceData>, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    /// Reads what the widget shows and notifies when that changed. The chart is synced every
    /// time: a new Y scale changes only the chart's spec, and the chart renders itself.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let shown = self.read_shown(cx);
        let changed = shown != self.shown;
        self.shown = shown;
        let chart_changed = self.sync_chart(cx);
        if changed || chart_changed {
            self.notify(cx);
        }
    }

    /// What the widget shows from the source now. Parts its composition does not keep at its
    /// size stay empty, so their changes never notify it.
    fn read_shown(&self, cx: &App) -> Shown {
        let src = self.source.read(cx);
        let availability = src.availability();
        let forms = self.setting.forms;
        let drawn = if availability == Availability::Available {
            let offered = forms & src.offered_forms();
            if offered.is_empty() {
                FormSet::VALUE
            } else {
                offered
            }
        } else {
            FormSet::empty()
        };
        let kept = if drawn.is_empty() {
            drawn
        } else {
            compose(drawn, self.size.get(), metrics(cx).ui_scale).forms
        };
        let device = match availability {
            Availability::Gone => self
                .setting
                .device_name
                .as_deref()
                .map(|d| SharedString::from(d.to_string())),
            _ if self.device_prefix => src.device_name().cloned(),
            _ => None,
        };
        let name = if kept.contains(FormSet::LABEL) || availability != Availability::Available {
            match device {
                Some(d) if !d.is_empty() => format!("{d}{SEP}{}", src.name()).into(),
                _ => src.name().clone(),
            }
        } else {
            SharedString::default()
        };
        let pick = |form: FormSet, text: &SharedString| {
            if kept.contains(form) {
                text.clone()
            } else {
                SharedString::default()
            }
        };
        let fraction = kept
            .contains(FormSet::BAR)
            .then(|| src.bar_fraction())
            .flatten();
        let bar = bar_bucket(fraction);
        // Keep the fill that was shown while the bucket stays, so a sample inside one bucket
        // changes nothing.
        let fraction = if bar == self.shown.bar {
            self.shown.fraction
        } else {
            fraction
        };
        let reason = match availability {
            Availability::Available => SharedString::default(),
            Availability::Gone => tr(cx, KEY_NOT_CONNECTED),
            Availability::NotReported => tr(cx, KEY_NOT_REPORTED),
            Availability::Unknown => tr(cx, KEY_UNKNOWN),
        };
        Shown {
            availability,
            forms: drawn,
            kept,
            name,
            value: pick(FormSet::VALUE, src.value_text()),
            stats: pick(FormSet::STATS, src.stats_text()),
            bar,
            fraction,
            reason,
            series: kept
                .contains(FormSet::CHART)
                .then(|| src.series())
                .flatten(),
        }
    }

    /// Creates, updates or drops the chart to match what is shown. Returns whether the chart
    /// was created or dropped, which the widget must render; a new spec only renders the chart.
    fn sync_chart(&mut self, cx: &mut Context<Self>) -> bool {
        let src = self.source.read(cx);
        let key = self
            .shown
            .series
            .zip(src.meta())
            .map(|(series, meta)| ChartKey {
                series,
                y: src.y_scale(),
                options: self.setting.options,
                format: meta.format,
            });
        let Some(key) = key else {
            self.chart_key = None;
            return self.chart.take().is_some();
        };
        if self.chart_key == Some(key) && self.chart.is_some() {
            return false;
        }
        let old_format = self.chart_key.map(|k| k.format);
        self.chart_key = Some(key);
        let spec = chart_spec(key, self.chart_cell, &self.settings, current(cx));
        match &self.chart {
            Some(chart) => {
                // A new source can bring a new value format (percent to bytes): the labels
                // follow it, not only the spec.
                let labels = (old_format != Some(key.format)).then(|| spark_labels(key.format));
                chart.update(cx, |v, cx| {
                    v.set_spec(spec, cx);
                    if let Some(labels) = labels {
                        v.set_labels(labels, cx);
                    }
                });
                false
            }
            None => {
                let hub = self.hub.clone();
                self.chart = Some(box_chart(spec, spark_labels(key.format), &hub, cx));
                true
            }
        }
    }

    /// Restyles the chart for a Cell or a Row.
    fn style_chart(&mut self, cell: bool, cx: &mut Context<Self>) {
        if self.chart_cell == cell {
            return;
        }
        self.chart_cell = cell;
        if let (Some(chart), Some(key)) = (&self.chart, self.chart_key) {
            let spec = chart_spec(key, cell, &self.settings, current(cx));
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }

    fn rebuild_chart(&mut self, cx: &mut Context<Self>) {
        if let (Some(chart), Some(key)) = (&self.chart, self.chart_key) {
            let spec = chart_spec(key, self.chart_cell, &self.settings, current(cx));
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }

    /// New chart settings (range, rounding).
    fn on_settings(&mut self, cx: &mut Context<Self>) {
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
        self.rebuild_chart(cx);
    }

    /// A new theme or UI scale: new chart colours, new sizes.
    fn on_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_chart(cx);
        self.notify(cx);
    }

    /// A new language: the unavailable line. The source re-translates the name and notifies.
    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.refresh(cx);
    }

    /// The probe class of the last render.
    fn class(&self) -> u32 {
        if self.shown.availability == Availability::Available {
            self.composed.class()
        } else {
            UNAVAILABLE_CLASS
        }
    }

    /// The probe class at `area`.
    fn class_at(&self, area: Size<Pixels>, scale: f32) -> u32 {
        if self.shown.availability == Availability::Available {
            compose(self.shown.forms, area, scale).class()
        } else {
            UNAVAILABLE_CLASS
        }
    }
}

/// The chart of a widget: its series in the GPU colours, filled unless the Fill option is off,
/// dashed with the Dashed option. A Cell's chart has grid lines and an outline; a Row's
/// sparkline has the sparkline grid.
fn chart_spec(key: ChartKey, cell: bool, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: key.series,
            fill: key.options.fill.then_some(theme.graph.gpu_fill),
            line: theme.graph.gpu_line,
            dashed: key.options.dashed,
            ..ChartSeries::default()
        }],
        y: key.y,
        range_ns: s.range_ns,
        grid: if cell {
            GridStyle {
                lines: true,
                baseline: false,
                outline: true,
                rounded: s.rounded,
                vertical_at: None,
            }
        } else {
            SPARK_GRID
        },
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

fn cached(chart: &Entity<ChartView>) -> AnyElement {
    chart
        .clone()
        .cached(StyleRefinement::default().size_full())
        .into_any_element()
}

/// A one-line text part: `style`, `color`, no wrap, ellipsized.
fn text_part(
    id: String,
    text: SharedString,
    style: gpui_kit::TextStyleRefinement,
    color: gpui_kit::Hsla,
) -> gpui_kit::Div {
    let mut el = div();
    *el.text_style() = style;
    el.debug_selector(move || id)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(color)
        .child(text)
}

/// The value at `size` 600 with tabular figures.
fn value_style(m: &UiMetrics, size: Pixels) -> gpui_kit::TextStyleRefinement {
    let mut style = numeric_style(m, size);
    style.font_weight = Some(FontWeight::SEMIBOLD);
    style
}

impl MetricWidget {
    fn meter(&self, id: u32, theme: &TlTheme) -> gpui_kit::Div {
        let p = part_id(id, "bar");
        div().debug_selector(move || p).w_full().child(Meter {
            fraction: self.shown.fraction,
            color: theme.graph.gpu_line,
        })
    }

    fn chart_part(&self, id: u32) -> gpui_kit::Div {
        let p = part_id(id, "chart");
        let mut el = div().debug_selector(move || p).relative();
        if let Some(chart) = &self.chart {
            el = el.child(cached(chart));
        }
        el
    }

    /// The Cell: header line, stats line, bar, then the chart filling the rest.
    fn cell(&self, forms: FormSet, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        let (id, s) = (self.setting.id, m.ui_scale);
        let mut el = div().flex().flex_col().size_full();
        if forms.intersects(FormSet::LABEL | FormSet::VALUE) {
            let mut header = div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[2])
                .h(px(CELL_HEADER_H * s));
            if forms.contains(FormSet::LABEL) {
                let name = self.shown.name.clone();
                header = header.child(
                    text_part(part_id(id, "name"), name, t_sm(m), theme.text_secondary)
                        .flex_1()
                        .min_w_0(),
                );
            } else {
                header = header.child(div().flex_1());
            }
            if forms.contains(FormSet::VALUE) {
                let value = self.shown.value.clone();
                let p = part_id(id, "value");
                let mut v = div();
                *v.text_style() = value_style(m, m.text.sm);
                header = header.child(
                    v.debug_selector(move || p)
                        .flex_none()
                        .whitespace_nowrap()
                        .text_color(theme.text_primary)
                        .child(fit_text(value)),
                );
            }
            el = el.child(header);
        }
        if forms.contains(FormSet::STATS) {
            let stats = self.shown.stats.clone();
            el = el.child(
                text_part(part_id(id, "stats"), stats, t_xs(m), theme.text_secondary)
                    .flex_none()
                    .h(px(CELL_STATS_H * s)),
            );
        }
        if forms.contains(FormSet::BAR) {
            el = el.child(self.meter(id, theme).flex_none().py(px(CELL_BAR_PAD * s)));
        }
        el.child(self.chart_part(id).flex_1().min_h_0().w_full())
    }

    /// The Tile: name, the value in large text, the stats line, and the bar at the bottom.
    fn tile(&self, forms: FormSet, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        let (id, s) = (self.setting.id, m.ui_scale);
        let mut el = div().flex().flex_col().size_full();
        if forms.contains(FormSet::LABEL) {
            let name = self.shown.name.clone();
            el = el.child(text_part(
                part_id(id, "name"),
                name,
                t_sm(m),
                theme.text_secondary,
            ));
        }
        if forms.contains(FormSet::VALUE) {
            let mut style = t_lg(m);
            style.font_features = numeric_style(m, m.text.lg).font_features;
            let value = self.shown.value.clone();
            el = el.child(text_part(
                part_id(id, "value"),
                value,
                style,
                theme.text_primary,
            ));
        }
        if forms.contains(FormSet::STATS) {
            let stats = self.shown.stats.clone();
            el = el.child(
                text_part(part_id(id, "stats"), stats, t_xs(m), theme.text_secondary)
                    .h(px(TILE_STATS_H * s)),
            );
        }
        el = el.child(div().flex_1());
        if forms.contains(FormSet::BAR) {
            el = el.child(self.meter(id, theme).flex_none());
        }
        el
    }

    /// The Row: name, bar, value, stats and the sparkline on one line. The bar and the
    /// sparkline grow with a taller row; the text stays centred.
    fn row(&self, forms: FormSet, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        let (id, s) = (self.setting.id, m.ui_scale);
        let mut el = div()
            .flex()
            .flex_row()
            .items_center()
            .size_full()
            .gap(px(ROW_GAP * s));
        if forms.contains(FormSet::LABEL) {
            let name = self.shown.name.clone();
            // The name column carries its own trailing space (`compose::row_parts_w`).
            el = el.child(
                div()
                    .flex_none()
                    .w(px((ROW_NAME_W - ROW_GAP) * s))
                    .child(text_part(
                        part_id(id, "name"),
                        name,
                        t_sm(m),
                        theme.text_secondary,
                    )),
            );
        }
        let bar = forms.contains(FormSet::BAR);
        if bar {
            el = el.child(self.meter(id, theme).flex_1().min_w(px(ROW_BAR_MIN_W * s)));
        }
        if forms.contains(FormSet::VALUE) {
            let value = self.shown.value.clone();
            let p = part_id(id, "value");
            let mut v = div();
            *v.text_style() = numeric_style(m, m.text.sm);
            let mut v = v
                .debug_selector(move || p)
                .min_w(px(ROW_VALUE_W * s))
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .text_color(theme.text_primary)
                .child(fit_text(value));
            // Without a bar or a sparkline the value takes the space left.
            if !forms.intersects(FormSet::BAR | FormSet::CHART) {
                v = v.flex_1().min_w_0();
            }
            el = el.child(v);
        }
        if forms.contains(FormSet::STATS) {
            let stats = self.shown.stats.clone();
            el = el.child(
                text_part(part_id(id, "stats"), stats, t_xs(m), theme.text_secondary)
                    .flex_none()
                    .max_w(px(ROW_STATS_W * s)),
            );
        }
        if forms.contains(FormSet::CHART) {
            let spark = self.chart_part(id).h_full();
            el = el.child(if bar {
                spark.flex_none().w(px(ROW_SPARK_W * s))
            } else {
                // The sparkline takes the bar's place.
                spark.flex_1().min_w(px(ROW_BAR_MIN_W * s))
            });
        }
        el
    }

    /// The unavailable widget: its name and the reason, centred.
    fn unavailable(&self, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        let id = self.setting.id;
        let name = self.shown.name.clone();
        let reason = self.shown.reason.clone();
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .child(text_part(
                part_id(id, "name"),
                name,
                t_sm(m),
                theme.text_secondary,
            ))
            .child(text_part(
                part_id(id, "reason"),
                reason,
                t_sm(m),
                theme.text_secondary,
            ))
    }
}

impl Render for MetricWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let scale = m.ui_scale;
        let available = self.shown.availability == Availability::Available;
        let forms = self.shown.forms;
        let body = if available {
            self.composed = compose(forms, self.size.get(), scale);
            // The size probe asked for this render after a resize changed the kept forms:
            // read the parts that are now kept.
            if self.composed.forms != self.shown.kept {
                self.shown = self.read_shown(cx);
                self.sync_chart(cx);
            }
            let cell = self.composed.composition == Composition::Cell;
            self.style_chart(cell, cx);
            let kept = self.composed.forms;
            match self.composed.composition {
                Composition::Cell => self.cell(kept, &m, &theme),
                Composition::Tile => self.tile(kept, &m, &theme),
                Composition::Row => self.row(kept, &m, &theme),
            }
        } else {
            self.unavailable(&m, &theme)
        };
        let class = self.class();
        let view = cx.entity_id();
        let probe = class_probe(
            Rc::clone(&self.size),
            class,
            move |area| {
                if available {
                    compose(forms, area, scale).class()
                } else {
                    UNAVAILABLE_CLASS
                }
            },
            view,
        );
        let id = widget_id(self.setting.id);
        div()
            .debug_selector(move || id)
            .relative()
            .size_full()
            .overflow_hidden()
            .child(body)
            .child(probe)
    }
}
