//! The Network page: one page per interface with the Throughput chart above the Details grid,
//! its scale (auto or link speed), the disconnected state and the page's demand
//! (docs/ui/performance.md UI-PRF-031, UI-PRF-020; PERF-018, PERF-054, PERF-055).
//!
//! The chart is a cached view that re-renders for its own series. The page re-renders only when
//! its header changes. The spec is built again only when the link speed, a chart setting or the
//! theme changed.
//!
//! A disconnected interface needs nothing from the page: the engine marks its samples invalid,
//! so the chart draws a gap after the last valid sample (E19-003).
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::fmt::{self, Write as _};
use std::net::Ipv6Addr;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, EntityId,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, StyleRefinement, Styled as _, Subscription, Window, canvas, div, fill,
    point, px, size,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::devices::{DeviceIdentity, NetKind};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{InvField, NetDetails, Snapshot, WifiInfo};
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::time::now_boot_ns;
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;

use super::actions::NoOp;
use super::details::{Fallback, KeyValueModel};
use super::page::{
    DetailsLinkClick, DetailsValuePrefix, DevicePage, DevicePageFrame, GRAPH_H_SINGLE, header_text,
    page_words,
};
use super::{DeviceKey, DeviceKind, DeviceRow, NetClass};
use crate::actions::SetSetting;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, SettingKey, SettingValue, TlSettings};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Debug selector of the chart box.
pub const THROUGHPUT_ID: &str = "perf-net-throughput";

const KEY_THROUGHPUT: &str = "perf-net-heading-throughput";
const KEY_DOWNLOAD: &str = "perf-net-legend-download";
const KEY_UPLOAD: &str = "perf-net-legend-upload";

const NS_PER_S: u64 = 1_000_000_000;
/// Alpha of the upload line against `net_line`.
const UPLOAD_ALPHA: f32 = 0.7;
/// Bits per second in one Mb/s, the unit of the sysfs `speed`.
const BPS_PER_MBPS: f32 = 1_000_000.;

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub throughput: SharedString,
    pub download: SharedString,
    pub upload: SharedString,
}

impl Words {
    /// Translates every word when the strings changed since the last call. True when it did.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        let t = |key| tr(cx, key);
        self.throughput = t(KEY_THROUGHPUT);
        self.download = t(KEY_DOWNLOAD);
        self.upload = t(KEY_UPLOAD);
        true
    }
}

/// The settings the page draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    /// `units.network`.
    unit: NetworkUnit,
    /// `units.network_auto_scale`.
    auto_scale: bool,
    /// `performance.network_dynamic_scale`.
    dynamic_scale: bool,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
            unit: s.units.network,
            auto_scale: s.units.network_auto_scale,
            dynamic_scale: s.performance.network_dynamic_scale,
        }
    }
}

/// The versions of the sections [`PageSettings`] reads.
fn section_versions(cx: &App) -> [u64; 4] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
        tl.version(Section::Units),
    ]
}

/// The Throughput chart's Y scale (UI-PRF-031): the link speed when the scale follows it and the
/// speed is known, otherwise auto, with powers of two under bytes (UI-PRF-001 fallback).
pub fn throughput_scale(
    dynamic_scale: bool,
    link_speed_bps: Option<f32>,
    unit: NetworkUnit,
) -> YScale {
    match link_speed_bps {
        Some(speed) if !dynamic_scale => YScale::LinkSpeed(speed),
        _ => YScale::AutoRate {
            iec: unit == NetworkUnit::Bytes,
        },
    }
}

/// The interface's sysfs link speed in bits per second, `None` when it is not Valid or not above
/// 0 (sysfs `speed` -1).
fn link_speed_bps(snap: &Snapshot, device: DeviceIdx) -> Option<f32> {
    snap.system
        .nets
        .iter()
        .find(|(i, _)| *i == device)
        .map(|(_, n)| n.link_speed_mbps)
        .filter(|m| m.validity == Validity::Valid && m.value > 0)
        .map(|m| m.value as f32 * BPS_PER_MBPS)
}

/// Download with fill, upload as a fainter dashed line.
fn throughput_spec(device: DeviceIdx, y: YScale, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.net_line;
    let id = |metric| SeriesId { device, metric };
    ChartSpec {
        series: smallvec![
            ChartSeries {
                id: id(MetricId::NET_RX),
                fill: Some(theme.graph.net_fill),
                line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: id(MetricId::NET_TX),
                line: Hsla {
                    a: line.a * UPLOAD_ALPHA,
                    ..line
                },
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y,
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
        },
        smooth: s.smooth,
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

fn throughput_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.throughput.clone()),
        value_fmt: ValueFormat::RateNetwork,
        value_align: ValueAlign::Right,
        range_word: w.throughput.clone(),
        legend: smallvec![
            (w.download.clone(), LegendStyle::Solid),
            (w.upload.clone(), LegendStyle::Dashed)
        ],
        show_axis_max: true,
    }
}

/// The chart state from the network source health: Failed with its error, else Live (the view
/// derives WarmingUp and Stale itself).
fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Net);
    match h.state {
        SourceState::Failed => ChartState::Failed(
            h.last_error
                .as_deref()
                .unwrap_or_default()
                .to_owned()
                .into(),
        ),
        _ => ChartState::Live,
    }
}

// ---- Details grid (UI-PRF-031, PERF-056, PERF-057) ----

