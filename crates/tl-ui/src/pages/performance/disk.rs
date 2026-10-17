//! The Disk page: one page per physical disk with the Active time and Transfer rate charts
//! above the Details grid, and the page's demand (docs/ui/performance.md UI-PRF-029,
//! UI-PRF-020; PERF-047, PERF-048; ADR-008).
//!
//! The charts are cached views that re-render for their own series. The page re-renders only
//! when its header or Details rows change. The specs are built again only when a chart setting
//! or the theme changed.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::ScrollbarHandle;
use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StyleRefinement, Styled as _, Subscription, Window,
    div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{
    DiskDetails, InvField, PartitionRow, PartitionTable, SmartKind, Snapshot,
};
use tl_model::system::{DiskTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::units;
use tl_model::validity::Validity;

use super::actions::{PerfEject, PerfShowSmart};
use super::details::{Fallback, KeyValueModel};
use super::memory::flat_static_table;
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, header_text, page_words};
use super::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, sidebar_order, type_number,
    visible_rows,
};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::{Section, TlSettings};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, SCROLLBAR_W, TableSort,
    TlTreeTable, TreeTableDelegate,
};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::windows::disk_in_use::start_eject;

/// Debug selectors of the chart boxes.
pub const ACTIVE_ID: &str = "perf-disk-active";
pub const TRANSFER_ID: &str = "perf-disk-transfer";

const KEY_ACTIVE: &str = "perf-disk-heading-active";
const KEY_TRANSFER: &str = "perf-disk-heading-transfer";
const KEY_READ: &str = "perf-disk-legend-read";
const KEY_WRITE: &str = "perf-disk-legend-write";
const KEY_THROUGHPUT: &str = "perf-disk-range-throughput";
const KEY_BTN_SMART: &str = "perf-disk-btn-smart";
const KEY_BTN_EJECT: &str = "perf-disk-btn-eject";
const KEY_MENU_SMART: &str = "perf-disk-menu-smart";
const KEY_MENU_EJECT: &str = "perf-disk-menu-eject";
const KEY_TIP_EJECT: &str = "perf-disk-tip-eject";
const KEY_PART_HEADING: &str = "perf-part-heading";
const KEY_PART_HEADING_PLAIN: &str = "perf-part-heading-plain";
const KEY_PART_NONE: &str = "perf-part-none";
const KEY_PART_UNALLOCATED: &str = "perf-part-unallocated";
const KEY_PART_MORE_MOUNTS: &str = "perf-part-more-mounts";
const KEY_PART_COLS: [&str; 10] = [
    "perf-part-col-partition",
    "perf-part-col-size",
    "perf-part-col-offset",
    "perf-part-col-device",
    "perf-part-col-mount",
    "perf-part-col-label",
    "perf-part-col-fs",
    "perf-part-col-usage",
    "perf-part-col-flags",
    "perf-part-col-type",
];

/// Debug selector of the partitions table box.
pub const PARTITIONS_TABLE_ID: &str = "perf-disk-partitions-table";
/// The partitions table scrolls beyond this many rows (UI-PRF-030).
pub const PARTITIONS_MAX_ROWS: usize = 12;

/// Debug selector of the `Show SMART data` button.
pub const SMART_BUTTON_ID: &str = "perf-disk-smart";
/// Debug selector of the `Eject` button.
pub const EJECT_BUTTON_ID: &str = "perf-disk-eject";

const NS_PER_S: u64 = 1_000_000_000;

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub active: SharedString,
    pub transfer: SharedString,
    pub read: SharedString,
    pub write: SharedString,
    pub throughput: SharedString,
    pub btn_smart: SharedString,
    pub btn_eject: SharedString,
    pub menu_smart: SharedString,
    pub menu_eject: SharedString,
    pub tip_eject: SharedString,
    pub part_none: SharedString,
    pub part_unallocated: SharedString,
    /// Header labels in `PartColumn::ALL` order.
    pub part_cols: [SharedString; 10],
}

impl Words {
    /// Translates every word when the strings changed since the last call. True when it did.
    pub(super) fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        let t = |key| tr(cx, key);
        self.active = t(KEY_ACTIVE);
        self.transfer = t(KEY_TRANSFER);
        self.read = t(KEY_READ);
        self.write = t(KEY_WRITE);
        self.throughput = t(KEY_THROUGHPUT);
        self.btn_smart = t(KEY_BTN_SMART);
        self.btn_eject = t(KEY_BTN_EJECT);
        self.menu_smart = t(KEY_MENU_SMART);
        self.menu_eject = t(KEY_MENU_EJECT);
        self.tip_eject = t(KEY_TIP_EJECT);
        self.part_none = t(KEY_PART_NONE);
        self.part_unallocated = t(KEY_PART_UNALLOCATED);
        self.part_cols = KEY_PART_COLS.map(t);
        true
    }
}

/// The settings the charts draw from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
}

impl PageSettings {
    pub(super) fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
        }
    }
}

/// The versions of the sections [`PageSettings`] and the disk number read.
pub(super) fn section_versions(cx: &App) -> [u64; 3] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
    ]
}

fn series(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

pub(super) fn spec(series: SmallVec<[ChartSeries; 4]>, y: YScale, s: &PageSettings) -> ChartSpec {
    ChartSpec {
        series,
        y,
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
            vertical_at: None,
        },
        smooth: s.smooth,
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

/// Active time with fill, 0..100.
pub(super) fn active_spec(device: DeviceIdx, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let series = smallvec![ChartSeries {
        id: series(device, MetricId::DISK_ACTIVE),
        fill: Some(theme.graph.disk_fill),
        line: theme.graph.disk_line,
        ..ChartSeries::default()
    }];
    let y = YScale::Fixed {
        min: 0.0,
        max: 100.0,
    };
    spec(series, y, s)
}

/// Read with fill and write as a dashed line, on a power-of-two scale.
pub(super) fn transfer_spec(device: DeviceIdx, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.disk_line;
    let series = smallvec![
        ChartSeries {
            id: series(device, MetricId::DISK_READ),
            fill: Some(theme.graph.disk_fill),
            line,
            ..ChartSeries::default()
        },
        ChartSeries {
            id: series(device, MetricId::DISK_WRITE),
            line,
            dashed: true,
            ..ChartSeries::default()
        },
    ];
    spec(series, YScale::AutoRate { iec: true }, s)
}

pub(super) fn active_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.active.clone()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Right,
        range_word: w.active.clone(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

pub(super) fn transfer_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.transfer.clone()),
        value_fmt: ValueFormat::RateIec,
        value_align: ValueAlign::Right,
        range_word: w.throughput.clone(),
        legend: smallvec![
            (w.read.clone(), LegendStyle::Solid),
            (w.write.clone(), LegendStyle::Dashed)
        ],
        show_axis_max: true,
    }
}

