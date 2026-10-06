//! The device sidebar card: the text builders for every device kind and the card element
//! (docs/ui/performance.md UI-PRF-011; PERF-001, PERF-005, PERF-008, PERF-013, PERF-053,
//! PERF-054, R-050, R-055). The sidebar (E24-004) and the mini window (E26) own the cards.
//!
//! The words are English constants that match `perf-card-*`, `perf-unknown-*` and
//! `perf-net-state-*` in `en-US.ftl`, like the header words in `page.rs`.

use std::fmt::Write as _;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Window, div, px,
};
use smallvec::SmallVec;
use tl_model::devices::{DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{DiskDetails, DiskDrive, Snapshot};
use tl_model::system::{LinkState, Metric, SensorKind, SensorReading};
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;

use super::actions::PerfSelectDevice;
use super::{DeviceKey, DeviceKind};
use crate::chart::{ChartBg, ChartLabels, ChartView, GridStyle, ValueAlign, ValueFormat};
use crate::components::icons::{TlIcon, icon};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::validity::copy_text;
use crate::text::style::label_style;
use crate::theme::{UiMetrics, current, metrics};

/// Card height, × ui_scale.
pub const CARD_H: f32 = 80.0;
/// Mini graph width, × ui_scale.
pub const CARD_GRAPH_W: f32 = 116.0;
/// Mini graph height, × ui_scale.
pub const CARD_GRAPH_H: f32 = 56.0;

/// Heading line height, × ui_scale.
const HEADING_LINE_H: f32 = 18.0;
/// Line 1 and line 2 height, × ui_scale.
const DETAIL_LINE_H: f32 = 16.0;
/// Opacity of a disconnected interface and of a hidden card in edit mode.
const DIM_OPACITY: f32 = 0.6;
/// Alpha of the pressed tint and of the focus ring.
const PRESSED_ALPHA: f32 = 0.6;
const FOCUS_RING_ALPHA: f32 = 0.5;

const HEADING_CPU: &str = "CPU";
const HEADING_MEMORY: &str = "Memory";
const HEADING_GPU: &str = "GPU ";
const HEADING_DISK: &str = "Disk ";
const HEADING_FAN: &str = "Fan ";
const HEADING_BATTERY: &str = "Battery";
const AGGREGATE_STORAGE: &str = "Aggregate storage";
const AGGREGATE_GRAPHICS: &str = "Aggregate graphics";
const SUFFIX_DISKS: &str = " disks";
const SUFFIX_ADAPTERS: &str = " adapters";
const MULTIPLE: &str = "Multiple";
const COMMIT: &str = "Commit ";
const UNKNOWN_PROCESSOR: &str = "Unknown processor";
const UNKNOWN_GPU: &str = "Unknown GPU";
/// Between two values on one line.
const SEP: &str = " · ";
/// Between the disk kind and its mount.
const KIND_SEP: &str = " - ";
const DOWN: &str = "↓ ";
const UP: &str = " ↑ ";

const STATE_DISCONNECTED: &str = "Disconnected";
const STATE_TESTING: &str = "Testing";
const STATE_DORMANT: &str = "Dormant";
const STATE_NOT_PRESENT: &str = "Not present";
const STATE_LOWER_LAYER_DOWN: &str = "Lower layer down";
const STATE_UNKNOWN: &str = "Unknown";

/// Byte offsets into a line where a Restricted value's lock icon goes (UI-FND-026).
pub type LockOffsets = SmallVec<[u16; 1]>;

/// The text of one card. The owner keeps it and refreshes it with [`update_card_lines`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardLines {
    pub heading: SharedString,
    pub line1: SharedString,
    pub line2: SharedString,
    /// Line 2 in `accent`: a connected interface's rate (R-055).
    pub line2_accent: bool,
    /// The full line 1; network cards add the NetworkManager connection on a second line.
    pub tooltip: Option<SharedString>,
    /// A network interface that is not connected. The card draws at opacity 0.6.
    pub disconnected: bool,
    /// A Restricted value has no text: the lock icon goes at each offset of its line.
    pub line1_locks: LockOffsets,
    pub line2_locks: LockOffsets,
}

/// What [`card_lines`] reads. `number` is the device's index among the visible devices of its
/// kind ([`super::type_number`]).
pub struct CardTextInput<'a> {
    pub kind: DeviceKind,
    pub number: u16,
    pub entry: &'a DeviceEntry,
    /// CPU model, disk models and mounts, interface types and battery details live in
    /// `inventory` and `device_details`; the live values in `system`.
    pub snap: &'a Snapshot,
    pub network_unit: NetworkUnit,
    pub auto_scale: bool,
    /// The fan's `[fans]` label (PERF-077, P3). Fan cards only.
    pub fan_label: Option<&'a str>,
    /// The fan's paired temperature sensor (PERF-077). Fan cards only.
    pub fan_temp: Option<DeviceIdx>,
}

/// The card text for `input`. Formats through `scratch`; every string is a new allocation, so
/// owners that refresh a card use [`update_card_lines`], which keeps unchanged strings.
pub fn card_lines(input: &CardTextInput, scratch: &mut String) -> CardLines {
    let mut lines = CardLines::default();
    update_card_lines(&mut lines, input, scratch);
    lines
}

/// Re-formats `lines` for `input`. Each string is written into `scratch` and replaces the old one
/// only when the text differs, so an unchanged line keeps its allocation. Returns true when
/// anything changed. Runs on `SERIES` (own ids), `SYSTEM` and `DEVICES` changes only.
pub fn update_card_lines(
    lines: &mut CardLines,
    input: &CardTextInput,
    scratch: &mut String,
) -> bool {
    let mut changed = false;
    let mut locks = LockOffsets::new();

    scratch.clear();
    write_heading(input, scratch);
    changed |= keep_or_set(&mut lines.heading, scratch);

    scratch.clear();
    write_line1(input, scratch, &mut locks);
    changed |= keep_or_set(&mut lines.line1, scratch);
    changed |= set_if_changed(&mut lines.line1_locks, &locks);

    scratch.clear();
    locks.clear();
    let net = (input.kind == DeviceKind::Net).then(|| net_state(input));
    let connected = net == Some(NetState::Connected);
    let disconnected = matches!(net, Some(NetState::Down(_)));
    write_line2(input, net, scratch, &mut locks);
    changed |= keep_or_set(&mut lines.line2, scratch);
    changed |= set_if_changed(&mut lines.line2_locks, &locks);
    changed |= set_if_changed(&mut lines.line2_accent, &connected);
    changed |= set_if_changed(&mut lines.disconnected, &disconnected);

    let connection = (input.kind == DeviceKind::Net)
        .then(|| net_connection(input))
        .flatten();
    changed |= match connection {
        Some(name) => {
            scratch.clear();
            scratch.push_str(&lines.line1);
            scratch.push('\n');
            scratch.push_str(name);
            match &mut lines.tooltip {
                Some(tip) => keep_or_set(tip, scratch),
                None => {
                    lines.tooltip = Some(scratch.clone().into());
                    true
                }
            }
        }
        None if lines.line1.is_empty() => lines.tooltip.take().is_some(),
        // Shares line 1's allocation.
        None if lines.tooltip.as_deref() == Some(&*lines.line1) => false,
        None => {
            lines.tooltip = Some(lines.line1.clone());
            true
        }
    };
    changed
}