pub const ROW_DOWNLOAD: &str = "perf-net-row-download";
pub const ROW_UPLOAD: &str = "perf-net-row-upload";
pub const ROW_TOTAL_DOWN: &str = "perf-net-row-total-down";
pub const ROW_TOTAL_UP: &str = "perf-net-row-total-up";
pub const ROW_ADAPTER: &str = "perf-net-row-adapter";
pub const ROW_INTERFACE: &str = "perf-net-row-interface";
pub const ROW_TYPE: &str = "perf-net-row-type";
pub const ROW_STATUS: &str = "perf-net-row-status";
pub const ROW_SSID: &str = "perf-net-row-ssid";
pub const ROW_SIGNAL: &str = "perf-net-row-signal";
pub const ROW_RX_BITRATE: &str = "perf-net-row-rx-bitrate";
pub const ROW_TX_BITRATE: &str = "perf-net-row-tx-bitrate";
pub const ROW_FREQUENCY: &str = "perf-net-row-frequency";
pub const ROW_SECURITY: &str = "perf-net-row-security";
pub const ROW_LINK_SPEED: &str = "perf-net-row-link-speed";
pub const ROW_IPV4: &str = "perf-net-row-ipv4";
pub const ROW_IPV6: &str = "perf-net-row-ipv6";
pub const ROW_MAC: &str = "perf-net-row-mac";
pub const ROW_IFINDEX: &str = "perf-net-row-ifindex";
pub const ROW_DRIVER: &str = "perf-net-row-driver";
pub const ROW_DISPLAY_UNIT: &str = "perf-net-row-display-unit";
pub const ROW_PUBLIC_IP: &str = "perf-net-row-public-ip";

/// The rows before the Wi-Fi rows, in grid order.
const HEAD_ROWS: [&str; 8] = [
    ROW_DOWNLOAD,
    ROW_UPLOAD,
    ROW_TOTAL_DOWN,
    ROW_TOTAL_UP,
    ROW_ADAPTER,
    ROW_INTERFACE,
    ROW_TYPE,
    ROW_STATUS,
];
/// The Wi-Fi rows, after Connection status.
const WIFI_ROWS: [&str; 6] = [
    ROW_SSID,
    ROW_SIGNAL,
    ROW_RX_BITRATE,
    ROW_TX_BITRATE,
    ROW_FREQUENCY,
    ROW_SECURITY,
];
/// The rows after the Wi-Fi rows.
const TAIL_ROWS: [&str; 8] = [
    ROW_LINK_SPEED,
    ROW_IPV4,
    ROW_IPV6,
    ROW_MAC,
    ROW_IFINDEX,
    ROW_DRIVER,
    ROW_DISPLAY_UNIT,
    ROW_PUBLIC_IP,
];

/// The rows [`update_net_details_p1`] fills. [`update_net_details_p2`] shows the rest except
/// Public IP, which E25-021 fills.
const P1_ROWS: [&str; 5] = [
    ROW_DOWNLOAD,
    ROW_UPLOAD,
    ROW_TOTAL_DOWN,
    ROW_TOTAL_UP,
    ROW_DISPLAY_UNIT,
];

/// The Display unit values and the menu words (PERF-057).
pub const KEY_UNIT_MBPS: &str = "perf-net-unit-mbps";
pub const KEY_UNIT_MIBS: &str = "perf-net-unit-mibs";
pub const KEY_UNIT_BITS_AUTO: &str = "perf-net-unit-bits-auto";
pub const KEY_UNIT_BYTES_AUTO: &str = "perf-net-unit-bytes-auto";
pub const KEY_UNIT_TOOLTIP: &str = "perf-net-unit-tooltip";
pub const KEY_UNIT_BITS: &str = "perf-net-unit-bits";
pub const KEY_UNIT_BYTES: &str = "perf-net-unit-bytes";