/// The chart state from the disk source health: Failed with its error, else Live (the views
/// derive WarmingUp and Stale themselves).
pub(super) fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Disk);
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

// ---- Details grid (UI-PRF-030, PERF-049) ----

pub const ROW_ACTIVE: &str = "perf-disk-row-active";
pub const ROW_RESPONSE: &str = "perf-disk-row-response";
pub const ROW_READ: &str = "perf-disk-row-read";
pub const ROW_WRITE: &str = "perf-disk-row-write";
pub const ROW_TOTAL_READ: &str = "perf-disk-row-total-read";
pub const ROW_TOTAL_WRITTEN: &str = "perf-disk-row-total-written";
pub const ROW_CAPACITY: &str = "perf-disk-row-capacity";
pub const ROW_FORMATTED: &str = "perf-disk-row-formatted";
pub const ROW_USED: &str = "perf-disk-row-used";
pub const ROW_FREE: &str = "perf-disk-row-free";
pub const ROW_SYSTEM: &str = "perf-disk-row-system";
pub const ROW_SWAP: &str = "perf-disk-row-swap";
pub const ROW_HARDWARE: &str = "perf-disk-row-hardware";
pub const ROW_TYPE: &str = "perf-disk-row-type";
pub const ROW_INTERFACE: &str = "perf-disk-row-interface";
pub const ROW_WWN: &str = "perf-disk-row-wwn";
pub const ROW_SERIAL: &str = "perf-disk-row-serial";
pub const ROW_ROTATION: &str = "perf-disk-row-rotation";
pub const ROW_TEMPERATURE: &str = "perf-disk-row-temperature";

/// The 19 Details rows in grid order (UI-PRF-030).
pub const DISK_DETAIL_ROWS: [&str; 19] = [
    ROW_ACTIVE,
    ROW_RESPONSE,
    ROW_READ,
    ROW_WRITE,
    ROW_TOTAL_READ,
    ROW_TOTAL_WRITTEN,
    ROW_CAPACITY,
    ROW_FORMATTED,
    ROW_USED,
    ROW_FREE,
    ROW_SYSTEM,
    ROW_SWAP,
    ROW_HARDWARE,
    ROW_TYPE,
    ROW_INTERFACE,
    ROW_WWN,
    ROW_SERIAL,
    ROW_ROTATION,
    ROW_TEMPERATURE,
];

/// The P1 rows this page fills, in grid order.
const P1_ROWS: [&str; 5] = [ROW_ACTIVE, ROW_RESPONSE, ROW_READ, ROW_WRITE, ROW_CAPACITY];

const KEY_YES: &str = "perf-disk-yes";
const KEY_NO: &str = "perf-disk-no";
const KEY_RPM: &str = "perf-disk-rpm-value";

/// The Type words, by the kind name the engine gives (the E04-021 `DiskKind` names).
pub(super) const KIND_KEYS: [(&str, &str); 8] = [
    ("NVMe", "perf-disk-kind-nvme"),
    ("SSD", "perf-disk-kind-ssd"),
    ("HDD", "perf-disk-kind-hdd"),
    ("eMMC", "perf-disk-kind-emmc"),
    ("SD", "perf-disk-kind-sd"),
    ("USB", "perf-disk-kind-usb"),
    ("Optical", "perf-disk-kind-optical"),
    ("Virtual", "perf-disk-kind-virtual"),
];

/// The word key of an engine kind name; `None` for a name this page does not know.
pub(super) fn kind_key(kind: &str) -> Option<&'static str> {
    KIND_KEYS.iter().find(|(k, _)| *k == kind).map(|(_, v)| *v)
}

/// The Disk Details model: 19 rows in order, all but the P1 rows hidden until
/// [`update_disk_details_p2`] shows them.
pub fn disk_details_model() -> KeyValueModel {
    let mut model = KeyValueModel::new(&DISK_DETAIL_ROWS).with_texts(&[
        (KEY_YES, &[]),
        (KEY_NO, &[]),
        (KEY_RPM, &["rpm"]),
        (KIND_KEYS[0].1, &[]),
        (KIND_KEYS[1].1, &[]),
        (KIND_KEYS[2].1, &[]),
        (KIND_KEYS[3].1, &[]),
        (KIND_KEYS[4].1, &[]),
        (KIND_KEYS[5].1, &[]),
        (KIND_KEYS[6].1, &[]),
        (KIND_KEYS[7].1, &[]),
    ]);
    for label in DISK_DETAIL_ROWS {
        if !P1_ROWS.contains(&label) {
            model.hide(label);
        }
    }
    model
}

/// What the P1 rows are read from.
#[derive(Clone, Copy)]
pub struct DiskDetailInputs<'a> {
    pub now_boot_ns: u64,
    /// `observed_end` of the system component, for the Stale age.
    pub observed_end_ns: u64,
    /// The disk's record in `Snapshot.system`; `None` before its first sample.
    pub totals: Option<&'a DiskTotals>,
    /// The disk source's last error, for Failed.
    pub error: Option<&'a str>,
}

