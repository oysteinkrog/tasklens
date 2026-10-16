//! The device sidebar card: the text builders for every device kind and the card element
//! (docs/ui/performance.md UI-PRF-011; PERF-001, PERF-005, PERF-008, PERF-013, PERF-053,
//! PERF-054, R-050, R-055). The sidebar (E24-004) and the mini window (E26) own the cards.
//!
//! The words come from `perf-card-*`, `perf-unknown-*`, `perf-net-state-*` and `perf-net-type-*`
//! in `en-US.ftl`, translated once per strings version into [`CardWords`] ([`card_words`]).

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

#[cfg(any(test, feature = "test-hooks"))]
use gpui_kit::canvas;
use gpui_kit::component::h_flex;
use gpui_kit::{
    AnyElement, App, AsyncApp, Context, CursorStyle, ElementId, Entity, EntityId, Font, FontWeight,
    Global, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Task,
    TextRun, WeakEntity, Window, black, div, font, px,
};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::{DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::series::SeriesView;
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{DiskDetails, DiskDrive, Snapshot};
use tl_model::system::{LinkState, Metric, SensorKind, SensorReading};
use tl_model::time::now_boot_ns;
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;
use unicode_segmentation::UnicodeSegmentation as _;

use super::actions::PerfSelectDevice;
use super::details::Template;
use super::{DeviceKey, DeviceKind};
use crate::chart::{
    ChartBg, ChartLabels, ChartView, GridStyle, ValueAlign, ValueFormat, format_value,
};
use crate::components::icons::{TlIcon, icon};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::validity::copy_text;
use crate::i18n::{Strings, tr, tr_en};
use crate::settings_global::TlSettings;
use crate::text::ellipsis::{ELLIPSIS, middle_ellipsis};
use crate::text::fonts::UI_FAMILY;
use crate::text::style::{TNUM, label_style, numeric_style};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Least card height, × ui_scale. A card grows when its wrapped text needs more.
pub const CARD_H: f32 = 80.0;
/// Mini graph width, × ui_scale. A card too narrow to hold it and the widest value makes the
/// graph narrower ([`card_graph_width`]).
pub const CARD_GRAPH_W: f32 = 116.0;
/// Narrowest mini graph, × ui_scale. With less room the card draws no graph and the text
/// column takes the whole card.
const CARD_GRAPH_MIN_W: f32 = 40.0;
/// Mini graph height, × ui_scale.
pub const CARD_GRAPH_H: f32 = 56.0;

/// Heading line height, × ui_scale.
const HEADING_LINE_H: f32 = 18.0;
/// Line 1 and line 2 height, × ui_scale.
const DETAIL_LINE_H: f32 = 16.0;
/// Space above and below the text column, × ui_scale (sp2).
const TEXT_PAD_Y: f32 = 4.0;
/// The R-051 stale floor: a series is stale when its newest sample is older than
/// max(3 periods, this), as `chart::is_stale`.
const STALE_FLOOR_NS: u64 = 1_500_000_000;
/// Opacity of a disconnected interface and of a hidden card in edit mode.
const DIM_OPACITY: f32 = 0.6;
/// Alpha of the pressed tint and of the focus ring.
const PRESSED_ALPHA: f32 = 0.6;
const FOCUS_RING_ALPHA: f32 = 0.5;

/// Between two values on one line.
const SEP: &str = " · ";
/// Between the disk kind and its mount.
const KIND_SEP: &str = " - ";
const DOWN: &str = "↓ ";
const UP: &str = " ↑ ";

/// The card words, translated once per strings version by [`card_words`]. [`CardWords::en`] is
/// in en-US.
#[derive(Clone, Debug)]
pub struct CardWords {
    pub overview: SharedString,
    /// The Overview card's static line 1 (D-022).
    pub overview_line1: SharedString,
    pub cpu: SharedString,
    pub memory: SharedString,
    /// "GPU {$n}".
    pub gpu: Template,
    /// "Disk {$n}".
    pub disk: Template,
    /// "Fan {$n}".
    pub fan: Template,
    pub battery: SharedString,
    pub aggregate_storage: SharedString,
    pub aggregate_graphics: SharedString,
    /// "{$k} disks".
    pub disks_count: Template,
    /// "{$k} adapters".
    pub adapters_count: Template,
    pub multiple: SharedString,
    /// "Commit {$pct} of limit".
    pub commit: Template,
    /// Why commit can pass 100%.
    pub commit_tip: SharedString,
    pub unknown_processor: SharedString,
    pub unknown_gpu: SharedString,
    /// PERF-054 state words.
    pub disconnected: SharedString,
    pub testing: SharedString,
    pub dormant: SharedString,
    pub not_present: SharedString,
    pub lower_layer_down: SharedString,
    pub unknown: SharedString,
    /// PERF-054 type words by catalog class.
    pub ethernet: SharedString,
    pub wifi: SharedString,
    pub vpn: SharedString,
    pub virtual_net: SharedString,
    pub other_net: SharedString,
}

impl CardWords {
    /// Translates every word with the current strings.
    pub fn new(cx: &App) -> Self {
        Self::build(|key| tr(cx, key), |key, args| Template::new(cx, key, args))
    }

    /// The en-US words, built once.
    pub fn en() -> &'static Self {
        static EN: LazyLock<CardWords> =
            LazyLock::new(|| CardWords::build(|key| tr_en(key, &[]), Template::en));
        &EN
    }

    fn build(
        t: impl Fn(&str) -> SharedString,
        template: impl Fn(&str, &[&str]) -> Template,
    ) -> Self {
        Self {
            overview: t("perf-card-heading-overview"),
            overview_line1: t("perf-card-overview-line1"),
            cpu: t("perf-card-heading-cpu"),
            memory: t("perf-card-heading-memory"),
            gpu: template("perf-card-heading-gpu", &["n"]),
            disk: template("perf-card-heading-disk", &["n"]),
            fan: template("perf-card-heading-fan", &["n"]),
            battery: t("perf-card-heading-battery"),
            aggregate_storage: t("perf-card-aggregate-storage"),
            aggregate_graphics: t("perf-card-aggregate-graphics"),
            disks_count: template("perf-card-disks-count", &["k"]),
            adapters_count: template("perf-card-adapters-count", &["k"]),
            multiple: t("perf-card-multiple"),
            commit: template("perf-card-commit", &["pct"]),
            commit_tip: t("perf-card-commit-tooltip"),
            unknown_processor: t("perf-unknown-processor"),
            unknown_gpu: t("perf-unknown-gpu"),
            disconnected: t("perf-net-state-disconnected"),
            testing: t("perf-net-state-testing"),
            dormant: t("perf-net-state-dormant"),
            not_present: t("perf-net-state-not-present"),
            lower_layer_down: t("perf-net-state-lower-layer-down"),
            unknown: t("perf-net-state-unknown"),
            ethernet: t("perf-net-type-ethernet"),
            wifi: t("perf-net-type-wifi"),
            vpn: t("perf-net-type-vpn"),
            virtual_net: t("perf-net-type-virtual"),
            other_net: t("perf-net-type-other"),
        }
    }
}

/// The words for one strings version, shared by every card owner.
struct CardWordsCache {
    version: u64,
    words: Rc<CardWords>,
}

impl Global for CardWordsCache {}

/// The card words for the current strings. They are translated only when the strings version
/// changed since the last call, so a frame makes no `tr` call. Without strings they are in en-US.
pub fn card_words(cx: &mut App) -> Rc<CardWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return en_words();
    };
    match cx.try_global::<CardWordsCache>() {
        Some(cache) if cache.version == version => cache.words.clone(),
        _ => {
            let words = Rc::new(CardWords::new(cx));
            cx.set_global(CardWordsCache {
                version,
                words: words.clone(),
            });
            words
        }
    }
}

/// [`CardWords::en`] behind an `Rc`, built once per thread.
fn en_words() -> Rc<CardWords> {
    thread_local! {
        static EN: Rc<CardWords> = Rc::new(CardWords::en().clone());
    }
    EN.with(Rc::clone)
}

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
    /// The full line 1; network cards use the adapter name and add the NetworkManager
    /// connection on a second line.
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
    pub words: &'a CardWords,
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