/// The Network Details model: the PERF-056 rows in order, the Wi-Fi rows only when `wifi`. All
/// but the P1 rows stay hidden until [`update_net_details_p2`] shows them; Public IP stays
/// hidden until E25-021.
pub fn net_details_model(wifi: bool) -> KeyValueModel {
    let mut labels: SmallVec<[&'static str; 22]> = SmallVec::new();
    labels.extend_from_slice(&HEAD_ROWS);
    if wifi {
        labels.extend_from_slice(&WIFI_ROWS);
    }
    labels.extend_from_slice(&TAIL_ROWS);
    let mut model = KeyValueModel::new(&labels).with_texts(&[
        (KEY_UNIT_MBPS, &[]),
        (KEY_UNIT_MIBS, &[]),
        (KEY_UNIT_BITS_AUTO, &[]),
        (KEY_UNIT_BYTES_AUTO, &[]),
        (KEY_UNIT_TOOLTIP, &[]),
        (KEY_UNKNOWN_ADAPTER, &[]),
        (KEY_LINK_SPEED, &["mbps"]),
        (KEY_LINK_SPEED_WIFI, &["rx", "tx"]),
        (KEY_SIGNAL, &["dbm", "pct"]),
        (KEY_SIGNAL_PCT, &["pct"]),
        (KEY_FREQUENCY, &["mhz", "ch"]),
        (KEY_LINK_LOCAL, &["addrs"]),
    ]);
    for (_, key) in TYPE_KEYS {
        model = model.with_texts(&[(key, &[])]);
    }
    for (_, key) in STATE_KEYS {
        model = model.with_texts(&[(key, &[])]);
    }
    for label in labels {
        if !P1_ROWS.contains(&label) {
            model.hide(label);
        }
    }
    model
}

const KEY_UNKNOWN_ADAPTER: &str = "perf-unknown-adapter";
const KEY_LINK_SPEED: &str = "perf-net-link-speed-value";
const KEY_LINK_SPEED_WIFI: &str = "perf-net-link-speed-wifi";
const KEY_SIGNAL: &str = "perf-net-signal-value";
const KEY_SIGNAL_PCT: &str = "perf-net-signal-pct";
const KEY_FREQUENCY: &str = "perf-net-frequency-value";
const KEY_LINK_LOCAL: &str = "perf-net-link-local";

/// The PERF-054 connection type words by the engine's `type_label`.
const TYPE_KEYS: [(&str, &str); 9] = [
    ("Ethernet", "perf-net-type-ethernet"),
    ("Wi-Fi", "perf-net-type-wifi"),
    ("Virtual", "perf-net-type-virtual"),
    ("USB network", "perf-net-type-usb"),
    ("Bridge", "perf-net-type-bridge"),
    ("Cellular", "perf-net-type-cellular"),
    ("Tunnel", "perf-net-type-tunnel"),
    ("VPN", "perf-net-type-vpn"),
    ("Other", "perf-net-type-other"),
];

/// The PERF-054 connection status words by link state.
const STATE_KEYS: [(LinkState, &str); 7] = [
    (LinkState::Up, "perf-net-state-connected"),
    (LinkState::Down, "perf-net-state-disconnected"),
    (LinkState::Testing, "perf-net-state-testing"),
    (LinkState::Dormant, "perf-net-state-dormant"),
    (LinkState::NotPresent, "perf-net-state-not-present"),
    (LinkState::LowerLayerDown, "perf-net-state-lower-layer-down"),
    (LinkState::Unknown, "perf-net-state-unknown"),
];

/// The key of the Display unit value for `unit` and `auto_scale`: `Mbps`, `MiB/s`,
/// `bits/s, auto` or `bytes/s, auto`.
pub fn display_unit_key(unit: NetworkUnit, auto_scale: bool) -> &'static str {
    match (unit, auto_scale) {
        (NetworkUnit::Bits, false) => KEY_UNIT_MBPS,
        (NetworkUnit::Bytes, false) => KEY_UNIT_MIBS,
        (NetworkUnit::Bits, true) => KEY_UNIT_BITS_AUTO,
        (NetworkUnit::Bytes, true) => KEY_UNIT_BYTES_AUTO,
    }
}

/// What the P1 rows are read from.
#[derive(Clone, Copy)]
pub struct NetDetailInputs<'a> {
    pub now_boot_ns: u64,
    /// `observed_end` of the system component, for the Stale age.
    pub observed_end_ns: u64,
    /// The interface's record in `Snapshot.system`; `None` before its first sample.
    pub totals: Option<&'a NetTotals>,
    /// `units.network`.
    pub unit: NetworkUnit,
    /// `units.network_auto_scale`.
    pub auto_scale: bool,
}

/// One rate row: Valid formats when the rate or the unit changed, Stale keeps the value and
/// marks it, Missing (no sample yet) reads "Collecting…", the rest show their fallback word.
fn rate_row(
    model: &mut KeyValueModel,
    label: &'static str,
    m: Metric<f64>,
    inp: &NetDetailInputs,
) -> bool {
    let (unit, auto) = (inp.unit, inp.auto_scale);
    let net_unit = match unit {
        NetworkUnit::Bits => NetUnit::Bits,
        NetworkUnit::Bytes => NetUnit::Bytes,
    };
    // The rate at f32 precision (far finer than the one printed decimal) in the low half and
    // the unit in the high half, so a unit change formats again.
    let tag = u64::from(unit == NetworkUnit::Bytes) | u64::from(auto) << 1;
    let bits = u64::from((m.value as f32).to_bits()) | tag << 32;
    let write = |o: &mut String| units::net_rate(o, m.value, net_unit, auto);
    match m.validity {
        Validity::Valid => model.set_valid(label, bits, write),
        Validity::Stale => {
            let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
            model.set_valid(label, bits, write) | model.set_stale(label, age_ns)
        }
        Validity::Missing => {
            model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None)
        }
        Validity::Unsupported => {
            model.set_fallback(label, m.validity, Fallback::NotSupported, None)
        }
        v => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// Fills the P1 rows (UI-PRF-031): Download and Upload speed in the network unit, Total
/// downloaded and Total uploaded since boot (IEC), and the Display unit value with its tooltip.
/// Each value formats again only when its raw value or the unit changed. True when any row
/// changed.
pub fn update_net_details_p1(model: &mut KeyValueModel, inp: &NetDetailInputs) -> bool {
    let mut changed = false;
    match inp.totals {
        Some(t) => {
            changed |= rate_row(model, ROW_DOWNLOAD, t.rx_bps, inp);
            changed |= rate_row(model, ROW_UPLOAD, t.tx_bps, inp);
            let (down, up) = (t.rx_total, t.tx_total);
            changed |= model.set_valid(ROW_TOTAL_DOWN, down, |o| units::bytes(o, down));
            changed |= model.set_valid(ROW_TOTAL_UP, up, |o| units::bytes(o, up));
        }
        None => {
            for label in [ROW_DOWNLOAD, ROW_UPLOAD, ROW_TOTAL_DOWN, ROW_TOTAL_UP] {
                changed |=
                    model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None);
            }
        }
    }
    changed | sync_display_unit(model, inp.unit, inp.auto_scale)
}