/// One row from a metric: Valid formats on a bits change, Stale keeps the value and marks it,
/// Missing (no sample yet) reads "Collecting…", the rest show their fallback word.
fn metric_row<T: Copy + Default>(
    model: &mut KeyValueModel,
    label: &'static str,
    m: Metric<T>,
    inp: &DiskDetailInputs,
    bits: impl Fn(T) -> u64,
    write: impl FnOnce(&mut String, T),
) -> bool {
    match m.validity {
        Validity::Valid => model.set_valid(label, bits(m.value), |o| write(o, m.value)),
        Validity::Stale => {
            let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
            model.set_valid(label, bits(m.value), |o| write(o, m.value))
                | model.set_stale(label, age_ns)
        }
        Validity::Missing => {
            model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None)
        }
        Validity::Unsupported => {
            model.set_fallback(label, m.validity, Fallback::NotSupported, None)
        }
        Validity::Failed => model.set_fallback(label, m.validity, Fallback::Unavailable, inp.error),
        v => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// Fills the five P1 rows from the disk's record (UI-PRF-030): Active time, Average response
/// time (two decimals), Read and Write speed (auto IEC, one decimal) and Capacity (IEC, two
/// decimals). Each value formats again only when its raw value changed. True when any row
/// changed.
pub fn update_disk_details_p1(model: &mut KeyValueModel, inp: &DiskDetailInputs) -> bool {
    let Some(t) = inp.totals else {
        let mut changed = false;
        for label in P1_ROWS {
            changed |= model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None);
        }
        return changed;
    };
    let f32_bits = |v: f32| u64::from(v.to_bits());
    let mut changed = false;
    changed |= metric_row(
        model,
        ROW_ACTIVE,
        t.active_pct,
        inp,
        f32_bits,
        units::percent,
    );
    changed |= metric_row(model, ROW_RESPONSE, t.response_ms, inp, f32_bits, |o, v| {
        units::response_ms(o, f64::from(v))
    });
    changed |= metric_row(
        model,
        ROW_READ,
        t.read_bps,
        inp,
        f64::to_bits,
        units::rate_iec_auto,
    );
    changed |= metric_row(
        model,
        ROW_WRITE,
        t.write_bps,
        inp,
        f64::to_bits,
        units::rate_iec_auto,
    );
    changed |= if t.capacity == 0 {
        model.set_fallback(
            ROW_CAPACITY,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        )
    } else {
        let n = t.capacity;
        model.set_valid(ROW_CAPACITY, n, |o| units::bytes(o, n))
    };
    changed
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

/// The Stale age of a row, from the system component's end time.
fn age_ns(inp: &DiskDetailInputs) -> u64 {
    inp.now_boot_ns.saturating_sub(inp.observed_end_ns)
}

/// The fallback for a field with no value. Unsupported reads "Not reported": the collector
/// gives it for a disk with no mounted filesystem.
fn field_fallback(
    model: &mut KeyValueModel,
    label: &'static str,
    validity: Validity,
    inp: &DiskDetailInputs,
) -> bool {
    match validity {
        Validity::Unsupported => model.set_fallback(label, validity, Fallback::NotReported, None),
        Validity::Missing => {
            model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None)
        }
        Validity::Failed => model.set_fallback(label, validity, Fallback::Unavailable, inp.error),
        v => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// A byte count row: Valid and Stale show the size, the rest their fallback.
fn bytes_row(
    model: &mut KeyValueModel,
    label: &'static str,
    f: InvField<u64>,
    inp: &DiskDetailInputs,
) -> bool {
    let mut changed = set_hidden(model, label, false);
    changed |= match f.validity {
        Validity::Valid | Validity::Stale => {
            let n = f.value;
            let mut c = model.set_valid(label, n, |o| units::bytes(o, n));
            if f.validity == Validity::Stale {
                c |= model.set_stale(label, age_ns(inp));
            }
            c
        }
        v => field_fallback(model, label, v, inp),
    };
    changed
}

/// A Yes or No row.
fn yes_no_row(
    model: &mut KeyValueModel,
    label: &'static str,
    f: InvField<bool>,
    inp: &DiskDetailInputs,
) -> bool {
    let mut changed = set_hidden(model, label, false);
    changed |= match f.validity {
        Validity::Valid | Validity::Stale => {
            let word = model.text(if f.value { KEY_YES } else { KEY_NO });
            let mut c = model.set_text(label, word.text());
            if f.validity == Validity::Stale {
                c |= model.set_stale(label, age_ns(inp));
            }
            c
        }
        v => field_fallback(model, label, v, inp),
    };
    changed
}

/// A text row from a string field: Valid and non-empty shows the text. With `hide_empty` a
/// missing value hides the row, otherwise it shows its fallback.
fn text_row(
    model: &mut KeyValueModel,
    label: &'static str,
    f: &InvField<Arc<str>>,
    hide_empty: bool,
    inp: &DiskDetailInputs,
) -> bool {
    let has = matches!(f.validity, Validity::Valid | Validity::Stale) && !f.value.is_empty();
    if hide_empty && !has {
        return set_hidden(model, label, true);
    }
    let mut changed = set_hidden(model, label, false);
    changed |= if has {
        let mut c = model.set_text(label, &f.value);
        if f.validity == Validity::Stale {
            c |= model.set_stale(label, age_ns(inp));
        }
        c
    } else {
        // A Valid but empty value reads as not reported.
        let v = if f.validity == Validity::Valid {
            Validity::Unsupported
        } else {
            f.validity
        };
        field_fallback(model, label, v, inp)
    };
    changed
}

/// Fills the 14 rows after the P1 rows (UI-PRF-030, PERF-049, PERF-053): the two totals,
/// Formatted, Used and Free space, System disk, Swap, Hardware name, Type, Interface, WWN,
/// Serial, Rotation rate and Temperature. WWN, Serial and Rotation rate are hidden when the
/// disk has none. Each value formats again only when it changed. True when any row changed.
pub fn update_disk_details_p2(
    model: &mut KeyValueModel,
    inp: &DiskDetailInputs,
    details: Option<&DiskDetails>,
) -> bool {
    let mut changed = false;
    match inp.totals {
        Some(t) => {
            changed |= set_hidden(model, ROW_TOTAL_READ, false);
            changed |= set_hidden(model, ROW_TOTAL_WRITTEN, false);
            for (label, n, rate) in [
                (ROW_TOTAL_READ, t.read_total, t.read_bps),
                (ROW_TOTAL_WRITTEN, t.written_total, t.write_bps),
            ] {
                // The totals share the diskstats read with the rates, so the rate's
                // validity decides whether the byte count can be shown. On the first
                // sample and after a counter reset the rates are WarmingUp but the
                // sampler still reads valid totals.
                changed |= match rate.validity {
                    Validity::Valid | Validity::Stale | Validity::WarmingUp => {
                        let mut c = model.set_valid(label, n, |o| units::bytes(o, n));
                        if rate.validity == Validity::Stale {
                            c |= model.set_stale(label, age_ns(inp));
                        }
                        c
                    }
                    v => field_fallback(model, label, v, inp),
                };
            }
        }
        None => {
            for label in [ROW_TOTAL_READ, ROW_TOTAL_WRITTEN] {
                changed |= set_hidden(model, label, false);
                changed |=
                    model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None);
            }
        }
    }
    let Some(d) = details else {
        for label in [
            ROW_FORMATTED,
            ROW_USED,
            ROW_FREE,
            ROW_SYSTEM,
            ROW_SWAP,
            ROW_HARDWARE,
            ROW_TYPE,
            ROW_INTERFACE,
        ] {
            changed |= set_hidden(model, label, false);
            changed |= model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None);
        }
        for label in [ROW_WWN, ROW_SERIAL, ROW_ROTATION] {
            changed |= set_hidden(model, label, true);
        }
        changed |= temperature_row(model, inp, None);
        return changed;
    };
    changed |= bytes_row(model, ROW_FORMATTED, d.formatted_bytes, inp);
    changed |= bytes_row(model, ROW_USED, d.used_bytes, inp);
    changed |= bytes_row(model, ROW_FREE, d.free_bytes, inp);
    changed |= yes_no_row(model, ROW_SYSTEM, d.system_disk, inp);
    changed |= yes_no_row(model, ROW_SWAP, d.swap, inp);
    changed |= text_row(model, ROW_HARDWARE, &d.model, false, inp);
    changed |= set_hidden(model, ROW_TYPE, false);
    changed |= match kind_key(&d.kind) {
        Some(key) => {
            let word = model.text(key);
            model.set_text(ROW_TYPE, word.text())
        }
        None => model.set_fallback(ROW_TYPE, Validity::Unsupported, Fallback::NotReported, None),
    };
    changed |= text_row(model, ROW_INTERFACE, &d.interface, false, inp);
    changed |= text_row(model, ROW_WWN, &d.wwn, true, inp);
    changed |= text_row(model, ROW_SERIAL, &d.serial, true, inp);
    match d.rotation_rpm {
        Some(rpm) => {
            changed |= set_hidden(model, ROW_ROTATION, false);
            let template = model.text(KEY_RPM);
            changed |= model.set_valid(ROW_ROTATION, u64::from(rpm), |o| {
                template.write_each(o, |_, o| {
                    let _ = write!(o, "{rpm}");
                });
            });
        }
        None => changed |= set_hidden(model, ROW_ROTATION, true),
    }
    changed |= temperature_row(model, inp, Some(d));
    changed
}

