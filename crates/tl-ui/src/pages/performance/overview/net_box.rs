//! The Overview page's Network box (UI-PRF-017, D-022, UI-PRF-032, PERF-054): the download and
//! upload charts of all interfaces, with the summed rates, the 60 s peaks and the totals since
//! boot. The busiest interface's name is on the title line.
//!
//! The rates and totals sum the present interfaces other than `lo` whose class the
//! `show_network_*` settings show, the set the engine's aggregate counts. The box formats its
//! text only when the inputs' hash changes and notifies only then. Pointer movement never
//! notifies.
//!
//! UI-PRF-019 gives the box three modes. `columns` is the Download and Upload columns above with
//! a vertical splitter between them. `combined` is one chart with download and upload as two
//! lines, under a rate header and over the Peak and Total rows of both directions.
//! `adapter_charts` is a grid of one combined chart per counted adapter, in cells of at least
//! 200 × 80 px. The adapter charts are built when the mode is shown and when the set of counted
//! adapters changes, never per tick.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::cell::Cell as StdCell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement, Styled as _,
    Subscription, Window, div, px, relative, size, white,
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
use super::cpu_box::value_text;
use super::disk_box::{GRID_CELL_MIN_H, charts_fit, more_line};
use super::inner_split::{InnerSplit, InnerSplitChanged, SplitAxis};
use super::layout::BoxMode;
use super::parts::{ChartSettings, box_chart, box_frame, chart_section_versions, class_probe};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, NetFormat, ValueAlign,
    ValueFormat, YScale, format_value,
};
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

/// The debug selector of the splitter between the stacked Download and Upload parts.
pub const NET_STACK_SPLITTER_ID: &str = "perf-overview-net-stack-splitter";
/// The debug selector of that splitter's accent line.
pub const NET_STACK_SPLITTER_LINE_ID: &str = "perf-overview-net-stack-splitter-line";
/// The debug selector of the splitter between the Download and Upload columns.
pub const NET_SPLITTER_ID: &str = "perf-overview-net-splitter";
/// The debug selector of the splitter's accent line.
pub const NET_SPLITTER_LINE_ID: &str = "perf-overview-net-splitter-line";
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

