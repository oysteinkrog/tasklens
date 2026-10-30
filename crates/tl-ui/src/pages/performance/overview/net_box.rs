//! The Overview page's Network box (UI-PRF-017, D-022, UI-PRF-032, PERF-054): the download and
//! upload charts of all interfaces, with the summed rates and the 60 s peaks on one line per
//! direction ("1.4 Mbps, peak 4.4 Mbps"). That line's tooltip gives the total since boot
//! (tasklens-o55.54). The busiest interface's name is on the title line.
//!
//! The rates and totals sum the present interfaces other than `lo` whose class the
//! `show_network_*` settings show, the set the engine's aggregate counts. The box formats its
//! text only when the inputs' hash changes and notifies only then. Pointer movement never
//! notifies.
//!
//! UI-PRF-019 gives the box four modes. `stacked` (the default) is Download over Upload with a
//! horizontal splitter. `columns` is the Download and Upload columns above with a vertical
//! splitter between them. Their Arrange choice (D-027) switches between the two. `combined` is
//! one chart with download and upload as two lines, under the rate line of both directions.
//! `adapter_charts` is a grid of one combined chart per counted adapter, in cells of at least
//! 200 × 80 px. The adapter charts are built when the mode is shown and when the set of counted
//! adapters changes, never per tick.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::cell::Cell as StdCell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement,
    Styled as _, Subscription, Window, div, px, relative, size, white,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{NetworkUnit, Settings};
use tl_model::snapshot::Snapshot;
use tl_model::units;
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::super::details::Template;
use super::cpu_box::value_text;
use super::disk_box::more_line;
use super::fit::{fit_text, whole_rows};
use super::inner_split::{InnerSplit, InnerSplitChanged, SplitAxis};
use super::layout::{Arrange, ArrangeChoice, BoxMode};
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
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::{Section, TlSettings};
use crate::text::style::{label_style, numeric_style, t_xs};
use crate::theme::derived::blend_over;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

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

/// The upload line: the network colour blended with white, as the sidebar draws it.
const UPLOAD_TINT: f32 = 0.3;

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

/// The aggregate net device's idx in `catalog`, `DeviceIdx(u16::MAX)` when it has none.
pub(super) fn aggregate(catalog: &DeviceCatalog) -> DeviceIdx {
    catalog
        .find(&DeviceIdentity::Aggregate(AggregateKind::Nets))
        .map_or(DeviceIdx(u16::MAX), |e| e.idx)
}

