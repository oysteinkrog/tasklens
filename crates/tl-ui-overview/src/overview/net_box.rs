//! The Overview page's Network box (UI-PRF-017, D-022, UI-PRF-032, PERF-054): the download and
//! upload charts of all interfaces, with the summed rates and the 60 s peaks on one line per
//! direction ("1.4 Mbps, peak 4.4 Mbps"). That line's tooltip gives the total since boot
//! (tasklens-o55.54). The busiest interface's name is on the title line.
//!
//! The rates and totals sum the present interfaces other than `lo` whose class the
//! `show_network_*` settings show. The charts and peaks read the engine's total of every class
//! minus the per-class totals of the hidden classes (`NetTotal`, tasklens-q4x76.22). The box formats its
//! text only when the inputs' hash changes. Pointer movement never notifies.
//!
//! UI-PRF-019 gives the box four modes. `stacked` (the default) is Download over Upload with a
//! horizontal splitter. `columns` is the Download and Upload columns above with a vertical
//! splitter between them. Their Arrange choice (D-027) switches between the two. `combined` is
//! one chart with download and upload as two lines, under the rate line of both directions.
//! `adapter_charts` is a grid of one combined chart per counted adapter, in cells of at least
//! 200 × 80 px. The adapter charts are built when the mode is shown and when the set of counted
//! adapters changes, never per tick.
//!
//! D-041, ADR-019 phase 2: the modes are presets of the box's widget layout. A preset is a flow
//! layout (`net_flow.rs`): the `rx` and `tx` widgets of the counted total in `stacked` and
//! `columns`, the `traffic` widget in `combined`, and the adapters group in `adapter_charts`.
//! Each widget draws its 1.2.0 part from the [`NetModel`] the box keeps (rates, peaks, totals,
//! adapter cells and "+n more") and the charts the box keeps: the Download and Upload parts
//! with `FlowLook::NetDirection`, the `combined` body with `FlowLook::NetCombined`, and the
//! adapter cells with `GroupFlowLook::NetCells`, which works out the whole cells that fit in
//! its own render. On a sample the box notifies only the widgets whose text changed, and itself
//! only when the busiest adapter on its title line changed. The box draws the title line and
//! the splitter between the Download and Upload parts. A grid layout (the user's own widgets,
//! after Apply) is drawn by the box's [`WidgetEditor`] and its `WidgetContainer`; so is the box
//! while Edit Widgets is on.

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
use std::cell::Cell;
use std::cell::{Cell as StdCell, RefCell};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EntityId, EventEmitter, FontWeight,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px, relative, size, white,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::{DeviceCatalog, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::Snapshot;
use tl_model::units;
use tl_model::validity::Validity;
use tl_ui_settings::layout::{ContainerLayout, NET_BOX_CONTAINER};
use tl_ui_settings::settings::{NetworkUnit, Settings};

use super::cpu_box::value_text;
use super::disk_box::more_line;
use super::fit::{fit_text, whole_rows};
use super::inner_split::{InnerSplit, InnerSplitChanged, SplitAxis};
use super::layout::{Arrange, ArrangeChoice, BoxMode};
use super::net_flow::{self, NetFlowKeys, NetLook, NetPlan, PlanCell};
use super::parts::{
    ChartSettings, ListFit, PLOT_LINE, box_chart, box_frame, chart_section_versions, class_probe,
    edit_chrome, grid_cells, list_fit,
};
use super::picked::{box_mask, box_snapshot};
use super::range::RangeText;
use super::timeline::{self, OverviewPause};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, NetFormat, ValueAlign,
    ValueFormat, YScale, format_value,
};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::core::DeviceKey;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::net_totals::NetTotal;
use crate::selector::TlSelector;
use crate::settings_global::{Section, TlSettings};
use crate::template::Template;
use crate::text::style::{label_style, numeric_style, t_xs};
use crate::theme::derived::blend_over;
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::widget::container::WidgetContainer;
use crate::widget::edit::{WidgetEditEvent, WidgetEditor};
use crate::widget::group_widget::GroupFlowLook;
use crate::widget::metric_widget::FlowLook;
use crate::widget::source::SourceStore;

/// The box's debug selector.
pub const NET_BOX_ID: &str = "perf-overview-box-net";

/// The box's layout modes (UI-PRF-019). `stacked` is the default since D-026: Download over
/// Upload. `columns` is the Download and Upload columns of UI-PRF-017.
///
/// The `stacked` split is the Download part's share of the body height, the `columns` split the
/// Download column's share of the width.
pub const MODES: &[BoxMode] = &[
    BoxMode {
        id: "stacked",
        label: "perf-overview-mode-stacked",
        split: Some(0.5),
    },
    BoxMode {
        id: "columns",
        label: "perf-overview-mode-columns",
        split: Some(0.5),
    },
    BoxMode {
        id: "combined",
        label: "perf-overview-mode-combined",
        split: None,
    },
    BoxMode {
        id: "adapter_charts",
        label: "perf-overview-mode-adapter-charts",
        split: None,
    },
];

/// The Arrange choice of the two-chart modes (UI-PRF-019, D-027): Stack vertically is
/// `stacked`, Side by side is `columns`.
const SWITCH: ArrangeChoice = ArrangeChoice::Modes {
    vertical: "stacked",
    horizontal: "columns",
};

/// The modes with an Arrange choice: `adapter_charts` saves its own, one column by default.
pub const ARRANGE: &[(&str, ArrangeChoice)] = &[
    ("stacked", SWITCH),
    ("columns", SWITCH),
    ("adapter_charts", ArrangeChoice::Saved(Arrange::Vertical)),
];

/// The smallest `adapter_charts` cell, logical px before `ui_scale`.
const GRID_CELL_MIN_W: f32 = 200.;
const GRID_CELL_MIN_H: f32 = 80.;

/// The debug selector of the `adapter_charts` grid viewport.
pub const GRID_ID: &str = "perf-overview-net-grid";

/// The gap between the Download and Upload parts in `stacked`, as an `sp` step: 6 px
/// (tasklens-o55.54).
const STACK_GAP: usize = 3;

/// The debug selector of the splitter between the stacked Download and Upload parts.
pub const NET_STACK_SPLITTER_ID: &str = "perf-overview-net-stack-splitter";
/// The debug selector of that splitter's accent line.
pub const NET_STACK_SPLITTER_LINE_ID: &str = "perf-overview-net-stack-splitter-line";
/// The debug selector of the splitter between the Download and Upload columns.
pub const NET_SPLITTER_ID: &str = "perf-overview-net-splitter";
/// The debug selector of the splitter's accent line.
pub const NET_SPLITTER_LINE_ID: &str = "perf-overview-net-splitter-line";
/// The debug selectors of the Download and Upload rate lines in `stacked`, `columns` and
/// `combined`: the rate, its peak, and the total since boot in the tooltip.
pub const DOWN_RATE_ID: &str = "perf-overview-net-down-rate";
pub const UP_RATE_ID: &str = "perf-overview-net-up-rate";
/// The debug selector of the `combined` chart.
pub const COMBINED_CHART_ID: &str = "perf-overview-net-combined-chart";

/// The debug selector of "No widgets. Use Edit Widgets to add some." (UI-PRF-061).
pub const EMPTY_ID: &str = "perf-overview-net-widgets-empty";

/// The most adapter charts the box builds; the rest count into "+n more".
pub const MAX_ADAPTER_CELLS: usize = 16;

/// Which parts the box shows (UI-PRF-019).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NetMode {
    Stacked,
    Columns,
    Combined,
    AdapterCharts,
}