/// The card text for `input`. Formats through `scratch`. Every string over 23 bytes (the
/// `SharedString` inline size) is a new allocation, so owners that refresh a card use
/// [`update_card_lines`], which keeps unchanged strings.
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

    let net_names = (input.kind == DeviceKind::Net).then(|| net_names(input));
    changed |= match (input.kind, net_names) {
        (DeviceKind::Memory, _) => match &mut lines.tooltip {
            Some(tip) => keep_or_set(tip, &input.words.commit_tip),
            None => {
                lines.tooltip = Some(input.words.commit_tip.clone());
                true
            }
        },
        (DeviceKind::Net, Some((adapter, Some(connection)))) => {
            scratch.clear();
            scratch.push_str(adapter.unwrap_or(&lines.line1));
            scratch.push('\n');
            scratch.push_str(connection);
            match &mut lines.tooltip {
                Some(tip) => keep_or_set(tip, scratch),
                None => {
                    lines.tooltip = Some(scratch.clone().into());
                    true
                }
            }
        }
        (DeviceKind::Net, Some((Some(adapter), None))) => match &mut lines.tooltip {
            Some(tip) => keep_or_set(tip, adapter),
            None => {
                lines.tooltip = Some(adapter.to_owned().into());
                true
            }
        },
        _ if lines.line1.is_empty() => lines.tooltip.take().is_some(),
        // Shares line 1's allocation.
        _ if lines.tooltip.as_deref() == Some(&*lines.line1) => false,
        _ => {
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

/// Writes `template` with `n` in its one hole.
fn write_n(out: &mut String, template: &Template, n: impl std::fmt::Display) {
    template.write_each(out, |_, o| {
        let _ = write!(o, "{n}");
    });
}

fn write_heading(input: &CardTextInput, out: &mut String) {
    let n = input.number;
    let w = input.words;
    match input.kind {
        DeviceKind::Overview => out.push_str(&w.overview),
        DeviceKind::Cpu => out.push_str(&w.cpu),
        DeviceKind::Memory => out.push_str(&w.memory),
        DeviceKind::Gpu => write_n(out, &w.gpu, n),
        DeviceKind::Disk => write_n(out, &w.disk, n),
        DeviceKind::Net => out.push_str(ifname(input.entry)),
        DeviceKind::Fan => match input.fan_label.and_then(|l| shown(l, Validity::Valid)) {
            Some(label) => out.push_str(label),
            None => write_n(out, &w.fan, n),
        },
        DeviceKind::Battery => {
            let kind = battery(input).and_then(|b| shown(&b.kind, Validity::Valid));
            out.push_str(kind.unwrap_or(&*w.battery));
        }
        DeviceKind::AggregateDisk => out.push_str(&w.aggregate_storage),
        DeviceKind::AggregateGpu => out.push_str(&w.aggregate_graphics),
    }
}

fn write_line1(input: &CardTextInput, out: &mut String, locks: &mut LockOffsets) {
    let snap = input.snap;
    let entry = input.entry;
    let w = input.words;
    match input.kind {
        DeviceKind::Overview => out.push_str(&w.overview_line1),
        DeviceKind::Cpu => {
            let p = &snap.inventory.processor.name;
            out.push_str(shown(&p.value, p.validity).unwrap_or(&*w.unknown_processor));
        }
        DeviceKind::Memory => {
            let mem = &snap.system.memory;
            value(out, locks, mem.validity, |o| {
                units::bytes_of(o, mem.used, mem.total);
            });
        }
        DeviceKind::Gpu => {
            out.push_str(shown(&entry.display_name, Validity::Valid).unwrap_or(&*w.unknown_gpu));
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
        DeviceKind::AggregateDisk => write_n(out, &w.disks_count, present_disks(snap).count()),
        DeviceKind::AggregateGpu => write_n(out, &w.adapters_count, present_gpus(snap).count()),
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
    let w = input.words;
    match input.kind {
        // Empty (D-022).
        DeviceKind::Overview => {}
        DeviceKind::Cpu => {
            let freq = sys.cpu.freq_mhz;
            value(out, locks, freq.validity, |o| {
                units::mhz(o, freq.value as f32)
            });
            optional_part(out, locks, sys.cpu.temp_c, units::celsius);
        }
        DeviceKind::Memory => {
            let mem = &sys.memory;
            let validity = if mem.validity.is_valid() && mem.commit_limit == 0 {
                Validity::Unsupported
            } else {
                mem.validity
            };
            w.commit.write_each(out, |_, out| {
                value(out, locks, validity, |o| {
                    units::percent(
                        o,
                        (mem.committed as f64 / mem.commit_limit as f64 * 100.0) as f32,
                    );
                });
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
            Some(NetState::Down(state)) => out.push_str(state_word(w, state)),
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
            write_common(out, kinds, &w.multiple);
        }
        DeviceKind::AggregateGpu => {
            // The vendor is the first word of the adapter name ("AMD", "NVIDIA", "Intel").
            let vendors =
                present_gpus(snap).filter_map(|e| e.display_name.split_whitespace().next());
            write_common(out, vendors, &w.multiple);
        }
    }
}

/// The common word of `words`, `multiple` when they differ, nothing when there is none.
fn write_common<'a>(out: &mut String, mut words: impl Iterator<Item = &'a str>, multiple: &str) {
    let Some(first) = words.next() else {
        return;
    };
    out.push_str(if words.all(|w| w == first) {
        first
    } else {
        multiple
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
fn state_word(w: &CardWords, state: LinkState) -> &str {
    match state {
        LinkState::Up | LinkState::Down => &w.disconnected,
        LinkState::Testing => &w.testing,
        LinkState::Dormant => &w.dormant,
        LinkState::NotPresent => &w.not_present,
        LinkState::LowerLayerDown => &w.lower_layer_down,
        LinkState::Unknown => &w.unknown,
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
    let w = input.words;
    label.unwrap_or(match input.entry.net_kind {
        Some(NetKind::Wired) => &*w.ethernet,
        Some(NetKind::Wireless) => &*w.wifi,
        Some(NetKind::VpnTunnel) => &*w.vpn,
        Some(NetKind::Virtual) => &*w.virtual_net,
        Some(NetKind::Other) | None => &*w.other_net,
    })
}

/// The adapter and NetworkManager connection names, when known.
fn net_names<'a>(input: &CardTextInput<'a>) -> (Option<&'a str>, Option<&'a str>) {
    let idx = input.entry.idx;
    let Some((_, details)) = input
        .snap
        .device_details
        .nets
        .iter()
        .find(|(i, _)| *i == idx)
    else {
        return (None, None);
    };
    (
        shown(&details.adapter_name.value, details.adapter_name.validity),
        details
            .connection
            .as_deref()
            .and_then(|connection| shown(connection, Validity::Valid)),
    )
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

/// The labels of a card's mini graph: no heading, no legend, no range word, no axis max. The
/// card draws the value in its heading row, so the graph draws none (UI-PRF-011).
pub fn card_chart_labels(value_fmt: ValueFormat) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
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

/// The fonts of the card text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardFont {
    /// The heading: `t_md` 500.
    Heading,
    /// Line 1 and line 2: `t_sm` 400.
    Detail,
    /// The value: `t_md` 400 with tabular figures.
    Value,
}

impl CardFont {
    pub fn size(self, m: &UiMetrics) -> Pixels {
        match self {
            Self::Heading | Self::Value => m.text.md,
            Self::Detail => m.text.sm,
        }
    }

    fn font(self) -> Font {
        let base = font(UI_FAMILY);
        match self {
            Self::Heading => Font {
                weight: FontWeight::MEDIUM,
                ..base
            },
            Self::Detail => Font {
                weight: FontWeight::NORMAL,
                ..base
            },
            Self::Value => Font {
                weight: FontWeight::NORMAL,
                features: TNUM.clone(),
                ..base
            },
        }
    }
}

/// One drawn row of a card line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardRow {
    pub text: SharedString,
    /// Byte offsets into `text` where a lock icon goes.
    pub locks: LockOffsets,
}

/// One card line in at most two rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardLineLayout {
    pub rows: SmallVec<[CardRow; 2]>,
    /// The line did not fit: its last row is cut in the middle and the tooltip has the full line.
    pub cut: bool,
}

/// The card text in rows, from [`card_text_layout`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardTextLayout {
    pub heading: CardLineLayout,
    /// The value fits after the last heading row. Otherwise it has a row of its own under it.
    pub value_inline: bool,
    /// The value is wider than the text column even on its own row, as on an edit-mode card at
    /// nav_width 180: it takes 2 rows, the number on the first and the unit on the second.
    pub value_split: bool,
    pub line1: CardLineLayout,
    pub line2: CardLineLayout,
}

impl CardTextLayout {
    /// Text column height, × ui_scale.
    pub fn text_height(&self) -> f32 {
        let value_row = if self.value_inline {
            0.0
        } else if self.value_split {
            2.0 * HEADING_LINE_H
        } else {
            HEADING_LINE_H
        };
        self.heading.rows.len() as f32 * HEADING_LINE_H
            + value_row
            + (self.line1.rows.len() + self.line2.rows.len()) as f32 * DETAIL_LINE_H
    }

    /// The card height: the text column with sp2 above and below it and the 1 px border, at
    /// least [`CARD_H`].
    pub fn card_height(&self, ui_scale: f32) -> Pixels {
        px((self.text_height() + 2.0 * (TEXT_PAD_Y + 1.0)).max(CARD_H) * ui_scale)
    }
}

/// The mini graph width of a card `card_w` wide: [`CARD_GRAPH_W`], or less so that the text
/// column is at least `text_min` (the card's widest value) wide. Zero, no graph, when less than
/// [`CARD_GRAPH_MIN_W`] is left.
pub fn card_graph_width(card_w: Pixels, text_min: Pixels, m: &UiMetrics) -> Pixels {
    let s = m.ui_scale;
    let room = card_w - px(2.0 * s) - m.sp[5] - m.sp[4] - text_min;
    let w = room.min(px(CARD_GRAPH_W * s));
    if w < px(CARD_GRAPH_MIN_W * s) {
        Pixels::ZERO
    } else {
        w
    }
}

/// The text column width of a card `card_w` wide with a `graph_w` graph: less the 1 px border
/// on both sides, the sp5 inset, and the graph with the sp4 gap after it.
pub fn card_text_width(card_w: Pixels, graph_w: Pixels, m: &UiMetrics) -> Pixels {
    let s = m.ui_scale;
    let graph = if graph_w > Pixels::ZERO {
        graph_w + m.sp[4]
    } else {
        Pixels::ZERO
    };
    (card_w - px(2.0 * s) - m.sp[5] - graph).max(Pixels::ZERO)
}

/// Lays out the card text in a column `text_w` wide (UI-PRF-011). Each line wraps at spaces to
/// at most 2 rows, and what does not fit row 2 is cut in the middle ([`middle_ellipsis`]). A
/// line whose first word does not fit is cut in the middle on one row. The value goes `gap`
/// after the last heading row when `value_w` fits there, else on a row of its own, and on 2 rows
/// (number, then unit) when it is wider than `text_w`; a value is never cut. `measure` returns the width of a string in a card font; `lock_w` is the width of
/// one lock icon.
pub fn card_text_layout(
    lines: &CardLines,
    value_w: Pixels,
    text_w: Pixels,
    gap: Pixels,
    lock_w: Pixels,
    measure: &mut impl FnMut(CardFont, &str) -> Pixels,
) -> CardTextLayout {
    let heading = wrap_line(
        &lines.heading,
        &[],
        CardFont::Heading,
        text_w,
        lock_w,
        measure,
    );
    let last_w = heading.rows.last().map_or(Pixels::ZERO, |row| {
        measure(CardFont::Heading, &row.text) + lock_w * row.locks.len() as f32
    });
    let value_inline = value_w == Pixels::ZERO || last_w + gap + value_w <= text_w;
    let value_split = !value_inline && value_w > text_w;
    let line1 = wrap_line(
        &lines.line1,
        &lines.line1_locks,
        CardFont::Detail,
        text_w,
        lock_w,
        measure,
    );
    let line2 = wrap_line(
        &lines.line2,
        &lines.line2_locks,
        CardFont::Detail,
        text_w,
        lock_w,
        measure,
    );
    CardTextLayout {
        heading,
        value_inline,
        value_split,
        line1,
        line2,
    }
}

/// Number of `locks` in `start..end`.
fn lock_count(locks: &[u16], start: usize, end: usize) -> usize {
    locks
        .iter()
        .filter(|&&at| (start..end).contains(&usize::from(at)))
        .count()
}

/// The locks in `start..end` of a line, as offsets into the row that starts at `start`.
fn row_locks(locks: &[u16], start: usize, end: usize) -> LockOffsets {
    locks
        .iter()
        .map(|&at| usize::from(at))
        .filter(|at| (start..end).contains(at))
        .map(|at| u16::try_from(at - start).unwrap_or(u16::MAX))
        .collect()
}

/// The places a line may wrap: the end of the text before each run of spaces and the start of
/// the text after it. Runs at the start or the end of the line are not places to wrap.
fn space_breaks(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let bytes = text.as_bytes();
    (1..bytes.len())
        .filter(move |&i| bytes[i] == b' ' && bytes[i - 1] != b' ')
        .filter_map(move |end| {
            let next = end + bytes[end..].iter().take_while(|&&b| b == b' ').count();
            (next < bytes.len()).then_some((end, next))
        })
}

/// `text` in at most 2 rows of `max` (see [`card_text_layout`]). A lock at the end of the text
/// belongs to the last row.
fn wrap_line(
    text: &SharedString,
    locks: &[u16],
    font: CardFont,
    max: Pixels,
    lock_w: Pixels,
    measure: &mut impl FnMut(CardFont, &str) -> Pixels,
) -> CardLineLayout {
    let len = text.len();
    if measure(font, text) + lock_w * locks.len() as f32 <= max {
        return CardLineLayout {
            rows: smallvec![CardRow {
                text: text.clone(),
                locks: LockOffsets::from_slice(locks),
            }],
            cut: false,
        };
    }
    // The widest row 1 that ends before a run of spaces; row 2 starts after the run.
    let mut best = None;
    for (end, next) in space_breaks(text) {
        let w = measure(font, &text[..end]) + lock_w * lock_count(locks, 0, next) as f32;
        if w > max {
            break;
        }
        best = Some((end, next));
    }
    let Some((end, next)) = best else {
        let (row, cut) = cut_row(text, locks, font, max, lock_w, measure);
        return CardLineLayout {
            rows: smallvec![row],
            cut,
        };
    };
    let row1 = CardRow {
        text: SharedString::from(text[..end].to_owned()),
        locks: row_locks(locks, 0, next),
    };
    let rest = &text[next..];
    let rest_locks = row_locks(locks, next, len + 1);
    let (row2, cut) = if measure(font, rest) + lock_w * rest_locks.len() as f32 <= max {
        let row = CardRow {
            text: SharedString::from(rest.to_owned()),
            locks: rest_locks,
        };
        (row, false)
    } else {
        cut_row(rest, &rest_locks, font, max, lock_w, measure)
    };
    CardLineLayout {
        rows: smallvec![row1, row2],
        cut,
    }
}

/// `text` on one row of `max`, cut in the middle. A lock in the cut-out part goes with it. The
/// bool is false when the text is kept whole, which [`middle_ellipsis`] does when the cut would
/// remove fewer than 2 characters.
fn cut_row(
    text: &str,
    locks: &[u16],
    font: CardFont,
    max: Pixels,
    lock_w: Pixels,
    measure: &mut impl FnMut(CardFont, &str) -> Pixels,
) -> (CardRow, bool) {
    let budget = (max - lock_w * locks.len() as f32).max(Pixels::ZERO);
    let mut width = |s: &str| measure(font, s);
    let out = middle_ellipsis(text, budget, &mut width);
    let points = if width(&out) <= budget {
        cut_points(text, &out)
    } else {
        Some(balanced_cut(text, budget, &mut width))
    };
    let Some((head, tail)) = points else {
        let row = CardRow {
            text: out,
            locks: LockOffsets::from_slice(locks),
        };
        return (row, false);
    };
    let mut out = String::with_capacity(head + ELLIPSIS.len_utf8() + text.len() - tail);
    out.push_str(&text[..head]);
    out.push(ELLIPSIS);
    out.push_str(&text[tail..]);
    let out = SharedString::from(out);
    let shift = head + ELLIPSIS.len_utf8();
    let locks = locks
        .iter()
        .map(|&at| usize::from(at))
        .filter_map(|at| {
            if at <= head {
                Some(at)
            } else {
                (at >= tail).then(|| at - tail + shift)
            }
        })
        .map(|at| u16::try_from(at).unwrap_or(u16::MAX))
        .collect();
    (CardRow { text: out, locks }, true)
}

/// The middle cut for text that [`middle_ellipsis`] cannot fit in `max`: it keeps up to 12
/// characters of the end whole, more than a narrow card has room for. This keeps graphemes from
/// the start and the end in turn while they fit. Returns the end of the kept start and the start
/// of the kept end, as byte offsets into `text`.
fn balanced_cut(
    text: &str,
    max: Pixels,
    measure: &mut impl FnMut(&str) -> Pixels,
) -> (usize, usize) {
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let mut buf = [0u8; 4];
    let mut budget = max - measure(ELLIPSIS.encode_utf8(&mut buf));
    // Graphemes `..head` and `tail..` are kept.
    let (mut head, mut tail) = (0, graphemes.len());
    let mut front = true;
    while head < tail {
        let w = measure(graphemes[if front { head } else { tail - 1 }].1);
        if w > budget {
            break;
        }
        budget -= w;
        if front {
            head += 1;
        } else {
            tail -= 1;
        }
        front = !front;
    }
    let at = |i: usize| graphemes.get(i).map_or(text.len(), |(byte, _)| *byte);
    (at(head), at(tail))
}

/// Where [`middle_ellipsis`] cut `text` into `out`: the end of the kept start and the start of
/// the kept end, as byte offsets into `text`. `None` when `out` is `text` whole.
fn cut_points(text: &str, out: &str) -> Option<(usize, usize)> {
    if out == text {
        return None;
    }
    let ellipsis = ELLIPSIS.len_utf8();
    out.match_indices(ELLIPSIS).find_map(|(head, _)| {
        let tail = text.len().checked_sub(out.len() - head - ellipsis)?;
        let same_head = text.get(..head) == Some(&out[..head]);
        let same_tail = text.get(tail..) == Some(&out[head + ellipsis..]);
        (tail >= head && same_head && same_tail).then_some((head, tail))
    })
}

/// Per-character advances of the card fonts, by character, font, size and window scale. A line
/// is measured as the sum of its characters, as
/// [`crate::text::metrics::TextMetricsCache::char_advance`] does for the label font, so a card
/// render shapes nothing after the first. Cleared when the `UiMetrics` style version changes.
#[derive(Default)]
struct CardAdvances {
    map: FxHashMap<(char, CardFont, u32, u32), Pixels>,
    style_version: u64,
}

impl Global for CardAdvances {}

impl CardAdvances {
    fn width(&mut self, text: &str, f: CardFont, m: &UiMetrics, window: &Window) -> Pixels {
        let size = f.size(m);
        let (size_bits, scale_bits) = (f32::from(size).to_bits(), window.scale_factor().to_bits());
        text.chars().fold(Pixels::ZERO, |sum, ch| {
            let advance = self
                .map
                .entry((ch, f, size_bits, scale_bits))
                .or_insert_with(|| {
                    let mut buf = [0u8; 4];
                    let one = ch.encode_utf8(&mut buf);
                    let run = TextRun {
                        len: one.len(),
                        font: f.font(),
                        color: black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    window
                        .text_system()
                        .layout_line(one, size, &[run], None)
                        .width
                });
            sum + *advance
        })
    }
}

/// The value in the heading row: the newest value of the graph's first series (UI-PRF-011).
#[derive(Clone, Debug, PartialEq)]
pub struct CardValue {
    /// The formatted value, or its UI-FND-026 glyph word. Restricted draws the lock icon.
    pub text: SharedString,
    pub validity: Validity,
    /// The widest texts of the value format in each unit it can step to. The heading row keeps
    /// room for the widest, so the rows do not move as the value changes.
    templates: &'static [&'static str],
}

impl CardValue {
    /// Reads the value of `chart`, or `None` when the chart has no series.
    pub fn of(chart: &ChartView, unit: NetworkUnit) -> Option<Self> {
        let (v, validity) = chart.value_now()?;
        let fmt = chart.labels.value_fmt;
        let text = match validity {
            Validity::Valid | Validity::Stale => {
                let mut out = String::new();
                format_value(fmt, v, unit, &mut out);
                SharedString::from(out)
            }
            Validity::Restricted => SharedString::default(),
            other => copy_text(other, &SharedString::default()),
        };
        Some(Self {
            text,
            validity,
            templates: value_templates(fmt, unit),
        })
    }

    /// `text_primary` when Valid, `status.warning` when Failed, else `text_secondary`, as the
    /// graph value label (UI-PRF-001).
    pub fn color(&self, theme: &TlTheme) -> Hsla {
        match self.validity {
            Validity::Valid => theme.text_primary,
            Validity::Failed => theme.status.warning,
            _ => theme.text_secondary,
        }
    }
}

// The widest text of each value format, one per unit it can step to, since which unit suffix
// is widest depends on the font. A scaled value prints at most "1023.9" (IEC, base 1024) or
// "999.9" (SI, base 1000) before it steps up a unit. Values past the largest unit, and formats
// with no unit steps (MHz, °C, W, RPM, counts), can print wider; the card then measures the live
// value too (`DeviceCard::prepare`).
const PERCENT_TEMPLATES: &[&str] = &["100.0%"];
const BYTES_TEMPLATES: &[&str] = &[
    "1023 B",
    "1023.99 KiB",
    "1023.99 MiB",
    "1023.99 GiB",
    "1023.99 TiB",
];
const RATE_IEC_TEMPLATES: &[&str] = &["1023.9 KiB/s", "1023.9 MiB/s", "1023.9 GiB/s"];
const RATE_BITS_TEMPLATES: &[&str] = &["999.9 Kbps", "999.9 Mbps", "999.9 Gbps"];
const MHZ_TEMPLATES: &[&str] = &["9999 MHz"];
const CELSIUS_TEMPLATES: &[&str] = &["100 °C"];
const WATTS_TEMPLATES: &[&str] = &["999.9 W"];
const RPM_TEMPLATES: &[&str] = &["9999 RPM"];
const COUNT_TEMPLATES: &[&str] = &["99\u{2009}999"];

/// The widest texts of `fmt` in `unit` (network rates only), as [`format_value`] prints them.
fn value_templates(fmt: ValueFormat, unit: NetworkUnit) -> &'static [&'static str] {
    match fmt {
        ValueFormat::Percent | ValueFormat::PercentOf { .. } => PERCENT_TEMPLATES,
        ValueFormat::Bytes => BYTES_TEMPLATES,
        ValueFormat::RateIec => RATE_IEC_TEMPLATES,
        ValueFormat::RateNetwork => match unit {
            NetworkUnit::Bits => RATE_BITS_TEMPLATES,
            NetworkUnit::Bytes => RATE_IEC_TEMPLATES,
        },
        ValueFormat::Mhz => MHZ_TEMPLATES,
        ValueFormat::Celsius => CELSIUS_TEMPLATES,
        ValueFormat::Watts => WATTS_TEMPLATES,
        ValueFormat::Rpm => RPM_TEMPLATES,
        ValueFormat::Count => COUNT_TEMPLATES,
    }
}

/// The boot time at which a chart of `series` turns Stale when no newer sample comes: just past
/// the newest sample of any series plus max(3 periods, 1.5 s), as `ChartView::derived_state`.
/// `None` without samples.
fn stale_deadline_ns(series: &[Option<Arc<SeriesView>>]) -> Option<u64> {
    let mut newest_ns = None;
    let mut period_ns = u64::MAX;
    for s in series.iter().flatten() {
        period_ns = period_ns.min(s.period_ns);
        if let Some(last) = s.samples.last() {
            newest_ns = Some(newest_ns.unwrap_or(0).max(last.t_ns));
        }
    }
    let wait = period_ns.saturating_mul(3).max(STALE_FLOOR_NS);
    Some(newest_ns?.saturating_add(wait).saturating_add(1))
}

/// What a card measured for one render.
#[derive(Clone, Debug, PartialEq)]
pub struct CardPrepared {
    pub layout: CardTextLayout,
    pub value: Option<CardValue>,
    pub height: Pixels,
    /// The mini graph width, zero when the card has no room for one ([`card_graph_width`]).
    pub graph_w: Pixels,
}

impl CardPrepared {
    /// The boot time at which this card's value turns Stale when no newer sample comes. `None`
    /// unless the value is Valid.
    pub fn stale_at(&self, chart: &ChartView) -> Option<u64> {
        self.value
            .as_ref()
            .filter(|v| v.validity == Validity::Valid)
            .and_then(|_| stale_deadline_ns(&chart.series))
    }
}

/// Draws its owner again at the stale deadline of a card value, so a Valid value turns Stale
/// when the samples stop. A new sample notifies through the chart, so the timer only fires when
/// samples stop. No per-frame polling.
#[derive(Default)]
pub struct StaleTimer {
    at: Option<u64>,
    task: Option<Task<()>>,
}

impl StaleTimer {
    /// The armed deadline, if any.
    pub fn at(&self) -> Option<u64> {
        self.at
    }

    /// Arms the timer for the deadline `at`, or drops it for `None`. A running timer for the
    /// same deadline is kept. `field` finds this timer in the owner when it fires.
    pub fn arm<V: 'static>(
        &mut self,
        at: Option<u64>,
        field: fn(&mut V) -> &mut StaleTimer,
        cx: &mut Context<V>,
    ) {
        if at == self.at {
            return;
        }
        self.at = at;
        self.task = at.map(|at| {
            let wait = Duration::from_nanos(at.saturating_sub(now_boot_ns()));
            cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| {
                    // Render arms the timer again if a value is still Valid.
                    field(this).at = None;
                    cx.notify();
                });
            })
        });
    }
}