/// Replaces `field` with `text` when they differ.
fn keep_or_set(field: &mut SharedString, text: &str) -> bool {
    if field.as_ref() == text {
        return false;
    }
    *field = SharedString::from(text.to_owned());
    true
}

fn set_if_changed<T: PartialEq + Clone>(field: &mut T, value: &T) -> bool {
    if field == value {
        return false;
    }
    *field = value.clone();
    true
}

/// Writes a value, or its UI-FND-026 glyph word when it is not Valid or Stale. Restricted writes
/// nothing and records a lock offset.
fn value(
    out: &mut String,
    locks: &mut LockOffsets,
    validity: Validity,
    write: impl FnOnce(&mut String),
) {
    match validity {
        Validity::Valid | Validity::Stale => write(out),
        Validity::Restricted => locks.push(u16::try_from(out.len()).unwrap_or(u16::MAX)),
        other => out.push_str(&copy_text(other, &SharedString::default())),
    }
}

/// An optional part such as a temperature: left out when there is no sensor (Unsupported) or no
/// reading yet (WarmingUp); otherwise " · " and the value or glyph.
fn optional_part<T: Copy + Default>(
    out: &mut String,
    locks: &mut LockOffsets,
    m: Metric<T>,
    write: impl FnOnce(&mut String, T),
) {
    if matches!(m.validity, Validity::Unsupported | Validity::WarmingUp) {
        return;
    }
    out.push_str(SEP);
    value(out, locks, m.validity, |o| write(o, m.value));
}

/// A present inventory or details text: Valid or Stale and not blank, trimmed.
fn shown(text: &str, validity: Validity) -> Option<&str> {
    let ok = matches!(validity, Validity::Valid | Validity::Stale) && !text.trim().is_empty();
    ok.then(|| text.trim())
}

fn write_heading(input: &CardTextInput, out: &mut String) {
    let n = input.number;
    match input.kind {
        DeviceKind::Cpu => out.push_str(HEADING_CPU),
        DeviceKind::Memory => out.push_str(HEADING_MEMORY),
        DeviceKind::Gpu => {
            let _ = write!(out, "{HEADING_GPU}{n}");
        }
        DeviceKind::Disk => {
            let _ = write!(out, "{HEADING_DISK}{n}");
        }
        DeviceKind::Net => out.push_str(ifname(input.entry)),
        DeviceKind::Fan => match input.fan_label.and_then(|l| shown(l, Validity::Valid)) {
            Some(label) => out.push_str(label),
            None => {
                let _ = write!(out, "{HEADING_FAN}{n}");
            }
        },
        DeviceKind::Battery => {
            let kind = battery(input).and_then(|b| shown(&b.kind, Validity::Valid));
            out.push_str(kind.unwrap_or(HEADING_BATTERY));
        }
        DeviceKind::AggregateDisk => out.push_str(AGGREGATE_STORAGE),
        DeviceKind::AggregateGpu => out.push_str(AGGREGATE_GRAPHICS),
    }
}

fn write_line1(input: &CardTextInput, out: &mut String, locks: &mut LockOffsets) {
    let snap = input.snap;
    let entry = input.entry;
    match input.kind {
        DeviceKind::Cpu => {
            let p = &snap.inventory.processor.name;
            out.push_str(shown(&p.value, p.validity).unwrap_or(UNKNOWN_PROCESSOR));
        }
        DeviceKind::Memory => {
            let mem = &snap.system.memory;
            value(out, locks, mem.validity, |o| {
                units::bytes_of(o, mem.used, mem.total);
            });
        }
        DeviceKind::Gpu => {
            out.push_str(shown(&entry.display_name, Validity::Valid).unwrap_or(UNKNOWN_GPU));
        }
        DeviceKind::Disk => {
            let (details, drive) = (disk_details(input), disk_drive(input));
            let kind = details
                .and_then(|d| shown(&d.kind, Validity::Valid))
                .or_else(|| drive.and_then(|d| shown(&d.kind.value, d.kind.validity)))
                .unwrap_or_else(|| disk_name(entry));
            out.push_str(kind);
            if let Some(mount) = primary_mount(details, drive) {
                out.push_str(KIND_SEP);
                out.push_str(mount);
            }
        }
        DeviceKind::Net => out.push_str(net_type_word(input)),
        DeviceKind::Fan => {
            let chip = match &entry.identity {
                DeviceIdentity::Fan { chip, .. } => &**chip,
                _ => &*entry.display_name,
            };
            out.push_str(
                input
                    .fan_label
                    .and_then(|l| shown(l, Validity::Valid))
                    .unwrap_or(chip),
            );
        }
        DeviceKind::Battery => {
            if let Some(b) = battery(input) {
                for part in [&b.vendor, &b.model] {
                    if let Some(part) = shown(&part.value, part.validity) {
                        if !out.is_empty() {
                            out.push(' ');
                        }
                        out.push_str(part);
                    }
                }
            }
            if out.is_empty() {
                out.push_str(&entry.display_name);
            }
        }
        DeviceKind::AggregateDisk => {
            let k = present_disks(snap).count();
            let _ = write!(out, "{k}{SUFFIX_DISKS}");
        }
        DeviceKind::AggregateGpu => {
            let k = present_gpus(snap).count();
            let _ = write!(out, "{k}{SUFFIX_ADAPTERS}");
        }
    }
}