impl NetMode {
    /// The mode `id`; an unknown id is the default.
    fn from_id(id: &str) -> Self {
        match id {
            "columns" => Self::Columns,
            "combined" => Self::Combined,
            "adapter_charts" => Self::AdapterCharts,
            _ => Self::Stacked,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Stacked => MODES[0].id,
            Self::Columns => MODES[1].id,
            Self::Combined => MODES[2].id,
            Self::AdapterCharts => MODES[3].id,
        }
    }

    fn look(self) -> NetLook {
        match self {
            Self::Stacked => NetLook::Stacked,
            Self::Columns => NetLook::Columns,
            Self::Combined => NetLook::Combined,
            Self::AdapterCharts => NetLook::AdapterCharts,
        }
    }
}

/// The page saves the share the box ended a drag or a double-click on.
impl EventEmitter<InnerSplitChanged> for NetBox {}

/// The debug selector of an adapter's `adapter_charts` cell, by its interface index.
pub fn cell_id(idx: DeviceIdx) -> String {
    format!("{NET_BOX_ID}-cell:{}", idx.0)
}

/// The debug selector of the adapter name in an `adapter_charts` cell header.
pub fn name_id(idx: DeviceIdx) -> String {
    format!("{NET_BOX_ID}-name:{}", idx.0)
}

/// The debug selector of the chart area of an adapter's `adapter_charts` cell, below its header.
pub fn chart_id(idx: DeviceIdx) -> String {
    format!("{NET_BOX_ID}-chart:{}", idx.0)
}

const KEY_TITLE: &str = "perf-overview-box-network";
const KEY_DOWNLOAD: &str = "perf-net-legend-download";
const KEY_UPLOAD: &str = "perf-net-legend-upload";
const KEY_PEAK: &str = "perf-overview-net-peak";
const KEY_TOTAL: &str = "perf-overview-net-total";
const KEY_WITH_PEAK: &str = "perf-overview-net-with-peak";
const KEY_MORE: &str = "perf-overview-more";
const KEY_RANGE: &str = "perf-net-heading-throughput";
const KEY_EMPTY: &str = "perf-widget-empty";

/// The upload line: the network colour blended with white, as the sidebar draws it.
pub(crate) const UPLOAD_TINT: f32 = 0.3;

/// The window of the peak rows: the last 60 s of the aggregate series.
const PEAK_WINDOW_NS: u64 = 60_000_000_000;

/// The loopback interface, never counted.
const LOOPBACK: &str = "lo";

/// The settings the box reads besides [`ChartSettings`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct NetSettings {
    unit: NetworkUnit,
    /// `units.network_auto_scale`: the rate rows print as every other network value (R-015).
    auto_scale: bool,
    wired: bool,
    wireless: bool,
    vpn: bool,
    virtual_: bool,
    other: bool,
}

impl NetSettings {
    fn from(s: &Settings) -> Self {
        let p = &s.performance;
        Self {
            unit: s.units.network,
            auto_scale: s.units.network_auto_scale,
            wired: p.show_network_wired,
            wireless: p.show_network_wireless,
            vpn: p.show_network_vpn,
            virtual_: p.show_network_virtual,
            other: p.show_network_other,
        }
    }

    /// Whether interfaces of `kind` are shown; no kind counts as Other (as `net_shown` in the
    /// engine reducer).
    fn shown(&self, kind: Option<NetKind>) -> bool {
        match kind.unwrap_or(NetKind::Other) {
            NetKind::Wired => self.wired,
            NetKind::Wireless => self.wireless,
            NetKind::VpnTunnel => self.vpn,
            NetKind::Virtual => self.virtual_,
            NetKind::Other => self.other,
        }
    }
}

/// The versions of the sections the box reads.
fn section_versions(cx: &gpui_kit::App) -> [u64; 4] {
    let [a, b, c] = chart_section_versions(cx);
    [a, b, c, TlSettings::get(cx).version(Section::Units)]
}

/// The window's network total in `catalog` for the classes `s` shows.
fn net_total(catalog: &DeviceCatalog, s: &Settings) -> NetTotal {
    NetTotal::of(catalog, &s.performance)
}

/// A chart's device and the per-class totals it takes away: the window's total, or one
/// interface with nothing taken away.
#[derive(Clone, Copy)]
struct Drawn<'a> {
    device: DeviceIdx,
    minus: &'a [DeviceIdx],
}

impl<'a> Drawn<'a> {
    /// The window's total; `DeviceIdx(u16::MAX)` when the catalog has none yet.
    fn total(t: &'a NetTotal) -> Self {
        Self {
            device: t.total.unwrap_or(DeviceIdx(u16::MAX)),
            minus: &t.hidden,
        }
    }

    fn interface(device: DeviceIdx) -> Self {
        Self { device, minus: &[] }
    }

    fn series(self, metric: MetricId) -> (SeriesId, Arc<[SeriesId]>) {
        let id = SeriesId {
            device: self.device,
            metric,
        };
        let minus = self
            .minus
            .iter()
            .map(|&device| SeriesId { device, metric })
            .collect();
        (id, minus)
    }
}

/// One direction's series: download filled, upload as a lighter line.
fn direction_series(drawn: Drawn<'_>, upload: bool, theme: &TlTheme) -> ChartSeries {
    let g = &theme.graph;
    if upload {
        let (id, minus) = drawn.series(MetricId::NET_TX);
        ChartSeries {
            id,
            minus,
            fill: None,
            line: blend_over(g.net_line, white().opacity(UPLOAD_TINT)),
            ..ChartSeries::default()
        }
    } else {
        let (id, minus) = drawn.series(MetricId::NET_RX);
        ChartSeries {
            id,
            minus,
            fill: Some(g.net_fill),
            line: g.net_line,
            ..ChartSeries::default()
        }
    }
}