/// The tooltip lines: the full text of each cut line, then the owner's tooltip (the Memory
/// commit note, a network adapter and connection). An owner tooltip that only repeats line 1
/// shows only when line 1 is cut. Empty when there is nothing to show.
pub fn card_tooltip(lines: &CardLines, layout: &CardTextLayout) -> SmallVec<[SharedString; 3]> {
    let mut out: SmallVec<[SharedString; 3]> = SmallVec::new();
    for (text, line) in [
        (&lines.heading, &layout.heading),
        (&lines.line1, &layout.line1),
        (&lines.line2, &layout.line2),
    ] {
        if line.cut {
            out.push(text.clone());
        }
    }
    if let Some(tip) = lines.tooltip.as_ref().filter(|tip| **tip != *lines.line1) {
        for part in tip.split('\n') {
            if !out.iter().any(|seen| &**seen == part) {
                out.push(if part.len() == tip.len() {
                    tip.clone()
                } else {
                    SharedString::from(part.to_owned())
                });
            }
        }
    }
    out
}

/// Persistent view for one complete sidebar card. The sidebar embeds this entity with
/// [`gpui_kit::Entity::cached`], so a clean sibling reuses its text, background and chart paint.
/// The card re-renders when its chart notifies (its own series), so its value follows the
/// graph.
pub struct CardView {
    key: DeviceKey,
    chart: Entity<ChartView>,
    lines: CardLines,
    selected: bool,
    focused: bool,
    edit_mode: bool,
    hidden_in_edit: bool,
    /// The mini window's cards show the pointer cursor on hover (UI-PRF-045).
    pointer: bool,
    width: Rc<Cell<Pixels>>,
    /// The height the sidebar lays this card out with. Render writes the measured height and
    /// notifies `sidebar` when it changed, so the cards below move on the next frame.
    height: Rc<Cell<Pixels>>,
    sidebar: EntityId,
    /// Draws the card again when its Valid value turns Stale.
    stale: StaleTimer,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    /// Whole-card paint callbacks. A cached paint replay does not increment this counter.
    #[cfg(any(test, feature = "test-hooks"))]
    pub paint_count: Rc<Cell<u32>>,
    /// What the last render measured.
    #[cfg(any(test, feature = "test-hooks"))]
    pub prepared: Option<CardPrepared>,
}