/// The Display unit row: its value from the unit settings, its tooltip, and the link.
fn sync_display_unit(model: &mut KeyValueModel, unit: NetworkUnit, auto_scale: bool) -> bool {
    let text = model.text(display_unit_key(unit, auto_scale));
    let tip = model.text(KEY_UNIT_TOOLTIP);
    let mut changed = model.set_text(ROW_DISPLAY_UNIT, text.text());
    if let Some(row) = model.rows.iter_mut().find(|r| r.label == ROW_DISPLAY_UNIT) {
        if row.tooltip.as_ref() != Some(tip.text()) {
            row.tooltip = Some(tip.text().clone());
            changed = true;
        }
        if !row.link || row.action.is_none() {
            row.link = true;
            // The page takes the grid's link clicks (`details_link_click`), so this action is
            // never dispatched; the grid draws a value as a link only when it has one.
            row.action = Some(Box::new(NoOp));
            changed = true;
        }
    }
    changed
}

/// What the P2 rows are read from.
#[derive(Clone, Copy, Default)]
pub struct NetP2Inputs<'a> {
    pub now_boot_ns: u64,
    /// `observed_end` of the system component, for the Link speed Stale age.
    pub observed_end_ns: u64,
    /// `observed_end` of the device details component, for the MAC address and Driver Stale
    /// age.
    pub details_observed_end_ns: u64,
    /// The interface's record in `Snapshot.device_details`; `None` before the first read.
    pub details: Option<&'a NetDetails>,
    /// The interface's record in `Snapshot.system` (1 Hz): state and link speed.
    pub totals: Option<&'a NetTotals>,
    /// The interface name from the device catalog (`enp38s0`).
    pub iface: Option<&'a str>,
}

/// Hides or shows `label`. True when that changed.
fn set_hidden(model: &mut KeyValueModel, label: &'static str, hidden: bool) -> bool {
    let was = model
        .rows
        .iter()
        .find(|r| r.label == label)
        .map(|r| r.hidden);
    if was == Some(hidden) {
        return false;
    }
    if hidden {
        model.hide(label);
    } else {
        model.show(label);
    }
    true
}

/// The row's tooltip. True when it changed.
fn set_row_tooltip(
    model: &mut KeyValueModel,
    label: &'static str,
    tip: Option<SharedString>,
) -> bool {
    let same = model
        .rows
        .iter()
        .find(|r| r.label == label)
        .is_some_and(|r| r.tooltip == tip);
    if same {
        return false;
    }
    model.set_tooltip(label, tip);
    true
}

/// A text row from an inventory string: Valid and non-empty shows the text, Unsupported and an
/// empty value read "Not reported", anything else "Unavailable".
fn field_row(
    model: &mut KeyValueModel,
    label: &'static str,
    f: &InvField<Arc<str>>,
    age_ns: u64,
) -> bool {
    match f.validity {
        Validity::Valid if !f.value.is_empty() => model.set_text(label, &f.value),
        Validity::Stale if !f.value.is_empty() => {
            model.set_text(label, &f.value) | model.set_stale(label, age_ns)
        }
        Validity::Valid | Validity::Stale | Validity::Unsupported => {
            model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None)
        }
        Validity::Missing => {
            model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None)
        }
        v => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// "Not reported" for a value the source does not give.
fn not_reported(model: &mut KeyValueModel, label: &'static str) -> bool {
    model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None)
}

/// The Connection type word key: the engine's PERF-054 label, else the class word of `kind`.
fn type_key(label: &str, kind: NetKind) -> &'static str {
    if let Some((_, key)) = TYPE_KEYS.iter().find(|(l, _)| *l == label) {
        return key;
    }
    match kind {
        NetKind::Wired => TYPE_KEYS[0].1,
        NetKind::Wireless => TYPE_KEYS[1].1,
        NetKind::VpnTunnel => TYPE_KEYS[7].1,
        NetKind::Virtual => TYPE_KEYS[2].1,
        NetKind::Other => TYPE_KEYS[8].1,
    }
}

/// True for an address in `fe80::/10`. `cidr` is `addr/prefix`.
fn is_link_local_v6(cidr: &str) -> bool {
    let addr = cidr.split('/').next().unwrap_or(cidr);
    addr.parse::<Ipv6Addr>()
        .is_ok_and(|a| a.segments()[0] & 0xffc0 == 0xfe80)
}