/// The chart of `series` over the box range.
fn spec_of(series: SmallVec<[ChartSeries; 4]>, unit: NetworkUnit, s: &ChartSettings) -> ChartSpec {
    ChartSpec {
        series,
        y: YScale::AutoRate {
            iec: unit == NetworkUnit::Bytes,
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

/// One direction's chart.
fn chart_spec(
    drawn: Drawn<'_>,
    upload: bool,
    unit: NetworkUnit,
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    spec_of(smallvec![direction_series(drawn, upload, theme)], unit, s)
}

/// Download and upload as two lines in one chart, for `combined` and the adapter cells.
fn pair_spec(drawn: Drawn<'_>, unit: NetworkUnit, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    spec_of(
        smallvec![
            direction_series(drawn, false, theme),
            direction_series(drawn, true, theme)
        ],
        unit,
        s,
    )
}

/// No heading and no value: the rows under the charts carry the numbers.
fn chart_labels(range_word: SharedString) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::RateNetwork,
        value_align: ValueAlign::Hidden,
        range_word,
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The summed rates, their validity and the summed totals since boot of the counted interfaces.
struct Sums {
    rx: f64,
    tx: f64,
    rx_ok: bool,
    tx_ok: bool,
    rx_total: u64,
    tx_total: u64,
    counted: usize,
    /// The counted interface with the highest rx + tx over its valid rates; ties go to the first.
    busiest: Option<DeviceIdx>,
}

/// Whether interface `idx` is one the box counts: present, not `lo`, and of a shown class.
fn counted(snap: &Snapshot, idx: DeviceIdx, s: &NetSettings) -> bool {
    let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == idx) else {
        return false;
    };
    let loopback =
        matches!(&entry.identity, DeviceIdentity::Net { name, .. } if &**name == LOOPBACK);
    entry.present && !loopback && s.shown(entry.net_kind)
}

fn sums(snap: &Snapshot, s: &NetSettings) -> Sums {
    let mut out = Sums {
        rx: 0.,
        tx: 0.,
        rx_ok: false,
        tx_ok: false,
        rx_total: 0,
        tx_total: 0,
        counted: 0,
        busiest: None,
    };
    let mut busiest_rate = f64::NEG_INFINITY;
    for (idx, n) in snap.system.nets.iter() {
        if !counted(snap, *idx, s) {
            continue;
        }
        out.counted += 1;
        let mut rate = 0.;
        if n.rx_bps.validity.is_valid() {
            out.rx += n.rx_bps.value;
            out.rx_ok = true;
            rate += n.rx_bps.value;
        }
        if n.tx_bps.validity.is_valid() {
            out.tx += n.tx_bps.value;
            out.tx_ok = true;
            rate += n.tx_bps.value;
        }
        if rate > busiest_rate {
            busiest_rate = rate;
            out.busiest = Some(*idx);
        }
        out.rx_total += n.rx_total;
        out.tx_total += n.tx_total;
    }
    out
}

/// The name of interface `idx`: the device details' adapter name when Valid or Stale and not
/// blank, else the interface name.
fn adapter_name(snap: &Snapshot, idx: DeviceIdx) -> Option<&str> {
    let detail = snap
        .device_details
        .nets
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, d)| &d.adapter_name)
        .filter(|f| matches!(f.validity, Validity::Valid | Validity::Stale))
        .map(|f| f.value.trim())
        .filter(|t| !t.is_empty());
    detail.or_else(|| {
        snap.devices
            .entries
            .iter()
            .find(|e| e.idx == idx)
            .and_then(|e| match &e.identity {
                DeviceIdentity::Net { name, .. } => Some(&**name),
                _ => None,
            })
    })
}

/// The highest valid sample of the window's total of `metric` within the last
/// [`PEAK_WINDOW_NS`] before its newest sample.
fn peak(snap: &Snapshot, total: &NetTotal, metric: MetricId) -> Option<f32> {
    let view = total.view(snap, metric)?;
    let samples = &view.samples;
    let from = samples.last()?.t_ns.saturating_sub(PEAK_WINDOW_NS);
    samples
        .iter()
        .filter(|s| s.is_valid() && s.t_ns >= from)
        .map(|s| s.value)
        .reduce(f32::max)
}

/// The versions of the window's total of `metric` and of the per-class totals it takes away,
/// mixed into one number; 0 for an absent series.
fn total_version(snap: &Snapshot, total: &NetTotal, metric: MetricId) -> u64 {
    let version = |device: Option<DeviceIdx>| {
        device
            .and_then(|device| snap.series.get(SeriesId { device, metric }))
            .map_or(0, |v| v.version)
    };
    total.hidden.iter().fold(version(total.total), |acc, &d| {
        acc.wrapping_mul(0x0100_0000_01b3) ^ version(Some(d))
    })
}

/// One `adapter_charts` cell: an interface, its name, its rates and its chart.
pub struct AdapterCell {
    idx: DeviceIdx,
    name: SharedString,
    /// "1.2 Mbps / 3.4 Mbps": download, then upload.
    rates: SharedString,
    chart: Entity<ChartView>,
}

/// What the Download and Upload parts, the `combined` body and the `adapter_charts` cells show,
/// kept by the box and drawn by the widgets' flow looks, and what the last render of the cells
/// showed. Index 0 is Download, 1 is Upload.
#[derive(Default)]
pub struct NetModel {
    /// The Download and Upload words.
    pub words: [SharedString; 2],
    /// The summed rates.
    pub rates: [SharedString; 2],
    /// ", peak 4.4 Mbps" after each rate; empty while there is no peak.
    pub peaks: [SharedString; 2],
    /// The summed totals since boot, which the rate lines' tooltips show.
    pub totals: [SharedString; 2],
    /// "Total (since boot)".
    pub total_word: SharedString,
    /// The counted adapters' cells, built only while `adapter_charts` is the mode.
    adapters: Vec<AdapterCell>,
    /// Counted adapters past `MAX_ADAPTER_CELLS`.
    extra: usize,
    /// "+n more" by the number of cells shown, empty when every counted adapter shows.
    more_texts: SmallVec<[SharedString; 17]>,
    /// How many `adapter_charts` cells the last render showed.
    pub cells: usize,
}

impl NetModel {
    /// The "+n more" text for `shown` cells shown.
    fn more_text(&self, shown: usize) -> SharedString {
        self.more_texts.get(shown).cloned().unwrap_or_default()
    }

    /// The counted adapters' cells: interface index, rate text ("1.2 Mbps / 3.4 Mbps") and
    /// chart.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn adapter_cells(&self) -> Vec<(DeviceIdx, SharedString, Entity<ChartView>)> {
        self.adapters
            .iter()
            .map(|a| (a.idx, a.rates.clone(), a.chart.clone()))
            .collect()
    }
}

/// The box's [`NetModel`], shared with the widgets that draw it. Two refs are equal when they
/// share the model.
#[derive(Clone, Default)]
pub struct NetModelRef(pub Rc<RefCell<NetModel>>);

impl PartialEq for NetModelRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// What a text sync changed: the Download and Upload rate lines the widgets draw, and the
/// busiest adapter on the title line.
#[derive(Clone, Copy, Default)]
struct Synced {
    parts: [bool; 2],
    title: bool,
}

/// The Network box.
pub struct NetBox {
    /// The range figures the page gives (UI-PRF-055).
    range_text: RangeText,
    hub: Entity<SnapshotHub>,
    down: Entity<ChartView>,
    up: Entity<ChartView>,
    /// Download and upload as two lines of the aggregate series (`combined`).
    combined: Entity<ChartView>,
    mode: NetMode,
    /// How `adapter_charts` places its cells (D-027).
    arrange: Arrange,
    /// The gap between the Download and Upload parts (`stacked`).
    stack_split: InnerSplit,
    /// The gap between the Download and Upload columns (`columns`).
    col_split: InnerSplit,
    /// The parts' text and the adapter cells, shared with the widgets that draw them.
    model: NetModelRef,
    /// The body size of the last paint, for the fit of the adapter cells.
    body: Rc<StdCell<Size<Pixels>>>,
    down_peak: Option<f32>,
    up_peak: Option<f32>,
    /// The download and upload peaks alone, as the copy text gives them.
    peak_texts: [SharedString; 2],
    /// The ", peak {peak}" text.
    with_peak: Template,
    /// The busiest adapter's name, the title line.
    adapter: SharedString,
    peak_word: SharedString,
    /// The last seen version of the aggregate download and upload series.
    peak_versions: [u64; 2],
    title: SharedString,
    /// The engine's total and the hidden classes' totals the charts and peaks take away.
    total: NetTotal,
    /// The first visible network card, which the title opens (crw.44).
    target: Option<DeviceKey>,
    catalog_version: u64,
    text_bits: u64,
    /// The page is held at a time (UI-PRF-057).
    paused: bool,
    settings: ChartSettings,
    net: NetSettings,
    settings_versions: [u64; 4],
    style_version: u64,
    strings_version: u64,
    /// The flow layout of the shown preset (ADR-019): which widgets the box shows.
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

/// The saved grid layout of the Network box, if any.
fn saved_grid(cx: &App) -> Option<ContainerLayout> {
    if !cx.has_global::<TlSettings>() {
        return None;
    }
    TlSettings::settings(cx)
        .performance
        .layouts
        .known(NET_BOX_CONTAINER)
        .filter(|l| l.preset.is_none())
        .cloned()
}

/// The keys of the default mode.
fn default_keys() -> NetFlowKeys<'static> {
    NetFlowKeys {
        mode: MODES[0].id,
        arrange: None,
        split: None,
    }
}