/// One direction's series: download filled, upload as a lighter line.
fn direction_series(device: DeviceIdx, upload: bool, theme: &TlTheme) -> ChartSeries {
    let g = &theme.graph;
    if upload {
        ChartSeries {
            id: SeriesId {
                device,
                metric: MetricId::NET_TX,
            },
            fill: None,
            line: blend_over(g.net_line, white().opacity(UPLOAD_TINT)),
            ..ChartSeries::default()
        }
    } else {
        ChartSeries {
            id: SeriesId {
                device,
                metric: MetricId::NET_RX,
            },
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
    device: DeviceIdx,
    upload: bool,
    unit: NetworkUnit,
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    spec_of(smallvec![direction_series(device, upload, theme)], unit, s)
}

/// Download and upload as two lines in one chart, for `combined` and the adapter cells.
fn pair_spec(
    device: DeviceIdx,
    unit: NetworkUnit,
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    spec_of(
        smallvec![
            direction_series(device, false, theme),
            direction_series(device, true, theme)
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

/// The highest valid sample of `id` within the last [`PEAK_WINDOW_NS`] before its newest sample.
fn peak(snap: &Snapshot, id: SeriesId) -> Option<f32> {
    let samples = &snap.series.get(id)?.samples;
    let from = samples.last()?.t_ns.saturating_sub(PEAK_WINDOW_NS);
    samples
        .iter()
        .filter(|s| s.is_valid() && s.t_ns >= from)
        .map(|s| s.value)
        .reduce(f32::max)
}

/// The version of series `id`, 0 when it has none.
fn series_version(snap: &Snapshot, id: SeriesId) -> u64 {
    snap.series.get(id).map_or(0, |v| v.version)
}

/// One `adapter_charts` cell: an interface, its name, its rates and its chart.
struct AdapterCell {
    idx: DeviceIdx,
    name: SharedString,
    /// "1.2 Mbps / 3.4 Mbps": download, then upload.
    rates: SharedString,
    chart: Entity<ChartView>,
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
    /// The counted adapters' cells, built only while `adapter_charts` is the mode.
    adapters: Vec<AdapterCell>,
    /// Counted adapters past `MAX_ADAPTER_CELLS`.
    extra: usize,
    /// How many cells the last `adapter_charts` render showed.
    cells: usize,
    body: Rc<StdCell<Size<Pixels>>>,
    down_text: SharedString,
    up_text: SharedString,
    down_total: SharedString,
    up_total: SharedString,
    down_peak: Option<f32>,
    up_peak: Option<f32>,
    down_peak_text: SharedString,
    up_peak_text: SharedString,
    /// ", peak 4.4 Mbps" after each rate; empty while there is no peak.
    down_peak_part: SharedString,
    up_peak_part: SharedString,
    /// The ", peak {peak}" text.
    with_peak: Template,
    /// The busiest adapter's name, the title line.
    adapter: SharedString,
    peak_word: SharedString,
    total_word: SharedString,
    /// The last seen version of the aggregate download and upload series.
    peak_versions: [u64; 2],
    title: SharedString,
    down_word: SharedString,
    up_word: SharedString,
    aggregate: DeviceIdx,
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
    _subs: [Subscription; 5],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
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
        let aggregate = aggregate(&snap.devices);
        let range = tr(cx, KEY_RANGE);
        let chart = |upload: bool, cx: &mut Context<Self>| {
            let spec = chart_spec(aggregate, upload, net.unit, &settings, &theme);
            let labels = chart_labels(range.clone());
            box_chart(spec, labels, hub, cx)
        };
        let (down, up) = (chart(false, cx), chart(true, cx));
        let combined = box_chart(
            pair_spec(aggregate, net.unit, &settings, &theme),
            chart_labels(range.clone()),
            hub,
            cx,
        );
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
            adapters: Vec::new(),
            extra: 0,
            cells: 0,
            body: Rc::new(StdCell::new(size(px(0.), px(0.)))),
            down_text: SharedString::default(),
            up_text: SharedString::default(),
            down_total: SharedString::default(),
            up_total: SharedString::default(),
            down_peak: None,
            up_peak: None,
            down_peak_text: SharedString::default(),
            up_peak_text: SharedString::default(),
            down_peak_part: SharedString::default(),
            up_peak_part: SharedString::default(),
            with_peak: Template::new(cx, KEY_WITH_PEAK, &["peak"]),
            adapter: SharedString::default(),
            peak_word: tr(cx, KEY_PEAK),
            total_word: tr(cx, KEY_TOTAL),
            peak_versions: [0; 2],
            title: tr(cx, KEY_TITLE),
            down_word: tr(cx, KEY_DOWNLOAD),
            up_word: tr(cx, KEY_UPLOAD),
            aggregate,
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
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_peaks(&snap, true);
        this.sync_text(&snap, true);
        this
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
            self.adapters.clear();
            self.extra = 0;
        }
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
        self.adapters.iter().map(|a| a.idx).collect()
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
            if self.mode == NetMode::AdapterCharts {
                cx.notify();
            }
        }
    }

    /// How many `adapter_charts` cells the last render showed.
    pub fn cells(&self) -> usize {
        self.cells
    }

    pub fn down_chart(&self) -> &Entity<ChartView> {
        &self.down
    }

    pub fn up_chart(&self) -> &Entity<ChartView> {
        &self.up
    }

    /// Download rate, upload rate, download peak, upload peak, download total, upload total.
    pub fn texts(&self) -> [&SharedString; 6] {
        [
            &self.down_text,
            &self.up_text,
            &self.down_peak_text,
            &self.up_peak_text,
            &self.down_total,
            &self.up_total,
        ]
    }

    /// "Network\t<adapter>", then "Download\t1.2 Mbps\tPeak 3.4 Mbps\tTotal (since boot) 3.40 GiB"
    /// and the same for upload.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\t');
        out.push_str(&self.adapter);
        out.push('\n');
        for (word, rate, peak, total) in [
            (
                &self.down_word,
                &self.down_text,
                &self.down_peak_text,
                &self.down_total,
            ),
            (
                &self.up_word,
                &self.up_text,
                &self.up_peak_text,
                &self.up_total,
            ),
        ] {
            out.push_str(word);
            out.push('\t');
            out.push_str(rate);
            out.push('\t');
            out.push_str(&self.peak_word);
            out.push(' ');
            out.push_str(peak);
            out.push('\t');
            out.push_str(&self.total_word);
            out.push(' ');
            out.push_str(total);
            out.push('\n');
        }
    }

    /// Reads the 60 s peaks when an aggregate series changed, or always with `force`. True when
    /// a peak value changed.
    fn sync_peaks(&mut self, snap: &Snapshot, force: bool) -> bool {
        let id = |metric| SeriesId {
            device: self.aggregate,
            metric,
        };
        let (rx, tx) = (id(MetricId::NET_RX), id(MetricId::NET_TX));
        let versions = [series_version(snap, rx), series_version(snap, tx)];
        if !force && versions == self.peak_versions {
            return false;
        }
        self.peak_versions = versions;
        let (down, up) = (peak(snap, rx), peak(snap, tx));
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
        let mut old = std::mem::take(&mut self.adapters);
        let mut changed = false;
        let mut total = 0;
        for (idx, n) in snap.system.nets.iter() {
            if !counted(snap, *idx, &self.net) {
                continue;
            }
            total += 1;
            if self.adapters.len() == MAX_ADAPTER_CELLS {
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
                    let spec = pair_spec(*idx, self.net.unit, &self.settings, &theme);
                    AdapterCell {
                        idx: *idx,
                        name,
                        rates,
                        chart: box_chart(spec, chart_labels(range.clone()), &hub, cx),
                    }
                }
            };
            self.adapters.push(cell);
        }
        changed |= !old.is_empty();
        let extra = total - self.adapters.len();
        changed |= extra != self.extra;
        self.extra = extra;
        changed
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
            let aggregate = aggregate(&snap.devices);
            if aggregate != self.aggregate {
                self.aggregate = aggregate;
                self.rebuild_specs(cx);
            }
        }
        let peaks = mask.contains(ComponentMask::SERIES) && self.sync_peaks(&snap, false);
        let text = self.sync_text(&snap, peaks);
        if self.sync_adapters(&snap, cx) || text {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The page was held at a time or made live: the rates, peaks and adapters read the other
    /// snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = timeline::paused(cx).is_some();
        let snap = box_snapshot(&self.hub, cx);
        self.sync_peaks(&snap, true);
        self.sync_text(&snap, true);
        self.sync_adapters(&snap, cx);
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// The rates and totals when their inputs changed, or always with `force`. True when the
    /// text was formatted again. While paused the totals since boot are "-": the history does
    /// not keep them.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> bool {
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
            return false;
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
        self.down_text = rate(s.rx_ok, s.rx);
        self.up_text = rate(s.tx_ok, s.tx);
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
        self.down_peak_text = peak_text(self.down_peak);
        self.up_peak_text = peak_text(self.up_peak);
        let part = |p: Option<f32>, text: &SharedString| {
            let mut out = String::new();
            if p.is_some() {
                let _ = self.with_peak.write(&mut out, &[text]);
            }
            SharedString::from(out)
        };
        let (down_part, up_part) = (
            part(self.down_peak, &self.down_peak_text),
            part(self.up_peak, &self.up_peak_text),
        );
        self.down_peak_part = down_part;
        self.up_peak_part = up_part;
        let paused = self.paused;
        let size = |bytes: u64| {
            if paused {
                return value_text(Validity::Missing, |_| {});
            }
            let mut out = String::new();
            units::bytes(&mut out, bytes);
            SharedString::from(out)
        };
        self.down_total = size(s.rx_total);
        self.up_total = size(s.tx_total);
        self.adapter = adapter.to_owned().into();
        true
    }

    /// Applies the chart, unit and interface class settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
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
        let respec = settings != self.settings || net.unit != self.net.unit;
        self.settings = settings;
        self.net = net;
        if respec {
            self.rebuild_specs(cx);
        }
        let snap = box_snapshot(&self.hub, cx);
        let text = self.sync_text(&snap, false);
        if self.sync_adapters(&snap, cx) || text {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// A new theme or UI scale: new chart colours, and the rows draw in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_specs(cx);
        cx.notify();
    }

    /// A new language: new title, words, the Peak and Total labels and the peak text.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        self.down_word = tr(cx, KEY_DOWNLOAD);
        self.up_word = tr(cx, KEY_UPLOAD);
        self.peak_word = tr(cx, KEY_PEAK);
        self.total_word = tr(cx, KEY_TOTAL);
        self.with_peak = Template::new(cx, KEY_WITH_PEAK, &["peak"]);
        let range = tr(cx, KEY_RANGE);
        for v in [self.down.clone(), self.up.clone(), self.combined.clone()]
            .into_iter()
            .chain(self.adapters.iter().map(|a| a.chart.clone()))
        {
            let labels = chart_labels(range.clone());
            v.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        let snap = box_snapshot(&self.hub, cx);
        self.sync_text(&snap, true);
        cx.notify();
    }

    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let pair = pair_spec(self.aggregate, self.net.unit, &self.settings, &theme);
        self.combined.update(cx, |v, cx| v.set_spec(pair, cx));
        for a in &self.adapters {
            let spec = pair_spec(a.idx, self.net.unit, &self.settings, &theme);
            a.chart.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        for (view, upload) in [(self.down.clone(), false), (self.up.clone(), true)] {
            let spec = chart_spec(
                self.aggregate,
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
        value_el = value_el.debug_selector(move || id.into());
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
        .debug_selector({
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
        .debug_selector(move || cell_id(idx))
        .flex()
        .flex_col()
        .flex_none()
        .w(cell.width)
        .h(cell.height)
        .overflow_hidden()
        .child(cell_head(a, m, theme))
        .child(
            div()
                .debug_selector(move || chart_id(idx))
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

impl NetBox {
    /// `stacked`: the Download part, the splitter and the Upload part, each the full width.
    fn stacked_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cells = 0;
        let share = self.split(m);
        let [down_line, up_line] = self.rate_lines(m, theme, window, cx);
        let down = column(down_line, &self.down);
        let up = column(up_line, &self.up);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cells = 0;
        let share = self.split(m);
        let [down_line, up_line] = self.rate_lines(m, theme, window, cx);
        let down = column(down_line, &self.down);
        let up = column(up_line, &self.up);
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

    /// The Download and Upload rate lines, each with its peak and its total in the tooltip.
    fn rate_lines(
        &self,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> [AnyElement; 2] {
        let down = RateLine {
            id: DOWN_RATE_ID,
            word: &self.down_word,
            rate: &self.down_text,
            peak: &self.down_peak_part,
            total_word: &self.total_word,
            total: &self.down_total,
        };
        let up = RateLine {
            id: UP_RATE_ID,
            word: &self.up_word,
            rate: &self.up_text,
            peak: &self.up_peak_part,
            total_word: &self.total_word,
            total: &self.up_total,
        };
        [
            rate_line(down, m, theme, window, cx),
            rate_line(up, m, theme, window, cx),
        ]
    }

    /// `combined`: the two rate lines side by side over one chart with both lines.
    fn combined_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cells = 0;
        let [down, up] = self.rate_lines(m, theme, window, cx);
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
                    .debug_selector(|| COMBINED_CHART_ID.into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        self.combined
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
            .into_any_element()
    }

    /// The "+n more" text for `shown` cells shown, empty when every counted adapter shows.
    fn more_text(&self, shown: usize, cx: &Context<Self>) -> SharedString {
        let n = self.adapters.len() + self.extra - shown;
        if n == 0 {
            SharedString::default()
        } else {
            tr_args(cx, KEY_MORE, &[("n", FluentValue::from(n))])
        }
    }

    /// `adapter_charts`: one combined chart per counted adapter in one column, one row or a
    /// grid. Only whole cells show; the adapters left out join "+n more" (UI-PRF-059).
    fn charts_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.adapters.len();
        let total = rows + self.extra;
        let arrange = self.arrange;
        let fit = charts_fit(arrange, self.body.get(), rows, total, m);
        self.cells = fit.shown;
        let cells = grid_cells(
            &fit.grid,
            self.adapters
                .iter()
                .take(fit.shown)
                .map(|a| chart_cell(a, fit.grid.cell, m, theme)),
            m.sp[2],
            GRID_ID,
        );
        // A "+n more" line that does not fit whole is left out (UI-PRF-017).
        let mut body = whole_rows().relative().size_full().gap(m.sp[2]).child(
            div()
                .debug_selector(|| GRID_ID.into())
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(cells),
        );
        let more = self.more_text(fit.shown, cx);
        if !more.is_empty() {
            body = body.child(more_line(more, m, theme));
        }
        let probe_m = *m;
        body.child(class_probe(
            Rc::clone(&self.body),
            fit.class(),
            move |b| charts_fit(arrange, b, rows, total, &probe_m).class(),
            cx.entity_id(),
        ))
        .into_any_element()
    }
}

impl Render for NetBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let body = match self.mode {
            NetMode::Stacked => self.stacked_body(&m, &theme, window, cx),
            NetMode::Columns => self.columns_body(&m, &theme, window, cx),
            NetMode::Combined => self.combined_body(&m, &theme, window, cx),
            NetMode::AdapterCharts => self.charts_body(&m, &theme, cx),
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