/// The Link speed row: Wi-Fi with both bit rates shows them, otherwise the sysfs speed.
fn link_speed_row(
    model: &mut KeyValueModel,
    t: Option<&NetTotals>,
    wifi: Option<&WifiInfo>,
    age_ns: u64,
) -> bool {
    if let Some(WifiInfo {
        rx_mbps: Some(rx),
        tx_mbps: Some(tx),
        ..
    }) = wifi
    {
        let (rx, tx) = (rx.round() as u32, tx.round() as u32);
        let template = model.text(KEY_LINK_SPEED_WIFI);
        let bits = u64::from(rx) | u64::from(tx) << 32;
        return model.set_valid(ROW_LINK_SPEED, bits, |o| {
            let _ = template.write(o, &[&rx, &tx]);
        });
    }
    match t {
        None => model.set_fallback(
            ROW_LINK_SPEED,
            Validity::WarmingUp,
            Fallback::Unavailable,
            None,
        ),
        Some(t) => {
            let m = t.link_speed_mbps;
            let valid = |model: &mut KeyValueModel| {
                let (mbps, template) = (m.value, model.text(KEY_LINK_SPEED));
                model.set_valid(ROW_LINK_SPEED, u64::from(mbps), |o| {
                    let _ = template.write(o, &[&mbps]);
                })
            };
            match m.validity {
                Validity::Valid if m.value > 0 => valid(model),
                Validity::Stale if m.value > 0 => {
                    valid(model) | model.set_stale(ROW_LINK_SPEED, age_ns)
                }
                Validity::Valid | Validity::Stale | Validity::Unsupported => {
                    not_reported(model, ROW_LINK_SPEED)
                }
                Validity::Missing => model.set_fallback(
                    ROW_LINK_SPEED,
                    Validity::WarmingUp,
                    Fallback::Unavailable,
                    None,
                ),
                v => model.set_fallback(ROW_LINK_SPEED, v, Fallback::Unavailable, None),
            }
        }
    }
}

/// The IPv4 and IPv6 rows. IPv6 shows the global addresses and puts the link-local ones in the
/// tooltip.
fn address_rows(model: &mut KeyValueModel, addresses: &[Arc<str>]) -> bool {
    let join = |it: Vec<&str>| it.join(", ");
    let v4: Vec<&str> = addresses
        .iter()
        .map(|a| &**a)
        .filter(|a| !a.contains(':'))
        .collect();
    let (local, global): (Vec<&str>, Vec<&str>) = addresses
        .iter()
        .map(|a| &**a)
        .filter(|a| a.contains(':'))
        .partition(|a| is_link_local_v6(a));
    let mut changed = false;
    changed |= if v4.is_empty() {
        not_reported(model, ROW_IPV4)
    } else {
        model.set_text(ROW_IPV4, &join(v4))
    };
    let tip = (!local.is_empty()).then(|| {
        let mut out = String::new();
        let _ = model
            .text(KEY_LINK_LOCAL)
            .write(&mut out, &[&join(local.clone())]);
        SharedString::from(out)
    });
    let (shown, tip) = if global.is_empty() {
        // Only link-local addresses: they are the value and the tooltip keeps them too.
        (local, tip)
    } else {
        (global, tip)
    };
    changed |= if shown.is_empty() {
        not_reported(model, ROW_IPV6)
    } else {
        model.set_text(ROW_IPV6, &join(shown))
    };
    changed | set_row_tooltip(model, ROW_IPV6, tip)
}

/// The six Wi-Fi rows. `None` reads "Unavailable" when NetworkManager and nl80211 both failed.
fn wifi_rows(model: &mut KeyValueModel, wifi: Option<&WifiInfo>) -> bool {
    let mut changed = false;
    let Some(w) = wifi else {
        for label in WIFI_ROWS {
            changed |= model.set_fallback(label, Validity::Failed, Fallback::Unavailable, None);
        }
        return changed;
    };
    changed |= if w.ssid.is_empty() {
        not_reported(model, ROW_SSID)
    } else {
        model.set_text(ROW_SSID, &w.ssid)
    };
    let mut text = String::new();
    let pct = w.signal_pct;
    let (key, args): (&str, [&dyn fmt::Display; 2]) = match w.signal_dbm.as_ref() {
        Some(dbm) => (KEY_SIGNAL, [dbm, &pct]),
        None => (KEY_SIGNAL_PCT, [&pct, &pct]),
    };
    let _ = model.text(key).write(&mut text, &args);
    changed |= model.set_text(ROW_SIGNAL, &text);
    for (label, rate) in [(ROW_RX_BITRATE, w.rx_mbps), (ROW_TX_BITRATE, w.tx_mbps)] {
        changed |= match rate {
            Some(r) => {
                let template = model.text(KEY_LINK_SPEED);
                let mbps = r.round() as u32;
                model.set_valid(label, u64::from(mbps), |o| {
                    let _ = template.write(o, &[&mbps]);
                })
            }
            None => not_reported(model, label),
        };
    }
    changed |= match w.channel {
        Some(ch) if w.freq_mhz > 0 => {
            let (mhz, template) = (w.freq_mhz, model.text(KEY_FREQUENCY));
            model.set_valid(ROW_FREQUENCY, u64::from(mhz) | u64::from(ch) << 32, |o| {
                let _ = template.write(o, &[&mhz, &ch]);
            })
        }
        _ => not_reported(model, ROW_FREQUENCY),
    };
    changed |= if w.security.is_empty() {
        not_reported(model, ROW_SECURITY)
    } else {
        model.set_text(ROW_SECURITY, &w.security)
    };
    changed
}