/// The Temperature row: the newest disk temperature, or the SMART one when the drive has no
/// hwmon sensor (PERF-053). "Not reported" with neither.
fn temperature_row(
    model: &mut KeyValueModel,
    inp: &DiskDetailInputs,
    details: Option<&DiskDetails>,
) -> bool {
    let mut changed = set_hidden(model, ROW_TEMPERATURE, false);
    let Some(t) = inp.totals else {
        return changed
            | model.set_fallback(
                ROW_TEMPERATURE,
                Validity::WarmingUp,
                Fallback::Unavailable,
                None,
            );
    };
    let mut temp = t.temp_c;
    if temp.validity == Validity::Unsupported
        && let Some(c) = details.and_then(|d| d.smart_temp_c)
    {
        temp = Metric::valid(c);
    }
    changed |= if temp.validity == Validity::Unsupported {
        model.set_fallback(
            ROW_TEMPERATURE,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        )
    } else {
        metric_row(
            model,
            ROW_TEMPERATURE,
            temp,
            inp,
            |v| u64::from(v.to_bits()),
            units::celsius,
        )
    };
    changed
}

/// The record of `device` in the snapshot.
fn disk_totals(snap: &Snapshot, device: DeviceIdx) -> Option<&DiskTotals> {
    snap.system
        .disks
        .iter()
        .find(|(i, _)| *i == device)
        .map(|(_, t)| t)
}

/// "Disk N": the index of `key` among the visible disks in sidebar order, counted as the
/// sidebar counts it (PERF-008). Runs on catalog and settings changes only.
fn disk_number(key: &DeviceKey, snap: &Snapshot, cx: &App) -> u16 {
    let p = &TlSettings::settings(cx).performance;
    let k = |s: &str| DeviceKey(Arc::from(s));
    let order: Vec<DeviceKey> = p.device_order.iter().map(|s| k(s)).collect();
    let overrides: HashMap<_, _> = p.device_overrides.iter().map(|(s, v)| (k(s), *v)).collect();
    let view = PerformanceSettingsView {
        order: &order,
        overrides: &overrides,
        show_disks: p.show_disks,
        show_gpus: p.show_gpus,
        show_fans: p.show_fans,
        show_disconnected_fans: p.show_disconnected_fans,
        show_batteries: p.show_batteries,
        show_network_wired: p.show_network_wired,
        show_network_wireless: p.show_network_wireless,
        show_network_vpn: p.show_network_vpn,
        show_network_virtual: p.show_network_virtual,
        show_network_other: p.show_network_other,
    };
    let mut rows = Vec::new();
    sidebar_order(&snap.devices, &snap.system.sensors, &view, &mut rows);
    let visible: Vec<DeviceRow> = visible_rows(&rows, &view).cloned().collect();
    type_number(&visible, key)
}

/// The partitions block (PERF-050): the heading and the rows the table shows.
#[derive(Clone, Debug)]
pub struct PartitionsBlock {
    pub heading: SharedString,
    pub rows: Arc<[PartitionRow]>,
}

/// The heading: `3 partitions - GPT`, `1 partition - MBR`, `3 partitions` for a table of kind
/// `none`, `No partitions` without rows.
fn partitions_heading(table: &PartitionTable, words: &Words, cx: &App) -> SharedString {
    if table.rows.is_empty() {
        return words.part_none.clone();
    }
    let n = FluentValue::from(table.rows.iter().filter(|r| !r.unallocated).count() as i64);
    if &*table.kind == "none" {
        return tr_args(cx, KEY_PART_HEADING_PLAIN, &[("n", n)]);
    }
    tr_args(
        cx,
        KEY_PART_HEADING,
        &[("n", n), ("table", FluentValue::from(&*table.kind))],
    )
}