impl NetBox {
    /// The network card the title opens, from the page. The same key does nothing.
    pub fn set_target(&mut self, target: Option<DeviceKey>, cx: &mut Context<Self>) {
        if self.target != target {
            self.target = target;
            cx.notify();
        }
    }

    pub fn new(hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let net = NetSettings::from(TlSettings::settings(cx));
        let snap = box_snapshot(hub, cx);
        let total = net_total(&snap.devices, TlSettings::settings(cx));
        let range = tr(cx, KEY_RANGE);
        let chart = |upload: bool, cx: &mut Context<Self>| {
            let spec = chart_spec(Drawn::total(&total), upload, net.unit, &settings, &theme);
            let labels = chart_labels(range.clone());
            box_chart(spec, labels, hub, cx)
        };
        let (down, up) = (chart(false, cx), chart(true, cx));
        let combined = box_chart(
            pair_spec(Drawn::total(&total), net.unit, &settings, &theme),
            chart_labels(range.clone()),
            hub,
            cx,
        );
        let model = NetModelRef(Rc::new(RefCell::new(NetModel {
            words: [tr(cx, KEY_DOWNLOAD), tr(cx, KEY_UPLOAD)],
            total_word: tr(cx, KEY_TOTAL),
            ..NetModel::default()
        })));
        let flow = net_flow::flow_layout(default_keys());
        let saved = saved_grid(cx);
        let plan = PlanCell::default();
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::new(hub)));
            let shown = saved.clone().unwrap_or_else(|| flow.clone());
            let plan = plan.clone();
            cx.new(|cx| {
                let mut c = WidgetContainer::new(NET_BOX_CONTAINER, shown, hub, store, cx);
                c.set_flow_placer(Some(net_flow::placer(plan)), cx);
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
            down,
            up,
            combined,
            mode: NetMode::Stacked,
            arrange: Arrange::Vertical,
            stack_split: InnerSplit::new(
                NET_STACK_SPLITTER_ID,
                NET_STACK_SPLITTER_LINE_ID,
                MODES[0].split.unwrap_or_default(),
            ),
            col_split: InnerSplit::new(
                NET_SPLITTER_ID,
                NET_SPLITTER_LINE_ID,
                MODES[1].split.unwrap_or_default(),
            ),
            model,
            body: Rc::new(StdCell::new(size(px(0.), px(0.)))),
            down_peak: None,
            up_peak: None,
            peak_texts: Default::default(),
            with_peak: Template::new(cx, KEY_WITH_PEAK, &["peak"]),
            adapter: SharedString::default(),
            peak_word: tr(cx, KEY_PEAK),
            peak_versions: [0; 2],
            title: tr(cx, KEY_TITLE),
            total,
            target: None,
            catalog_version: snap.devices.version,
            text_bits: 0,
            paused: timeline::paused(cx).is_some(),
            settings,
            net,
            settings_versions: section_versions(cx),
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
        this.sync_peaks(&snap, true);
        this.sync_text(&snap, true);
        this
    }

    /// The editor of the box's widgets (Edit Widgets, UI-PRF-061).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The container of the box's widgets.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The flow layout of the shown preset. Its preset keeps the mode and, for
    /// `adapter_charts`, the arrangement; the split is the box's own, so a drag does not lay
    /// the widgets out again.
    pub fn flow(&self) -> &ContainerLayout {
        &self.flow
    }

    /// The saved grid layout the box shows, if any.
    pub fn grid(&self) -> Option<&ContainerLayout> {
        self.saved_grid.as_ref()
    }

    /// The plan of the last flow render: the widgets the placer places.
    pub fn flow_plan(&self) -> Option<NetPlan> {
        self.plan.plan()
    }

    /// The shared model the widgets of the flow layout draw.
    pub fn model(&self) -> &NetModelRef {
        &self.model
    }

    /// Whether the box draws the widgets of its flow layout itself (no grid layout, not
    /// editing).
    fn host_draws(&self) -> bool {
        !self.editing && self.saved_grid.is_none()
    }

    /// Gets the container ready for Edit Widgets: with a flow layout it takes the preset's
    /// widgets that the last render drew, placed where they were drawn, so the editor pins them
    /// there. With a grid layout the container already holds it.
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

    /// The flow layout of the shown mode, and of the arrangement for `adapter_charts`. Returns
    /// whether it changed.
    fn sync_flow(&mut self, cx: &mut Context<Self>) -> bool {
        let flow = net_flow::flow_layout(NetFlowKeys {
            mode: self.mode.id(),
            arrange: (self.mode == NetMode::AdapterCharts).then_some(self.arrange),
            split: None,
        });
        if flow == self.flow {
            return false;
        }
        self.flow = flow;
        if self.host_draws() {
            self.show_saved(cx);
        }
        true
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// The Download (`parts[0]`) or Upload (`parts[1]`) rate line changed: the widgets that
    /// draw them render again, and nothing else. When the box draws no rate lines (a grid layout
    /// keeps its own sources, `adapter_charts` shows none) nothing renders; before the first
    /// flow render makes a widget's entity, the box renders.
    fn notify_parts(&mut self, parts: [bool; 2], cx: &mut Context<Self>) {
        if !self.host_draws() || parts == [false, false] {
            return;
        }
        let ids: SmallVec<[Option<u32>; 2]> = match self.mode {
            NetMode::Stacked | NetMode::Columns => {
                let ids = [net_flow::down_of(&self.flow), net_flow::up_of(&self.flow)];
                ids.into_iter()
                    .zip(parts)
                    .filter_map(|(id, changed)| changed.then_some(id))
                    .collect()
            }
            NetMode::Combined => smallvec![net_flow::traffic_of(&self.flow)],
            NetMode::AdapterCharts => return,
        };
        let widgets: SmallVec<[_; 2]> = ids
            .iter()
            .map(|id| id.and_then(|id| self.container.read(cx).widget(id).cloned()))
            .collect();
        if widgets.iter().any(Option::is_none) {
            self.notify(cx);
            return;
        }
        for w in widgets.into_iter().flatten() {
            w.update(cx, |w, cx| w.flow_changed(cx));
        }
    }

    /// The adapter cells changed: the adapters group draws them, so only it renders again.
    /// Outside `adapter_charts` and with a grid layout nothing renders; before the first flow
    /// render makes the group's entity, the box renders.
    fn notify_adapters(&mut self, cx: &mut Context<Self>) {
        if !self.host_draws() || self.mode != NetMode::AdapterCharts {
            return;
        }
        let group = net_flow::adapters_of(&self.flow)
            .and_then(|id| self.container.read(cx).group(id).cloned());
        match group {
            Some(g) => g.update(cx, |g, cx| g.flow_changed(cx)),
            None => self.notify(cx),
        }
    }

    /// Renders what a text sync and an adapter sync changed: the box for its title line, the
    /// widgets of the rate lines, and the adapters group.
    fn notify_changed(&mut self, synced: Synced, adapters: bool, cx: &mut Context<Self>) {
        if synced.title {
            self.notify(cx);
        }
        self.notify_parts(synced.parts, cx);
        if adapters {
            self.notify_adapters(cx);
        }
    }

    /// The shown mode's id.
    pub fn mode(&self) -> &'static str {
        self.mode.id()
    }

    /// Shows mode `id`; an unknown id is the default mode. Only this box renders again.
    pub fn set_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let mode = NetMode::from_id(id);
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        if mode == NetMode::AdapterCharts {
            let snap = box_snapshot(&self.hub, cx);
            self.sync_adapters(&snap, cx);
        } else {
            let mut model = self.model.0.borrow_mut();
            model.adapters.clear();
            model.extra = 0;
            model.more_texts.clear();
        }
        self.sync_flow(cx);
        cx.notify();
    }

    /// The Download part's share of the body as laid out in `stacked` (height) or `columns`
    /// (width), after the clamps; 0 in the other modes.
    pub fn split(&self, m: &UiMetrics) -> f32 {
        match self.mode {
            NetMode::Stacked => {
                self.stack_split
                    .share(SplitAxis::Column, m.sp[STACK_GAP], m.ui_scale)
            }
            NetMode::Columns => self.col_split.share(SplitAxis::Row, m.sp[5], m.ui_scale),
            _ => 0.,
        }
    }

    /// Takes the saved share for the shown mode. Nothing changes while the gap is held.
    pub fn set_split(&mut self, share: f32, cx: &mut Context<Self>) {
        let changed = match self.mode {
            NetMode::Stacked => self.stack_split.set_share(share),
            NetMode::Columns => self.col_split.set_share(share),
            _ => false,
        };
        if changed {
            cx.notify();
        }
    }

    fn stack_inner_split(&mut self) -> &mut InnerSplit {
        &mut self.stack_split
    }

    fn col_inner_split(&mut self) -> &mut InnerSplit {
        &mut self.col_split
    }

    /// The `combined` chart.
    pub fn combined_chart(&self) -> &Entity<ChartView> {
        &self.combined
    }

    /// The interface indexes of the `adapter_charts` cells, in catalog order.
    pub fn adapter_idxs(&self) -> Vec<DeviceIdx> {
        self.model
            .0
            .borrow()
            .adapters
            .iter()
            .map(|a| a.idx)
            .collect()
    }

    /// How `adapter_charts` places its cells.
    pub fn arrange(&self) -> Arrange {
        self.arrange
    }

    /// Places the `adapter_charts` cells as `arrange` says. Only this box renders again, and
    /// only when it shows `adapter_charts`.
    pub fn set_arrange(&mut self, arrange: Arrange, cx: &mut Context<Self>) {
        if arrange != self.arrange {
            self.arrange = arrange;
            self.sync_flow(cx);
            if self.mode == NetMode::AdapterCharts {
                cx.notify();
            }
        }
    }

    /// How many `adapter_charts` cells the last render showed.
    pub fn cells(&self) -> usize {
        self.model.0.borrow().cells
    }

    pub fn down_chart(&self) -> &Entity<ChartView> {
        &self.down
    }

    pub fn up_chart(&self) -> &Entity<ChartView> {
        &self.up
    }

    /// Download rate, upload rate, download peak, upload peak, download total, upload total.
    pub fn texts(&self) -> [SharedString; 6] {
        let model = self.model.0.borrow();
        [
            model.rates[0].clone(),
            model.rates[1].clone(),
            self.peak_texts[0].clone(),
            self.peak_texts[1].clone(),
            model.totals[0].clone(),
            model.totals[1].clone(),
        ]
    }

    /// "Network\t<adapter>", then "Download\t1.2 Mbps\tPeak 3.4 Mbps\tTotal (since boot) 3.40 GiB"
    /// and the same for upload.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\t');
        out.push_str(&self.adapter);
        out.push('\n');
        let model = self.model.0.borrow();
        for i in 0..2 {
            out.push_str(&model.words[i]);
            out.push('\t');
            out.push_str(&model.rates[i]);
            out.push('\t');
            out.push_str(&self.peak_word);
            out.push(' ');
            out.push_str(&self.peak_texts[i]);
            out.push('\t');
            out.push_str(&model.total_word);
            out.push(' ');
            out.push_str(&model.totals[i]);
            out.push('\n');
        }
    }

    /// Reads the 60 s peaks when an aggregate series changed, or always with `force`. True when
    /// a peak value changed.
    fn sync_peaks(&mut self, snap: &Snapshot, force: bool) -> bool {
        let (rx, tx) = (MetricId::NET_RX, MetricId::NET_TX);
        let versions = [
            total_version(snap, &self.total, rx),
            total_version(snap, &self.total, tx),
        ];
        if !force && versions == self.peak_versions {
            return false;
        }
        self.peak_versions = versions;
        let (down, up) = (peak(snap, &self.total, rx), peak(snap, &self.total, tx));
        let changed = down.map(f32::to_bits) != self.down_peak.map(f32::to_bits)
            || up.map(f32::to_bits) != self.up_peak.map(f32::to_bits);
        self.down_peak = down;
        self.up_peak = up;
        changed
    }

    /// Brings the `adapter_charts` cells up to date with the counted adapters: a new adapter
    /// builds a chart, a gone one drops its cell, a kept one keeps its chart. True when a cell
    /// changed. Does nothing in the other modes.
    fn sync_adapters(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        if self.mode != NetMode::AdapterCharts {
            return false;
        }
        let unit = NetFormat {
            unit: self.net.unit,
            auto_scale: self.net.auto_scale,
        };
        let theme = current(cx).clone();
        let hub = self.hub.clone();
        let range = tr(cx, KEY_RANGE);
        let (mut old, old_extra) = {
            let mut model = self.model.0.borrow_mut();
            (std::mem::take(&mut model.adapters), model.extra)
        };
        let mut adapters = Vec::new();
        let mut changed = false;
        let mut total = 0;
        for (idx, n) in snap.system.nets.iter() {
            if !counted(snap, *idx, &self.net) {
                continue;
            }
            total += 1;
            if adapters.len() == MAX_ADAPTER_CELLS {
                continue;
            }
            let name: SharedString = adapter_name(snap, *idx).unwrap_or("").to_owned().into();
            let rate = |m: &tl_model::system::Metric<f64>| {
                value_text(m.validity, |out| {
                    format_value(ValueFormat::RateNetwork, m.value as f32, unit, out);
                })
            };
            let rates: SharedString = format!("{} / {}", rate(&n.rx_bps), rate(&n.tx_bps)).into();
            let cell = match old.iter().position(|c| c.idx == *idx) {
                Some(i) => {
                    let mut c = old.remove(i);
                    changed |= c.name != name || c.rates != rates;
                    c.name = name;
                    c.rates = rates;
                    c
                }
                None => {
                    changed = true;
                    let spec = pair_spec(
                        Drawn::interface(*idx),
                        self.net.unit,
                        &self.settings,
                        &theme,
                    );
                    AdapterCell {
                        idx: *idx,
                        name,
                        rates,
                        chart: box_chart(spec, chart_labels(range.clone()), &hub, cx),
                    }
                }
            };
            adapters.push(cell);
        }
        changed |= !old.is_empty();
        let extra = total - adapters.len();
        changed |= extra != old_extra;
        let shown_max = adapters.len();
        {
            let mut model = self.model.0.borrow_mut();
            model.adapters = adapters;
            model.extra = extra;
        }
        if changed || self.model.0.borrow().more_texts.len() != shown_max + 1 {
            self.sync_more_texts(cx);
        }
        changed
    }

    /// "+n more" for each number of cells shown.
    fn sync_more_texts(&mut self, cx: &mut Context<Self>) {
        let (rows, total) = {
            let model = self.model.0.borrow();
            (model.adapters.len(), model.adapters.len() + model.extra)
        };
        let texts: SmallVec<[SharedString; 17]> = (0..=rows)
            .map(|shown| {
                let n = total - shown;
                if n == 0 {
                    SharedString::default()
                } else {
                    tr_args(cx, KEY_MORE, &[("n", FluentValue::from(n))])
                }
            })
            .collect();
        self.model.0.borrow_mut().more_texts = texts;
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
        if !(devices
            || mask.intersects(
                ComponentMask::SYSTEM | ComponentMask::SERIES | ComponentMask::DEVICE_DETAILS,
            ))
        {
            return;
        }
        let snap = box_snapshot(&hub, cx);
        if devices && snap.devices.version != self.catalog_version {
            self.catalog_version = snap.devices.version;
            let total = net_total(&snap.devices, TlSettings::settings(cx));
            if total != self.total {
                self.total = total;
                self.rebuild_specs(cx);
                self.sync_peaks(&snap, true);
            }
        }
        let peaks = mask.contains(ComponentMask::SERIES) && self.sync_peaks(&snap, false);
        let synced = self.sync_text(&snap, peaks);
        let adapters = self.sync_adapters(&snap, cx);
        self.notify_changed(synced, adapters, cx);
    }

    /// The page was held at a time or made live: the rates, peaks and adapters read the other
    /// snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = timeline::paused(cx).is_some();
        let snap = box_snapshot(&self.hub, cx);
        self.sync_peaks(&snap, true);
        self.sync_text(&snap, true);
        self.sync_adapters(&snap, cx);
        self.notify(cx);
        self.notify_parts([true; 2], cx);
        self.notify_adapters(cx);
    }

    /// The rates and totals when their inputs changed, or always with `force`. Says whether
    /// the rate lines' text or the title line changed. While paused the totals since boot are
    /// "-": the history does not keep them.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> Synced {
        let s = sums(snap, &self.net);
        let mut h = FxHasher::default();
        h.write_u64(s.rx.to_bits());
        h.write_u64(s.tx.to_bits());
        h.write_u8(u8::from(s.rx_ok) | u8::from(s.tx_ok) << 1);
        h.write_u64(s.rx_total);
        h.write_u64(s.tx_total);
        h.write_usize(s.counted);
        let adapter = s
            .busiest
            .and_then(|idx| adapter_name(snap, idx))
            .unwrap_or("");
        adapter.hash(&mut h);
        self.down_peak.map(f32::to_bits).hash(&mut h);
        self.up_peak.map(f32::to_bits).hash(&mut h);
        self.net.unit.hash(&mut h);
        self.net.auto_scale.hash(&mut h);
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return Synced::default();
        }
        self.text_bits = bits;
        let unit = NetFormat {
            unit: self.net.unit,
            auto_scale: self.net.auto_scale,
        };
        let rate = |ok: bool, v: f64| {
            let validity = if ok {
                Validity::Valid
            } else if s.counted == 0 {
                Validity::Missing
            } else {
                Validity::WarmingUp
            };
            value_text(validity, |out| {
                format_value(ValueFormat::RateNetwork, v as f32, unit, out);
            })
        };
        let rates = [rate(s.rx_ok, s.rx), rate(s.tx_ok, s.tx)];
        let peak_text = |p: Option<f32>| {
            let validity = if p.is_some() {
                Validity::Valid
            } else if s.counted == 0 {
                Validity::Missing
            } else {
                Validity::WarmingUp
            };
            value_text(validity, |out| {
                format_value(ValueFormat::RateNetwork, p.unwrap_or(0.), unit, out);
            })
        };
        self.peak_texts = [peak_text(self.down_peak), peak_text(self.up_peak)];
        let part = |p: Option<f32>, text: &SharedString| {
            let mut out = String::new();
            if p.is_some() {
                let _ = self.with_peak.write(&mut out, &[text]);
            }
            SharedString::from(out)
        };
        let peaks = [
            part(self.down_peak, &self.peak_texts[0]),
            part(self.up_peak, &self.peak_texts[1]),
        ];
        let paused = self.paused;
        let size = |bytes: u64| {
            if paused {
                return value_text(Validity::Missing, |_| {});
            }
            let mut out = String::new();
            units::bytes(&mut out, bytes);
            SharedString::from(out)
        };
        let totals = [size(s.rx_total), size(s.tx_total)];
        let parts = {
            let mut model = self.model.0.borrow_mut();
            let changed = [0, 1].map(|i| {
                model.rates[i] != rates[i]
                    || model.peaks[i] != peaks[i]
                    || model.totals[i] != totals[i]
            });
            model.rates = rates;
            model.peaks = peaks;
            model.totals = totals;
            changed
        };
        let title = *self.adapter != *adapter;
        if title {
            self.adapter = adapter.to_owned().into();
        }
        Synced { parts, title }
    }

    /// Applies the chart, unit and interface class settings when one of their sections changed,
    /// and shows a saved grid layout that came, went or changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        self.sync_grid(cx);
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let settings = ChartSettings::read(cx);
        let net = NetSettings::from(TlSettings::settings(cx));
        if settings == self.settings && net == self.net {
            return;
        }
        let snap = box_snapshot(&self.hub, cx);
        let total = net_total(&snap.devices, TlSettings::settings(cx));
        let new_total = total != self.total;
        let respec = settings != self.settings || net.unit != self.net.unit || new_total;
        self.settings = settings;
        self.net = net;
        self.total = total;
        if respec {
            self.rebuild_specs(cx);
        }
        if new_total {
            self.sync_peaks(&snap, true);
        }
        let synced = self.sync_text(&snap, false);
        let adapters = self.sync_adapters(&snap, cx);
        self.notify_changed(synced, adapters, cx);
    }

    /// A new theme or UI scale: new chart colours, and the parts draw in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_specs(cx);
        cx.notify();
        self.notify_parts([true; 2], cx);
        self.notify_adapters(cx);
    }

    /// A new language: new title, words, the Peak and Total labels and the peak text.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        {
            let (down, up, total) = (tr(cx, KEY_DOWNLOAD), tr(cx, KEY_UPLOAD), tr(cx, KEY_TOTAL));
            let mut model = self.model.0.borrow_mut();
            model.words = [down, up];
            model.total_word = total;
        }
        self.peak_word = tr(cx, KEY_PEAK);
        self.with_peak = Template::new(cx, KEY_WITH_PEAK, &["peak"]);
        let range = tr(cx, KEY_RANGE);
        let views: SmallVec<[Entity<ChartView>; 8]> =
            [self.down.clone(), self.up.clone(), self.combined.clone()]
                .into_iter()
                .chain(
                    self.model
                        .0
                        .borrow()
                        .adapters
                        .iter()
                        .map(|a| a.chart.clone()),
                )
                .collect();
        for v in views {
            let labels = chart_labels(range.clone());
            v.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        self.sync_more_texts(cx);
        let snap = box_snapshot(&self.hub, cx);
        self.sync_text(&snap, true);
        cx.notify();
        self.notify_parts([true; 2], cx);
        self.notify_adapters(cx);
    }

    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let pair = pair_spec(
            Drawn::total(&self.total),
            self.net.unit,
            &self.settings,
            &theme,
        );
        self.combined.update(cx, |v, cx| v.set_spec(pair, cx));
        let cells: SmallVec<[(DeviceIdx, Entity<ChartView>); 8]> = self
            .model
            .0
            .borrow()
            .adapters
            .iter()
            .map(|a| (a.idx, a.chart.clone()))
            .collect();
        for (idx, chart) in cells {
            let spec = pair_spec(Drawn::interface(idx), self.net.unit, &self.settings, &theme);
            chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        for (view, upload) in [(self.down.clone(), false), (self.up.clone(), true)] {
            let spec = chart_spec(
                Drawn::total(&self.total),
                upload,
                self.net.unit,
                &self.settings,
                &theme,
            );
            view.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }
}