impl CardView {
    pub fn new(
        key: DeviceKey,
        chart: Entity<ChartView>,
        width: Rc<Cell<Pixels>>,
        height: Rc<Cell<Pixels>>,
        sidebar: EntityId,
    ) -> Self {
        Self {
            key,
            chart,
            lines: CardLines::default(),
            selected: false,
            focused: false,
            edit_mode: false,
            hidden_in_edit: false,
            pointer: false,
            width,
            height,
            sidebar,
            stale: StaleTimer::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            paint_count: Rc::new(Cell::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            prepared: None,
        }
    }

    /// Replaces the formatted text and invalidates only this card.
    pub fn set_lines(&mut self, lines: CardLines, cx: &mut Context<Self>) {
        if self.lines != lines {
            self.lines = lines;
            cx.notify();
        }
    }

    pub fn set_selected(&mut self, selected: bool, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }

    pub fn set_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.focused != focused {
            self.focused = focused;
            cx.notify();
        }
    }

    pub fn set_pointer(&mut self, pointer: bool, cx: &mut Context<Self>) {
        if self.pointer != pointer {
            self.pointer = pointer;
            cx.notify();
        }
    }

    /// Invalidates theme-dependent text and surfaces without notifying the sidebar.
    pub fn invalidate_style(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }
}