/// The columns of the partitions table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PartColumn {
    Partition,
    Size,
    Offset,
    Device,
    Mount,
    Label,
    FileSystem,
    Usage,
    Flags,
    Type,
}

impl PartColumn {
    pub const ALL: [Self; 10] = [
        Self::Partition,
        Self::Size,
        Self::Offset,
        Self::Device,
        Self::Mount,
        Self::Label,
        Self::FileSystem,
        Self::Usage,
        Self::Flags,
        Self::Type,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    /// Mount path and Type share the rest of the width.
    fn flex(self) -> bool {
        matches!(self, Self::Mount | Self::Type)
    }

    /// Logical px at 100%. A flex column's width is its width before the first fit.
    pub fn width(self) -> f32 {
        match self {
            Self::Partition | Self::Label => 120.,
            Self::Size | Self::FileSystem => 90.,
            Self::Offset | Self::Device | Self::Flags => 110.,
            Self::Usage => 140.,
            Self::Mount | Self::Type => 150.,
        }
    }

    fn kind(self) -> ColumnKind {
        match self {
            Self::Size | Self::Offset | Self::Usage => ColumnKind::Numeric,
            Self::Partition => ColumnKind::Path,
            _ => ColumnKind::Text,
        }
    }
}

impl TableColumn for PartColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Partition => "part-partition",
            Self::Size => "part-size",
            Self::Offset => "part-offset",
            Self::Device => "part-device",
            Self::Mount => "part-mount",
            Self::Label => "part-label",
            Self::FileSystem => "part-fs",
            Self::Usage => "part-usage",
            Self::Flags => "part-flags",
            Self::Type => "part-type",
        }
    }
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }
    fn pinned_first(self) -> bool {
        false
    }
    fn min_width(self) -> Option<f32> {
        self.flex().then_some(100.)
    }
}

/// The table has no sort; this state only names the first column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PartSort;

impl TableSort<PartColumn> for PartSort {
    fn column(&self) -> PartColumn {
        PartColumn::Partition
    }
    fn ascending(&self) -> bool {
        true
    }
}