/// A row with a label on the left and a value on the right. `id` is the value's debug selector.
/// `extra` follows the value and gives way first ("1.4 Mbps" then ", peak 4.4 Mbps").
fn stat_row(
    id: Option<&'static str>,
    label: &SharedString,
    value: &SharedString,
    extra: &SharedString,
    m: &UiMetrics,
    theme: &TlTheme,
) -> Div {
    // Each text style goes in first: assigning it later would drop the nowrap, the ellipsis and
    // the color, and a long value would wrap onto the next row.
    // The extra part gives way first, then the label, so the value keeps its width as long as
    // it can.
    let mut label_el = div();
    *label_el.text_style() = t_xs(m);
    let label_el = label_el
        .min_w_0()
        .flex_shrink(LABEL_SHRINK)
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(label.clone());
    let mut value_el = div();
    *value_el.text_style() = numeric_style(m, m.text.sm);
    let mut value_el = value_el
        .flex()
        .flex_row()
        .justify_end()
        .min_w_0()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(fit_text(value.clone()));
    if !extra.is_empty() {
        let mut extra_el = div();
        *extra_el.text_style() = numeric_style(m, m.text.xs);
        value_el = value_el.child(
            extra_el
                .flex()
                .items_center()
                .min_w_0()
                .flex_shrink(EXTRA_SHRINK)
                .whitespace_nowrap()
                .text_color(theme.text_secondary)
                .child(fit_text(extra.clone()).start()),
        );
    }
    if let Some(id) = id {
        value_el = value_el.tl_selector(move || id.into());
    }
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(m.sp[2])
        .h(px(PLOT_LINE * m.ui_scale))
        .child(label_el)
        .child(value_el)
}