/// Fills the static and Wi-Fi rows (UI-PRF-031, PERF-054, 056, 058, 059): Adapter name,
/// Interface name, Connection type and status, the Wi-Fi rows when the model has them, Link
/// speed, the addresses, MAC address, Interface index and Driver. Each value formats again only
/// when it changed. Public IP stays hidden. True when any row changed.
pub fn update_net_details_p2(model: &mut KeyValueModel, inp: &NetP2Inputs) -> bool {
    let has_wifi = model.rows.iter().any(|r| r.label == ROW_SSID);
    let mut changed = false;
    let mut shown: SmallVec<[&'static str; 22]> = SmallVec::new();
    shown.extend_from_slice(&HEAD_ROWS[4..]);
    if has_wifi {
        shown.extend_from_slice(&WIFI_ROWS);
    }
    shown.extend(
        TAIL_ROWS
            .iter()
            .copied()
            .filter(|l| *l != ROW_DISPLAY_UNIT && *l != ROW_PUBLIC_IP),
    );
    for label in &shown {
        changed |= set_hidden(model, label, false);
    }

    changed |= match inp.iface {
        Some(name) => model.set_text(ROW_INTERFACE, name),
        None => model.set_fallback(
            ROW_INTERFACE,
            Validity::WarmingUp,
            Fallback::Unavailable,
            None,
        ),
    };
    changed |= match inp.totals {
        Some(t) => {
            let key = STATE_KEYS
                .iter()
                .find(|(s, _)| *s == t.state)
                .map_or(STATE_KEYS[6].1, |(_, k)| k);
            let word = model.text(key);
            model.set_text(ROW_STATUS, word.text())
        }
        None => model.set_fallback(ROW_STATUS, Validity::WarmingUp, Fallback::Unavailable, None),
    };

    let Some(d) = inp.details else {
        for label in [
            ROW_ADAPTER,
            ROW_TYPE,
            ROW_IPV4,
            ROW_IPV6,
            ROW_MAC,
            ROW_IFINDEX,
            ROW_DRIVER,
        ] {
            changed |= model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None);
        }
        changed |= link_speed_row(model, inp.totals, None, 0);
        changed |= set_row_tooltip(model, ROW_IPV6, None);
        if has_wifi {
            for label in WIFI_ROWS {
                changed |=
                    model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None);
            }
        }
        return changed;
    };
    changed |= if matches!(d.adapter_name.validity, Validity::Valid | Validity::Stale)
        && !d.adapter_name.value.is_empty()
    {
        model.set_text(ROW_ADAPTER, &d.adapter_name.value)
    } else {
        let word = model.text(KEY_UNKNOWN_ADAPTER);
        model.set_text(ROW_ADAPTER, word.text())
    };
    let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
    let details_age_ns = inp.now_boot_ns.saturating_sub(inp.details_observed_end_ns);
    let word = model.text(type_key(&d.type_label, d.kind));
    changed |= model.set_text(ROW_TYPE, word.text());
    if has_wifi {
        changed |= wifi_rows(model, d.wifi.as_ref());
    }
    changed |= link_speed_row(model, inp.totals, d.wifi.as_ref(), age_ns);
    changed |= address_rows(model, &d.addresses);
    changed |= field_row(model, ROW_MAC, &d.mac, details_age_ns);
    let index = d.ifindex;
    changed |= model.set_valid(ROW_IFINDEX, u64::from(index), |o| {
        let _ = write!(o, "{index}");
    });
    changed |= field_row(model, ROW_DRIVER, &d.driver, details_age_ns);
    changed
}

/// The four signal bar colours for `pct` (UI-PRF-031): bar `i` is `accent` when the strength is
/// at least 25, 50, 75 and 100 for bars 1 to 4, else `divider`.
pub fn signal_bar_colors(pct: u8, theme: &TlTheme) -> [Hsla; 4] {
    std::array::from_fn(|i| {
        if u32::from(pct) >= 25 * (i as u32 + 1) {
            theme.accent
        } else {
            theme.divider
        }
    })
}

/// Side of the signal icon in logical pixels.
const SIGNAL_ICON: f32 = 16.;
/// Width of one bar and the gap between bars.
const SIGNAL_BAR_W: f32 = 3.;
const SIGNAL_BAR_GAP: f32 = 1.;

/// The 16 px signal icon: four bottom aligned bars 4, 8, 12 and 16 px high, drawn as quads.
/// Decorative: the row's text carries the value.
fn signal_icon(pct: u8, theme: &TlTheme, scale: f32) -> AnyElement {
    let colors = signal_bar_colors(pct, theme);
    let side = px(SIGNAL_ICON * scale);
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            for (i, color) in colors.iter().enumerate() {
                let h = px(4. * (i as f32 + 1.) * scale);
                let x = bounds.left() + px(i as f32 * (SIGNAL_BAR_W + SIGNAL_BAR_GAP) * scale);
                let bar = Bounds::new(
                    point(x, bounds.bottom() - h),
                    size(px(SIGNAL_BAR_W * scale), h),
                );
                window.paint_quad(fill(bar, *color));
            }
        },
    )
    .w(side)
    .h(side)
    .flex_none()
    .into_any_element()
}

/// The record of `device` in the snapshot.
fn net_totals(snap: &Snapshot, device: DeviceIdx) -> Option<&NetTotals> {
    snap.system
        .nets
        .iter()
        .find(|(i, _)| *i == device)
        .map(|(_, t)| t)
}

/// One network interface's device page.
pub struct NetPage {
    key: DeviceKey,
    row: DeviceRow,
    throughput: Entity<ChartView>,
    /// E25-007 and E25-020 fill the rows.
    details: KeyValueModel,
    /// `unit`, `auto_scale` and `dynamic_scale` with the chart settings.
    settings: PageSettings,
    settings_versions: [u64; 4],
    /// Valid sysfs speed in bits per second.
    link_speed_bps: Option<f32>,
    // E25-021: public_ip: PublicIpRowState (added by o55.22 as a chain edit, R-262)
    header: (SharedString, SharedString),
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    /// Opens the Display unit menu under the value; built once.
    link_click: DetailsLinkClick,
    /// The Display unit link's focus handle, the page's block control in `focus_order`
    /// (UI-SHL-121).
    unit_focus: FocusHandle,
    /// `unit_focus` by the Display unit row's flat index, for the grid.
    link_focus: Rc<[(usize, FocusHandle)]>,
    /// The Wi-Fi signal strength the Signal row's icon shows; `None` without Wi-Fi data.
    signal: Option<u8>,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl NetPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = current(cx).clone();
        let settings = PageSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let link_speed = link_speed_bps(&snap, row.idx);
        let wifi = row.net_class == Some(NetClass::Wireless);
        let mut words = Words::default();
        words.refresh(cx);