fn write_line2(
    input: &CardTextInput,
    net: Option<NetState>,
    out: &mut String,
    locks: &mut LockOffsets,
) {
    let snap = input.snap;
    let sys = &snap.system;
    let idx = input.entry.idx;
    match input.kind {
        DeviceKind::Cpu => {
            let freq = sys.cpu.freq_mhz;
            value(out, locks, freq.validity, |o| {
                units::mhz(o, freq.value as f32)
            });
            optional_part(out, locks, sys.cpu.temp_c, units::celsius);
        }
        DeviceKind::Memory => {
            let mem = &sys.memory;
            out.push_str(COMMIT);
            let validity = if mem.validity.is_valid() && mem.commit_limit == 0 {
                Validity::Unsupported
            } else {
                mem.validity
            };
            value(out, locks, validity, |o| {
                units::percent(
                    o,
                    (mem.committed as f64 / mem.commit_limit as f64 * 100.0) as f32,
                );
            });
        }
        DeviceKind::Gpu => {
            let temp = sys
                .gpus
                .iter()
                .find(|(i, _)| *i == idx)
                .map_or_else(Metric::default, |(_, g)| g.temp_c);
            value(out, locks, temp.validity, |o| units::celsius(o, temp.value));
        }
        DeviceKind::Disk => {
            let model = disk_details(input)
                .and_then(|d| shown(&d.model.value, d.model.validity))
                .or_else(|| disk_drive(input).and_then(|d| shown(&d.model.value, d.model.validity)))
                .unwrap_or_else(|| disk_name(input.entry));
            out.push_str(model);
            let temp = sys
                .disks
                .iter()
                .find(|(i, _)| *i == idx)
                .map_or_else(Metric::default, |(_, d)| d.temp_c);
            optional_part(out, locks, temp, units::celsius);
        }
        DeviceKind::Net => match net {
            Some(NetState::Down(state)) => out.push_str(state_word(state)),
            Some(NetState::Connected) => {
                if let Some((_, t)) = sys.nets.iter().find(|(i, _)| *i == idx) {
                    write_rates(input, t.rx_bps, t.tx_bps, out, locks);
                }
            }
            Some(NetState::NoData) | None => {
                value(out, locks, Validity::WarmingUp, |_| {});
            }
        },
        DeviceKind::Fan => {
            let rpm = sensor(snap, idx, SensorKind::Fan).map_or_else(Metric::default, |s| s.value);
            value(out, locks, rpm.validity, |o| {
                let _ = write!(o, "{:.0} RPM", rpm.value.max(0.0).round());
            });
            if let Some(t) = input.fan_temp {
                let temp = sensor(snap, t, SensorKind::Temperature)
                    .map_or_else(Metric::default, |s| s.value);
                optional_part(out, locks, temp, units::celsius);
            }
        }
        DeviceKind::Battery => match battery(input) {
            Some(b) => {
                value(out, locks, b.percentage.validity, |o| {
                    units::percent(o, b.percentage.value);
                });
                if let Some(state) = shown(&b.state, Validity::Valid) {
                    out.push_str(SEP);
                    out.push_str(state);
                }
            }
            None => value(out, locks, Validity::WarmingUp, |_| {}),
        },
        DeviceKind::AggregateDisk => {
            let details = &snap.device_details.disks;
            let kinds = present_disks(snap).filter_map(|e| {
                details
                    .iter()
                    .find(|(i, _)| *i == e.idx)
                    .and_then(|(_, d)| shown(&d.kind, Validity::Valid))
            });
            write_common(out, kinds);
        }
        DeviceKind::AggregateGpu => {
            // The vendor is the first word of the adapter name ("AMD", "NVIDIA", "Intel").
            let vendors =
                present_gpus(snap).filter_map(|e| e.display_name.split_whitespace().next());
            write_common(out, vendors);
        }
    }
}

/// The common word of `words`, "Multiple" when they differ, nothing when there is none.
fn write_common<'a>(out: &mut String, mut words: impl Iterator<Item = &'a str>) {
    let Some(first) = words.next() else {
        return;
    };
    out.push_str(if words.all(|w| w == first) {
        first
    } else {
        MULTIPLE
    });
}

/// "↓ 1.2 Mbps ↑ 0.3 Mbps" when both rates are non-zero, else the total rate (table (d)).
fn write_rates(
    input: &CardTextInput,
    rx: Metric<f64>,
    tx: Metric<f64>,
    out: &mut String,
    locks: &mut LockOffsets,
) {
    let unit = match input.network_unit {
        NetworkUnit::Bits => NetUnit::Bits,
        NetworkUnit::Bytes => NetUnit::Bytes,
    };
    let auto = input.auto_scale;
    let shows = |m: Metric<f64>| matches!(m.validity, Validity::Valid | Validity::Stale);
    if !shows(rx) || !shows(tx) {
        let bad = if shows(rx) { tx.validity } else { rx.validity };
        value(out, locks, bad, |_| {});
        return;
    }
    if rx.value > 0.0 && tx.value > 0.0 {
        out.push_str(DOWN);
        units::net_rate(out, rx.value, unit, auto);
        out.push_str(UP);
        units::net_rate(out, tx.value, unit, auto);
    } else {
        units::net_rate(out, rx.value + tx.value, unit, auto);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NetState {
    Connected,
    Down(LinkState),
    /// No totals for the interface yet.
    NoData,
}

fn net_state(input: &CardTextInput) -> NetState {
    let idx = input.entry.idx;
    match input.snap.system.nets.iter().find(|(i, _)| *i == idx) {
        Some((_, t)) if t.state == LinkState::Up => NetState::Connected,
        Some((_, t)) => NetState::Down(t.state),
        None => NetState::NoData,
    }
}

/// PERF-054 state words.
fn state_word(state: LinkState) -> &'static str {
    match state {
        LinkState::Up | LinkState::Down => STATE_DISCONNECTED,
        LinkState::Testing => STATE_TESTING,
        LinkState::Dormant => STATE_DORMANT,
        LinkState::NotPresent => STATE_NOT_PRESENT,
        LinkState::LowerLayerDown => STATE_LOWER_LAYER_DOWN,
        LinkState::Unknown => STATE_UNKNOWN,
    }
}

/// The PERF-054 type word: the details' type label, else a word for the catalog class.
fn net_type_word<'a>(input: &CardTextInput<'a>) -> &'a str {
    let idx = input.entry.idx;
    let label = input
        .snap
        .device_details
        .nets
        .iter()
        .find(|(i, _)| *i == idx)
        .and_then(|(_, n)| shown(&n.type_label, Validity::Valid));
    label.unwrap_or(match input.entry.net_kind {
        Some(NetKind::Wired) => "Ethernet",
        Some(NetKind::Wireless) => "Wi-Fi",
        Some(NetKind::VpnTunnel) => "VPN",
        Some(NetKind::Virtual) => "Virtual",
        Some(NetKind::Other) | None => "Other",
    })
}