/// The flex shrink of a stat row label: large, so the label shrinks to nothing before the value
/// shrinks. A factor below 1 would not work: flex then removes only that part of the overflow.
const LABEL_SHRINK: f32 = 1000.;

/// The flex shrink of the part after a value: larger than `LABEL_SHRINK`, so the peak gives way
/// before the label.
const EXTRA_SHRINK: f32 = 1_000_000.;

/// One direction's line: "Download 1.4 Mbps, peak 4.4 Mbps". Its tooltip gives the total since
/// boot ("Total (since boot) 3.40 GiB"), formatted only when the tooltip opens.
struct RateLine<'a> {
    id: &'static str,
    word: &'a SharedString,
    rate: &'a SharedString,
    peak: &'a SharedString,
    total_word: &'a SharedString,
    total: &'a SharedString,
}

fn rate_line(
    l: RateLine,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let row = stat_row(Some(l.id), l.word, l.rate, l.peak, m, theme);
    let (word, total) = (l.total_word.clone(), l.total.clone());
    let state = HoverTooltip::state(l.id, window, cx);
    HoverTooltip::attach(
        &state,
        row,
        DELAY_CONTROL,
        move |_, cx| TlTooltip::text(format!("{word} {total}")).view(cx),
        window,
        cx,
    )
    .into_any_element()
}