        let throughput = {
            let y = throughput_scale(settings.dynamic_scale, link_speed, settings.unit);
            let (spec, labels) = (
                throughput_spec(row.idx, y, &settings, &theme),
                throughput_labels(&words),
            );
            cx.new(|cx| {
                let mut view = ChartView::new(spec, labels, &hub, cx);
                // The value is download plus upload.
                view.set_value_sum(true, cx);
                view
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            throughput,
            details: net_details_model(wifi),
            settings,
            settings_versions: section_versions(cx),
            link_speed_bps: link_speed,
            header: (SharedString::default(), SharedString::default()),
            style_version: metrics(cx).style_version,
            words,
            hub,
            demand,
            id,
            active: false,
            window: None,
            focus: cx.focus_handle(),
            unit_focus: cx.focus_handle(),
            link_focus: Rc::from(Vec::new()),
            signal: None,
            link_click: {
                let page = cx.weak_entity();
                Rc::new(move |n, bounds, window, cx| {
                    let _ = page.update(cx, |p, cx| p.on_details_link(n, bounds, window, cx));
                })
            },
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn throughput(&self) -> &Entity<ChartView> {
        &self.throughput
    }

    pub fn unit(&self) -> NetworkUnit {
        self.settings.unit
    }

    pub fn auto_scale(&self) -> bool {
        self.settings.auto_scale
    }

    pub fn dynamic_scale(&self) -> bool {
        self.settings.dynamic_scale
    }

    pub fn link_speed_bps(&self) -> Option<f32> {
        self.link_speed_bps
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// This page's demand part while it is active: network only.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            nets: true,
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part());
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let snap = Arc::clone(hub.read(cx).current());
        self.apply_snapshot(&snap, ev.mask, cx);
    }

    /// Brings the link speed, chart state, Details rows and header up to date with the
    /// components in `mask`. Notifies only when the header or a row changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        if mask.contains(ComponentMask::SYSTEM) {
            let speed = link_speed_bps(snap, self.row.idx);
            if speed != self.link_speed_bps {
                self.link_speed_bps = speed;
                self.rebuild_spec(cx);
            }
        }
        if mask.intersects(
            ComponentMask::SYSTEM | ComponentMask::DEVICE_DETAILS | ComponentMask::DEVICES,
        ) && self.sync_details(snap)
        {
            cx.notify();
        }
        if mask.contains(ComponentMask::HEALTH) {
            let state = health_state(snap);
            self.throughput.update(cx, |v, cx| v.set_state(state, cx));
        }
        if mask.intersects(
            ComponentMask::DEVICES | ComponentMask::INVENTORY | ComponentMask::DEVICE_DETAILS,
        ) && let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx)
        {
            let header = header_text(page_words(cx), DeviceKind::Net, 0, entry, snap);
            if header != self.header {
                self.header = header;
                cx.notify();
            }
        }
    }

    /// A link click on visible Details row `n`: the Display unit value opens its menu under
    /// `bounds`; any other link dispatches its own action.
    fn on_details_link(
        &mut self,
        n: usize,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.details.rows.iter().filter(|r| !r.hidden).nth(n) else {
            return;
        };
        if row.label == ROW_DISPLAY_UNIT {
            self.open_unit_menu(bounds, window, cx);
        } else if let Some(action) = &row.action {
            window.dispatch_action(action.boxed_clone(), cx);
        }
    }

    /// The Display unit menu (PERF-057): Bits per second and Bytes per second, the current one
    /// checked, each setting `units.network`. Opens with its top left at the value's bottom left.
    fn open_unit_menu(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let unit = self.settings.unit;
        let (bits, bytes) = (tr(cx, KEY_UNIT_BITS), tr(cx, KEY_UNIT_BYTES));
        let set = |value: &'static str| {
            Box::new(SetSetting {
                key: SettingKey::UnitsNetwork,
                value: SettingValue::Str(SharedString::new_static(value)),
            })
        };
        let menu = TlMenu::build(window, cx, self.focus.clone(), |menu, _, _| {
            menu.radio_item(bits, unit == NetworkUnit::Bits, set("bits"))
                .radio_item(bytes, unit == NetworkUnit::Bytes, set("bytes"))
        });
        let at = point(bounds.left(), bounds.bottom());
        open_context_menu_at(at, window, cx, menu);
    }

    /// Brings the Details rows up to date. True when one changed.
    fn sync_details(&mut self, snap: &Snapshot) -> bool {
        let inputs = NetDetailInputs {
            now_boot_ns: now_boot_ns(),
            observed_end_ns: snap.system.header.observed_end,
            totals: net_totals(snap, self.row.idx),
            unit: self.settings.unit,
            auto_scale: self.settings.auto_scale,
        };
        let details = snap
            .device_details
            .nets
            .iter()
            .find(|(i, _)| *i == self.row.idx)
            .map(|(_, d)| d);
        let iface = snap
            .devices
            .entries
            .iter()
            .find(|e| e.idx == self.row.idx)
            .and_then(|e| match &e.identity {
                DeviceIdentity::Net { name, .. } => Some(&**name),
                _ => None,
            });
        let p2 = NetP2Inputs {
            now_boot_ns: inputs.now_boot_ns,
            observed_end_ns: inputs.observed_end_ns,
            details_observed_end_ns: snap.device_details.header.observed_end,
            details,
            totals: inputs.totals,
            iface,
        };
        let changed = update_net_details_p1(&mut self.details, &inputs)
            | update_net_details_p2(&mut self.details, &p2);
        self.signal = details.and_then(|d| d.wifi.as_ref()).map(|w| w.signal_pct);
        self.sync_link_focus();
        changed
    }

    /// Points `link_focus` at the Display unit row's flat index; rebuilt only when it moved.
    fn sync_link_focus(&mut self) {
        let at = self
            .details
            .rows
            .iter()
            .filter(|r| !r.hidden)
            .position(|r| r.label == ROW_DISPLAY_UNIT);
        if self.link_focus.first().map(|(i, _)| *i) == at {
            return;
        }
        self.link_focus = match at {
            Some(i) => Rc::from([(i, self.unit_focus.clone())]),
            None => Rc::from(Vec::new()),
        };
    }

    /// Hands the chart its spec from the current settings, link speed and theme. The view keeps
    /// an unchanged spec.
    fn rebuild_spec(&self, cx: &mut App) {
        let s = self.settings;
        let theme = current(cx).clone();
        let y = throughput_scale(s.dynamic_scale, self.link_speed_bps, s.unit);
        let spec = throughput_spec(self.row.idx, y, &s, &theme);
        self.throughput.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    /// Applies the page's settings when one of their sections changed. The value text follows
    /// `units.network` in the chart itself.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_spec(cx);
        // The rates and the Display unit follow `units.network`.
        let snap = Arc::clone(self.hub.read(cx).current());
        if self.sync_details(&snap) {
            cx.notify();
        }
    }

    /// A new theme: the chart takes the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_spec(cx);
        cx.notify();
    }

    /// New strings: the chart labels and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        let labels = throughput_labels(&self.words);
        self.throughput.update(cx, |v, cx| v.set_labels(labels, cx));
        let snap = Arc::clone(self.hub.read(cx).current());
        self.apply_snapshot(&snap, ComponentMask::DEVICES, cx);
        // The Display unit text and tooltip come from the new strings.
        self.details.refresh_strings(cx);
        self.sync_details(&snap);
        cx.notify();
    }
}