const KEY_TITLE: &str = "perf-overview-box-network";
const KEY_DOWNLOAD: &str = "perf-net-legend-download";
const KEY_UPLOAD: &str = "perf-net-legend-upload";
const KEY_PEAK: &str = "perf-overview-net-peak";
const KEY_TOTAL: &str = "perf-overview-net-total";
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
fn aggregate(catalog: &DeviceCatalog) -> DeviceIdx {
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
    hub: Entity<SnapshotHub>,
    down: Entity<ChartView>,
    up: Entity<ChartView>,
    /// Download and upload as two lines of the aggregate series (`combined`).
    combined: Entity<ChartView>,
    mode: NetMode,
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
    settings: ChartSettings,
    net: NetSettings,
    settings_versions: [u64; 4],
    style_version: u64,
    strings_version: u64,
    _subs: [Subscription; 4],
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
        let snap = Arc::clone(hub.read(cx).current());
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
            hub: hub.clone(),
            down,
            up,
            combined,
            mode: NetMode::Stacked,
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
            settings,
            net,
            settings_versions: section_versions(cx),
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
            let snap = Arc::clone(self.hub.read(cx).current());
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
            NetMode::Stacked => self
                .stack_split
                .share(SplitAxis::Column, m.sp[5], m.ui_scale),
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

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let devices = ev.mask.contains(ComponentMask::DEVICES);
        if !(devices
            || ev.mask.intersects(
                ComponentMask::SYSTEM | ComponentMask::SERIES | ComponentMask::DEVICE_DETAILS,
            ))
        {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        if devices && snap.devices.version != self.catalog_version {
            self.catalog_version = snap.devices.version;
            let aggregate = aggregate(&snap.devices);
            if aggregate != self.aggregate {
                self.aggregate = aggregate;
                self.rebuild_specs(cx);
            }
        }
        let peaks = ev.mask.contains(ComponentMask::SERIES) && self.sync_peaks(&snap, false);
        let text = self.sync_text(&snap, peaks);
        if self.sync_adapters(&snap, cx) || text {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The rates and totals when their inputs changed, or always with `force`. True when the
    /// text was formatted again.
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
        let size = |bytes: u64| {
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
        let snap = Arc::clone(self.hub.read(cx).current());
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

    /// A new language: new title, words and the Peak and Total labels.
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
        let range = tr(cx, KEY_RANGE);
        for v in [self.down.clone(), self.up.clone(), self.combined.clone()]
            .into_iter()
            .chain(self.adapters.iter().map(|a| a.chart.clone()))
        {
            let labels = chart_labels(range.clone());
            v.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        let snap = Arc::clone(self.hub.read(cx).current());
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

/// A row with a label on the left and a value on the right.
fn stat_row(
    label: &SharedString,
    value: &SharedString,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let mut label_el = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(label.clone());
    *label_el.text_style() = t_xs(m);
    let mut value_el = div()
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_color(theme.text_primary)
        .child(value.clone());
    *value_el.text_style() = numeric_style(m, m.text.sm);
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(m.sp[2])
        .h(px(LABEL_ROW * m.ui_scale))
        .child(label_el)
        .child(value_el)
        .into_any_element()
}

/// One direction: "Download 1.2 Mbps", its chart, then the Peak and Total rows.
struct Column<'a> {
    chart: &'a Entity<ChartView>,
    word: &'a SharedString,
    rate: &'a SharedString,
    peak_word: &'a SharedString,
    peak: &'a SharedString,
    total_word: &'a SharedString,
    total: &'a SharedString,
}

fn column(c: Column, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .gap(m.sp[2])
        .child(stat_row(c.word, c.rate, m, theme))
        .child(
            div().relative().flex_1().min_h_0().w_full().child(
                c.chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
        )
        .child(stat_row(c.peak_word, c.peak, m, theme))
        .child(stat_row(c.total_word, c.total, m, theme))
        .into_any_element()
}

/// A half of a `combined` row: label and value.
fn half(label: &SharedString, value: &SharedString, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .child(stat_row(label, value, m, theme))
        .into_any_element()
}

/// A `combined` row: the download half, then the upload half.
fn pair_row(
    label: (&SharedString, &SharedString),
    down: &SharedString,
    up: &SharedString,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .gap(m.sp[5])
        .child(half(label.0, down, m, theme))
        .child(half(label.1, up, m, theme))
        .into_any_element()
}

/// An `adapter_charts` cell header: the adapter name and its two rates.
fn cell_head(a: &AdapterCell, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let mut name = div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_primary)
        .child(a.name.clone());
    *name.text_style() = label_style(m, m.text.sm, FontWeight::MEDIUM);
    let mut rates = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(a.rates.clone());
    *rates.text_style() = numeric_style(m, m.text.sm);
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * m.ui_scale))
        .child(name)
        .child(rates)
        .into_any_element()
}

/// An `adapter_charts` cell: the header over the combined chart.
fn chart_cell(a: &AdapterCell, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let idx = a.idx;
    div()
        .debug_selector(move || cell_id(idx))
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h(px(GRID_CELL_MIN_H * m.ui_scale))
        .gap(m.sp[2])
        .child(cell_head(a, m, theme))
        .child(
            div().relative().flex_1().min_h_0().child(
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cells = 0;
        let share = self.split(m);
        let down = column(
            Column {
                chart: &self.down,
                word: &self.down_word,
                rate: &self.down_text,
                peak_word: &self.peak_word,
                peak: &self.down_peak_text,
                total_word: &self.total_word,
                total: &self.down_total,
            },
            m,
            theme,
        );
        let up = column(
            Column {
                chart: &self.up,
                word: &self.up_word,
                rate: &self.up_text,
                peak_word: &self.peak_word,
                peak: &self.up_peak_text,
                total_word: &self.total_word,
                total: &self.up_total,
            },
            m,
            theme,
        );
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[5])
            .child(
                div()
                    .flex()
                    .flex_none()
                    .w_full()
                    .h(relative(share))
                    .child(down),
            )
            .child(div().flex().flex_1().min_h_0().w_full().child(up))
            .child(self.stack_split.probe(Self::stack_inner_split, cx))
            .child(self.stack_split.gap(
                SplitAxis::Column,
                share,
                m.sp[5],
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cells = 0;
        let share = self.split(m);
        let down = column(
            Column {
                chart: &self.down,
                word: &self.down_word,
                rate: &self.down_text,
                peak_word: &self.peak_word,
                peak: &self.down_peak_text,
                total_word: &self.total_word,
                total: &self.down_total,
            },
            m,
            theme,
        );
        let up = column(
            Column {
                chart: &self.up,
                word: &self.up_word,
                rate: &self.up_text,
                peak_word: &self.peak_word,
                peak: &self.up_peak_text,
                total_word: &self.total_word,
                total: &self.up_total,
            },
            m,
            theme,
        );
        div()
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .gap(m.sp[5])
            .child(
                div()
                    .flex()
                    .flex_none()
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

    /// `combined`: the rates, one chart with both lines, then the Peak and Total rows.
    fn combined_body(&mut self, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
        self.cells = 0;
        let words = (&self.down_word, &self.up_word);
        let peak = (&self.peak_word, &self.peak_word);
        let total = (&self.total_word, &self.total_word);
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[2])
            .child(pair_row(words, &self.down_text, &self.up_text, m, theme))
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
            .child(pair_row(
                peak,
                &self.down_peak_text,
                &self.up_peak_text,
                m,
                theme,
            ))
            .child(pair_row(total, &self.down_total, &self.up_total, m, theme))
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

    /// `adapter_charts`: a grid of one combined chart per counted adapter.
    fn charts_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.adapters.len();
        let total = rows + self.extra;
        let shown = charts_fit(self.body.get(), rows, total, m);
        self.cells = shown.shown;
        let mut body = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .gap(m.sp[2]);
        let mut grid = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .gap(m.sp[2]);
        for chunk in self.adapters[..shown.shown].chunks(shown.cols) {
            let mut line = div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(GRID_CELL_MIN_H * m.ui_scale))
                .gap(m.sp[2]);
            for a in chunk {
                line = line.child(chart_cell(a, m, theme));
            }
            for _ in chunk.len()..shown.cols {
                line = line.child(div().flex_1().min_w_0());
            }
            grid = grid.child(line);
        }
        body = body.child(grid);
        let more = self.more_text(shown.shown, cx);
        if !more.is_empty() {
            body = body.child(more_line(more, m, theme));
        }
        let probe_m = *m;
        body.child(class_probe(
            Rc::clone(&self.body),
            shown.class(),
            move |b| charts_fit(b, rows, total, &probe_m).class(),
            cx.entity_id(),
        ))
        .into_any_element()
    }
}

impl Render for NetBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let body = match self.mode {
            NetMode::Stacked => self.stacked_body(&m, &theme, cx),
            NetMode::Columns => self.columns_body(&m, &theme, cx),
            NetMode::Combined => self.combined_body(&m, &theme),
            NetMode::AdapterCharts => self.charts_body(&m, &theme, cx),
        };
        box_frame(
            NET_BOX_ID,
            self.target.clone(),
            self.title.clone(),
            SharedString::default(),
            self.adapter.clone(),
            body,
            &m,
            &theme,
        )
        .size_full()
    }
}