impl Render for CardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let mut card = DeviceCard {
            key: self.key.clone(),
            chart: self.chart.clone(),
            lines: self.lines.clone(),
            selected: self.selected,
            focused: self.focused,
            edit_mode: self.edit_mode,
            hidden_in_edit: self.hidden_in_edit,
            pointer: self.pointer,
            width: self.width.get(),
            prepared: None,
        };
        let prepared = card.prepare(window, cx);
        let height = prepared.height;
        let stale_at = prepared.stale_at(self.chart.read(cx));
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.prepared = Some(prepared.clone());
        }
        if self.height.get() != height {
            self.height.set(height);
            App::notify(cx, self.sidebar);
        }
        self.stale
            .arm(stale_at, |this: &mut Self| &mut this.stale, cx);

        #[cfg(any(test, feature = "test-hooks"))]
        {
            let paints = Rc::clone(&self.paint_count);
            div().relative().size_full().child(card).child(
                canvas(|_, _, _| {}, move |_, _, _, _| paints.set(paints.get() + 1))
                    .absolute()
                    .inset_0(),
            )
        }
        #[cfg(not(any(test, feature = "test-hooks")))]
        card
    }
}

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
    /// The pointer cursor on hover instead of the arrow (the mini window, UI-PRF-045).
    pub pointer: bool,
    pub width: Pixels,
    /// Filled by [`DeviceCard::prepare`]. Owners pass `None`.
    pub prepared: Option<CardPrepared>,
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

    /// `accent` for a connected interface's rate (R-055), else `text_secondary`.
    pub fn line2_color(&self, theme: &TlTheme) -> Hsla {
        if self.lines.line2_accent {
            theme.accent
        } else {
            theme.text_secondary
        }
    }

    /// Reads the value from the chart and lays the text out for `width`. [`CardView`] calls it
    /// before render to learn the card height; render calls it when no one did.
    pub fn prepare(&mut self, window: &Window, cx: &mut App) -> &CardPrepared {
        if self.prepared.is_none() {
            let m = *metrics(cx);
            let unit = if cx.has_global::<TlSettings>() {
                TlSettings::settings(cx).units.network
            } else {
                NetworkUnit::default()
            };
            let value = CardValue::of(self.chart.read(cx), unit);
            if !cx.has_global::<CardAdvances>() {
                cx.set_global(CardAdvances::default());
            }
            let advances = cx.global_mut::<CardAdvances>();
            if advances.style_version != m.style_version {
                advances.map.clear();
                advances.style_version = m.style_version;
            }
            let mut measure = |f: CardFont, s: &str| advances.width(s, f, &m, window);
            // The widest text of the format, or the live text when it is wider still. Rounded
            // up to a whole pixel, as layout rounds up the text box.
            let value_w = value.as_ref().map_or(Pixels::ZERO, |v| {
                let w = v
                    .templates
                    .iter()
                    .fold(measure(CardFont::Value, &v.text), |w, t| {
                        w.max(measure(CardFont::Value, t))
                    });
                px(f32::from(w).ceil())
            });
            let graph_w = card_graph_width(self.width, value_w, &m);
            let layout = card_text_layout(
                &self.lines,
                value_w,
                card_text_width(self.width, graph_w, &m),
                m.sp[3],
                m.icon.xs,
                &mut measure,
            );
            let height = layout.card_height(m.ui_scale);
            self.prepared = Some(CardPrepared {
                layout,
                value,
                height,
                graph_w,
            });
        }
        self.prepared.as_ref().expect("prepared above")
    }
}