/// One direction: its rate line right over its chart.
fn column(line: AnyElement, chart: &Entity<ChartView>) -> AnyElement {
    whole_rows()
        .flex_1()
        .min_w_0()
        .h_full()
        .child(line)
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(chart.clone().cached(StyleRefinement::default().size_full())),
        )
        .into_any_element()
}

/// An `adapter_charts` cell header: the adapter name and its two rates.
fn cell_head(a: &AdapterCell, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    // The text style is set first: setting it replaces the nowrap, ellipsis and colour set
    // before it, and a Side by side cell is narrow enough for a long name to wrap.
    let mut name = div();
    *name.text_style() = label_style(m, m.text.sm, FontWeight::MEDIUM);
    let name = name
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_primary)
        .tl_selector({
            let idx = a.idx;
            move || name_id(idx)
        })
        .child(a.name.clone());
    let mut rates = div();
    *rates.text_style() = numeric_style(m, m.text.sm);
    let rates = rates
        .flex()
        .min_w_0()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(fit_text(a.rates.clone()));
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[4])
        .h(px(PLOT_LINE * m.ui_scale))
        .child(name)
        .child(rates)
        .into_any_element()
}

/// The smallest `adapter_charts` cell, in px.
fn cell_min(m: &UiMetrics) -> Size<Pixels> {
    size(
        px(GRID_CELL_MIN_W * m.ui_scale),
        px(GRID_CELL_MIN_H * m.ui_scale),
    )
}

/// The `adapter_charts` grid for `rows` cells in `body`, placed as `arrange` says (`list_fit`).
/// Only whole cells show (UI-PRF-059); an adapter past `MAX_ADAPTER_CELLS` or a cell that would
/// be cut is counted in the "+n more" line, which takes its height from the body first. Before
/// the first paint the body is empty and every adapter gets a cell.
fn charts_fit(
    arrange: Arrange,
    body: Size<Pixels>,
    rows: usize,
    total: usize,
    m: &UiMetrics,
) -> ListFit {
    list_fit(
        arrange,
        rows,
        total,
        body,
        cell_min(m),
        m.sp[2],
        px(LABEL_ROW * m.ui_scale) + m.sp[2],
    )
}

/// An `adapter_charts` cell of `cell` px: the header right over the combined chart.
fn chart_cell(a: &AdapterCell, cell: Size<Pixels>, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let idx = a.idx;
    div()
        .tl_selector(move || cell_id(idx))
        .flex()
        .flex_col()
        .flex_none()
        .w(cell.width)
        .h(cell.height)
        .overflow_hidden()
        .child(cell_head(a, m, theme))
        .child(
            div()
                .tl_selector(move || chart_id(idx))
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    a.chart
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
        )
        .into_any_element()
}

/// The Download (`upload` false) or Upload rate line of `model`, with its peak and its total in
/// the tooltip.
fn model_rate_line(
    model: &NetModel,
    upload: bool,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let i = usize::from(upload);
    let line = RateLine {
        id: if upload { UP_RATE_ID } else { DOWN_RATE_ID },
        word: &model.words[i],
        rate: &model.rates[i],
        peak: &model.peaks[i],
        total_word: &model.total_word,
        total: &model.totals[i],
    };
    rate_line(line, m, theme, window, cx)
}

/// One direction's part of `stacked` and `columns` (UI-PRF-017): its rate line right over its
/// chart. The Download and Upload widgets draw it (`FlowLook::NetDirection`).
pub(crate) fn net_direction_element(
    model: &NetModelRef,
    upload: bool,
    chart: &Entity<ChartView>,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let line = model_rate_line(&model.0.borrow(), upload, m, theme, window, cx);
    column(line, chart)
}

