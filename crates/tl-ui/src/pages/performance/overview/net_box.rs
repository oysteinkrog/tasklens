//! The Overview page's Network box (UI-PRF-017, D-022, UI-PRF-032, PERF-054): the download and
//! upload charts of all interfaces, with the summed rates and the totals since boot.
//!
//! The rates and totals sum the present interfaces other than `lo` whose class the
//! `show_network_*` settings show, the set the engine's aggregate counts. The box formats its
//! text only when the inputs' hash changes and notifies only then. Pointer movement never
//! notifies.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Window, div, px, white,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{NetworkUnit, Settings};
use tl_model::snapshot::Snapshot;
use tl_model::units;
use tl_model::validity::Validity;

use super::cpu_box::value_text;
use super::parts::{ChartSettings, box_chart, box_frame, chart_section_versions};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::{Section, TlSettings};
use crate::text::style::{numeric_style, t_xs};
use crate::theme::derived::blend_over;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const NET_BOX_ID: &str = "perf-overview-box-net";

const KEY_TITLE: &str = "perf-overview-box-network";
const KEY_DOWNLOAD: &str = "perf-net-legend-download";
const KEY_UPLOAD: &str = "perf-net-legend-upload";
const KEY_SINCE_BOOT: &str = "perf-overview-since-boot";
const KEY_RANGE: &str = "perf-net-heading-throughput";

/// The upload line: the network colour blended with white, as the sidebar draws it.
const UPLOAD_TINT: f32 = 0.3;

/// The loopback interface, never counted.
const LOOPBACK: &str = "lo";

/// The settings the box reads besides [`ChartSettings`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct NetSettings {
    unit: NetworkUnit,
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

/// One direction's chart: download filled, upload as a lighter line.
fn chart_spec(
    device: DeviceIdx,
    upload: bool,
    unit: NetworkUnit,
    s: &ChartSettings,
    theme: &TlTheme,
) -> ChartSpec {
    let g = &theme.graph;
    let series = if upload {
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
    };
    ChartSpec {
        series: smallvec![series],
        y: YScale::AutoRate {
            iec: unit == NetworkUnit::Bytes,
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
    };
    for (idx, n) in snap.system.nets.iter() {
        let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == *idx) else {
            continue;
        };
        let loopback =
            matches!(&entry.identity, DeviceIdentity::Net { name, .. } if &**name == LOOPBACK);
        if !entry.present || loopback || !s.shown(entry.net_kind) {
            continue;
        }
        out.counted += 1;
        if n.rx_bps.validity.is_valid() {
            out.rx += n.rx_bps.value;
            out.rx_ok = true;
        }
        if n.tx_bps.validity.is_valid() {
            out.tx += n.tx_bps.value;
            out.tx_ok = true;
        }
        out.rx_total += n.rx_total;
        out.tx_total += n.tx_total;
    }
    out
}

/// The Network box.
pub struct NetBox {
    hub: Entity<SnapshotHub>,
    down: Entity<ChartView>,
    up: Entity<ChartView>,
    down_text: SharedString,
    up_text: SharedString,
    down_total: SharedString,
    up_total: SharedString,
    title: SharedString,
    down_word: SharedString,
    up_word: SharedString,
    aggregate: DeviceIdx,
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
        let mut this = Self {
            hub: hub.clone(),
            down,
            up,
            down_text: SharedString::default(),
            up_text: SharedString::default(),
            down_total: SharedString::default(),
            up_total: SharedString::default(),
            title: tr(cx, KEY_TITLE),
            down_word: tr(cx, KEY_DOWNLOAD),
            up_word: tr(cx, KEY_UPLOAD),
            aggregate,
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
        this.sync_text(&snap, true, cx);
        this
    }

    pub fn down_chart(&self) -> &Entity<ChartView> {
        &self.down
    }

    pub fn up_chart(&self) -> &Entity<ChartView> {
        &self.up
    }

    /// Download rate, upload rate, download since boot, upload since boot.
    pub fn texts(&self) -> [&SharedString; 4] {
        [
            &self.down_text,
            &self.up_text,
            &self.down_total,
            &self.up_total,
        ]
    }

    /// "Network", then "Download\t1.2 Mbps\t3.40 GiB since boot" and the same for upload.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\n');
        for (word, rate, total) in [
            (&self.down_word, &self.down_text, &self.down_total),
            (&self.up_word, &self.up_text, &self.up_total),
        ] {
            out.push_str(word);
            out.push('\t');
            out.push_str(rate);
            out.push('\t');
            out.push_str(total);
            out.push('\n');
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
                self.rebuild_specs(cx);
            }
        }
        if self.sync_text(&snap, false, cx) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The rates and totals when their inputs changed, or always with `force`. True when the
    /// text was formatted again.
    fn sync_text(&mut self, snap: &Snapshot, force: bool, cx: &mut Context<Self>) -> bool {
        let s = sums(snap, &self.net);
        let mut h = FxHasher::default();
        h.write_u64(s.rx.to_bits());
        h.write_u64(s.tx.to_bits());
        h.write_u8(u8::from(s.rx_ok) | u8::from(s.tx_ok) << 1);
        h.write_u64(s.rx_total);
        h.write_u64(s.tx_total);
        h.write_usize(s.counted);
        self.net.unit.hash(&mut h);
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        let unit = self.net.unit;
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
        let since = |bytes: u64| {
            let mut size = String::new();
            units::bytes(&mut size, bytes);
            tr_args(cx, KEY_SINCE_BOOT, &[("size", FluentValue::from(size))])
        };
        self.down_total = since(s.rx_total);
        self.up_total = since(s.tx_total);
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
        if self.sync_text(&snap, false, cx) {
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

    /// A new language: new title, words and "since boot".
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        self.down_word = tr(cx, KEY_DOWNLOAD);
        self.up_word = tr(cx, KEY_UPLOAD);
        let range = tr(cx, KEY_RANGE);
        for v in [self.down.clone(), self.up.clone()] {
            let labels = chart_labels(range.clone());
            v.update(cx, |v, cx| v.set_labels(labels, cx));
        }
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_text(&snap, true, cx);
        cx.notify();
    }

    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
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

/// One direction: its chart, then "Download 1.2 Mbps" and the total since boot.
fn column(
    chart: &Entity<ChartView>,
    word: &SharedString,
    rate: &SharedString,
    total: &SharedString,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let row_h = px(LABEL_ROW * m.ui_scale);
    let mut word_el = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(word.clone());
    *word_el.text_style() = t_xs(m);
    let mut rate_el = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(rate.clone());
    *rate_el.text_style() = numeric_style(m, m.text.sm);
    let mut total_el = div()
        .flex_none()
        .h(row_h)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(total.clone());
    *total_el.text_style() = t_xs(m);
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .gap(m.sp[2])
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(chart.clone().cached(StyleRefinement::default().size_full())),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .justify_between()
                .h(row_h)
                .child(word_el)
                .child(rate_el),
        )
        .child(total_el)
        .into_any_element()
}

impl Render for NetBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let body = div()
            .flex()
            .flex_row()
            .size_full()
            .gap(m.sp[5])
            .child(column(
                &self.down,
                &self.down_word,
                &self.down_text,
                &self.down_total,
                &m,
                &theme,
            ))
            .child(column(
                &self.up,
                &self.up_word,
                &self.up_text,
                &self.up_total,
                &m,
                &theme,
            ));
        box_frame(
            NET_BOX_ID,
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