impl RenderOnce for DeviceCard {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.prepare(window, cx);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = m.ui_scale;
        let opacity = self.opacity();
        let (selected_border, focus_ring) = (self.draws_selected_border(), self.draws_focus_ring());
        let id = ElementId::Name(SharedString::from(self.key.0.clone()));
        let line2_color = self.line2_color(&theme);
        let CardPrepared {
            layout,
            value,
            height,
            graph_w,
        } = self.prepared.take().expect("prepared above");
        let lines = self.lines;
        let key = self.key;
        let cursor = if self.pointer {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        };

        let inline = value
            .as_ref()
            .filter(|_| layout.value_inline)
            .map(|v| value_element(v, v.text.clone(), 0, &key, &m, &theme));
        let text_key = key.0.clone();
        let mut text = div()
            .debug_selector(move || format!("perf-card-text:{text_key}"))
            .relative()
            .flex()
            .flex_col()
            .items_start()
            .justify_center()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(line_rows(
                LineSpec {
                    key: &key,
                    name: "heading",
                    selector: "perf-card-heading",
                    height: HEADING_LINE_H * s,
                    style: label_style(&m, CardFont::Heading.size(&m), FontWeight::MEDIUM),
                    color: theme.text_primary,
                },
                &layout.heading,
                inline,
                &m,
            ));
        if let Some(v) = value.as_ref().filter(|_| !layout.value_inline) {
            // Split at the last space: the number on the first row, the unit on the second. A
            // text with no space (a glyph word, the lock) stays on the first row.
            let parts = match v.text.rsplit_once(' ').filter(|_| layout.value_split) {
                Some((number, unit)) => [
                    Some(SharedString::from(number.to_owned())),
                    Some(SharedString::from(unit.to_owned())),
                ],
                None => [Some(v.text.clone()), None],
            };
            let rows = if layout.value_split { 2 } else { 1 };
            for (i, part) in parts.into_iter().take(rows).enumerate() {
                text = text.child(
                    div()
                        .debug_selector(|| "perf-card-value-row".into())
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(px(HEADING_LINE_H * s))
                        .children(part.map(|t| value_element(v, t, i, &key, &m, &theme))),
                );
            }
        }
        text = text
            .child(line_rows(
                LineSpec {
                    key: &key,
                    name: "line1",
                    selector: "perf-card-line1",
                    height: DETAIL_LINE_H * s,
                    style: label_style(&m, CardFont::Detail.size(&m), FontWeight::NORMAL),
                    color: theme.text_secondary,
                },
                &layout.line1,
                None,
                &m,
            ))
            .child(line_rows(
                LineSpec {
                    key: &key,
                    name: "line2",
                    selector: "perf-card-line2",
                    height: DETAIL_LINE_H * s,
                    style: label_style(&m, CardFont::Detail.size(&m), FontWeight::NORMAL),
                    color: line2_color,
                },
                &layout.line2,
                None,
                &m,
            ));
        let mut tip = card_tooltip(&lines, &layout);
        if !tip.is_empty() {
            let state = HoverTooltip::state(
                ElementId::NamedChild(std::sync::Arc::new(id.clone()), "tooltip".into()),
                window,
                cx,
            );
            let content = if tip.len() == 1 {
                TlTooltip::text(tip.swap_remove(0))
            } else {
                TlTooltip::lines(tip.into_vec())
            };
            text = HoverTooltip::attach(&state, text, DELAY_CONTROL, content.build(), window, cx);
        }

        // A card with no room for a graph keeps the chart mounted, 0 px wide, out of the row and
        // unpainted. The chart's notify then still reaches the cached card, so the value updates.
        let mut graph = div()
            .debug_selector(|| "perf-card-graph".into())
            .flex_none()
            .w(graph_w)
            .h(px(CARD_GRAPH_H * s))
            .child(self.chart.cached(StyleRefinement::default().size_full()));
        if graph_w == Pixels::ZERO {
            graph = graph.absolute().invisible();
        }

        let pressed = theme.row_selected.opacity(PRESSED_ALPHA);
        let hover = theme.row_hover;
        let mut card = h_flex()
            .id(id)
            .debug_selector(|| "perf-card".into())
            .relative()
            .flex_none()
            .w(self.width)
            .h(height)
            .pl(m.sp[5])
            .gap(m.sp[4])
            .bg(theme.list_bg)
            .border(px(s))
            .border_color(theme.divider)
            .rounded(m.radius_card)
            .cursor(cursor)
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
        let edge = px(s);
        if selected_border {
            card = card.child(
                // Absolute insets count from inside the card border; this covers it.
                div()
                    .debug_selector(|| "perf-card-selected".into())
                    .absolute()
                    .inset(-edge)
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
                    .inset(-(edge + out))
                    .border(px(s))
                    .border_color(theme.accent.opacity(FOCUS_RING_ALPHA))
                    .rounded(m.radius_card + out),
            );
        }
        card
    }
}

/// How one card line is drawn.
struct LineSpec<'a> {
    key: &'a DeviceKey,
    /// The line in row selectors: "perf-card-row:{key}:{name}:{row}".
    name: &'static str,
    /// The selector of the whole line.
    selector: &'static str,
    /// Row height, scaled.
    height: f32,
    style: gpui_kit::TextStyleRefinement,
    color: Hsla,
}

/// The rows of one card line. Each row is one unwrapped run as wide as its text, with a 12 px
/// lock icon at each lock offset. `trailing` (the value) goes sp3 after the last row.
fn line_rows(
    spec: LineSpec,
    line: &CardLineLayout,
    mut trailing: Option<AnyElement>,
    m: &UiMetrics,
) -> AnyElement {
    let LineSpec {
        key,
        name,
        selector,
        height,
        style,
        color,
    } = spec;
    let mut col = div()
        .debug_selector(move || selector.into())
        .flex()
        .flex_col()
        .flex_none()
        .items_start();
    // The style first: it replaces the whole text refinement, colour included.
    *col.text_style() = style;
    col = col.text_color(color);
    let last = line.rows.len().saturating_sub(1);
    for (i, row) in line.rows.iter().enumerate() {
        let row_key = key.0.clone();
        let mut el = h_flex()
            .debug_selector(move || format!("perf-card-row:{row_key}:{name}:{i}"))
            .flex_none()
            .h(px(height))
            .whitespace_nowrap();
        let text = &row.text;
        let mut start = 0usize;
        for &at in &row.locks {
            let at = usize::from(at).min(text.len());
            if at > start {
                el = el.child(SharedString::from(text[start..at].to_owned()));
            }
            el = el.child(
                div()
                    .debug_selector(|| "perf-card-lock".into())
                    .flex_none()
                    .child(icon(TlIcon::Lock, m.icon.xs, color)),
            );
            start = at;
        }
        if row.locks.is_empty() {
            el = el.child(text.clone());
        } else if start < text.len() {
            el = el.child(SharedString::from(text[start..].to_owned()));
        }
        if i == last
            && let Some(value) = trailing.take()
        {
            el = el.child(div().flex_none().ml(m.sp[3]).child(value));
        }
        col = col.child(el);
    }
    col.into_any_element()
}