/// The NetworkManager connection name, when known.
fn net_connection<'a>(input: &CardTextInput<'a>) -> Option<&'a str> {
    let idx = input.entry.idx;
    input
        .snap
        .device_details
        .nets
        .iter()
        .find(|(i, _)| *i == idx)
        .and_then(|(_, n)| n.connection.as_deref())
        .and_then(|c| shown(c, Validity::Valid))
}

fn ifname(entry: &DeviceEntry) -> &str {
    match &entry.identity {
        DeviceIdentity::Net { name, .. } => name,
        _ => &entry.display_name,
    }
}

fn disk_name(entry: &DeviceEntry) -> &str {
    match &entry.identity {
        DeviceIdentity::Disk { name, .. } => name,
        _ => &entry.display_name,
    }
}

fn disk_details<'a>(input: &CardTextInput<'a>) -> Option<&'a DiskDetails> {
    let idx = input.entry.idx;
    input
        .snap
        .device_details
        .disks
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, d)| d)
}

fn disk_drive<'a>(input: &CardTextInput<'a>) -> Option<&'a DiskDrive> {
    let idx = input.entry.idx;
    input
        .snap
        .inventory
        .disks
        .iter()
        .find(|d| d.device == Some(idx))
}

/// The disk's primary mount: "/" when the disk holds it, else the first mount in partition
/// order, else the first inventory volume.
fn primary_mount<'a>(
    details: Option<&'a DiskDetails>,
    drive: Option<&'a DiskDrive>,
) -> Option<&'a str> {
    fn pick<'a>(mut mounts: impl Iterator<Item = &'a str> + Clone) -> Option<&'a str> {
        mounts.clone().find(|m| *m == "/").or_else(|| mounts.next())
    }
    let from_partitions = details
        .and_then(|d| d.partitions.as_deref())
        .and_then(|t| pick(t.rows.iter().filter_map(|r| r.mount.as_deref())));
    from_partitions.or_else(|| drive.and_then(|d| pick(d.volumes.iter().map(|v| &**v))))
}

fn battery<'a>(input: &CardTextInput<'a>) -> Option<&'a tl_model::snapshot::BatteryDetails> {
    let idx = input.entry.idx;
    input
        .snap
        .device_details
        .batteries
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, b)| b)
}

fn sensor(snap: &Snapshot, idx: DeviceIdx, kind: SensorKind) -> Option<&SensorReading> {
    snap.system
        .sensors
        .iter()
        .find(|(i, s)| *i == idx && s.kind == kind)
        .map(|(_, s)| s)
}

/// Present whole disks (no partitions).
fn present_disks(snap: &Snapshot) -> impl Iterator<Item = &DeviceEntry> {
    snap.devices.entries.iter().filter(|e| {
        e.present && e.parent.is_none() && matches!(e.identity, DeviceIdentity::Disk { .. })
    })
}

fn present_gpus(snap: &Snapshot) -> impl Iterator<Item = &DeviceEntry> {
    snap.devices
        .entries
        .iter()
        .filter(|e| e.present && matches!(e.identity, DeviceIdentity::Gpu { .. }))
}

/// The labels of a card's mini graph: no heading, value at the top-left, no legend, no range
/// word, no axis max (UI-PRF-011).
pub fn card_chart_labels(value_fmt: ValueFormat) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt,
        value_align: ValueAlign::Left,
        range_word: "",
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The grid of a card's mini graph: lines and outline on.
pub fn card_chart_grid() -> GridStyle {
    GridStyle {
        lines: true,
        baseline: false,
        outline: true,
        rounded: false,
    }
}

/// The surface a card's mini graph sits on.
pub const CARD_CHART_BG: ChartBg = ChartBg::List;

/// One sidebar card. Built by its owner's render; the chart entity is the owner's.
#[derive(IntoElement)]
pub struct DeviceCard {
    pub key: DeviceKey,
    pub chart: Entity<ChartView>,
    pub lines: CardLines,
    pub selected: bool,
    pub focused: bool,
    pub edit_mode: bool,
    pub hidden_in_edit: bool,
    pub width: Pixels,
}

impl DeviceCard {
    /// 0.6 for a disconnected interface and for a hidden card in edit mode, else 1.
    pub fn opacity(&self) -> f32 {
        if self.lines.disconnected || (self.edit_mode && self.hidden_in_edit) {
            DIM_OPACITY
        } else {
            1.0
        }
    }

    /// The 2 px selected border. Edit mode draws none.
    pub fn draws_selected_border(&self) -> bool {
        self.selected && !self.edit_mode
    }

    /// The 1 px focus ring around the selected border.
    pub fn draws_focus_ring(&self) -> bool {
        self.draws_selected_border() && self.focused
    }
}

