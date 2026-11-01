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
//!
//! A widget of a flow layout that its host draws (the GPU box presets, `overview/gpu_flow.rs`)
//! has a [`FlowLook`] instead: it draws the 1.1.0 part at its place (an `engine_charts` cell, a
//! spark row, the section chart, or its value as plain text), whatever its size. The host gives
//! the name the preset's row shows and the chart it keeps for the part, so the widget builds no
//! chart of its own. The value and the bar still come from the widget's source.

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
use tl_model::validity::Validity;

use super::compose::{
    CELL_BAR_PAD, CELL_HEADER_H, CELL_STATS_H, Composed, Composition, ROW_BAR_MIN_W, ROW_GAP,
    ROW_NAME_W, ROW_SPARK_W, ROW_STATS_W, ROW_VALUE_W, TILE_STATS_H, compose, row_forms,
};
use super::source::{Availability, SourceData};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{ChartBg, ChartSeries, ChartSpec, ChartView, GridStyle, ValueFormat, YScale};
use crate::components::validity::copy_text;
use crate::hub::SnapshotHub;
use crate::i18n::{Strings, tr};
use crate::pages::performance::overview::fit::fit_text;
use crate::pages::performance::overview::gpu_flow::line_style;
use crate::pages::performance::overview::mem_box::MeterRow;
use crate::pages::performance::overview::parts::{
    ChartSettings, Meter, PLOT_LINE, SPARK_GRID, box_chart, chart_section_versions, class_probe,
    spark_labels, spark_row,
};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_lg, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The joint between the device name and the metric name.
const SEP: &str = " · ";

const KEY_NOT_CONNECTED: &str = "perf-widget-not-connected";
const KEY_UNKNOWN: &str = "perf-widget-unknown";

/// The width the first line keeps free in Edit Widgets, from the widget's left edge: the grip
/// and the gear, each `edit::HANDLE` wide, logical px.
const EDIT_INSET: f32 = 2. * super::edit::HANDLE;
/// The edge every part keeps free in Edit Widgets, px at every scale: the selected outline.
const EDIT_EDGE: f32 = super::edit::OUTLINE;
/// The right edge every part keeps free in Edit Widgets, logical px: the resize corner.
const EDIT_RIGHT: f32 = super::edit::CORNER;

/// The class the size probe uses for an unavailable widget, which has no composition.
const UNAVAILABLE_CLASS: u32 = u32::MAX;
/// The class of a widget with a [`FlowLook`], which draws the same part at every size.
const FLOW_CLASS: u32 = u32::MAX - 1;

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

/// How a widget of a host-drawn flow layout draws: the GPU box part of 1.1.0 (ADR-019 "Flow
/// placement", UI-PRF-017, UI-PRF-019). `label` is the name the preset's row shows (a shared
/// encode and decode engine has one "Video Encode/Decode" row), and `chart` the chart the host
/// keeps for the part.
#[derive(Clone, PartialEq)]
pub enum FlowLook {
    /// An `engine_charts` cell: the name and the value on a 16 px line over the chart. The
    /// chart area has the debug selector `chart_id`.
    Cell {
        label: SharedString,
        chart: Entity<ChartView>,
        chart_id: SharedString,
    },
    /// A spark row (`parts::spark_row`): name, meter, value and the sparkline column; with no
    /// chart the column stays empty, so the meters line up. `meter` is `parts::meter_fits`.
    Row {
        label: SharedString,
        chart: Option<Entity<ChartView>>,
        meter: bool,
    },
    /// The section chart, alone.
    Chart(Entity<ChartView>),
    /// The value alone in the small secondary text, as the temperature and power line draws it.
    Text,
}

impl FlowLook {
    /// The parts the look reads from the source.
    fn forms(&self) -> FormSet {
        match self {
            Self::Cell { .. } => FormSet::LABEL | FormSet::VALUE,
            Self::Row { .. } => FormSet::LABEL | FormSet::VALUE | FormSet::BAR,
            Self::Chart(_) => FormSet::empty(),
            Self::Text => FormSet::VALUE,
        }
    }