impl DevicePage for NetPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, cx: &mut App) -> AnyElement {
        let h = GRAPH_H_SINGLE * metrics(cx).ui_scale;
        div()
            .debug_selector(|| THROUGHPUT_ID.into())
            .flex_none()
            .w_full()
            .h(px(h))
            .child(
                self.throughput
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// None in P1; E25-031 adds "Network settings" in P3.
    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    /// E25-029 replaces this with the full Network copy text.
    fn copy_text(&self, out: &mut String) {
        out.push_str(&self.header.0);
        out.push('\n');
        self.details.copy_into(out);
    }

    /// No header toggles; the Display unit link is the page's one block control.
    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        smallvec![self.unit_focus.clone()]
    }

    fn details_link_focus(&self) -> Option<Rc<[(usize, FocusHandle)]>> {
        Some(self.link_focus.clone())
    }

    fn details_link_click(&self) -> Option<DetailsLinkClick> {
        Some(self.link_click.clone())
    }

    /// The 4 bar signal icon before the Signal value.
    fn details_value_prefix(&self) -> Option<DetailsValuePrefix> {
        let pct = self.signal?;
        let at = self
            .details
            .rows
            .iter()
            .filter(|r| !r.hidden)
            .position(|r| r.label == ROW_SIGNAL)?;
        Some(Rc::new(move |n, _, cx| {
            if n != at {
                return None;
            }
            let (theme, m) = (current(cx).clone(), *metrics(cx));
            let gap = m.sp[2];
            let width = px(SIGNAL_ICON * m.ui_scale) + gap;
            let icon = div()
                .flex_none()
                .mr(gap)
                .child(signal_icon(pct, &theme, m.ui_scale))
                .into_any_element();
            Some((width, icon))
        }))
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else {
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }
}

impl Focusable for NetPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for NetPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        self.details.refresh_strings(cx);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div().size_full().track_focus(&self.focus).child(
            DevicePageFrame {
                page: self,
                m: &m,
                theme: &theme,
            }
            .render(window, cx),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_speed_scale_unknown_falls_back() {
        assert_eq!(
            throughput_scale(false, Some(1000. * BPS_PER_MBPS), NetworkUnit::Bits),
            YScale::LinkSpeed(1_000_000_000.)
        );
        assert_eq!(
            throughput_scale(false, None, NetworkUnit::Bits),
            YScale::AutoRate { iec: false }
        );
        assert_eq!(
            throughput_scale(false, None, NetworkUnit::Bytes),
            YScale::AutoRate { iec: true }
        );
        assert_eq!(
            throughput_scale(true, Some(1000. * BPS_PER_MBPS), NetworkUnit::Bits),
            YScale::AutoRate { iec: false },
            "the dynamic scale ignores the link speed"
        );
    }
}