impl RenderOnce for DeviceCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = m.ui_scale;
        let opacity = self.opacity();
        let (selected_border, focus_ring) = (self.draws_selected_border(), self.draws_focus_ring());
        let id = ElementId::Name(SharedString::from(self.key.0.clone()));
        let line2_color = if self.lines.line2_accent {
            theme.accent
        } else {
            theme.text_secondary
        };
        let lines = self.lines;

        let mut text = v_flex()
            .debug_selector(|| "perf-card-text".into())
            .relative()
            .flex_1()
            .min_w_0()
            .justify_center()
            .child(line(
                "perf-card-heading",
                lines.heading.clone(),
                &[],
                HEADING_LINE_H * s,
                label_style(&m, m.text.md, FontWeight::MEDIUM),
                theme.text_primary,
                &m,
            ))
            .child(line(
                "perf-card-line1",
                lines.line1.clone(),
                &lines.line1_locks,
                DETAIL_LINE_H * s,
                label_style(&m, m.text.sm, FontWeight::NORMAL),
                theme.text_secondary,
                &m,
            ))
            .child(line(
                "perf-card-line2",
                lines.line2.clone(),
                &lines.line2_locks,
                DETAIL_LINE_H * s,
                label_style(&m, m.text.sm, FontWeight::NORMAL),
                line2_color,
                &m,
            ));
        if let Some(tip) = lines.tooltip {
            let state = HoverTooltip::state(
                ElementId::NamedChild(std::sync::Arc::new(id.clone()), "tooltip".into()),
                window,
                cx,
            );
            let content = if tip.contains('\n') {
                TlTooltip::lines(
                    tip.split('\n')
                        .map(|l| SharedString::from(l.to_owned()))
                        .collect(),
                )
            } else {
                TlTooltip::text(tip)
            };
            text = HoverTooltip::attach(&state, text, DELAY_CONTROL, content.build(), window, cx);
        }

        let graph = div()
            .debug_selector(|| "perf-card-graph".into())
            .flex_none()
            .w(px(CARD_GRAPH_W * s))
            .h(px(CARD_GRAPH_H * s))
            .child(self.chart.cached(StyleRefinement::default().size_full()));

        let key = self.key;
        let pressed = theme.row_selected.opacity(PRESSED_ALPHA);
        let hover = theme.row_hover;
        let mut card = h_flex()
            .id(id)
            .debug_selector(|| "perf-card".into())
            .relative()
            .flex_none()
            .w(self.width)
            .h(px(CARD_H * s))
            .pl(m.sp[5])
            .gap(m.sp[4])
            .bg(theme.list_bg)
            .border_1()
            .border_color(theme.divider)
            .rounded(m.radius_card)
            .cursor_default()
            .hover(move |st| st.bg(hover))
            .active(move |st| st.bg(pressed))
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(PerfSelectDevice { key: key.clone() }), cx);
            })
            .child(graph)
            .child(text);
        if opacity < 1.0 {
            card = card.opacity(opacity);
        }
        if selected_border {
            card = card.child(
                div()
                    .debug_selector(|| "perf-card-selected".into())
                    .absolute()
                    .inset_0()
                    .border(px(2.0 * s))
                    .border_color(theme.accent)
                    .rounded(m.radius_card),
            );
        }
        if focus_ring {
            // 1 px ring, 2 px outside the card edge.
            let out = px(3.0 * s);
            card = card.child(
                div()
                    .debug_selector(|| "perf-card-focus-ring".into())
                    .absolute()
                    .inset(-out)
                    .border(px(s))
                    .border_color(theme.accent.opacity(FOCUS_RING_ALPHA))
                    .rounded(m.radius_card + out),
            );
        }
        card
    }
}