/// `combined`: the two rate lines side by side over one chart with both lines. The traffic
/// widget draws it (`FlowLook::NetCombined`).
pub(crate) fn net_combined_element(
    model: &NetModelRef,
    chart: &Entity<ChartView>,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (down, up) = {
        let md = model.0.borrow();
        (
            model_rate_line(&md, false, m, theme, window, cx),
            model_rate_line(&md, true, m, theme, window, cx),
        )
    };
    let half = |line| div().flex().flex_col().flex_1().min_w_0().child(line);
    whole_rows()
        .size_full()
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .gap(m.sp[5])
                .child(half(down))
                .child(half(up)),
        )
        .child(
            div()
                .tl_selector(|| COMBINED_CHART_ID.into())
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(chart.clone().cached(StyleRefinement::default().size_full())),
        )
        .into_any_element()
}

/// `adapter_charts`: one combined chart per counted adapter of `model` that fill `body` in one
/// column, one row or a grid, as `arrange` says. Only whole cells show; the adapters left out
/// join "+n more" (UI-PRF-059). The number of cells shown is stored in `model`; `view` renders
/// again when a resize changes the fit. The adapters group draws it (`GroupFlowLook::NetCells`).
pub(crate) fn net_cells_element(
    model: &NetModelRef,
    body: &Rc<StdCell<Size<Pixels>>>,
    arrange: Arrange,
    m: &UiMetrics,
    theme: &TlTheme,
    view: EntityId,
) -> AnyElement {
    let mut guard = model.0.borrow_mut();
    let md = &mut *guard;
    let rows = md.adapters.len();
    let total = rows + md.extra;
    let fit = charts_fit(arrange, body.get(), rows, total, m);
    md.cells = fit.shown;
    let cells = grid_cells(
        &fit.grid,
        md.adapters
            .iter()
            .take(fit.shown)
            .map(|a| chart_cell(a, fit.grid.cell, m, theme)),
        m.sp[2],
        GRID_ID,
    );
    // A "+n more" line that does not fit whole is left out (UI-PRF-017).
    let mut el = whole_rows().relative().size_full().gap(m.sp[2]).child(
        div()
            .tl_selector(|| GRID_ID.into())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(cells),
    );
    let more = md.more_text(fit.shown);
    if !more.is_empty() {
        el = el.child(more_line(more, m, theme));
    }
    let probe_m = *m;
    el.child(class_probe(
        Rc::clone(body),
        fit.class(),
        move |b| charts_fit(arrange, b, rows, total, &probe_m).class(),
        view,
    ))
    .into_any_element()
}

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

/// The widgets a flow render drew: the Download part (or the one widget that fills the body)
/// and the Upload part.
type Slots = [Option<u32>; 2];

impl NetBox {
    /// The Download or Upload part, drawn by its widget when the layout has it.
    fn direction(
        &mut self,
        upload: bool,
        slots: &mut Slots,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chart = if upload { &self.up } else { &self.down }.clone();
        let id = if upload {
            net_flow::up_of(&self.flow)
        } else {
            net_flow::down_of(&self.flow)
        };
        let look = FlowLook::NetDirection {
            model: self.model.clone(),
            upload,
            chart: chart.clone(),
        };
        let widget = id.and_then(|id| {
            self.container
                .update(cx, |c, cx| c.flow_widget(id, look, cx))
                .map(|w| (id, w))
        });
        match widget {
            Some((id, w)) => {
                slots[usize::from(upload)] = Some(id);
                cached_full(w)
            }
            None => net_direction_element(&self.model, upload, &chart, m, theme, window, cx),
        }
    }

    /// `stacked`: the Download part, the splitter and the Upload part, each the full width.
    fn stacked_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        slots: &mut Slots,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share = self.split(m);
        let down = self.direction(false, slots, m, theme, window, cx);
        let up = self.direction(true, slots, m, theme, window, cx);
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[STACK_GAP])
            // Clipped, so a box too low for both parts never draws below its body. It
            // gives way when the body is lower than its share.
            .child(
                div()
                    .flex()
                    .flex_shrink(1.)
                    .min_h_0()
                    .w_full()
                    .h(relative(share))
                    .overflow_hidden()
                    .child(down),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(up),
            )
            .child(self.stack_split.probe(Self::stack_inner_split, cx))
            .child(self.stack_split.gap(
                SplitAxis::Column,
                share,
                m.sp[STACK_GAP],
                Self::stack_inner_split,
                cx,
            ))
            .into_any_element()
    }

    /// `columns`: the Download column, the splitter and the Upload column.
    fn columns_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        slots: &mut Slots,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share = self.split(m);
        let down = self.direction(false, slots, m, theme, window, cx);
        let up = self.direction(true, slots, m, theme, window, cx);
        div()
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .gap(m.sp[5])
            .child(
                div()
                    .flex()
                    .flex_shrink(1.)
                    .min_w_0()
                    .h_full()
                    .w(relative(share))
                    .child(down),
            )
            .child(div().flex().flex_1().min_w_0().h_full().child(up))
            .child(self.col_split.probe(Self::col_inner_split, cx))
            .child(
                self.col_split
                    .gap(SplitAxis::Row, share, m.sp[5], Self::col_inner_split, cx),
            )
            .into_any_element()
    }

    /// `combined`: the two rate lines over one chart, drawn by the traffic widget.
    fn combined_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        slots: &mut Slots,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = FlowLook::NetCombined {
            model: self.model.clone(),
            chart: self.combined.clone(),
        };
        let widget = net_flow::traffic_of(&self.flow).and_then(|id| {
            self.container
                .update(cx, |c, cx| c.flow_widget(id, look, cx))
                .map(|w| (id, w))
        });
        match widget {
            Some((id, w)) => {
                slots[0] = Some(id);
                cached_full(w)
            }
            None => net_combined_element(&self.model, &self.combined, m, theme, window, cx),
        }
    }

    /// `adapter_charts`: the cells, drawn by the adapters group.
    fn charts_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        slots: &mut Slots,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = GroupFlowLook::NetCells {
            model: self.model.clone(),
            body: Rc::clone(&self.body),
            arrange: self.arrange,
        };
        let group = net_flow::adapters_of(&self.flow).and_then(|id| {
            self.container
                .update(cx, |c, cx| c.flow_group(id, look, cx))
                .map(|g| (id, g))
        });
        match group {
            Some((id, g)) => {
                slots[0] = Some(id);
                cached_full(g)
            }
            None => net_cells_element(
                &self.model,
                &self.body,
                self.arrange,
                m,
                theme,
                cx.entity_id(),
            ),
        }
    }
}

impl Render for NetBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
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
            if self.mode != NetMode::AdapterCharts {
                self.model.0.borrow_mut().cells = 0;
            }
            let mut slots: Slots = [None, None];
            let body = match self.mode {
                NetMode::Stacked => self.stacked_body(&m, &theme, &mut slots, window, cx),
                NetMode::Columns => self.columns_body(&m, &theme, &mut slots, window, cx),
                NetMode::Combined => self.combined_body(&m, &theme, &mut slots, window, cx),
                NetMode::AdapterCharts => self.charts_body(&m, &theme, &mut slots, cx),
            };
            let gap = match self.mode {
                NetMode::Stacked => m.sp[STACK_GAP],
                NetMode::Columns => m.sp[5],
                _ => px(0.),
            };
            let plan = NetPlan {
                look: self.mode.look(),
                share: self.split(&m),
                gap: f32::from(gap),
                first: slots[0],
                second: slots[1],
            };
            if self.plan.set(plan) {
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
            NET_BOX_ID,
            self.target.clone(),
            self.title.clone(),
            SharedString::default(),
            self.adapter.clone(),
            &self.range_text,
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .size_full()
    }
}