    fn label(&self) -> SharedString {
        match self {
            Self::Cell { label, .. } | Self::Row { label, .. } => label.clone(),
            Self::Chart(_) | Self::Text => SharedString::default(),
        }
    }
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
    /// Edit Widgets is on: the first line keeps the grip and the gear's width free at its start.
    edit_inset: bool,
    shown: Shown,
    chart: Option<Entity<ChartView>>,
    chart_key: Option<ChartKey>,
    /// Whether the chart is styled for a Cell (outlined) or a Row (sparkline).
    chart_cell: bool,
    /// The 1.1.0 part the widget draws, while its host draws its flow layout.
    flow: Option<FlowLook>,
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
        Self::with_flow(setting, source, hub, None, cx)
    }

    /// A widget that draws `flow` (see [`FlowLook`]) from the start, so it builds no chart.
    pub fn with_flow(
        setting: WidgetSetting,
        source: Entity<SourceData>,
        hub: &Entity<SnapshotHub>,
        flow: Option<FlowLook>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            setting,
            hub: hub.clone(),
            _source_sub: cx.observe(&source, Self::on_source),
            source,
            device_prefix: false,
            edit_inset: false,
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
            flow,
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

    /// The 1.1.0 part the widget draws for its host, if any.
    pub fn flow(&self) -> Option<&FlowLook> {
        self.flow.as_ref()
    }

    /// The host draws the widget's flow layout and gives the part it draws, or `None` when the
    /// widget draws its composition again. It renders again only when the look changed.
    pub fn set_flow(&mut self, flow: Option<FlowLook>, cx: &mut Context<Self>) {
        if self.flow == flow {
            return;
        }
        self.flow = flow;
        self.shown = self.read_shown(cx);
        self.sync_chart(cx);
        self.notify(cx);
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
        // A widget on the grid draws its composition.
        if self.setting.cell.is_some() {
            self.flow = None;
        }
        if self.source != source {
            self._source_sub = cx.observe(&source, Self::on_source);
            self.source = source;
        }
        self.shown = self.read_shown(cx);
        self.sync_chart(cx);
        self.notify(cx);
    }

    /// Edit Widgets on or off. While on, the parts keep the outline's 2 px free on every side
    /// and the resize corner's width free on the right, and the first text line starts right of
    /// the edit grip and gear, so the chrome covers no text (UI-PRF-061).
    pub fn set_edit_inset(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.edit_inset == on {
            return;
        }
        self.edit_inset = on;
        self.notify(cx);
    }

    /// The left margin of the first text line: the grip and the gear in Edit Widgets, past the
    /// edge the parts keep free, else 0.
    fn lead(&self, s: f32) -> Pixels {
        px(if self.edit_inset {
            EDIT_INSET * s - EDIT_EDGE
        } else {
            0.
        })
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
        if let Some(look) = &self.flow {
            return self.read_flow(look, src, availability);
        }
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
            compose_at(
                drawn,
                self.size.get(),
                metrics(cx).ui_scale,
                self.edit_inset,
            )
            .forms
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
            Availability::NotReported => tr(cx, src.not_reported_key()),
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

    /// What a widget with `look` shows: the look's name, and the value and bar from the source.
    /// The part draws whether or not the source is available. An unavailable source has no
    /// fill and the glyph 1.1.0 shows for it: "-" for a GPU with no record, the unsupported
    /// glyph for a metric the GPU does not report.
    fn read_flow(&self, look: &FlowLook, src: &SourceData, availability: Availability) -> Shown {
        let kept = look.forms();
        let fraction = kept
            .contains(FormSet::BAR)
            .then(|| src.bar_fraction())
            .flatten();
        let bar = bar_bucket(fraction);
        let fraction = if bar == self.shown.bar {
            self.shown.fraction
        } else {
            fraction
        };
        Shown {
            availability,
            forms: kept,
            kept,
            name: look.label(),
            value: match availability {
                _ if !kept.contains(FormSet::VALUE) => SharedString::default(),
                Availability::Available => src.value_text().clone(),
                Availability::NotReported => {
                    copy_text(Validity::Unsupported, &SharedString::default())
                }
                Availability::Gone | Availability::Unknown => {
                    copy_text(Validity::Missing, &SharedString::default())
                }
            },
            stats: SharedString::default(),
            bar,
            fraction,
            reason: SharedString::default(),
            // The host's chart: the widget builds none.
            series: None,
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
        if self.flow.is_some() {
            FLOW_CLASS
        } else if self.shown.availability == Availability::Available {
            self.composed.class()
        } else {
            UNAVAILABLE_CLASS
        }
    }

    /// The probe class at `area`.
    fn class_at(&self, area: Size<Pixels>, scale: f32) -> u32 {
        if self.flow.is_some() {
            FLOW_CLASS
        } else if self.shown.availability == Availability::Available {
            compose_at(self.shown.forms, area, scale, self.edit_inset).class()
        } else {
            UNAVAILABLE_CLASS
        }
    }
}

/// The composition of `forms` at `area`. In Edit Widgets (`edit`) a Row drops its parts by the
/// width left inside the edit margins, so no part reaches under the resize corner; the
/// composition itself is the applied view's.
fn compose_at(forms: FormSet, area: Size<Pixels>, scale: f32, edit: bool) -> Composed {
    let composed = compose(forms, area, scale);
    if !edit || composed.composition != Composition::Row {
        return composed;
    }
    let s = if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    };
    // Without a name the whole line starts right of the grip and the gear.
    let lead = if forms.contains(FormSet::LABEL) {
        0.
    } else {
        EDIT_INSET * s - EDIT_EDGE
    };
    let inner = f32::from(area.width) - 2. * EDIT_EDGE - EDIT_RIGHT * s - lead;
    // Whatever is left must hold the value, which a Row never drops (`compose::row_forms`).
    Composed {
        composition: Composition::Row,
        forms: row_forms(composed.forms, inner.max(0.) / s),
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
        let mut lead = self.lead(s);
        if forms.intersects(FormSet::LABEL | FormSet::VALUE) {
            let mut header = div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[2])
                .ml(std::mem::take(&mut lead))
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
                    .ml(std::mem::take(&mut lead))
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
        let lead = self.lead(s);
        if forms.contains(FormSet::LABEL) {
            let name = self.shown.name.clone();
            el = el.child(
                text_part(part_id(id, "name"), name, t_sm(m), theme.text_secondary).ml(lead),
            );
        }
        if forms.contains(FormSet::VALUE) {
            let mut style = t_lg(m);
            style.font_features = numeric_style(m, m.text.lg).font_features;
            let value = self.shown.value.clone();
            el = el.child(
                // The name line can be a pixel shorter than the gear, so the value keeps clear
                // of the gear too.
                text_part(part_id(id, "value"), value, style, theme.text_primary).ml(lead),
            );
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
        let lead = self.lead(s);
        let mut el = div()
            .flex()
            .flex_row()
            .items_center()
            .size_full()
            .gap(px(ROW_GAP * s));
        if !forms.contains(FormSet::LABEL) {
            // In Edit Widgets the line starts right of the grip and the gear.
            el = el.pl(lead);
        } else {
            let name = self.shown.name.clone();
            // The name column carries its own trailing space (`compose::row_parts_w`).
            el = el.child(div().flex_none().w(px((ROW_NAME_W - ROW_GAP) * s)).child(
                text_part(part_id(id, "name"), name, t_sm(m), theme.text_secondary).ml(lead),
            ));
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

impl MetricWidget {
    /// The 1.1.0 part of `look`. Every part but the text fills the place the host gives.
    fn flow_part(&self, look: &FlowLook, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
        let (id, s) = (self.setting.id, m.ui_scale);
        match look {
            FlowLook::Cell {
                chart, chart_id, ..
            } => {
                // The text style is set first: setting it replaces the nowrap, ellipsis and
                // colour set before it, and a Side by side cell is narrow enough for a long
                // name to wrap.
                let mut name = div();
                *name.text_style() = t_xs(m);
                let p = part_id(id, "name");
                let name = name
                    .debug_selector(move || p)
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(theme.text_secondary)
                    .child(self.shown.name.clone());
                let mut value = div();
                *value.text_style() = numeric_style(m, m.text.sm);
                let p = part_id(id, "value");
                let value = value
                    .debug_selector(move || p)
                    .flex()
                    .min_w_0()
                    .whitespace_nowrap()
                    .text_color(theme.text_primary)
                    .child(fit_text(self.shown.value.clone()));
                let chart_id = chart_id.to_string();
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .items_center()
                            .gap(m.sp[2])
                            .h(px(PLOT_LINE * s))
                            .child(name)
                            .child(value),
                    )
                    .child(
                        div()
                            .debug_selector(move || chart_id)
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(cached(chart)),
                    )
                    .into_any_element()
            }
            FlowLook::Row { chart, meter, .. } => {
                let row = MeterRow {
                    label: self.shown.name.clone(),
                    value: self.shown.value.clone(),
                    fraction: self.shown.fraction,
                    color: theme.graph.gpu_line,
                };
                spark_row(&row, chart.as_ref(), *meter, m, theme)
            }
            FlowLook::Chart(chart) => div()
                .relative()
                .size_full()
                .child(cached(chart))
                .into_any_element(),
            FlowLook::Text => {
                // The line's style, with tabular digits: the box places the value at the
                // width of its shape in this style (tasklens-lhliq.7).
                let mut text = div();
                *text.text_style() = line_style(m);
                let p = part_id(id, "value");
                text.debug_selector(move || p)
                    .flex_none()
                    .h(px(LABEL_ROW * s))
                    .whitespace_nowrap()
                    .text_color(theme.text_secondary)
                    .child(self.shown.value.clone())
                    .into_any_element()
            }
        }
    }
}

impl Render for MetricWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        if let Some(look) = self.flow.clone() {
            let part = self.flow_part(&look, &m, &theme);
            let id = widget_id(self.setting.id);
            let root = div().debug_selector(move || id).relative();
            // The text sizes itself, so the values of one line sit side by side.
            let root = if look == FlowLook::Text {
                root.flex_none()
            } else {
                root.size_full().overflow_hidden()
            };
            return root.child(part).into_any_element();
        }
        let scale = m.ui_scale;
        let available = self.shown.availability == Availability::Available;
        let forms = self.shown.forms;
        let body = if available {
            self.composed = compose_at(forms, self.size.get(), scale, self.edit_inset);
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
        let edit = self.edit_inset;
        let probe = class_probe(
            Rc::clone(&self.size),
            class,
            move |area| {
                if available {
                    compose_at(forms, area, scale, edit).class()
                } else {
                    UNAVAILABLE_CLASS
                }
            },
            view,
        );
        let id = widget_id(self.setting.id);
        // In Edit Widgets the parts keep clear of the outline and the resize corner.
        let body = if self.edit_inset {
            div()
                .size_full()
                .pt(px(EDIT_EDGE))
                .pl(px(EDIT_EDGE))
                .pb(px(EDIT_EDGE))
                .pr(px(EDIT_RIGHT * scale))
                .child(body)
        } else {
            body
        };
        div()
            .debug_selector(move || id)
            .relative()
            .size_full()
            .overflow_hidden()
            .child(body)
            .child(probe)
            .into_any_element()
    }
}