fn partition_specs(words: &Words) -> Vec<ColumnSpec<PartColumn>> {
    PartColumn::ALL
        .iter()
        .map(|&id| {
            let label = words.part_cols[id.index()].clone();
            ColumnSpec {
                id,
                label: label.clone(),
                full_name: label.clone(),
                group: label,
                align: match id.kind() {
                    ColumnKind::Numeric => ColumnAlign::Right,
                    _ => ColumnAlign::Left,
                },
                kind: id.kind(),
                sortable: false,
                template: None,
                default_width: px(id.width()),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

/// The flat, static table of partitions (E25-017).
pub struct PartitionsDelegate {
    rows: Arc<[PartitionRow]>,
    /// `Additional mount points: ...` per row, built with the strings.
    tips: Vec<Option<SharedString>>,
    version: u64,
    specs: Vec<ColumnSpec<PartColumn>>,
    layout: ColumnLayout<PartColumn>,
    unallocated: SharedString,
    /// `text_secondary`, for the Unallocated rows.
    dim: Hsla,
}

impl PartitionsDelegate {
    fn new(words: &Words, dim: Hsla, ui_scale: f32) -> Self {
        let specs = partition_specs(words);
        let mut layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &PartColumn::ALL,
            &PartColumn::ALL,
        );
        layout.rebuild(ui_scale);
        Self {
            rows: Arc::from([]),
            tips: Vec::new(),
            version: 0,
            specs,
            layout,
            unallocated: words.part_unallocated.clone(),
            dim,
        }
    }

    /// Takes new rows and their tooltips. False, with the version unchanged, when both equal
    /// the current ones.
    fn set_rows(&mut self, rows: Arc<[PartitionRow]>, tips: Vec<Option<SharedString>>) -> bool {
        if (Arc::ptr_eq(&self.rows, &rows) || self.rows == rows) && self.tips == tips {
            return false;
        }
        self.rows = rows;
        self.tips = tips;
        self.version += 1;
        true
    }

    fn set_words(&mut self, words: &Words) {
        self.specs = partition_specs(words);
        self.unallocated = words.part_unallocated.clone();
        self.version += 1;
    }

    fn set_dim(&mut self, dim: Hsla) -> bool {
        if self.dim == dim {
            return false;
        }
        self.dim = dim;
        self.version += 1;
        true
    }

    /// Re-lays the columns at `ui_scale`. False when the scale is unchanged.
    fn set_scale(&mut self, ui_scale: f32) -> bool {
        if (self.layout.ui_scale() - ui_scale).abs() < f32::EPSILON {
            return false;
        }
        self.layout.rebuild(ui_scale);
        self.version += 1;
        true
    }

    /// Splits the width the fixed columns leave in a body `body_px` wide (logical px at the
    /// layout's scale, scrollbar excluded) between the two flex columns, never below their
    /// minimum. True when a width changed.
    fn fit_flex_columns(&mut self, body_px: f32) -> bool {
        let scale = self.layout.ui_scale().max(0.1);
        let fixed: f32 = PartColumn::ALL
            .iter()
            .filter(|c| !c.flex())
            .map(|c| self.layout.widths.get(c).map_or(0.0, |w| w.as_f32()))
            .sum();
        let share = ((body_px / scale - SCROLLBAR_W - fixed) / 2.)
            .max(100.)
            .floor();
        let mut changed = false;
        for col in PartColumn::ALL.into_iter().filter(|c| c.flex()) {
            let current = self.layout.widths.get(&col).map_or(0.0, |w| w.as_f32());
            if (share - current).abs() >= 0.5 {
                self.layout.widths.insert(col, px(share));
                changed = true;
            }
        }
        if changed {
            self.layout.rebuild(scale);
            self.version += 1;
        }
        changed
    }

    /// The table scrolls, so it is a Tab stop (UI-PRC-016).
    pub fn scrolls(&self) -> bool {
        self.rows.len() > PARTITIONS_MAX_ROWS
    }
}

/// The text of one partitions-table cell, shared by the table and the copy text.
fn partition_cell_text(row: &PartitionRow, col: PartColumn, unallocated: &str, out: &mut String) {
    let usage = match row.usage_pct.validity {
        Validity::Valid | Validity::Stale => Some(row.usage_pct.value),
        _ => None,
    };
    let dash = |out: &mut String, text: Option<&str>| out.push_str(text.unwrap_or("-"));
    match col {
        PartColumn::Partition => out.push_str(if row.unallocated { "-" } else { &row.name }),
        PartColumn::Size => units::bytes(out, row.size_bytes),
        PartColumn::Offset => units::bytes(out, row.start_bytes),
        PartColumn::Type if row.unallocated => out.push_str(unallocated),
        PartColumn::Type => out.push_str(&row.type_name),
        _ if row.unallocated => out.push('-'),
        PartColumn::Device => dash(out, row.holder.as_deref()),
        PartColumn::Mount => dash(out, row.mount.as_deref()),
        PartColumn::Label => dash(out, row.label.as_deref()),
        PartColumn::FileSystem => dash(out, row.fs_type.as_deref()),
        PartColumn::Usage => match usage {
            Some(pct) => units::percent(out, pct),
            None => out.push('-'),
        },
        PartColumn::Flags => {
            if row.flags.is_empty() {
                out.push('-');
            }
            for (i, flag) in row.flags.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(flag);
            }
        }
    }
}

impl TreeTableDelegate for PartitionsDelegate {
    type RowId = u32;
    type Column = PartColumn;
    type Sort = PartSort;

    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_id(&self, ix: usize) -> u32 {
        ix as u32
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        ((id as usize) < self.rows.len()).then_some(id as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, ix: usize) -> RowFlags {
        if self.rows.get(ix).is_some_and(|r| r.unallocated) {
            RowFlags {
                bits: RowFlags::SECONDARY_NAME,
                ..RowFlags::default()
            }
        } else {
            RowFlags::default()
        }
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<PartColumn> {
        &self.layout
    }
    fn column_spec(&self, id: PartColumn) -> &ColumnSpec<PartColumn> {
        &self.specs[id.index()]
    }
    fn cell(&self, ix: usize, col: PartColumn, out: &mut CellOut) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        out.validity = Validity::Valid;
        if row.unallocated {
            out.text_color = Some(self.dim);
            out.selected_text_color = Some(self.dim);
        }
        let usage = match row.usage_pct.validity {
            Validity::Valid | Validity::Stale => Some(row.usage_pct.value),
            _ => None,
        };
        if col == PartColumn::Usage && !row.unallocated {
            out.bar = usage.map(|pct| pct / 100.);
            out.bits = u64::from(usage.unwrap_or(f32::NAN).to_bits());
        }
        if col == PartColumn::Mount && out.want_tooltip && !row.extra_mounts.is_empty() {
            out.tooltip = self.tips.get(ix).cloned().flatten();
        }
        if !out.want_text {
            return;
        }
        partition_cell_text(row, col, &self.unallocated, &mut out.text);
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        let primary = self.rows.get(ix).map_or_else(SharedString::default, |r| {
            if r.unallocated {
                SharedString::from("-")
            } else {
                SharedString::from(r.name.clone())
            }
        });
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary,
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn name_column(&self) -> Option<PartColumn> {
        Some(PartColumn::Partition)
    }
    fn header_total(&self, _: PartColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> PartSort {
        PartSort
    }
    fn selectable(&self, _: usize) -> bool {
        false
    }
    fn context_menu(
        &mut self,
        _: &[u32],
        _: Option<PartColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }
    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_sort(&mut self, _: PartColumn, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<PartColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }
    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }
    fn empty_text(&self) -> Option<SharedString> {
        None
    }
    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }
    fn columns_fixed(&self) -> bool {
        true
    }
    fn tab_stop(&self) -> bool {
        self.scrolls()
    }
}

/// The Disk device page.
pub struct DiskPage {
    key: DeviceKey,
    row: DeviceRow,
    active: Entity<ChartView>,
    transfer: Entity<ChartView>,
    /// The rows from `update_disk_details_p1` and `update_disk_details_p2`.
    details: KeyValueModel,
    /// The partitions block; `None` until the table is read.
    partitions: Option<PartitionsBlock>,
    /// The table behind `partitions`, for the heading when the strings change.
    partition_table: Option<Arc<PartitionTable>>,
    partitions_table: Option<Entity<TlTreeTable<PartitionsDelegate>>>,
    /// The table body width of the last frame, for the flex columns.
    partitions_body: f32,
    /// The `Show SMART data` button and menu item are present: UDisks2 reads SMART, or the
    /// smartctl fallback applies (the engine sets `NeedsFallback` only when smartctl exists).
    smart_available: bool,
    /// The `Eject` button and menu item are present (ejectable or removable drive).
    ejectable: bool,
    smart_focus: FocusHandle,
    eject_focus: FocusHandle,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    is_active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    /// The page root.
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl DiskPage {
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
        let mut words = Words::default();
        words.refresh(cx);

        let idx = row.idx;
        let active = {
            let (spec, labels) = (active_spec(idx, &settings, &theme), active_labels(&words));
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };
        let transfer = {
            let (spec, labels) = (
                transfer_spec(idx, &settings, &theme),
                transfer_labels(&words),
            );
            cx.new(|cx| {
                let mut view = ChartView::new(spec, labels, &hub, cx);
                // The value is read plus write.
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
            active,
            transfer,
            details: disk_details_model(),
            partitions: None,
            partition_table: None,
            partitions_table: None,
            partitions_body: 0.,
            smart_available: false,
            ejectable: false,
            smart_focus: cx.focus_handle(),
            eject_focus: cx.focus_handle(),
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            hub,
            demand,
            id,
            is_active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn active_chart(&self) -> &Entity<ChartView> {
        &self.active
    }

    pub fn transfer_chart(&self) -> &Entity<ChartView> {
        &self.transfer
    }

    pub fn smart_available(&self) -> bool {
        self.smart_available
    }

    pub fn ejectable(&self) -> bool {
        self.ejectable
    }

    pub fn smart_focus(&self) -> &FocusHandle {
        &self.smart_focus
    }

    pub fn eject_focus(&self) -> &FocusHandle {
        &self.eject_focus
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// The partitions block, once the table is read.
    pub fn partitions(&self) -> Option<&PartitionsBlock> {
        self.partitions.as_ref()
    }

    /// The partitions table, once the page has rendered with rows.
    pub fn partitions_table(&self) -> Option<&Entity<TlTreeTable<PartitionsDelegate>>> {
        self.partitions_table.as_ref()
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// This page's demand part while it is active: disks only.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            disks: true,
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.is_active {
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

    /// Brings the header, the button flags and the chart state up to date with the components
    /// in `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        let details = mask.contains(ComponentMask::DEVICE_DETAILS);
        let d = snap
            .device_details
            .disks
            .iter()
            .find(|(i, _)| *i == self.row.idx)
            .map(|(_, d)| d);
        if details {
            let smart = d.is_some_and(|d| {
                matches!(
                    d.smart,
                    SmartKind::Ata | SmartKind::Nvme | SmartKind::NeedsFallback
                )
            });
            let eject = d.is_some_and(|d| d.ejectable || d.removable);
            if (smart, eject) != (self.smart_available, self.ejectable) {
                (self.smart_available, self.ejectable) = (smart, eject);
                changed = true;
            }
        }
        if mask.contains(ComponentMask::HEALTH) {
            let state = health_state(snap);
            for chart in [&self.active, &self.transfer] {
                let state = state.clone();
                chart.update(cx, |v, cx| v.set_state(state, cx));
            }
        }
        if mask.intersects(
            ComponentMask::SYSTEM | ComponentMask::HEALTH | ComponentMask::DEVICE_DETAILS,
        ) {
            let inputs = DiskDetailInputs {
                now_boot_ns: now_boot_ns(),
                observed_end_ns: snap.system.header.observed_end,
                totals: disk_totals(snap, self.row.idx),
                error: snap.health.get(SourceId::Disk).last_error.as_deref(),
            };
            changed |= update_disk_details_p1(&mut self.details, &inputs);
            changed |= update_disk_details_p2(&mut self.details, &inputs, d);
        }
        if details {
            let table = d.and_then(|d| d.partitions.clone());
            let same = match (&self.partition_table, &table) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
                (None, None) => true,
                _ => false,
            };
            if !same {
                self.partition_table = table;
                self.sync_partitions(cx);
                changed = true;
            }
        }
        // The model comes from the disk details or the inventory.
        if details || mask.intersects(ComponentMask::DEVICES.union(ComponentMask::INVENTORY)) {
            changed |= self.sync_header(snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// Rebuilds the block and the table rows from `partition_table`.
    fn sync_partitions(&mut self, cx: &mut Context<Self>) {
        self.partitions = self.partition_table.as_deref().map(|t| PartitionsBlock {
            heading: partitions_heading(t, &self.words, cx),
            rows: Arc::clone(&t.rows),
        });
        let Some(table) = &self.partitions_table else {
            return;
        };
        let (rows, tips) = self.rows_and_tips(cx);
        table.update(cx, |t, cx| {
            if t.delegate_mut().set_rows(rows, tips) {
                t.on_new_version(cx);
            }
        });
    }

    /// The rows of the table and the tooltip of each row's Mount path.
    fn rows_and_tips(&self, cx: &App) -> (Arc<[PartitionRow]>, Vec<Option<SharedString>>) {
        let rows = self
            .partitions
            .as_ref()
            .map_or_else(|| Arc::from([]), |p| Arc::clone(&p.rows));
        let tips = rows
            .iter()
            .map(|r| {
                (!r.extra_mounts.is_empty()).then(|| {
                    let paths = r.extra_mounts.join(", ");
                    tr_args(
                        cx,
                        KEY_PART_MORE_MOUNTS,
                        &[("paths", FluentValue::from(paths))],
                    )
                })
            })
            .collect();
        (rows, tips)
    }

    /// Creates the partitions table on the first render with rows, and keeps its scale, colour
    /// and flex columns current.
    fn sync_partitions_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.partitions.as_ref().is_none_or(|p| p.rows.is_empty()) {
            return;
        }
        let (scale, dim) = (metrics(cx).ui_scale, current(cx).text_secondary);
        let table = match &self.partitions_table {
            Some(table) => table.clone(),
            None => {
                let (rows, tips) = self.rows_and_tips(cx);
                let mut delegate = PartitionsDelegate::new(&self.words, dim, scale);
                delegate.set_rows(rows, tips);
                let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
                self.partitions_table = Some(table.clone());
                table
            }
        };
        let rescaled = table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            let changed = d.set_scale(scale) | d.set_dim(dim);
            if changed {
                t.on_new_version(cx);
            }
            changed
        });
        if rescaled {
            self.partitions_body = 0.;
        }
        let body = ScrollbarHandle::viewport_bounds(&table.read(cx).scroll_handle())
            .size
            .width
            .as_f32();
        if body > 0. && body != self.partitions_body {
            self.partitions_body = body;
            table.update(cx, |t, cx| {
                if t.delegate_mut().fit_flex_columns(body) {
                    t.on_new_version(cx);
                }
            });
        }
    }

    /// The header from the catalog entry and the disk model. True when it changed.
    fn sync_header(&mut self, snap: &Snapshot, cx: &mut App) -> bool {
        let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx) else {
            return false;
        };
        let number = disk_number(&self.key, snap, cx);
        let header = header_text(page_words(cx), DeviceKind::Disk, number, entry, snap);
        if header == self.header {
            return false;
        }
        self.header = header;
        true
    }

    /// Applies the chart settings and the disk number when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        let performance_moved = versions[0] != self.settings_versions[0];
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new != self.settings {
            self.settings = new;
            self.rebuild_specs(cx);
        }
        // `device_order` and the visibility settings move the number in "Disk N".
        if performance_moved {
            let snap = Arc::clone(self.hub.read(cx).current());
            if self.sync_header(&snap, cx) {
                cx.notify();
            }
        }
    }

    fn rebuild_specs(&self, cx: &mut App) {
        let (idx, s) = (self.row.idx, self.settings);
        let theme = current(cx).clone();
        let active = active_spec(idx, &s, &theme);
        let transfer = transfer_spec(idx, &s, &theme);
        self.active.update(cx, |v, cx| v.set_spec(active, cx));
        self.transfer.update(cx, |v, cx| v.set_spec(transfer, cx));
    }

    /// A new theme: the charts take the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_specs(cx);
        cx.notify();
    }

    /// New strings: the chart labels and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        let (active, transfer) = (active_labels(&self.words), transfer_labels(&self.words));
        self.active.update(cx, |v, cx| v.set_labels(active, cx));
        self.transfer.update(cx, |v, cx| v.set_labels(transfer, cx));
        self.sync_partitions(cx);
        if let Some(table) = &self.partitions_table {
            let words = self.words.clone();
            table.update(cx, |t, cx| {
                t.delegate_mut().set_words(&words);
                t.on_new_version(cx);
            });
        }
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_header(&snap, cx);
        cx.notify();
    }

    /// Sends the eject of this disk and follows it (PERF-052).
    fn on_eject(&mut self, a: &PerfEject, _: &mut Window, cx: &mut Context<Self>) {
        if a.key != self.key {
            cx.propagate();
            return;
        }
        let (key, hub, label) = (self.key.clone(), self.hub.clone(), self.header.0.clone());
        start_eject(key, self.row.idx, hub, label, cx);
    }

    /// `chart` in a full-width box of `height`.
    fn chart_box(id: &'static str, chart: &Entity<ChartView>, height: f32) -> gpui_kit::Div {
        div()
            .debug_selector(move || id.into())
            .flex_none()
            .w_full()
            .h(px(height))
            .child(chart.clone().cached(StyleRefinement::default().size_full()))
    }
}

impl DevicePage for DiskPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        let mut out = SmallVec::new();
        // Dispatched on the page, so it works wherever focus is.
        if self.smart_available {
            let (page_focus, key) = (self.focus.clone(), self.key.clone());
            out.push(
                TlButton::new(SMART_BUTTON_ID, TlButtonKind::Secondary)
                    .size(TlButtonSize::Sm)
                    .label(self.words.btn_smart.clone())
                    .tooltip(self.words.btn_smart.clone())
                    .focus_handle(self.smart_focus.clone())
                    .on_click(move |_, window, cx| {
                        page_focus.dispatch_action(&PerfShowSmart { key: key.clone() }, window, cx);
                    })
                    .into_any_element(),
            );
        }
        if self.ejectable {
            let (page_focus, key) = (self.focus.clone(), self.key.clone());
            out.push(
                TlButton::new(EJECT_BUTTON_ID, TlButtonKind::Secondary)
                    .size(TlButtonSize::Sm)
                    .icon(TlIcon::Eject)
                    .label(self.words.btn_eject.clone())
                    .tooltip(self.words.tip_eject.clone())
                    .focus_handle(self.eject_focus.clone())
                    .on_click(move |_, window, cx| {
                        page_focus.dispatch_action(&PerfEject { key: key.clone() }, window, cx);
                    })
                    .into_any_element(),
            );
        }
        out
    }

    fn graphs(&self, _: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let h = GRAPH_H_STACKED * m.ui_scale;
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(Self::chart_box(ACTIVE_ID, &self.active, h))
            .child(Self::chart_box(TRANSFER_ID, &self.transfer, h))
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 3]> {
        let mut blocks = SmallVec::new();
        let Some(block) = &self.partitions else {
            return blocks;
        };
        let (m, theme) = (metrics(cx), current(cx));
        let mut heading = div()
            .text_color(theme.text_primary)
            .child(block.heading.clone());
        *heading.text_style() = label_style(m, m.text.md, FontWeight::MEDIUM);
        let mut el = v_flex().gap(m.sp[4]).child(heading);
        if let Some(table) = self
            .partitions_table
            .as_ref()
            .filter(|_| !block.rows.is_empty())
        {
            el = el.child(
                flat_static_table(table, block.rows.len(), PARTITIONS_MAX_ROWS, m)
                    .debug_selector(|| PARTITIONS_TABLE_ID.into()),
            );
        }
        blocks.push(el.into_any_element());
        blocks
    }

    fn menu_items(&self, mut menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        if self.smart_available {
            let key = self.key.clone();
            menu = menu.item(
                self.words.menu_smart.clone(),
                Box::new(PerfShowSmart { key }),
            );
        }
        if self.ejectable {
            let key = self.key.clone();
            menu = menu.item(self.words.menu_eject.clone(), Box::new(PerfEject { key }));
        }
        menu
    }

    /// The Disk copy text (PERF-011, UI-PRF-022): the header lines, the Details rows, then the
    /// Partitions block with one paragraph per table row. The SMART window is not part of it.
    fn copy_text(&self, out: &mut String) {
        let (word, model) = &self.header;
        out.push_str(word);
        out.push('\n');
        if !model.is_empty() {
            out.push_str(model);
            out.push('\n');
        }
        let rows_from = out.len();
        self.details.copy_into(out);
        if out.len() > rows_from {
            out.push('\n');
        }
        let Some(block) = &self.partitions else {
            return;
        };
        out.push('\n');
        out.push_str(&block.heading);
        out.push('\n');
        for (i, row) in block.rows.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            for col in PartColumn::ALL {
                out.push_str("    ");
                out.push_str(&self.words.part_cols[col.index()]);
                out.push_str(": ");
                partition_cell_text(row, col, &self.words.part_unallocated, out);
                out.push('\n');
            }
        }
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        let mut out = SmallVec::new();
        if self.smart_available {
            out.push(self.smart_focus.clone());
        }
        if self.ejectable {
            out.push(self.eject_focus.clone());
        }
        out
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.is_active == active {
            return;
        }
        self.is_active = active;
        if active {
            self.send_demand(cx);
        } else {
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }
}

impl Focusable for DiskPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DiskPage {
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
        self.sync_partitions_table(window, cx);
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_eject))
            .child(
                DevicePageFrame {
                    page: self,
                    m: &m,
                    theme: &theme,
                }
                .render(window, cx),
            )
    }
}
