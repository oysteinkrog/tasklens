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
//! On a device page the chart shows "Failed" with the collector's error while the collector of
//! its kind has failed, as the 1.3.0 page charts did (`SourceData::chart_state`).
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
//!
//! The Memory box's flow looks (`FlowLook::MemRow` and `FlowLook::MemCell`, `overview/mem_flow.rs`)
//! draw a 1.2.0 row or `charts` cell from the rows' model the box keeps, as the cores group
//! draws the core rows. The box updates the model on a sample and notifies only the widgets of
//! the rows that changed ([`MetricWidget::flow_changed`]).
//!
//! A widget in a device page container draws its Cell in the page style (ADR-019 phase 3): its
//! chart is a 1.3.0 page chart with the heading, value, legend and range text inside it
//! (`look::ChartLook::page_labels`), and the Cell needs the page minimum
//! (`compose::page_cell_min`). The style follows the container, never the placement.
//!
//! The Network box's flow looks (`FlowLook::NetDirection` and `FlowLook::NetCombined`,
//! `overview/net_flow.rs`) draw a 1.2.0 Download or Upload part, or the `combined` body, from
//! the rate lines the box keeps. The box notifies only these widgets when a rate, peak or total
//! changed.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative, size,
};
use tl_model::ids::SeriesId;
use tl_model::validity::Validity;
use tl_ui_settings::layout::{FormSet, WidgetSetting};

use super::compose::{
    CELL_BAR_PAD, CELL_HEADER_H, CELL_STATS_H, Composed, Composition, ROW_BAR_MIN_W, ROW_GAP,
    ROW_NAME_W, ROW_SPARK_W, ROW_STATS_W, ROW_VALUE_W, TILE_STATS_H, compose_as, compose_styled,
    row_forms,
};
use super::grid::is_page;
use super::look::{ChartLook, PageChart, accent};
use super::meta::{Lines, ValueText};
use super::source::{Availability, SourceData};
use crate::overview::fit::fit_text;
use crate::overview::gpu_flow::line_style;
use crate::overview::mem_box::{MemRowsRef, MeterRow, mem_cell_part, mem_row_part};
use crate::overview::net_box::{NetModelRef, net_combined_element, net_direction_element};
use crate::overview::parts::{
    ChartSettings, CoreBarsLayout, CoreCellFit, METER_H, Meter, PLOT_LINE, SPARK_GRID, box_chart,
    chart_section_versions, class_probe, core_bars_layout_marked, core_cell, spark_row,
};
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::{Strings, tr};
use tl_ui_base::selector::TlSelector;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::text::style::{numeric_style, t_lg, t_sm, t_xs};
use tl_ui_base::theme::{TlTheme, UiMetrics, current, metrics};
use tl_ui_chart::chart::labels::LABEL_ROW;
use tl_ui_chart::chart::{ChartBg, ChartSpec, ChartView, GridStyle, ValueFormat, YScale};
use tl_ui_kit::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use tl_ui_kit::components::validity::copy_text;
use tl_ui_perf_core::core::actions::PerfShowMiniWindow;
use tl_ui_perf_core::core::core_clocks::snapshot_line_set;

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

/// The space above and below a core row's sparkline, logical px (`parts::core_sparks`).
const CORE_SPARK_INSET: f32 = 2.;

/// The debug selector of the root of widget `id` in container `container` (ADR-019: widget
/// ids repeat across containers, so every widget selector has the container id).
pub fn widget_id(container: &str, id: u32) -> String {
    format!("perf-widget-{container}-{id}")
}

/// The debug selector of a part of widget `id` in container `container`: "name", "value",
/// "stats", "bar", "chart" or "reason" (the unavailable line).
pub fn part_id(container: &str, id: u32, part: &str) -> String {
    format!("perf-widget-{container}-{id}-{part}")
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
    /// A Memory box row (UI-PRF-017): row `index`'s label, value, meter and percent from the
    /// box's model, on one or two lines as the box's row geometry says. In `rows` the row's
    /// sparkline sits beside it (`spark`), the text part taking the geometry's share.
    MemRow {
        rows: MemRowsRef,
        index: usize,
        spark: Option<Entity<ChartView>>,
    },
    /// A `charts` cell of the Memory box (D-029): row `index`'s name, value and percent over
    /// its chart.
    MemCell {
        rows: MemRowsRef,
        index: usize,
        chart: Entity<ChartView>,
    },
    /// A Network box part of `stacked` and `columns` (UI-PRF-017): the Download (`upload`
    /// false) or Upload rate line from the box's model, with its 60 s peak and its total since
    /// boot in the tooltip, over the box's chart of that direction.
    NetDirection {
        model: NetModelRef,
        upload: bool,
        chart: Entity<ChartView>,
    },
    /// The Network box's `combined` body: both rate lines from the box's model over the box's
    /// chart with both lines.
    NetCombined {
        model: NetModelRef,
        chart: Entity<ChartView>,
    },
}