/// One single-line text row with an end ellipsis. A line with lock offsets is split into text
/// runs with a 12 px lock icon at each offset.
fn line(
    selector: &'static str,
    text: SharedString,
    locks: &[u16],
    height: f32,
    style: gpui_kit::TextStyleRefinement,
    color: Hsla,
    m: &UiMetrics,
) -> AnyElement {
    let mut row = div()
        .debug_selector(move || selector.into())
        .flex_none()
        .h(px(height))
        .min_w_0()
        .overflow_hidden()
        .text_color(color);
    *row.text_style() = style;
    if locks.is_empty() {
        return row
            .flex()
            .items_center()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(text)
            .into_any_element();
    }
    let mut row = row.flex().flex_row().items_center().whitespace_nowrap();
    let mut start = 0usize;
    for &at in locks {
        let at = usize::from(at).min(text.len());
        if at > start {
            row = row.child(
                div()
                    .flex_none()
                    .child(SharedString::from(text[start..at].to_owned())),
            );
        }
        row = row.child(
            div()
                .debug_selector(|| "perf-card-lock".into())
                .flex_none()
                .child(icon(TlIcon::Lock, m.icon.xs, color)),
        );
        start = at;
    }
    if start < text.len() {
        row = row.child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(SharedString::from(text[start..].to_owned())),
        );
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tl_model::snapshot::{
        BatteryDetails, DeviceDetailsSnapshot, InventorySnapshot, NetDetails, PartitionRow,
        PartitionTable, SystemSnapshot,
    };
    use tl_model::system::{DiskTotals, GpuTotals, NetTotals};
    use tl_model::table::Total;
    use tl_model::time::BootId;

    use super::*;

    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    fn text(t: &str) -> Total<Arc<str>> {
        Total {
            value: Arc::from(t),
            validity: Validity::Valid,
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity, display_name: &str) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from(display_name),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
        }
    }

    fn snap(
        system: SystemSnapshot,
        inventory: InventorySnapshot,
        details: DeviceDetailsSnapshot,
    ) -> Snapshot {
        let mut s = Snapshot::empty(BootId(1), 1);
        s.system = Arc::new(system);
        s.inventory = Arc::new(inventory);
        s.device_details = Arc::new(details);
        s
    }

    fn input<'a>(
        kind: DeviceKind,
        entry: &'a DeviceEntry,
        snap: &'a Snapshot,
    ) -> CardTextInput<'a> {
        CardTextInput {
            kind,
            number: 0,
            entry,
            snap,
            network_unit: NetworkUnit::Bits,
            auto_scale: false,
            fan_label: None,
            fan_temp: None,
        }
    }

    fn lines(input: &CardTextInput) -> CardLines {
        card_lines(input, &mut String::new())
    }

    fn strs(l: &CardLines) -> (&str, &str, &str) {
        (&l.heading, &l.line1, &l.line2)
    }

    fn cpu_system(freq: Metric<u32>, temp: Metric<f32>) -> SystemSnapshot {
        let mut sys = SystemSnapshot::default();
        sys.cpu.freq_mhz = freq;
        sys.cpu.temp_c = temp;
        sys
    }

    #[test]
    fn card_lines_cpu() {
        let cpu = entry(0, DeviceIdentity::CpuPackage, "CPU");
        let mut inv = InventorySnapshot::default();
        inv.processor.name = text("AMD Ryzen 7 7800X3D 8-Core Processor");
        let s = snap(
            cpu_system(Metric::valid(4631), Metric::valid(52.0)),
            inv.clone(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Cpu, &cpu, &s));
        assert_eq!(
            strs(&l),
            (
                "CPU",
                "AMD Ryzen 7 7800X3D 8-Core Processor",
                "4631 MHz · 52 °C"
            )
        );
        assert!(!l.line2_accent && !l.disconnected);
        assert_eq!(
            l.tooltip.as_deref(),
            Some("AMD Ryzen 7 7800X3D 8-Core Processor")
        );
        assert!(l.line1_locks.is_empty() && l.line2_locks.is_empty());

        // No CPU hwmon: the temperature part is left out.
        let s = snap(
            cpu_system(Metric::valid(4631), Metric::with(Validity::Unsupported)),
            inv.clone(),
            DeviceDetailsSnapshot::default(),
        );
        assert_eq!(lines(&input(DeviceKind::Cpu, &cpu, &s)).line2, "4631 MHz");

        // A failed temperature shows its glyph word in place of the value only.
        let s = snap(
            cpu_system(Metric::valid(4631), Metric::with(Validity::Failed)),
            inv,
            DeviceDetailsSnapshot::default(),
        );
        assert_eq!(
            lines(&input(DeviceKind::Cpu, &cpu, &s)).line2,
            "4631 MHz · !"
        );

        // Nothing reported yet.
        let s = snap(
            SystemSnapshot::default(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Cpu, &cpu, &s));
        assert_eq!(strs(&l), ("CPU", "Unknown processor", "…"));
    }

    #[test]
    fn card_lines_memory() {
        let mem = entry(1, DeviceIdentity::Memory, "Memory");
        let mut sys = SystemSnapshot::default();
        sys.memory.validity = Validity::Valid;
        sys.memory.used = (34.68 * GIB) as u64;
        sys.memory.total = (63.16 * GIB) as u64;
        sys.memory.committed = 69;
        sys.memory.commit_limit = 100;
        let s = snap(
            sys,
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Memory, &mem, &s));
        assert_eq!(
            strs(&l),
            ("Memory", "34.68 GiB / 63.16 GiB", "Commit 69.0%")
        );

        let s = snap(
            SystemSnapshot::default(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Memory, &mem, &s));
        assert_eq!(strs(&l), ("Memory", "…", "Commit …"));
    }

    fn net_entry(kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            net_kind: kind,
            ..entry(
                2,
                DeviceIdentity::Net {
                    netns_ino: 1,
                    ifindex: 2,
                    name: Arc::from("enp38s0"),
                },
                "enp38s0",
            )
        }
    }

    fn net_system(rx: f64, tx: f64, state: LinkState) -> SystemSnapshot {
        let sys = SystemSnapshot {
            nets: Arc::from(vec![(
                DeviceIdx(2),
                NetTotals {
                    device: DeviceIdx(2),
                    rx_bps: Metric::valid(rx),
                    tx_bps: Metric::valid(tx),
                    rx_total: 0,
                    tx_total: 0,
                    link_speed_mbps: Metric::valid(1000),
                    state,
                },
            )]),
            ..SystemSnapshot::default()
        };
        sys
    }

    fn net_details(type_label: &str, connection: Option<&str>) -> DeviceDetailsSnapshot {
        DeviceDetailsSnapshot {
            nets: Arc::from(vec![(
                DeviceIdx(2),
                NetDetails {
                    type_label: Arc::from(type_label),
                    connection: connection.map(Arc::from),
                    adapter_name: text("Intel I225-V"),
                    driver: text("igc"),
                    mac: text("00:11:22:33:44:55"),
                    ifindex: 2,
                    link_speed_mbps: Total::default(),
                    state: LinkState::Up,
                    addresses: Arc::from(vec![]),
                    kind: NetKind::Wired,
                    wifi: None,
                },
            )]),
            ..DeviceDetailsSnapshot::default()
        }
    }

    #[test]
    fn card_lines_network_both_directions() {
        let net = net_entry(Some(NetKind::Wired));
        // 1.2 Mbps down, 0.3 Mbps up, in bytes per second.
        let s = snap(
            net_system(150_000.0, 37_500.0, LinkState::Up),
            InventorySnapshot::default(),
            net_details("Ethernet", Some("Wired connection 1")),
        );
        let l = lines(&input(DeviceKind::Net, &net, &s));
        assert_eq!(strs(&l), ("enp38s0", "Ethernet", "↓ 1.2 Mbps ↑ 0.3 Mbps"));
        assert!(l.line2_accent);
        assert!(!l.disconnected);
        assert_eq!(l.tooltip.as_deref(), Some("Ethernet\nWired connection 1"));
    }

    #[test]
    fn card_lines_network_one_direction() {
        let net = net_entry(Some(NetKind::Wireless));
        let s = snap(
            net_system(150_000.0, 0.0, LinkState::Up),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Net, &net, &s));
        // No details yet: the type word comes from the catalog class.
        assert_eq!(strs(&l), ("enp38s0", "Wi-Fi", "1.2 Mbps"));
        assert!(l.line2_accent);
        assert_eq!(l.tooltip.as_deref(), Some("Wi-Fi"));

        // Bytes with auto-scale.
        let i = CardTextInput {
            network_unit: NetworkUnit::Bytes,
            auto_scale: true,
            ..input(DeviceKind::Net, &net, &s)
        };
        assert_eq!(lines(&i).line2, "146.5 KiB/s");

        // Down: the state word, not the accent, and the card dims.
        let s = snap(
            net_system(0.0, 0.0, LinkState::Down),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Net, &net, &s));
        assert_eq!(l.line2, "Disconnected");
        assert!(!l.line2_accent);
        assert!(l.disconnected);
        for (state, word) in [
            (LinkState::Testing, "Testing"),
            (LinkState::Dormant, "Dormant"),
            (LinkState::NotPresent, "Not present"),
            (LinkState::LowerLayerDown, "Lower layer down"),
            (LinkState::Unknown, "Unknown"),
        ] {
            let s = snap(
                net_system(0.0, 0.0, state),
                InventorySnapshot::default(),
                DeviceDetailsSnapshot::default(),
            );
            assert_eq!(lines(&input(DeviceKind::Net, &net, &s)).line2, word);
        }
    }

    fn disk_entry() -> DeviceEntry {
        entry(
            3,
            DeviceIdentity::Disk {
                major: 259,
                minor: 0,
                name: Arc::from("nvme0n1"),
                wwn_or_serial: None,
            },
            "nvme0n1",
        )
    }

    fn partition(name: &str, mount: Option<&str>) -> PartitionRow {
        PartitionRow {
            name: Arc::from(name),
            start_bytes: 0,
            size_bytes: 0,
            holder: None,
            mount: mount.map(Arc::from),
            extra_mounts: Arc::from(vec![]),
            label: None,
            fs_type: None,
            usage_pct: Total::default(),
            flags: Arc::from(vec![]),
            type_name: Arc::from(""),
            read_only: false,
            unallocated: false,
        }
    }

    fn disk_snap(mounts: &[Option<&str>], temp: Metric<f32>) -> Snapshot {
        let partitions = (!mounts.is_empty()).then(|| {
            Arc::new(PartitionTable {
                kind: Arc::from("GPT"),
                rows: mounts
                    .iter()
                    .enumerate()
                    .map(|(i, m)| partition(&format!("nvme0n1p{}", i + 1), *m))
                    .collect(),
            })
        });
        let details = DiskDetails {
            kind: Arc::from("NVMe"),
            model: text("Samsung SSD 990 PRO 2TB"),
            partitions,
            ..DiskDetails::default()
        };
        let sys = SystemSnapshot {
            disks: Arc::from(vec![(
                DeviceIdx(3),
                DiskTotals {
                    device: DeviceIdx(3),
                    active_pct: Metric::valid(0.1),
                    read_bps: Metric::valid(0.0),
                    write_bps: Metric::valid(0.0),
                    response_ms: Metric::valid(0.0),
                    read_total: 0,
                    written_total: 0,
                    capacity: 0,
                    temp_c: temp,
                },
            )]),
            ..SystemSnapshot::default()
        };
        snap(
            sys,
            InventorySnapshot::default(),
            DeviceDetailsSnapshot {
                disks: Arc::from(vec![(DeviceIdx(3), details)]),
                ..DeviceDetailsSnapshot::default()
            },
        )
    }

    #[test]
    fn card_lines_disk_with_and_without_mount() {
        let disk = disk_entry();
        let s = disk_snap(&[Some("/boot"), Some("/")], Metric::valid(41.0));
        let l = lines(&CardTextInput {
            number: 1,
            ..input(DeviceKind::Disk, &disk, &s)
        });
        assert_eq!(
            strs(&l),
            ("Disk 1", "NVMe - /", "Samsung SSD 990 PRO 2TB · 41 °C")
        );

        // No "/": the first mount.
        let s = disk_snap(&[None, Some("/data")], Metric::with(Validity::Unsupported));
        let l = lines(&input(DeviceKind::Disk, &disk, &s));
        assert_eq!(
            strs(&l),
            ("Disk 0", "NVMe - /data", "Samsung SSD 990 PRO 2TB")
        );

        // No mount: the kind only.
        let s = disk_snap(&[], Metric::with(Validity::Unsupported));
        assert_eq!(lines(&input(DeviceKind::Disk, &disk, &s)).line1, "NVMe");

        // Restricted temperature: the model, the separator, then a lock at the line end.
        let s = disk_snap(&[], Metric::with(Validity::Restricted));
        let l = lines(&input(DeviceKind::Disk, &disk, &s));
        assert_eq!(l.line2, "Samsung SSD 990 PRO 2TB · ");
        assert_eq!(&l.line2_locks[..], &[l.line2.len() as u16]);
    }

    fn sensor_reading(idx: u16, kind: SensorKind, value: f32) -> (DeviceIdx, SensorReading) {
        (
            DeviceIdx(idx),
            SensorReading {
                device: DeviceIdx(idx),
                kind,
                value: Metric::valid(value),
                pwm_pct: Metric::with(Validity::Unsupported),
                fan_control: None,
                fan_connected: true,
            },
        )
    }

    fn battery_details(kind: &str) -> BatteryDetails {
        BatteryDetails {
            kind: Arc::from(kind),
            vendor: text("Logitech"),
            model: text("MX Master 3"),
            serial: Total::default(),
            native_path: Arc::from("hidpp_battery_0"),
            percentage: Total {
                value: 85.0,
                validity: Validity::Valid,
            },
            energy_wh: Total::default(),
            energy_rate_w: Total::default(),
            voltage_v: Total::default(),
            state: Arc::from("Discharging"),
            time_to_full_s: Total::default(),
            time_to_empty_s: Total::default(),
            charge_cycles: Total::default(),
            technology: Arc::from(""),
            capacity_pct: Total::default(),
            energy_empty_wh: Total::default(),
            energy_full_wh: Total::default(),
            energy_full_design_wh: Total::default(),
            voltage_min_design_v: Total::default(),
            voltage_max_design_v: Total::default(),
            power_supply: false,
            charge_threshold: None,
            temperature_c: Total::default(),
            history: None,
        }
    }

    fn gpu_entry(idx: u16, name: &str) -> DeviceEntry {
        entry(
            idx,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from(format!("0000:0{idx}:00.0")),
            },
            name,
        )
    }

    #[test]
    fn card_lines_fan_battery_aggregate() {
        // Fan: number and chip, RPM and the paired sensor.
        let fan = entry(
            4,
            DeviceIdentity::Fan {
                chip: Arc::from("nct6799"),
                index: 2,
            },
            "nct6799 fan2",
        );
        let sys = SystemSnapshot {
            sensors: Arc::from(vec![
                sensor_reading(4, SensorKind::Fan, 1200.4),
                sensor_reading(5, SensorKind::Temperature, 45.0),
            ]),
            ..SystemSnapshot::default()
        };
        let s = snap(
            sys,
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&CardTextInput {
            number: 2,
            ..input(DeviceKind::Fan, &fan, &s)
        });
        assert_eq!(strs(&l), ("Fan 2", "nct6799", "1200 RPM"));
        let l = lines(&CardTextInput {
            fan_label: Some("CPU fan"),
            fan_temp: Some(DeviceIdx(5)),
            ..input(DeviceKind::Fan, &fan, &s)
        });
        assert_eq!(strs(&l), ("CPU fan", "CPU fan", "1200 RPM · 45 °C"));

        // Battery: kind word, vendor and model, percentage and state.
        let mouse = entry(
            6,
            DeviceIdentity::Battery {
                native_path: Arc::from("hidpp_battery_0"),
            },
            "MX Master 3",
        );
        let s = snap(
            SystemSnapshot::default(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot {
                batteries: Arc::from(vec![(DeviceIdx(6), battery_details("Mouse"))]),
                ..DeviceDetailsSnapshot::default()
            },
        );
        let l = lines(&input(DeviceKind::Battery, &mouse, &s));
        assert_eq!(
            strs(&l),
            ("Mouse", "Logitech MX Master 3", "85.0% · Discharging")
        );
        let s = snap(
            SystemSnapshot::default(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Battery, &mouse, &s));
        assert_eq!(strs(&l), ("Battery", "MX Master 3", "…"));

        // Aggregates: counts and the common kind or vendor.
        let disk = disk_entry();
        let mut s = disk_snap(&[], Metric::with(Validity::Unsupported));
        let usb = entry(
            7,
            DeviceIdentity::Disk {
                major: 8,
                minor: 0,
                name: Arc::from("sda"),
                wwn_or_serial: None,
            },
            "sda",
        );
        let part = DeviceEntry {
            parent: Some(DeviceIdx(3)),
            ..entry(
                8,
                DeviceIdentity::Disk {
                    major: 259,
                    minor: 1,
                    name: Arc::from("nvme0n1p1"),
                    wwn_or_serial: None,
                },
                "nvme0n1p1",
            )
        };
        let (a, b) = (
            gpu_entry(9, "AMD Radeon RX 7900 XTX"),
            gpu_entry(10, "AMD Radeon 780M"),
        );
        s.devices = Arc::new(tl_model::devices::DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![disk.clone(), part, a.clone(), b.clone()]),
        });
        let agg = entry(
            11,
            DeviceIdentity::Aggregate(tl_model::devices::AggregateKind::Disks),
            "",
        );
        let l = lines(&input(DeviceKind::AggregateDisk, &agg, &s));
        assert_eq!(strs(&l), ("Aggregate storage", "1 disks", "NVMe"));
        let l = lines(&input(DeviceKind::AggregateGpu, &agg, &s));
        assert_eq!(strs(&l), ("Aggregate graphics", "2 adapters", "AMD"));

        let mut details = (*s.device_details).clone();
        let mut disks = details.disks.to_vec();
        disks.push((
            DeviceIdx(7),
            DiskDetails {
                kind: Arc::from("USB"),
                ..DiskDetails::default()
            },
        ));
        details.disks = Arc::from(disks);
        s.device_details = Arc::new(details);
        s.devices = Arc::new(tl_model::devices::DeviceCatalog {
            version: 2,
            entries: Arc::from(vec![disk, usb, a, gpu_entry(12, "NVIDIA GeForce RTX 4080")]),
        });
        let l = lines(&input(DeviceKind::AggregateDisk, &agg, &s));
        assert_eq!(strs(&l), ("Aggregate storage", "2 disks", "Multiple"));
        let l = lines(&input(DeviceKind::AggregateGpu, &agg, &s));
        assert_eq!(l.line2, "Multiple");
    }

    #[test]
    fn card_lines_gpu() {
        let gpu = gpu_entry(9, "AMD Radeon RX 7900 XTX");
        let mut sys = SystemSnapshot::default();
        let mut totals = GpuTotals {
            device: DeviceIdx(9),
            util_pct: Metric::valid(10.9),
            mem_util_pct: Metric::default(),
            mem_used: Metric::default(),
            mem_total: Metric::default(),
            shared_used: Metric::default(),
            shared_total: Metric::default(),
            enc_pct: Metric::default(),
            dec_pct: Metric::default(),
            temp_c: Metric::valid(52.0),
            power_w: Metric::default(),
            power_limit_w: Metric::default(),
            clock_gfx_mhz: Metric::default(),
            clock_mem_mhz: Metric::default(),
            fan_pct: Metric::default(),
            pcie_rx_bps: Metric::default(),
            pcie_tx_bps: Metric::default(),
        };
        sys.gpus = Arc::from(vec![(DeviceIdx(9), totals)]);
        let s = snap(
            sys.clone(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Gpu, &gpu, &s));
        assert_eq!(strs(&l), ("GPU 0", "AMD Radeon RX 7900 XTX", "52 °C"));

        totals.temp_c = Metric::with(Validity::Unsupported);
        sys.gpus = Arc::from(vec![(DeviceIdx(9), totals)]);
        let s = snap(
            sys,
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let nameless = gpu_entry(9, " ");
        let l = lines(&input(DeviceKind::Gpu, &nameless, &s));
        assert_eq!(strs(&l), ("GPU 0", "Unknown GPU", "n/a"));
    }

    #[test]
    fn card_lines_unchanged_keeps_allocation() {
        let net = net_entry(Some(NetKind::Wired));
        let s = snap(
            net_system(150_000.0, 37_500.0, LinkState::Up),
            InventorySnapshot::default(),
            net_details("Ethernet", Some("Wired connection 1")),
        );
        let i = input(DeviceKind::Net, &net, &s);
        let mut scratch = String::new();
        let first = card_lines(&i, &mut scratch);
        assert_eq!(
            card_lines(&i, &mut scratch),
            first,
            "same inputs, equal lines"
        );

        // The owner's compare-and-keep step.
        let mut kept = first.clone();
        assert!(!update_card_lines(&mut kept, &i, &mut scratch));
        let same = |a: &SharedString, b: &SharedString| std::ptr::eq(a.as_ptr(), b.as_ptr());
        assert!(same(&kept.heading, &first.heading));
        assert!(same(&kept.line1, &first.line1));
        assert!(same(&kept.line2, &first.line2));
        assert!(same(
            kept.tooltip.as_ref().expect("tooltip"),
            first.tooltip.as_ref().expect("tooltip")
        ));

        // A new rate replaces line 2 only.
        let s2 = snap(
            net_system(300_000.0, 37_500.0, LinkState::Up),
            InventorySnapshot::default(),
            net_details("Ethernet", Some("Wired connection 1")),
        );
        assert!(update_card_lines(
            &mut kept,
            &input(DeviceKind::Net, &net, &s2),
            &mut scratch
        ));
        assert_eq!(kept.line2, "↓ 2.4 Mbps ↑ 0.3 Mbps");
        assert!(same(&kept.line1, &first.line1));
        assert!(!same(&kept.line2, &first.line2));

        // Without a connection the tooltip shares line 1's allocation.
        let cpu = entry(0, DeviceIdentity::CpuPackage, "CPU");
        let s = snap(
            SystemSnapshot::default(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Cpu, &cpu, &s));
        assert!(same(l.tooltip.as_ref().expect("tooltip"), &l.line1));
    }
}