/// Value row `row` showing `text`, all or part of `v`: tabular `t_md` 400 in its validity
/// colour, or the lock icon when Restricted.
fn value_element(
    v: &CardValue,
    text: SharedString,
    row: usize,
    key: &DeviceKey,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let value_key = key.0.clone();
    let mut el = div()
        .debug_selector(move || format!("perf-card-row:{value_key}:value:{row}"))
        .flex()
        .flex_none()
        .items_center();
    if v.validity == Validity::Restricted {
        return el
            .child(icon(TlIcon::Lock, m.icon.xs, theme.text_secondary))
            .into_any_element();
    }
    // The style first: it replaces the whole text refinement.
    *el.text_style() = numeric_style(m, CardFont::Value.size(m));
    el.whitespace_nowrap()
        .text_color(v.color(theme))
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
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
            words: CardWords::en(),
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
    fn card_lines_overview() {
        let cpu = entry(0, DeviceIdentity::CpuPackage, "CPU");
        let s = snap(
            cpu_system(Metric::valid(4631), Metric::valid(52.0)),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Overview, &cpu, &s));
        assert_eq!(
            strs(&l),
            ("Overview", "CPU, memory, GPU, disk, network", "")
        );
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
        sys.memory.committed = 2_443;
        sys.memory.commit_limit = 1_000;
        let s = snap(
            sys,
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Memory, &mem, &s));
        assert_eq!(
            strs(&l),
            ("Memory", "34.68 GiB / 63.16 GiB", "Commit 244.3% of limit")
        );
        assert_eq!(
            l.tooltip.as_deref(),
            Some(
                "Memory the system has promised to programs, as a share of the commit limit. \
                 It can pass 100% because Linux allows overcommit."
            )
        );

        let s = snap(
            SystemSnapshot::default(),
            InventorySnapshot::default(),
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Memory, &mem, &s));
        assert_eq!(strs(&l), ("Memory", "…", "Commit … of limit"));
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
        SystemSnapshot {
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
        }
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
        assert_eq!(
            l.tooltip.as_deref(),
            Some("Intel I225-V\nWired connection 1")
        );
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
            mem_reserved: Metric::default(),
            enc_sessions: Metric::default(),
            enc_dec_shared: false,
            temp_c: Metric::valid(52.0),
            temp_edge_c: Metric::default(),
            temp_junction_c: Metric::default(),
            temp_mem_c: Metric::default(),
            power_w: Metric::default(),
            power_limit_w: Metric::default(),
            clock_gfx_mhz: Metric::default(),
            clock_mem_mhz: Metric::default(),
            fan_pct: Metric::default(),
            pcie_rx_bps: Metric::default(),
            pcie_tx_bps: Metric::default(),
            engines: Default::default(),
            fdinfo_partial: false,
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

        // The owner's compare-and-keep step. `SharedString` keeps up to 23 bytes inline, so only
        // longer strings own an allocation: line 2 and the tooltip here.
        let mut kept = first.clone();
        assert!(!update_card_lines(&mut kept, &i, &mut scratch));
        assert_eq!(kept, first);
        let same = |a: &SharedString, b: &SharedString| std::ptr::eq(a.as_ptr(), b.as_ptr());
        assert!(kept.line2.len() > 23 && same(&kept.line2, &first.line2));
        let tip = |l: &CardLines| l.tooltip.clone().expect("tooltip");
        assert!(tip(&kept).len() > 23 && same(&tip(&kept), &tip(&first)));

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
        assert!(!same(&kept.line2, &first.line2));
        assert!(same(&tip(&kept), &tip(&first)));

        // Without a connection the tooltip shares line 1's allocation.
        let cpu = entry(0, DeviceIdentity::CpuPackage, "CPU");
        let mut inv = InventorySnapshot::default();
        inv.processor.name = text("AMD Ryzen 7 7800X3D 8-Core Processor");
        let s = snap(
            SystemSnapshot::default(),
            inv,
            DeviceDetailsSnapshot::default(),
        );
        let l = lines(&input(DeviceKind::Cpu, &cpu, &s));
        assert!(same(&tip(&l), &l.line1));
    }

    /// The templates are what the formatter prints just below each unit step.
    #[test]
    fn value_templates_match_the_widest_formatted_values() {
        let f = |fmt: ValueFormat, v: f32, unit: NetworkUnit| {
            let mut out = String::new();
            format_value(fmt, v, unit, &mut out);
            out
        };
        let (kib, mib) = (1024.0_f32, 1024.0_f32 * 1024.0);
        for (fmt, unit, v) in [
            (ValueFormat::RateIec, NetworkUnit::Bits, 1023.9 * kib),
            (ValueFormat::RateIec, NetworkUnit::Bits, 1023.9 * mib),
            (ValueFormat::RateNetwork, NetworkUnit::Bytes, 1023.9 * kib),
            (ValueFormat::RateNetwork, NetworkUnit::Bytes, 1023.9 * mib),
            // Bits per second over 8: 999.9 Kbps and 999.9 Mbps.
            (ValueFormat::RateNetwork, NetworkUnit::Bits, 999.9e3 / 8.0),
            (ValueFormat::RateNetwork, NetworkUnit::Bits, 999.9e6 / 8.0),
            (ValueFormat::Bytes, NetworkUnit::Bits, 1023.99 * kib),
            (ValueFormat::Percent, NetworkUnit::Bits, 100.0),
        ] {
            let text = f(fmt, v, unit);
            assert!(
                value_templates(fmt, unit).contains(&text.as_str()),
                "{fmt:?} {unit:?}: {text}"
            );
        }
        // One step more and the unit steps up, to a shorter text.
        assert_eq!(
            f(ValueFormat::RateIec, 1023.96 * kib, NetworkUnit::Bits),
            "1.0 MiB/s"
        );
    }

    #[test]
    fn card_graph_width_leaves_room_for_the_widest_value() {
        let m = crate::theme::metrics::compute(&Default::default(), 0);
        let value = px(80.);
        // 252 less sp4 on each side: the full graph, 98 px of text.
        assert_eq!(card_graph_width(px(236.), value, &m), px(CARD_GRAPH_W));
        assert_eq!(card_text_width(px(236.), px(CARD_GRAPH_W), &m), px(98.));
        // nav_width 180: the graph gives way to the value.
        let graph = card_graph_width(px(164.), value, &m);
        assert_eq!(graph, px(62.));
        assert_eq!(card_text_width(px(164.), graph, &m), value);
        // Too narrow for the least graph: the text takes the card.
        assert_eq!(card_graph_width(px(120.), value, &m), Pixels::ZERO);
        assert_eq!(card_text_width(px(120.), Pixels::ZERO, &m), px(106.));
    }

    #[test]
    fn stale_deadline_matches_chart_staleness() {
        use tl_model::ids::{MetricId, SeriesId};
        use tl_model::series::Sample;
        let series = |period_ns: u64, newest: u64| {
            Some(Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(0),
                    metric: MetricId::CPU_TOTAL,
                },
                version: 1,
                period_ns,
                samples: Arc::from([Sample::valid(newest, 1.)]),
            }))
        };
        assert_eq!(stale_deadline_ns(&[None]), None);
        for (period, newest) in [(100_000_000, 7_000_000_000), (1_000_000_000, 9_000_000_000)] {
            let at = stale_deadline_ns(&[series(period, newest), None]).expect("deadline");
            assert!(!crate::chart::is_stale(newest, at - 1, period), "{period}");
            assert!(crate::chart::is_stale(newest, at, period), "{period}");
        }
        // The newest sample of any series and the shortest period count.
        let at = stale_deadline_ns(&[series(1_000_000_000, 5), series(100_000_000, 9)]);
        assert_eq!(at, Some(9 + 1_500_000_000 + 1));
    }

    /// Monospace test measure: 1 px per character in every font, the ellipsis included.
    fn mono(_: CardFont, s: &str) -> Pixels {
        px(s.chars().count() as f32)
    }

    fn card_text(heading: &str, line1: &str, line2: &str) -> CardLines {
        CardLines {
            heading: SharedString::from(heading.to_owned()),
            line1: SharedString::from(line1.to_owned()),
            line2: SharedString::from(line2.to_owned()),
            ..CardLines::default()
        }
    }

    /// `card_text_layout` with a 1 px gap and a 2 px lock.
    fn layout(lines: &CardLines, value_w: f32, text_w: f32) -> CardTextLayout {
        card_text_layout(lines, px(value_w), px(text_w), px(1.), px(2.), &mut mono)
    }

    fn rows(line: &CardLineLayout) -> Vec<&str> {
        line.rows.iter().map(|r| &*r.text).collect()
    }

    #[test]
    fn card_text_layout_fits_in_one_row() {
        let lines = card_text("GPU 0", "AMD Radeon", "52 °C");
        let l = layout(&lines, 6., 20.);
        assert_eq!(rows(&l.heading), ["GPU 0"]);
        assert_eq!(rows(&l.line1), ["AMD Radeon"]);
        assert_eq!(rows(&l.line2), ["52 °C"]);
        assert!(l.value_inline, "5 + 1 + 6 fits 20");
        assert!(!l.heading.cut && !l.line1.cut && !l.line2.cut);
        assert_eq!(l.card_height(1.0), px(80.));
        assert_eq!(l.card_height(1.5), px(120.));
        assert!(card_tooltip(&lines, &l).is_empty());
    }

    #[test]
    fn card_text_layout_wraps_to_two_rows() {
        let l = layout(&card_text("GPU 0", "NVIDIA GeForce RTX 4090", ""), 6., 16.);
        assert_eq!(rows(&l.line1), ["NVIDIA GeForce", "RTX 4090"]);
        assert!(!l.line1.cut);
        // An empty line keeps its row.
        assert_eq!(rows(&l.line2), [""]);
        // 18 + 2 × 16 + 16 + 10 = 76: still the least height.
        assert_eq!(l.card_height(1.0), px(80.));
    }

    #[test]
    fn card_text_layout_middle_cut_past_two_rows() {
        let name = "NVIDIA GeForce RTX 4090 Laptop GPU 16 GB";
        assert_eq!(name.len(), 40);
        let lines = CardLines {
            tooltip: Some(SharedString::from(name)),
            ..card_text("GPU 0", name, "52 °C")
        };
        let l = layout(&lines, 6., 16.);
        // Row 2 keeps the last 12 characters (middle_ellipsis) and as much of the start as fits.
        assert_eq!(rows(&l.line1), ["NVIDIA GeForce", "RTX…op GPU 16 GB"]);
        assert!(l.line1.cut);
        assert!(
            l.line1
                .rows
                .iter()
                .all(|r| mono(CardFont::Detail, &r.text) <= px(16.))
        );
        assert_eq!(&*card_tooltip(&lines, &l), [SharedString::from(name)]);
        // 18 + 2 × 16 + 16 + 10 = 76.
        assert_eq!(l.card_height(1.0), px(80.));
    }

    #[test]
    fn card_text_layout_cuts_a_name_without_spaces() {
        // middle_ellipsis keeps 12 characters of the end, which do not fit 10 px: the balanced
        // cut keeps both ends instead.
        let l = layout(&card_text("enp0s20f0u1u2u3", "Ethernet", ""), 0., 10.);
        assert_eq!(rows(&l.heading), ["enp0s…u2u3"]);
        assert!(l.heading.cut);
        let l = layout(&card_text("enp0s20f0u1u2u3", "Ethernet", ""), 0., 14.);
        assert_eq!(rows(&l.heading), ["e…0s20f0u1u2u3"]);
        // A path keeps its last component.
        let l = layout(
            &card_text("Disk 0", "NVMe - /run/media/oystein/Backup", ""),
            0.,
            20.,
        );
        assert_eq!(rows(&l.line1), ["NVMe -", "/run/media/o…/Backup"]);
        assert!(l.line1.cut);
    }

    #[test]
    fn card_text_layout_value_takes_its_own_row() {
        let lines = card_text("Aggregate graphics", "2 adapters", "AMD");
        let l = layout(&lines, 6., 20.);
        assert_eq!(rows(&l.heading), ["Aggregate graphics"]);
        assert!(!l.value_inline, "18 + 1 + 6 > 20");
        // 18 + 18 + 2 × 16.
        assert_eq!(l.text_height(), 68.);
        assert_eq!(l.card_height(1.0), px(80.));
        // No value: nothing to place.
        assert!(layout(&lines, 0., 20.).value_inline);

        // Seven rows grow the card: 2 × 18 + 18 + 4 × 16 + 10.
        let tall = card_text("Aggregate graphics", "aaaa bbbb cccc", "dddd eeee ffff");
        let l = layout(&tall, 6., 10.);
        assert_eq!(rows(&l.heading), ["Aggregate", "graphics"]);
        assert!(!l.value_inline, "8 + 1 + 6 > 10");
        assert_eq!(rows(&l.line1), ["aaaa bbbb", "cccc"]);
        assert_eq!(rows(&l.line2), ["dddd eeee", "ffff"]);
        assert_eq!(l.text_height(), 118.);
        assert_eq!(l.card_height(1.0), px(128.));
    }

    #[test]
    fn card_text_layout_splits_a_value_wider_than_the_column() {
        // "1023.9 KiB/s" is 12 wide; the column is 10.
        let lines = card_text("eth0", "Ethernet", "R: 0 B/s");
        let l = layout(&lines, 12., 10.);
        assert!(!l.value_inline && l.value_split);
        // 18 + 2 × 18 + 2 × 16.
        assert_eq!(l.text_height(), 86.);
        assert_eq!(l.card_height(1.0), px(96.));
        // A value that fits its own row keeps one row.
        let l = layout(&lines, 10., 10.);
        assert!(!l.value_inline && !l.value_split);
        assert_eq!(l.text_height(), 68.);
    }

    #[test]
    fn card_text_layout_keeps_locks_with_their_row() {
        // A Restricted temperature: a lock at the end of the line.
        let text = "Samsung SSD 990 PRO · ";
        let lines = CardLines {
            line2_locks: smallvec![text.len() as u16],
            ..card_text("Disk 0", "NVMe", text)
        };
        let l = layout(&lines, 0., 14.);
        assert_eq!(rows(&l.line2), ["Samsung SSD", "990 PRO · "]);
        assert!(l.line2.rows[0].locks.is_empty());
        // "·" is 2 bytes: the lock sits at byte 11, the end of row 2.
        assert_eq!(&l.line2.rows[1].locks[..], [11]);
        assert!(!l.line2.cut);

        // Cut: the lock at the end stays at the end of the cut row.
        let text = "Samsung SSD 990 PRO 2TB NVMe M.2 · ";
        let lines = CardLines {
            line2_locks: smallvec![text.len() as u16],
            ..card_text("Disk 0", "NVMe", text)
        };
        let l = layout(&lines, 0., 14.);
        let last = &l.line2.rows[1];
        assert!(l.line2.cut);
        assert_eq!(&last.locks[..], [last.text.len() as u16]);
        assert!(mono(CardFont::Detail, &last.text) + px(2.) <= px(14.));
    }
}