impl FlowLook {
    /// The parts the look reads from the source.
    fn forms(&self) -> FormSet {
        match self {
            Self::Cell { .. } => FormSet::LABEL | FormSet::VALUE,
            Self::Row { .. } => FormSet::LABEL | FormSet::VALUE | FormSet::BAR,
            // The Memory and Network looks read the box's model, not the source.
            Self::Chart(_)
            | Self::MemRow { .. }
            | Self::MemCell { .. }
            | Self::NetDirection { .. }
            | Self::NetCombined { .. } => FormSet::empty(),
            Self::Text => FormSet::VALUE,
        }
    }

    fn label(&self) -> SharedString {
        match self {
            Self::Cell { label, .. } | Self::Row { label, .. } => label.clone(),
            Self::Chart(_)
            | Self::Text
            | Self::MemRow { .. }
            | Self::MemCell { .. }
            | Self::NetDirection { .. }
            | Self::NetCombined { .. } => SharedString::default(),
        }
    }
}

/// How a widget draws one member of a group widget (`group_widget.rs`): the composition every
/// item of the group uses, whatever the item's size, and whether the item keeps the edit grip
/// and gear's width free at the start of its first line. The group keeps the outline and corner
/// margins itself, so the item gets no other edit margin.
///
/// `core` draws a Row as a core row of the CPU box (`parts::core_board`, D-040): name,
/// sparkline, meter, percent and the clock column the group hands in ([`MetricWidget::set_clock`]),
/// dropping parts by the core rows' fit rules. `mark` widens the clock column for the BogoMIPS
/// mark. A cores group of `load` in Rows draws its items so, in every layout (tasklens-6swdr.12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemberLook {
    pub composition: Option<Composition>,
    pub lead: bool,
    pub core: bool,
    pub mark: bool,
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
    /// The tooltip on the page chart's heading (`SourceData::tip`), while the name shows.
    tip: Option<SharedString>,
}

/// What the chart's spec and labels are built from: the metric's lines, the widget's options and
/// the source's series (`look.rs`), the scale and the value format.
#[derive(Clone, Debug, PartialEq)]
struct ChartKey {
    look: ChartLook,
    y: YScale,
    format: ValueFormat,
    /// The page chart parts of a page style Cell; `None` in a box and in a Row.
    page: Option<PageChart>,
}

/// One widget of a container.
pub struct MetricWidget {
    setting: WidgetSetting,
    /// The id of the container the widget is in, for its debug selectors.
    container: SharedString,
    /// The container is a device page: the Cell draws in the page style.
    page: bool,
    hub: Entity<SnapshotHub>,
    source: Entity<SourceData>,
    /// Whether the name has the GPU's name in front (the box shows two GPUs).
    device_prefix: bool,
    /// Edit Widgets is on: the first line keeps the grip and the gear's width free at its start.
    edit_inset: bool,
    /// How the widget draws as a member of a group widget.
    member: MemberLook,
    shown: Shown,
    chart: Option<Entity<ChartView>>,
    chart_key: Option<ChartKey>,
    /// Whether the chart is styled for a Cell (outlined) or a Row (sparkline).
    chart_cell: bool,
    /// The 1.1.0 part the widget draws, while its host draws its flow layout.
    flow: Option<FlowLook>,
    /// The clock column of a core row ([`MemberLook::core`]): "3.9 GHz", from the group.
    clock: SharedString,
    /// The bar's colour: the accent of the source's device (`look::accent`).
    bar_color: Hsla,
    /// The widget's size at its last paint, or the size the container gave.
    size: Rc<Cell<Size<Pixels>>>,
    composed: Composed,
    /// The core row fit of the last render, as probe class bits ([`core_class`]); 0 for other
    /// widgets.
    core_bits: u32,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    _source_sub: Subscription,
    _subs: [Subscription; 3],
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl MetricWidget {
    /// A widget for `setting` on `source` (from the page's `SourceStore`) in container
    /// `container`.
    pub fn new(
        container: SharedString,
        setting: WidgetSetting,
        source: Entity<SourceData>,
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_flow(container, setting, source, hub, None, cx)
    }

    /// A widget that draws `flow` (see [`FlowLook`]) from the start, so it builds no chart.
    pub fn with_flow(
        container: SharedString,
        setting: WidgetSetting,
        source: Entity<SourceData>,
        hub: &Entity<SnapshotHub>,
        flow: Option<FlowLook>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            setting,
            page: is_page(&container),
            container,
            hub: hub.clone(),
            _source_sub: cx.observe(&source, Self::on_source),
            source,
            device_prefix: false,
            edit_inset: false,
            member: MemberLook::default(),
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
                tip: None,
            },
            chart: None,
            chart_key: None,
            chart_cell: true,
            flow,
            clock: SharedString::default(),
            bar_color: current(cx).graph.gpu_line,
            size: Rc::new(Cell::new(size(px(0.), px(0.)))),
            composed: Composed {
                composition: Composition::Row,
                forms: FormSet::empty(),
            },
            core_bits: 0,
            settings: ChartSettings::read(cx),
            settings_versions: chart_section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
            _subs: [
                cx.observe_global::<TlSettings>(Self::on_settings),
                cx.observe_global::<UiMetrics>(Self::on_style),
                cx.observe_global::<Strings>(Self::on_strings),
            ],
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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

    /// The host changed what its look draws (a Memory row's text or meter): one render of this
    /// widget. Without a look nothing happens.
    pub fn flow_changed(&mut self, cx: &mut Context<Self>) {
        if self.flow.is_some() {
            self.notify(cx);
        }
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

    /// The widget is a member of a group widget and draws as `look` says. It renders again only
    /// when the look changed.
    pub fn set_member(&mut self, look: MemberLook, cx: &mut Context<Self>) {
        if self.member == look {
            return;
        }
        self.member = look;
        self.shown = self.read_shown(cx);
        self.sync_chart(cx);
        self.notify(cx);
    }

    /// The clock column of a core row ("3.9 GHz"). It renders again only when the text changed.
    pub fn set_clock(&mut self, clock: SharedString, cx: &mut Context<Self>) {
        if self.clock == clock {
            return;
        }
        self.clock = clock;
        if self.member.core {
            self.notify(cx);
        }
    }

    /// The clock column's text, as shown on a core row.
    pub fn clock_text(&self) -> &SharedString {
        &self.clock
    }

    /// The left margin of the first text line: the grip and the gear in Edit Widgets, past the
    /// edge the parts keep free, else 0.
    fn lead(&self, s: f32) -> Pixels {
        px(if self.edit_inset || self.member.lead {
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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
                self.member,
                self.page,
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
            tip: kept
                .contains(FormSet::LABEL)
                .then(|| src.tip().cloned())
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
            tip: None,
        }
    }

    /// Creates, updates or drops the chart to match what is shown. Returns whether the chart
    /// was created or dropped, which the widget must render; a new spec only renders the chart.
    /// What the chart is built from now, while the widget shows a chart.
    fn chart_key(&self, cx: &App) -> Option<ChartKey> {
        self.shown.series?;
        let src = self.source.read(cx);
        let meta = src.meta()?;
        let page = (self.page && self.chart_cell).then(|| PageChart {
            name: src.name().clone(),
            forms: self.shown.kept & (FormSet::LABEL | FormSet::VALUE),
            sum: meta.value_text_in(true) == ValueText::Sum,
        });
        Some(ChartKey {
            page,
            look: ChartLook {
                kind: src.kind(),
                metric: src.selector().metric.0.clone(),
                lines: meta.lines,
                smoothed: meta.smoothed,
                options: self.setting.options,
                kernel: self.settings.kernel,
                series: src.series_list().iter().copied().collect(),
                minus: src.minus_list().iter().copied().collect(),
                page: self.page,
            },
            y: src.y_scale(),
            format: meta.format,
        })
    }

    /// Brings the chart up to date with the source: its spec and labels ([`Self::sync_spec`])
    /// and the state it shows over its data (a failed collector on a device page,
    /// `SourceData::chart_state`). True when a chart was made or dropped. A new state renders
    /// only the chart, and a new chart takes the state before its first paint.
    fn sync_chart(&mut self, cx: &mut Context<Self>) -> bool {
        let made = self.sync_spec(cx);
        if let Some(chart) = &self.chart {
            let state = self.source.read(cx).chart_state();
            let page = self.chart_key.as_ref().is_some_and(|k| k.page.is_some());
            let text = page.then(|| self.page_value_text(cx)).flatten();
            // Core clocks names each core's line, with its peak and the base clock line, as
            // the CPU page's Core clocks chart (tasklens-hzire.5). The chart keeps an equal set.
            let lines = self
                .chart_key
                .as_ref()
                .is_some_and(|k| k.look.lines == Lines::PerCore)
                .then(|| snapshot_line_set(self.hub.read(cx).current(), cx));
            chart.update(cx, |v, cx| {
                v.set_state(state, cx);
                if page {
                    v.set_value_text(text, cx);
                }
                if lines.is_some() {
                    v.set_line_set(lines, cx);
                }
            });
        }
        made
    }

    /// The value a page chart shows at the right of its heading when it is not the newest
    /// sample of its line: a value of a total ("2.80 GiB / 15.92 GiB", "31.9 / 300 W",
    /// "1905 / 3105 MHz"), as the 1.3.0 GPU page charts show it. `None` lets the chart write
    /// its newest sample, as it does while the value is not known.
    fn page_value_text(&self, cx: &App) -> Option<SharedString> {
        let src = self.source.read(cx);
        let meta = src.meta()?;
        (meta.value_text_in(true) == ValueText::OfTotal && src.value().is_some())
            .then(|| src.value_text().clone())
            .filter(|t| !t.is_empty())
    }

    fn sync_spec(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.chart_key(cx) else {
            self.chart_key = None;
            return self.chart.take().is_some();
        };
        if self.chart_key.as_ref() == Some(&key) && self.chart.is_some() {
            return false;
        }
        // The spec and the labels follow the key: a new source can bring a new value format
        // (percent to bytes), a new line count or the kernel line.
        let spec = chart_spec(&key, self.chart_cell, &self.settings, current(cx));
        let labels = match &key.page {
            Some(page) => key.look.page_labels(key.format, page, cx),
            None => key.look.labels(key.format, self.chart_cell, cx),
        };
        let sum = key.page.as_ref().is_some_and(|p| p.sum);
        // A page chart and a box chart are made differently: a change between them makes a new
        // chart.
        let same_kind = self
            .chart_key
            .as_ref()
            .is_some_and(|k| k.page.is_some() == key.page.is_some());
        let page = key.page.is_some();
        self.chart_key = Some(key);
        match &self.chart {
            Some(chart) if same_kind => {
                chart.update(cx, |v, cx| {
                    v.set_spec(spec, cx);
                    v.set_labels(labels, cx);
                    v.set_value_sum(sum, cx);
                });
                false
            }
            _ => {
                let hub = self.hub.clone();
                self.chart = Some(if page {
                    page_chart(spec, labels, sum, &hub, cx)
                } else {
                    box_chart(spec, labels, &hub, cx)
                });
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
        self.rebuild_chart(cx);
    }

    /// Builds the chart's spec and labels again from the current settings, theme and strings.
    fn rebuild_chart(&mut self, cx: &mut Context<Self>) {
        if self.chart.is_some() {
            self.chart_key = None;
            self.sync_chart(cx);
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
        self.rebuild_chart(cx);
        self.refresh(cx);
    }

    /// The probe class of the last render.
    fn class(&self) -> u32 {
        if self.flow.is_some() {
            FLOW_CLASS
        } else if self.shown.availability == Availability::Available {
            self.composed.class() | self.core_bits
        } else {
            UNAVAILABLE_CLASS
        }
    }

    /// The probe class at `area`.
    fn class_at(&self, area: Size<Pixels>, scale: f32) -> u32 {
        if self.flow.is_some() {
            FLOW_CLASS
        } else if self.shown.availability == Availability::Available {
            probe_class(
                self.shown.forms,
                area,
                scale,
                self.edit_inset,
                self.member,
                self.page,
            )
        } else {
            UNAVAILABLE_CLASS
        }
    }
}

/// The composition of `forms` at `area`, or the composition a group member keeps. In Edit
/// Widgets (`edit`) a Row drops its parts by the width left inside the edit margins, so no part
/// reaches under the resize corner; the composition itself is the applied view's. A group
/// member with the edit lead drops them by the width right of the grip and the gear.
fn compose_at(
    forms: FormSet,
    area: Size<Pixels>,
    scale: f32,
    edit: bool,
    member: MemberLook,
    page: bool,
) -> Composed {
    let composed = match member.composition {
        Some(c) => compose_as(c, forms, area, scale),
        None => compose_styled(forms, area, scale, page),
    };
    if member.core && composed.composition == Composition::Row {
        return core_composed(forms, area, scale, member);
    }
    if !(edit || member.lead) || composed.composition != Composition::Row {
        return composed;
    }
    let s = sane(scale);
    // Without a name the whole line starts right of the grip and the gear.
    let lead = if forms.contains(FormSet::LABEL) {
        0.
    } else {
        EDIT_INSET * s - EDIT_EDGE
    };
    let margins = if edit {
        2. * EDIT_EDGE + EDIT_RIGHT * s
    } else {
        0.
    };
    let inner = f32::from(area.width) - margins - lead;
    // Whatever is left must hold the value, which a Row never drops (`compose::row_forms`).
    Composed {
        composition: Composition::Row,
        forms: row_forms(composed.forms, inner.max(0.) / s),
    }
}

fn sane(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    }
}

/// The width a core row's first line keeps free for the edit grip and gear, px.
fn core_lead(member: MemberLook, s: f32) -> f32 {
    if member.lead {
        EDIT_INSET * s - EDIT_EDGE
    } else {
        0.
    }
}

/// The core row layout of one item at `area`: one cell as wide as the item right of the lead,
/// at least one row high, by the core rows' fit rules (`parts::core_bars_layout_marked`).
fn core_layout(area: Size<Pixels>, s: f32, member: MemberLook) -> CoreBarsLayout {
    let w = (f32::from(area.width) - core_lead(member, s)).max(0.);
    let h = f32::from(area.height).max(LABEL_ROW * s);
    core_bars_layout_marked(1, px(w), px(h), s, member.mark)
}

/// What a core row keeps at `area`: the name and the percent always, the sparkline and the
/// meter while the fit has room for them.
fn core_composed(forms: FormSet, area: Size<Pixels>, scale: f32, member: MemberLook) -> Composed {
    let s = sane(scale);
    let parts = core_cell(&core_layout(area, s, member), s);
    let mut kept = FormSet::LABEL | FormSet::VALUE;
    kept.set(FormSet::CHART, parts.spark_w > px(0.));
    kept.set(FormSet::BAR, parts.meter_w > px(0.));
    Composed {
        composition: Composition::Row,
        forms: forms & kept,
    }
}

/// The probe class bits of a core row's fit at `area`: the clock column comes and goes inside
/// one set of kept forms. 0 for other widgets.
fn core_class(forms: FormSet, area: Size<Pixels>, scale: f32, member: MemberLook) -> u32 {
    if !member.core || member.composition != Some(Composition::Row) || forms.is_empty() {
        return 0;
    }
    let fit = core_layout(area, sane(scale), member).fit;
    let n = match fit {
        CoreCellFit::Full => 1,
        CoreCellFit::NoGhz => 2,
        CoreCellFit::SparkShrinks => 3,
        CoreCellFit::MeterShrinks => 4,
        CoreCellFit::NoSpark => 5,
    };
    n << 12
}

/// The probe class of `forms` at `area`: the composition and kept forms, and a core row's fit.
fn probe_class(
    forms: FormSet,
    area: Size<Pixels>,
    scale: f32,
    edit: bool,
    member: MemberLook,
    page: bool,
) -> u32 {
    compose_at(forms, area, scale, edit, member, page).class()
        | core_class(forms, area, scale, member)
}

/// The chart of a widget: its series from the metric, the widget's options and the source
/// (`look.rs`), in its device's accent. A Cell's chart has grid lines and an outline; a Row's
/// sparkline has the sparkline grid.
fn chart_spec(key: &ChartKey, cell: bool, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    let (series, stack) = key.look.series(theme);
    // A page chart is drawn as the 1.3.0 page charts are (`disk::spec`): on the window
    // background, with the hover line.
    let page = key.page.is_some();
    ChartSpec {
        series,
        stack,
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
        background: if page { ChartBg::Window } else { ChartBg::List },
        hover: page,
    }
}

/// The chart of a page style Cell: a 1.3.0 page chart (`aggregate::chart_box`), with the
/// heading row and the range text inside it and the device history at 1 h, 24 h and 7 d. With
/// `sum` its value is the sum of its lines' newest samples.
fn page_chart<T: 'static>(
    spec: ChartSpec,
    labels: tl_ui_chart::chart::ChartLabels,
    sum: bool,
    hub: &Entity<SnapshotHub>,
    cx: &mut Context<T>,
) -> Entity<ChartView> {
    cx.new(|cx| {
        let snap = std::sync::Arc::clone(hub.read(cx).current());
        let mut view = ChartView::new(spec, labels, hub, cx)
            .with_newest_marks()
            .with_device_history(&snap);
        view.set_value_sum(sum, cx);
        view
    })
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
    el.tl_selector(move || id)
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
    fn meter(&self, id: u32, _theme: &TlTheme) -> gpui_kit::Div {
        let p = part_id(&self.container, id, "bar");
        div().tl_selector(move || p).w_full().child(Meter {
            fraction: self.shown.fraction,
            color: self.bar_color,
        })
    }

    /// A core row of the CPU box (`parts::core_board`): the name at the left, the sparkline,
    /// the meter, the percent and the clock at the right, placed by the core rows' fit at the
    /// row's width. The meter and the sparkline grow with a taller row (D-040); the text stays
    /// centred. Parts sit at offsets that stay put inside one fit, so a resize in one probe
    /// class needs no render.
    fn core_row(&self, forms: FormSet, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        let (id, s) = (self.setting.id, m.ui_scale);
        let lead = px(core_lead(self.member, s));
        let l = core_layout(self.size.get(), s, self.member);
        let parts = core_cell(&l, s);
        let cell = l.cell_w;
        let mut el = div().relative().size_full();
        // The text style goes first: setting it replaces the nowrap and colour set before it.
        let line = |part: &str, style: gpui_kit::TextStyleRefinement| {
            let p = part_id(&self.container, id, part);
            let mut el = div();
            *el.text_style() = style;
            el.tl_selector(move || p)
                .absolute()
                .top_0()
                .bottom_0()
                .flex()
                .items_center()
                .whitespace_nowrap()
                .text_color(theme.text_secondary)
        };
        if forms.contains(FormSet::LABEL) {
            let name = line("name", t_xs(m)).left(lead);
            el = el.child(name.child(self.shown.name.clone()));
        }
        if forms.contains(FormSet::CHART) && parts.spark_w > px(0.) {
            let p = part_id(&self.container, id, "chart");
            let inset = px(CORE_SPARK_INSET * s);
            let mut spark = div()
                .tl_selector(move || p)
                .absolute()
                .top(inset)
                .bottom(inset)
                .left(lead + parts.spark_x);
            spark = if l.fit == CoreCellFit::SparkShrinks {
                spark.right(cell - parts.spark_x - parts.spark_w)
            } else {
                spark.w(parts.spark_w)
            };
            if let Some(chart) = &self.chart {
                spark = spark.child(cached(chart));
            }
            el = el.child(spark);
        }
        if forms.contains(FormSet::BAR) && parts.meter_w > px(0.) {
            let p = part_id(&self.container, id, "bar");
            let right = cell - parts.meter_x - parts.meter_w;
            let mut meter = div()
                .tl_selector(move || p)
                .absolute()
                .top_0()
                .bottom_0()
                .right(right)
                .flex()
                .items_center();
            meter = if l.fit == CoreCellFit::SparkShrinks {
                meter.w(parts.meter_w)
            } else {
                meter.left(lead + parts.meter_x)
            };
            let fill = self.shown.fraction.map(|f| f.clamp(0., 1.));
            let track = div()
                .relative()
                .w_full()
                .min_h(px(METER_H * s))
                .h(relative(METER_H / LABEL_ROW))
                .rounded(m.radius_small)
                .bg(theme.divider)
                .when_some(fill.filter(|f| *f > 0.), |d, f| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(f))
                            .rounded(m.radius_small)
                            .bg(self.bar_color),
                    )
                });
            el = el.child(meter.child(track));
        }
        if forms.contains(FormSet::VALUE) {
            let pct = line("value", numeric_style(m, m.text.xs))
                .right(cell - parts.pct_right)
                .justify_end();
            el = el.child(pct.child(self.shown.value.clone()));
        }
        if parts.ghz {
            let clock = line("clock", numeric_style(m, m.text.xs))
                .right_0()
                .justify_end();
            el = el.child(clock.child(self.clock.clone()));
        }
        el
    }

    /// The hover area over a page chart's heading that shows the name's tooltip (the GPU
    /// Power and Clocks texts, or the GPU probe's text), as the 1.3.0 GPU page charts do
    /// (UI-PRF-032). Only on a page style Cell with its name, outside Edit Widgets.
    fn heading_tip(
        &self,
        available: bool,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let text = self.shown.tip.clone()?;
        if !self.page
            || !available
            || self.edit_inset
            || self.composed.composition != Composition::Cell
        {
            return None;
        }
        let s = m.ui_scale;
        let kept = self.composed.forms;
        let mut top = 0.;
        if kept.contains(FormSet::STATS) {
            top += CELL_STATS_H;
        }
        if kept.contains(FormSet::BAR) {
            top += METER_H + 2. * CELL_BAR_PAD;
        }
        let id = part_id(&self.container, self.setting.id, "tip");
        let trigger = div()
            .tl_selector({
                let id = id.clone();
                move || id
            })
            .absolute()
            .top(px(top * s))
            .left_0()
            .w_1_2()
            .h(px(LABEL_ROW * s) + m.sp[3]);
        let state = HoverTooltip::state(SharedString::from(id), window, cx);
        Some(
            HoverTooltip::attach(
                &state,
                trigger,
                DELAY_CONTROL,
                TlTooltip::text(text).build(),
                window,
                cx,
            )
            .into_any_element(),
        )
    }

    fn chart_part(&self, id: u32) -> gpui_kit::Div {
        let p = part_id(&self.container, id, "chart");
        let mut el = div().tl_selector(move || p).relative();
        if let Some(chart) = &self.chart {
            el = el.child(cached(chart));
        }
        el
    }

    /// The page style Cell: the stats line and the bar, then the page chart, which draws the
    /// name, the value, the legend and the range text itself. In Edit Widgets the chart starts
    /// under the grip and the gear, so they cover none of its heading.
    fn page_cell(&self, forms: FormSet, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        let (id, s) = (self.setting.id, m.ui_scale);
        let mut el = div().flex().flex_col().size_full();
        if self.edit_inset {
            el = el.pt(px(super::edit::HANDLE * s - EDIT_EDGE));
        }
        if forms.contains(FormSet::STATS) {
            let stats = self.shown.stats.clone();
            el = el.child(
                text_part(
                    part_id(&self.container, id, "stats"),
                    stats,
                    t_xs(m),
                    theme.text_secondary,
                )
                .flex_none()
                .h(px(CELL_STATS_H * s)),
            );
        }
        if forms.contains(FormSet::BAR) {
            el = el.child(self.meter(id, theme).flex_none().py(px(CELL_BAR_PAD * s)));
        }
        el.child(self.chart_part(id).flex_1().min_h_0().w_full())
    }

    /// The Cell: header line, stats line, bar, then the chart filling the rest.
    fn cell(&self, forms: FormSet, m: &UiMetrics, theme: &TlTheme) -> gpui_kit::Div {
        if self.page {
            return self.page_cell(forms, m, theme);
        }
        let (id, s) = (self.setting.id, m.ui_scale);
        let mut el = div().flex().flex_col().size_full();
        let mut lead = self.lead(s);
        // With no text line above it, the chart's legend is the first text line. In Edit
        // Widgets the chart then starts under the grip and the gear, as the page style Cell
        // does, so they cover none of its legend (tasklens-mo5t5).
        let text_first = forms.intersects(FormSet::LABEL | FormSet::VALUE | FormSet::STATS);
        if lead > px(0.) && !text_first && forms.contains(FormSet::CHART) {
            el = el.pt(px(super::edit::HANDLE * s - EDIT_EDGE));
        }
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
                    text_part(
                        part_id(&self.container, id, "name"),
                        name,
                        t_sm(m),
                        theme.text_secondary,
                    )
                    .flex_1()
                    .min_w_0(),
                );
            } else {
                header = header.child(div().flex_1());
            }
            if forms.contains(FormSet::VALUE) {
                let value = self.shown.value.clone();
                let p = part_id(&self.container, id, "value");
                let mut v = div();
                *v.text_style() = value_style(m, m.text.sm);
                header = header.child(
                    v.tl_selector(move || p)
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
                text_part(
                    part_id(&self.container, id, "stats"),
                    stats,
                    t_xs(m),
                    theme.text_secondary,
                )
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
                text_part(
                    part_id(&self.container, id, "name"),
                    name,
                    t_sm(m),
                    theme.text_secondary,
                )
                .ml(lead),
            );
        }
        if forms.contains(FormSet::VALUE) {
            let mut style = t_lg(m);
            style.font_features = numeric_style(m, m.text.lg).font_features;
            let value = self.shown.value.clone();
            el = el.child(
                // The name line can be a pixel shorter than the gear, so the value keeps clear
                // of the gear too.
                text_part(
                    part_id(&self.container, id, "value"),
                    value,
                    style,
                    theme.text_primary,
                )
                .ml(lead),
            );
        }
        if forms.contains(FormSet::STATS) {
            let stats = self.shown.stats.clone();
            el = el.child(
                text_part(
                    part_id(&self.container, id, "stats"),
                    stats,
                    t_xs(m),
                    theme.text_secondary,
                )
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
            el = el.child(
                div().flex_none().w(px((ROW_NAME_W - ROW_GAP) * s)).child(
                    text_part(
                        part_id(&self.container, id, "name"),
                        name,
                        t_sm(m),
                        theme.text_secondary,
                    )
                    .ml(lead),
                ),
            );
        }
        let bar = forms.contains(FormSet::BAR);
        if bar {
            el = el.child(self.meter(id, theme).flex_1().min_w(px(ROW_BAR_MIN_W * s)));
        }
        if forms.contains(FormSet::VALUE) {
            let value = self.shown.value.clone();
            let p = part_id(&self.container, id, "value");
            let mut v = div();
            *v.text_style() = numeric_style(m, m.text.sm);
            let mut v = v
                .tl_selector(move || p)
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
                text_part(
                    part_id(&self.container, id, "stats"),
                    stats,
                    t_xs(m),
                    theme.text_secondary,
                )
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
                part_id(&self.container, id, "name"),
                name,
                t_sm(m),
                theme.text_secondary,
            ))
            .child(text_part(
                part_id(&self.container, id, "reason"),
                reason,
                t_sm(m),
                theme.text_secondary,
            ))
    }
}

impl MetricWidget {
    /// The 1.1.0 part of `look`. Every part but the text fills the place the host gives.
    fn flow_part(
        &self,
        look: &FlowLook,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
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
                let p = part_id(&self.container, id, "name");
                let name = name
                    .tl_selector(move || p)
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(theme.text_secondary)
                    .child(self.shown.name.clone());
                let mut value = div();
                *value.text_style() = numeric_style(m, m.text.sm);
                let p = part_id(&self.container, id, "value");
                let value = value
                    .tl_selector(move || p)
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
                            .tl_selector(move || chart_id)
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
            FlowLook::MemRow { rows, index, spark } => {
                let geom = rows.1.get();
                let spark = spark.as_ref().map(|c| (c, geom.share));
                mem_row_part(&rows.0.borrow(), *index, geom.wide, spark, m, theme)
            }
            FlowLook::MemCell { rows, index, chart } => {
                mem_cell_part(&rows.0.borrow(), *index, chart, m, theme)
            }
            FlowLook::NetDirection {
                model,
                upload,
                chart,
            } => net_direction_element(model, *upload, chart, m, theme, window, cx),
            FlowLook::NetCombined { model, chart } => {
                net_combined_element(model, chart, m, theme, window, cx)
            }
            FlowLook::Text => {
                // The line's style, with tabular digits: the box places the value at the
                // width of its shape in this style (tasklens-lhliq.7).
                let mut text = div();
                *text.text_style() = line_style(m);
                let p = part_id(&self.container, id, "value");
                text.tl_selector(move || p)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        if let Some(look) = self.flow.clone() {
            let part = self.flow_part(&look, &m, &theme, window, cx);
            let id = widget_id(&self.container, self.setting.id);
            let root = div().tl_selector(move || id).relative();
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
        self.bar_color = {
            let src = self.source.read(cx);
            accent(src.kind(), src.selector().metric.as_str(), &theme).line
        };
        let body = if available {
            self.composed = compose_at(
                forms,
                self.size.get(),
                scale,
                self.edit_inset,
                self.member,
                self.page,
            );
            self.core_bits = core_class(forms, self.size.get(), scale, self.member);
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
                Composition::Row if self.member.core => self.core_row(kept, &m, &theme),
                Composition::Row => self.row(kept, &m, &theme),
            }
        } else {
            self.unavailable(&m, &theme)
        };
        let class = self.class();
        let view = cx.entity_id();
        let edit = self.edit_inset;
        let member = self.member;
        let page = self.page;
        let probe = class_probe(
            Rc::clone(&self.size),
            class,
            move |area| {
                if available {
                    probe_class(forms, area, scale, edit, member, page)
                } else {
                    UNAVAILABLE_CLASS
                }
            },
            view,
        );
        let id = widget_id(&self.container, self.setting.id);
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
        let mut root = div()
            .tl_selector(move || id)
            .relative()
            .size_full()
            .overflow_hidden()
            .child(body)
            .child(probe);
        if let Some(trigger) = self.heading_tip(available, &m, window, cx) {
            root = root.child(trigger);
        }
        // A double-click on a page chart opens the mini window, as the 1.3.0 graph area does
        // (ADR-019 phase 3 "Double-click"). Not in Edit Widgets.
        if self.page && !self.edit_inset && self.chart.is_some() {
            let el_id = SharedString::from(widget_id(&self.container, self.setting.id));
            return root
                .id(el_id)
                .on_click(|ev, window, cx| {
                    if ev.click_count() == 2 {
                        window.dispatch_action(Box::new(PerfShowMiniWindow), cx);
                    }
                })
                .into_any_element();
        }
        root.into_any_element()
    }
}
