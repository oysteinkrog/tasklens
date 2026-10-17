//! The Memory page: the Physical memory, Commit and Swap charts stacked above the composition
//! bar and the Details grid, the two page-menu check items and the page's demand
//! (docs/ui/performance.md UI-PRF-026, UI-PRF-020; PERF-018, PERF-026, PERF-041; ADR-008).
//!
//! The charts are cached views that re-render for their own series. The page re-renders only
//! when a chart appears or goes, or its header or Details rows change. The specs are built again
//! only when a total, a chart setting or the theme changed.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::base::ScrollbarHandle;
use gpui_kit::component::v_flex;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyView, AnyWindowHandle, App, AppContext as _,
    AsyncApp, Context, Div, ElementId, Entity, EntityId, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, WeakEntity, Window, div, px, radians,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{
    InvField, InventorySnapshot, MemoryDetails, MemoryModule, Snapshot, SwapDevice, SwapKindModel,
};
use tl_model::system::{MemoryTotals, ZramTotals};
use tl_model::time::now_boot_ns;
use tl_model::units;
use tl_model::validity::Validity;

use super::actions::{PerfToggleMemoryComposition, PerfToggleMemorySwapGraph};
use super::details::{Fallback, KeyValueModel, render_details};
use super::memory_bar::{Composition, CompositionBarView};
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, page_words};
use super::{DeviceKey, DeviceRow};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::TlMenu;
use crate::components::motion::{M_BASE, M_FAST, ease_out_cubic};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings, animations_on};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HEADER_H_ONE, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, SCROLLBAR_W,
    TableSort, TlTreeTable, TreeTableDelegate,
};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Debug selectors of the chart boxes.
pub const PHYSICAL_ID: &str = "perf-mem-physical";
pub const COMMIT_ID: &str = "perf-mem-commit";
pub const SWAP_ID: &str = "perf-mem-swap";

/// Element ids and debug selectors of the grids in the "Compressed memory" block. Each zram
/// grid adds its index. Every grid sits in a parent with its own id, because `render_details`
/// gives all grids the same id and GPUI keys element state by the whole id path: without the
/// parent, the grids would share focus, hover and text selection with the Details grid.
pub const ZRAM_GRID_ID: &str = "perf-mem-zram-grid";
pub const ZSWAP_GRID_ID: &str = "perf-mem-zswap-grid";

const KEY_PHYSICAL: &str = "perf-mem-heading-physical";
const KEY_COMMIT: &str = "perf-mem-heading-commit";
const KEY_SWAP: &str = "perf-mem-heading-swap";
const KEY_USED: &str = "perf-mem-legend-used";
const KEY_COMMITTED: &str = "perf-mem-legend-committed";
const KEY_COMMIT_TIP: &str = "perf-mem-commit-tooltip";
const KEY_MENU_COMPOSITION: &str = "perf-mem-menu-composition";
const KEY_MENU_SWAP: &str = "perf-mem-menu-swap";
const KEY_RANGE_MEMORY: &str = "perf-mem-range-memory";
const KEY_RANGE_SWAP: &str = "perf-mem-range-swap";
const KEY_BLOCK_COMPRESSED: &str = "perf-mem-block-compressed";
const KEY_ZSWAP: &str = "perf-mem-zswap";
const KEY_BLOCK_SWAP: &str = "perf-mem-block-swap";
const KEY_SWAP_COLS: [&str; 5] = [
    "perf-swap-col-name",
    "perf-swap-col-type",
    "perf-swap-col-size",
    "perf-swap-col-used",
    "perf-swap-col-priority",
];
const KEY_SWAP_TYPES: [&str; 3] = [
    "perf-swap-type-partition",
    "perf-swap-type-file",
    "perf-swap-type-zram",
];

/// Debug selector of the "Swap devices" table box.
pub const SWAP_TABLE_ID: &str = "perf-mem-swap-table";
/// The table shows at most this many rows; more rows scroll.
pub const SWAP_TABLE_MAX_ROWS: usize = 8;

const NS_PER_S: u64 = 1_000_000_000;
/// Alpha of the dirty-bytes line against `mem_line`.
const DIRTY_ALPHA: f32 = 0.6;

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub physical: SharedString,
    pub commit: SharedString,
    pub swap: SharedString,
    pub used: SharedString,
    pub committed: SharedString,
    pub commit_tip: SharedString,
    pub menu_composition: SharedString,
    pub menu_swap: SharedString,
    pub range_memory: SharedString,
    pub range_swap: SharedString,
    pub block_compressed: SharedString,
    pub zswap: SharedString,
    pub block_swap: SharedString,
    pub block_modules: SharedString,
    pub swap_table: SwapWords,
}

/// The words of the "Swap devices" table.
#[derive(Clone, Debug, Default)]
pub struct SwapWords {
    /// Header labels in `SwapColumn::ALL` order.
    pub columns: [SharedString; 5],
    /// Partition, file and zram.
    pub types: [SharedString; 3],
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
        self.physical = t(KEY_PHYSICAL);
        self.commit = t(KEY_COMMIT);
        self.swap = t(KEY_SWAP);
        self.used = t(KEY_USED);
        self.committed = t(KEY_COMMITTED);
        self.commit_tip = t(KEY_COMMIT_TIP);
        self.menu_composition = t(KEY_MENU_COMPOSITION);
        self.menu_swap = t(KEY_MENU_SWAP);
        self.range_memory = t(KEY_RANGE_MEMORY);
        self.range_swap = t(KEY_RANGE_SWAP);
        self.block_compressed = t(KEY_BLOCK_COMPRESSED);
        self.zswap = t(KEY_ZSWAP);
        self.block_swap = t(KEY_BLOCK_SWAP);
        self.block_modules = t(KEY_BLOCK_MODULES);
        self.swap_table = SwapWords {
            columns: KEY_SWAP_COLS.map(t),
            types: KEY_SWAP_TYPES.map(t),
        };
        true
    }
}

/// The settings the page draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    composition: bool,
    swap: bool,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
            composition: s.performance.memory_composition,
            swap: s.performance.memory_swap_graph,
        }
    }
}

/// The versions of the sections [`PageSettings`] reads.
fn section_versions(cx: &App) -> [u64; 3] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
    ]
}

/// `MemTotal`, `CommitLimit` and `SwapTotal` in bytes, from `Snapshot.system`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Totals {
    mem: u64,
    commit_limit: u64,
    swap: u64,
}

impl Totals {
    fn of(snap: &Snapshot) -> Self {
        let m = &snap.system.memory;
        Self {
            mem: m.total,
            commit_limit: m.commit_limit,
            swap: m.swap_total,
        }
    }
}

fn series(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

fn spec(series: SmallVec<[ChartSeries; 4]>, y: YScale, s: &PageSettings) -> ChartSpec {
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

/// Used with fill, committed as a dashed line and dirty bytes as a faint line, 0..MemTotal.
fn physical_spec(device: DeviceIdx, total: u64, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.mem_line;
    let series = smallvec![
        ChartSeries {
            id: series(device, MetricId::MEM_USED),
            fill: Some(theme.graph.mem_fill),
            line,
            ..ChartSeries::default()
        },
        ChartSeries {
            id: series(device, MetricId::MEM_COMMIT),
            line,
            dashed: true,
            ..ChartSeries::default()
        },
        ChartSeries {
            id: series(device, MetricId::MEM_DIRTY),
            line: Hsla {
                a: line.a * DIRTY_ALPHA,
                ..line
            },
            line_width: px(1.),
            ..ChartSeries::default()
        },
    ];
    spec(series, YScale::FixedTotal(total as f32), s)
}

/// Committed bytes with fill, 0..max(CommitLimit, peak).
fn commit_spec(device: DeviceIdx, limit: u64, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let series = smallvec![ChartSeries {
        id: series(device, MetricId::MEM_COMMIT),
        fill: Some(theme.graph.mem_fill),
        line: theme.graph.mem_line,
        ..ChartSeries::default()
    }];
    spec(series, YScale::Commit(limit as f32), s)
}

/// Swap used with fill, 0..SwapTotal.
fn swap_spec(device: DeviceIdx, total: u64, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let series = smallvec![ChartSeries {
        id: series(device, MetricId::SWAP_USED),
        fill: Some(theme.graph.mem_fill),
        line: theme.graph.mem_line,
        ..ChartSeries::default()
    }];
    spec(series, YScale::FixedTotal(total as f32), s)
}

fn physical_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.physical.clone()),
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Right,
        range_word: w.range_memory.clone(),
        // The dirty line has no legend entry.
        legend: smallvec![
            (w.used.clone(), LegendStyle::Solid),
            (w.committed.clone(), LegendStyle::Dashed)
        ],
        show_axis_max: false,
    }
}

/// The value is `Committed_AS / CommitLimit`; the axis text shows when the peak passes 100%.
fn commit_labels(w: &Words, limit: u64) -> ChartLabels {
    ChartLabels {
        heading: Some(w.commit.clone()),
        value_fmt: ValueFormat::PercentOf { total: limit },
        value_align: ValueAlign::Right,
        range_word: w.commit.clone(),
        legend: SmallVec::new(),
        show_axis_max: true,
    }
}

fn swap_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.swap.clone()),
        value_fmt: ValueFormat::Bytes,
        value_align: ValueAlign::Right,
        range_word: w.range_swap.clone(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The chart state from the memory source health: Failed with its error, else Live (the views
/// derive WarmingUp and Stale themselves).
fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Memory);
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

// ---- Details grid (UI-PRF-028, PERF-043) ----

pub const ROW_IN_USE: &str = "perf-mem-row-in-use";
pub const ROW_AVAILABLE: &str = "perf-mem-row-available";
pub const ROW_COMMITTED: &str = "perf-mem-row-committed";
pub const ROW_CACHED: &str = "perf-mem-row-cached";
pub const ROW_SLAB_RECLAIMABLE: &str = "perf-mem-row-slab-reclaimable";
pub const ROW_SLAB_UNRECLAIMABLE: &str = "perf-mem-row-slab-unreclaimable";
/// Rows 7 to 11 are E25-011's (memory modules); hidden until it fills them.
pub const ROW_SPEED: &str = "perf-mem-row-speed";
pub const ROW_SLOTS: &str = "perf-mem-row-slots";
pub const ROW_FORM_FACTOR: &str = "perf-mem-row-form-factor";
pub const ROW_TYPE: &str = "perf-mem-row-type";
pub const ROW_RESERVED: &str = "perf-mem-row-hw-reserved";
pub const ROW_SWAP_USED: &str = "perf-mem-row-swap-used";
pub const ROW_SWAP_AVAILABLE: &str = "perf-mem-row-swap-available";
pub const ROW_COMPRESSED: &str = "perf-mem-row-compressed";
pub const ROW_SAVINGS: &str = "perf-mem-row-savings";
pub const ROW_HUGE_PAGES: &str = "perf-mem-row-huge-pages";

/// The 16 Details rows in grid order (UI-PRF-028).
pub const MEMORY_DETAIL_ROWS: [&str; 16] = [
    ROW_IN_USE,
    ROW_AVAILABLE,
    ROW_COMMITTED,
    ROW_CACHED,
    ROW_SLAB_RECLAIMABLE,
    ROW_SLAB_UNRECLAIMABLE,
    ROW_SPEED,
    ROW_SLOTS,
    ROW_FORM_FACTOR,
    ROW_TYPE,
    ROW_RESERVED,
    ROW_SWAP_USED,
    ROW_SWAP_AVAILABLE,
    ROW_COMPRESSED,
    ROW_SAVINGS,
    ROW_HUGE_PAGES,
];

/// The rows this page fills, in grid order.
const LIVE_ROWS: [&str; 11] = [
    ROW_IN_USE,
    ROW_AVAILABLE,
    ROW_COMMITTED,
    ROW_CACHED,
    ROW_SLAB_RECLAIMABLE,
    ROW_SLAB_UNRECLAIMABLE,
    ROW_SWAP_USED,
    ROW_SWAP_AVAILABLE,
    ROW_COMPRESSED,
    ROW_SAVINGS,
    ROW_HUGE_PAGES,
];

/// "{$count} × {$size}".
const KEY_HUGE_PAGES_VALUE: &str = "perf-mem-huge-pages-value";

/// "{$mts} MT/s".
const KEY_SPEED_VALUE: &str = "perf-mem-speed-value";
/// "{$used} of {$total}".
const KEY_SLOTS_VALUE: &str = "perf-mem-slots-value";
/// "Sum of module sizes minus MemTotal".
const KEY_RESERVED_TIP: &str = "perf-mem-hw-reserved-tooltip";

/// Compressed and Savings show from this many compressed bytes.
pub const COMPRESSED_MIN: u64 = 1 << 20;

/// The Memory Details model: 16 rows in order, rows 7 to 11 hidden until the first
/// [`update_memory_details_modules`].
pub fn memory_details_model() -> KeyValueModel {
    let mut model = KeyValueModel::new(&MEMORY_DETAIL_ROWS).with_texts(&[
        (KEY_HUGE_PAGES_VALUE, &["count", "size"]),
        (KEY_SPEED_VALUE, &["mts"]),
        (KEY_SLOTS_VALUE, &["used", "total"]),
        (KEY_RESERVED_TIP, &[]),
    ]);
    for label in [
        ROW_SPEED,
        ROW_SLOTS,
        ROW_FORM_FACTOR,
        ROW_TYPE,
        ROW_RESERVED,
    ] {
        model.hide(label);
    }
    model
}

/// What the live rows are read from.
#[derive(Clone, Copy)]
pub struct MemoryDetailInputs<'a> {
    pub mem: &'a MemoryTotals,
    pub now_boot_ns: u64,
    /// `observed_end` of the system component, for the Stale age.
    pub observed_end_ns: u64,
    /// The memory source's last error, for Failed.
    pub error: Option<&'a str>,
}

/// zram `mm_stat` totals when zram is present, else zeros.
fn zram_of(m: &MemoryTotals) -> ZramTotals {
    match m.zram.validity {
        Validity::Valid | Validity::Stale => m.zram.value,
        _ => ZramTotals::default(),
    }
}

/// Compressed bytes (zram `mem_used_total` + Zswap) and the bytes saved
/// ((zram `orig_data_size` + Zswapped) − (zram `compr_data_size` + Zswap)), PERF-043.
pub fn compression(m: &MemoryTotals) -> (u64, u64) {
    let z = zram_of(m);
    let compressed = z.mem_used.saturating_add(m.zswap);
    let savings = z
        .orig_data
        .saturating_add(m.zswapped)
        .saturating_sub(z.compr_data.saturating_add(m.zswap));
    (compressed, savings)
}

/// A huge page size in whole units: "2 MiB", "1 GiB".
fn page_size(out: &mut String, n: u64) {
    const GIB: u64 = 1 << 30;
    if n >= GIB && n.is_multiple_of(GIB) {
        let _ = write!(out, "{} GiB", n / GIB);
    } else {
        units::mib_whole(out, n);
    }
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

/// Sets `label`'s tooltip to a fixed text unless it already has it.
fn fixed_tooltip(model: &mut KeyValueModel, label: &'static str, text: &'static str) {
    let has = model
        .rows
        .iter()
        .find(|r| r.label == label)
        .is_some_and(|r| r.tooltip.as_deref() == Some(text));
    if !has {
        model.set_tooltip(label, Some(SharedString::new_static(text)));
    }
}

/// Fills the 11 live rows from `/proc/meminfo` and the zram totals (PERF-043), and applies
/// the hidden-when-missing rules: Compressed and Savings from 1 MiB compressed, Huge pages
/// when there are any. Each value formats again only when its raw bytes changed. True when
/// any row changed.
pub fn update_memory_details(model: &mut KeyValueModel, inp: &MemoryDetailInputs) -> bool {
    let m = inp.mem;
    let mut changed = false;
    match m.validity {
        Validity::Valid | Validity::Stale => {}
        v => {
            let (v, error) = match v {
                Validity::Failed => (v, inp.error),
                Validity::WarmingUp => (v, None),
                _ => (Validity::Unsupported, None),
            };
            for label in LIVE_ROWS {
                changed |= model.set_fallback(label, v, Fallback::Unavailable, error);
            }
            return changed;
        }
    }
    let stale = m.validity == Validity::Stale;
    let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
    let bytes = |model: &mut KeyValueModel, label: &'static str, n: u64| {
        let c = model.set_valid(label, n, |o| units::bytes(o, n));
        c | (stale && model.set_stale(label, age_ns))
    };

    let in_use = if m.available > m.total {
        m.total.saturating_sub(m.free)
    } else {
        m.total - m.available
    };
    changed |= bytes(model, ROW_IN_USE, in_use);
    changed |= bytes(model, ROW_AVAILABLE, m.available);
    // Both halves in the bits, so either change formats again.
    let (used, limit) = (m.committed, m.commit_limit);
    let bits = used.rotate_left(32) ^ limit;
    changed |= model.set_valid(ROW_COMMITTED, bits, |o| units::bytes_of(o, used, limit));
    changed |= stale && model.set_stale(ROW_COMMITTED, age_ns);
    changed |= bytes(model, ROW_CACHED, m.cached);
    changed |= bytes(model, ROW_SLAB_RECLAIMABLE, m.slab_reclaimable);
    fixed_tooltip(model, ROW_SLAB_RECLAIMABLE, "SReclaimable");
    changed |= bytes(model, ROW_SLAB_UNRECLAIMABLE, m.slab_unreclaimable);
    fixed_tooltip(model, ROW_SLAB_UNRECLAIMABLE, "SUnreclaim");
    changed |= bytes(
        model,
        ROW_SWAP_USED,
        m.swap_total.saturating_sub(m.swap_free),
    );
    changed |= bytes(model, ROW_SWAP_AVAILABLE, m.swap_free);

    let (compressed, savings) = compression(m);
    let show = compressed >= COMPRESSED_MIN;
    changed |= set_hidden(model, ROW_COMPRESSED, !show);
    changed |= set_hidden(model, ROW_SAVINGS, !show);
    if show {
        changed |= bytes(model, ROW_COMPRESSED, compressed);
        changed |= bytes(model, ROW_SAVINGS, savings);
    }

    let (count, size) = (m.huge_pages_total, m.huge_page_size);
    let show = count > 0;
    changed |= set_hidden(model, ROW_HUGE_PAGES, !show);
    if show {
        let template = model.text(KEY_HUGE_PAGES_VALUE);
        let bits = count.rotate_left(32) ^ size;
        changed |= model.set_valid(ROW_HUGE_PAGES, bits, |o| {
            template.write_each(o, |i, o| match i {
                0 => units::count(o, count),
                _ => page_size(o, size),
            });
        });
        changed |= stale && model.set_stale(ROW_HUGE_PAGES, age_ns);
    }
    changed
}

/// Fills rows 7 to 11 from the SMBIOS module inventory (PERF-043, PERF-046). Speed is the
/// configured speed of the first populated module. Rows 7 to 10 read Not reported when the
/// inventory has no module keys. Hardware reserved is the sum of the module sizes minus
/// `mem_total`, clamped at 0, and hidden without modules or a total. True when any row changed.
pub fn update_memory_details_modules(
    model: &mut KeyValueModel,
    inv: &InventorySnapshot,
    mem_total: u64,
) -> bool {
    let shown = |v: Validity| matches!(v, Validity::Valid | Validity::Stale);
    let rows = [ROW_SPEED, ROW_SLOTS, ROW_FORM_FACTOR, ROW_TYPE];
    let mut changed = false;
    for label in rows {
        changed |= set_hidden(model, label, false);
    }
    let fallback = match inv.header.validity {
        v @ (Validity::WarmingUp | Validity::Failed) => Some(v),
        _ if inv.modules.is_empty() => Some(Validity::Unsupported),
        _ => None,
    };
    if let Some(v) = fallback {
        for label in rows {
            changed |= model.set_fallback(label, v, Fallback::NotReported, None);
        }
        changed |= set_hidden(model, ROW_RESERVED, true);
        return changed;
    }
    let first = &inv.modules[0];

    let speed = &first.configured_speed_mts;
    if shown(speed.validity) && speed.value > 0 {
        let mts = speed.value;
        let template = model.text(KEY_SPEED_VALUE);
        changed |= model.set_valid(ROW_SPEED, u64::from(mts), |o| {
            template.write_each(o, |_, o| {
                let _ = write!(o, "{mts}");
            });
        });
    } else {
        changed |= model.set_fallback(
            ROW_SPEED,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        );
    }

    let slots = &inv.slots_total;
    if shown(slots.validity) {
        let (used, total) = (inv.modules.len() as u64, u64::from(slots.value));
        let template = model.text(KEY_SLOTS_VALUE);
        changed |= model.set_valid(ROW_SLOTS, used << 32 | total, |o| {
            template.write_each(o, |i, o| match i {
                0 => {
                    let _ = write!(o, "{used}");
                }
                _ => {
                    let _ = write!(o, "{total}");
                }
            });
        });
    } else {
        changed |= model.set_fallback(
            ROW_SLOTS,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        );
    }

    for (label, field) in [
        (ROW_FORM_FACTOR, &first.form_factor),
        (ROW_TYPE, &first.kind),
    ] {
        if shown(field.validity) {
            changed |= model.set_text(label, &field.value);
        } else {
            changed |=
                model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None);
        }
    }

    let hide = mem_total == 0;
    changed |= set_hidden(model, ROW_RESERVED, hide);
    if !hide {
        let sum = inv
            .modules
            .iter()
            .filter(|m| shown(m.capacity_bytes.validity))
            .fold(0u64, |a, m| a.saturating_add(m.capacity_bytes.value));
        let reserved = sum.saturating_sub(mem_total);
        changed |= model.set_valid(ROW_RESERVED, reserved, |o| units::bytes(o, reserved));
        let tip = model.text(KEY_RESERVED_TIP).text().clone();
        let has = model
            .rows
            .iter()
            .find(|r| r.label == ROW_RESERVED)
            .is_some_and(|r| r.tooltip.as_ref() == Some(&tip));
        if !has {
            model.set_tooltip(ROW_RESERVED, Some(tip));
            changed = true;
        }
    }
    changed
}

// ---- "Compressed memory" block (UI-PRF-028 block 1, PERF-044) ----

pub const ROW_ZRAM_DEVICE: &str = "perf-mem-zram-device";
pub const ROW_ZRAM_SIZE: &str = "perf-mem-zram-size";
pub const ROW_ZRAM_STORED: &str = "perf-mem-zram-stored";
pub const ROW_ZRAM_COMPRESSED: &str = "perf-mem-zram-compressed";
pub const ROW_ZRAM_MEM_USED: &str = "perf-mem-zram-mem-used";
pub const ROW_ZRAM_RATIO: &str = "perf-mem-zram-ratio";
pub const ROW_ZRAM_ALGORITHM: &str = "perf-mem-zram-algorithm";
pub const ROW_ZSWAP_ENABLED: &str = "perf-mem-zswap-enabled";
pub const ROW_ZSWAP_POOL: &str = "perf-mem-zswap-pool";
/// The zswap grid's Stored row has the zram Stored label.
pub const ROW_ZSWAP_STORED: &str = ROW_ZRAM_STORED;

/// "{$ratio} : 1".
const KEY_RATIO_VALUE: &str = "perf-mem-ratio-value";
const KEY_YES: &str = "perf-mem-yes";
const KEY_NO: &str = "perf-mem-no";

const ZRAM_ROWS: [&str; 7] = [
    ROW_ZRAM_DEVICE,
    ROW_ZRAM_SIZE,
    ROW_ZRAM_STORED,
    ROW_ZRAM_COMPRESSED,
    ROW_ZRAM_MEM_USED,
    ROW_ZRAM_RATIO,
    ROW_ZRAM_ALGORITHM,
];

/// The Details grids of the "Compressed memory" block: one per zram device, then zswap.
/// `names` holds the sub-heading of each zram grid (`zram0`).
pub struct CompressedBlock {
    pub zram: Vec<KeyValueModel>,
    pub zswap: KeyValueModel,
    pub names: Vec<SharedString>,
    /// The zswap grid is enabled; with no zram device it decides whether the block shows.
    zswap_enabled: bool,
}

impl Default for CompressedBlock {
    fn default() -> Self {
        Self::new()
    }
}

fn zram_model() -> KeyValueModel {
    KeyValueModel::new(&ZRAM_ROWS).with_texts(&[(KEY_RATIO_VALUE, &["ratio"])])
}

fn zswap_model() -> KeyValueModel {
    let mut model = KeyValueModel::new(&[ROW_ZSWAP_ENABLED, ROW_ZSWAP_POOL, ROW_ZSWAP_STORED])
        .with_texts(&[(KEY_YES, &[]), (KEY_NO, &[])]);
    model.hide(ROW_ZSWAP_POOL);
    model.hide(ROW_ZSWAP_STORED);
    model
}

impl CompressedBlock {
    /// An empty block, which is hidden.
    pub fn new() -> Self {
        Self {
            zram: Vec::new(),
            zswap: zswap_model(),
            names: Vec::new(),
            zswap_enabled: false,
        }
    }

    /// The block shows when there is a zram device or zswap is enabled.
    pub fn is_hidden(&self) -> bool {
        self.zram.is_empty() && !self.zswap_enabled
    }

    /// Translates the words of every grid when the strings changed. True when they did.
    pub fn refresh_strings(&mut self, cx: &App) -> bool {
        let mut changed = self.zswap.refresh_strings(cx);
        for model in &mut self.zram {
            changed |= model.refresh_strings(cx);
        }
        changed
    }
}

/// What the block is read from.
#[derive(Clone, Copy)]
pub struct CompressedInputs<'a> {
    pub mem: &'a MemoryTotals,
    pub details: &'a MemoryDetails,
}

/// FNV-1a over the bytes of a name, the bits of a text row.
fn text_bits(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Fills one zram grid from the device's own `mm_stat` fields. When its `mm_stat` read failed
/// (`stats` is `None`), those rows read Unavailable.
fn update_zram_grid(
    model: &mut KeyValueModel,
    dev: &tl_model::snapshot::ZramDetail,
    stats: Option<ZramTotals>,
) -> bool {
    let mut changed = false;
    changed |= model.set_valid(ROW_ZRAM_DEVICE, text_bits(&dev.name), |o| {
        o.push_str("/dev/");
        o.push_str(&dev.name);
    });
    let size = dev.disksize;
    changed |= model.set_valid(ROW_ZRAM_SIZE, size, |o| units::bytes(o, size));
    match dev.algorithm.as_deref() {
        Some(a) => {
            changed |= model.set_valid(ROW_ZRAM_ALGORITHM, text_bits(a), |o| o.push_str(a));
        }
        None => {
            changed |= model.set_fallback(
                ROW_ZRAM_ALGORITHM,
                Validity::Unsupported,
                Fallback::NotReported,
                None,
            );
        }
    }
    let Some(z) = stats else {
        for label in [
            ROW_ZRAM_STORED,
            ROW_ZRAM_COMPRESSED,
            ROW_ZRAM_MEM_USED,
            ROW_ZRAM_RATIO,
        ] {
            changed |= model.set_fallback(label, Validity::Failed, Fallback::Unavailable, None);
        }
        return changed;
    };
    changed |= model.set_valid(ROW_ZRAM_STORED, z.orig_data, |o| {
        units::bytes(o, z.orig_data)
    });
    changed |= model.set_valid(ROW_ZRAM_COMPRESSED, z.compr_data, |o| {
        units::bytes(o, z.compr_data)
    });
    changed |= model.set_valid(ROW_ZRAM_MEM_USED, z.mem_used, |o| {
        units::bytes(o, z.mem_used)
    });
    if z.compr_data == 0 {
        changed |= model.set_text(ROW_ZRAM_RATIO, "-");
    } else {
        let template = model.text(KEY_RATIO_VALUE);
        let ratio = z.orig_data as f64 / z.compr_data as f64;
        let bits = ratio.to_bits();
        changed |= model.set_valid(ROW_ZRAM_RATIO, bits, |o| {
            template.write_each(o, |_, o| {
                let _ = write!(o, "{ratio:.1}");
            });
        });
    }
    changed
}

/// Brings the block up to date with the zram devices, `mm_stat` totals and zswap parameters
/// (PERF-044). A grid formats a value again only when its raw bytes changed. True when
/// anything shown changed, or a grid was added or removed.
pub fn update_compressed_block(block: &mut CompressedBlock, inp: &CompressedInputs) -> bool {
    let mut changed = false;
    let devices = &inp.details.zram;
    let same_names = block.names.len() == devices.len()
        && block
            .names
            .iter()
            .zip(devices.iter())
            .all(|(n, d)| n == &*d.name);
    if !same_names {
        block.names = devices
            .iter()
            .map(|d| SharedString::from(d.name.to_string()))
            .collect();
        block.zram = devices.iter().map(|_| zram_model()).collect();
        changed = true;
    }
    // A lone device without its own stats falls back to the summed totals, which are its own.
    let lone = match inp.mem.zram.validity {
        Validity::Valid | Validity::Stale if devices.len() == 1 => Some(inp.mem.zram.value),
        _ => None,
    };
    for (model, dev) in block.zram.iter_mut().zip(devices.iter()) {
        changed |= update_zram_grid(model, dev, dev.stats.or(lone));
    }

    let z = &mut block.zswap;
    let zswap = &inp.details.zswap;
    let info = match zswap.validity {
        Validity::Valid | Validity::Stale => zswap.value.as_ref(),
        _ => None,
    };
    let enabled = info.is_some_and(|i| i.enabled);
    changed |= enabled != block.zswap_enabled;
    block.zswap_enabled = enabled;
    if zswap.validity == Validity::Failed {
        changed |= z.set_fallback(
            ROW_ZSWAP_ENABLED,
            Validity::Failed,
            Fallback::Unavailable,
            None,
        );
    } else {
        let key = if enabled { KEY_YES } else { KEY_NO };
        let word = z.text(key).text().clone();
        changed |= z.set_text(ROW_ZSWAP_ENABLED, &word);
    }
    changed |= set_hidden(z, ROW_ZSWAP_POOL, !enabled);
    changed |= set_hidden(z, ROW_ZSWAP_STORED, !enabled);
    if enabled {
        let m = inp.mem;
        match info.and_then(|i| i.max_pool_percent) {
            Some(pct) => {
                let pool = (u128::from(m.total) * u128::from(pct) / 100) as u64;
                changed |= z.set_valid(ROW_ZSWAP_POOL, pool, |o| units::bytes(o, pool));
            }
            None => {
                changed |= z.set_fallback(
                    ROW_ZSWAP_POOL,
                    Validity::Unsupported,
                    Fallback::NotReported,
                    None,
                );
            }
        }
        let stored = m.zswapped;
        changed |= z.set_valid(ROW_ZSWAP_STORED, stored, |o| units::bytes(o, stored));
    }
    changed
}

// ---- "Memory modules" block (UI-PRF-028 block 3, PERF-046) ----

pub const ROW_MOD_SLOT: &str = "perf-mem-mod-slot";
pub const ROW_MOD_CAPACITY: &str = "perf-mem-mod-capacity";
pub const ROW_MOD_SPEED: &str = "perf-mem-mod-speed";
pub const ROW_MOD_CONFIGURED: &str = "perf-mem-mod-configured";
pub const ROW_MOD_TYPE: &str = "perf-mem-mod-type";
pub const ROW_MOD_FORM_FACTOR: &str = "perf-mem-mod-form-factor";
pub const ROW_MOD_MANUFACTURER: &str = "perf-mem-mod-manufacturer";
pub const ROW_MOD_PART: &str = "perf-mem-mod-part";
pub const ROW_MOD_RANK: &str = "perf-mem-mod-rank";
pub const ROW_MOD_VOLTAGE: &str = "perf-mem-mod-voltage";
pub const ROW_MOD_DETAILS: &str = "perf-mem-mod-details";

/// Debug selector of the chevron button of module `n`, with `-{n}` appended.
pub const MODULE_CHEVRON_ID: &str = "perf-mem-module-chevron";
/// Element id and debug selector of the grid of module `n`, with `-{n}` appended in the
/// selector. A parent id of its own, for the reason given at [`ZRAM_GRID_ID`].
pub const MODULE_GRID_ID: &str = "perf-mem-module-grid";
/// Debug selector of the "Module details: Not reported" grid.
pub const MODULES_NOT_REPORTED_ID: &str = "perf-mem-modules-not-reported";

const KEY_BLOCK_MODULES: &str = "perf-mem-block-modules";
/// "Slots used: {$used} of {$total}".
const KEY_SLOTS_LINE: &str = "perf-mem-slots-line";
/// "Slots used: {$used}".
const KEY_SLOTS_LINE_USED: &str = "perf-mem-slots-line-used";
/// "Memory module {$n}".
const KEY_MODULE_TITLE: &str = "perf-mem-module-title";
/// "{$v} V".
const KEY_VOLTAGE_VALUE: &str = "perf-mem-voltage-value";

/// Blocks start collapsed when there are more modules than this.
pub const MODULES_EXPANDED_MAX: usize = 4;

const MODULE_ROWS: [&str; 10] = [
    ROW_MOD_SLOT,
    ROW_MOD_CAPACITY,
    ROW_MOD_SPEED,
    ROW_MOD_CONFIGURED,
    ROW_MOD_TYPE,
    ROW_MOD_FORM_FACTOR,
    ROW_MOD_MANUFACTURER,
    ROW_MOD_PART,
    ROW_MOD_RANK,
    ROW_MOD_VOLTAGE,
];

/// The chevron's turn animation after a toggle.
#[derive(Clone, Copy, Debug)]
struct ChevronAnim {
    module: usize,
    /// True while the module opens, false while it closes.
    opening: bool,
    /// Bumped on each toggle, so a new toggle restarts the animation.
    generation: u32,
}

/// One collapsible grid per populated DIMM under the swap table. No serial is read or shown
/// (D-007).
pub struct ModulesBlock {
    /// "Slots used: 2 of 4", or "Slots used: 2" without the slot total. Empty when not shown.
    pub slots_line: SharedString,
    /// "Memory module N" with its grid, in SMBIOS order.
    pub modules: Vec<(SharedString, KeyValueModel)>,
    pub expanded: Vec<bool>,
    /// The inventory has no module keys: one grid row "Module details: Not reported" shows
    /// instead of the list.
    pub not_reported: bool,
    /// The inventory is still warming up: the block does not show.
    hidden: bool,
    /// Holds the "Module details" row and the line and title texts.
    words: KeyValueModel,
    used: usize,
    slots_total: Option<u32>,
    anim: Option<ChevronAnim>,
}

impl Default for ModulesBlock {
    fn default() -> Self {
        Self::new()
    }
}

fn module_model() -> KeyValueModel {
    let mut model = KeyValueModel::new(&MODULE_ROWS)
        .with_texts(&[(KEY_SPEED_VALUE, &["mts"]), (KEY_VOLTAGE_VALUE, &["v"])]);
    model.hide(ROW_MOD_CONFIGURED);
    model
}

impl ModulesBlock {
    /// An empty block, which is hidden.
    pub fn new() -> Self {
        Self {
            slots_line: SharedString::default(),
            modules: Vec::new(),
            expanded: Vec::new(),
            not_reported: false,
            hidden: true,
            words: KeyValueModel::new(&[ROW_MOD_DETAILS]).with_texts(&[
                (KEY_SLOTS_LINE, &["used", "total"]),
                (KEY_SLOTS_LINE_USED, &["used"]),
                (KEY_MODULE_TITLE, &["n"]),
            ]),
            used: 0,
            slots_total: None,
            anim: None,
        }
    }

    /// The block does not show while the inventory is warming up.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// The "Module details: Not reported" grid.
    pub fn details_model(&self) -> &KeyValueModel {
        &self.words
    }

    /// Opens or closes module `i`. False when there is no such module.
    pub fn toggle(&mut self, i: usize) -> bool {
        let Some(open) = self.expanded.get_mut(i) else {
            return false;
        };
        *open = !*open;
        let generation = self.anim.map_or(0, |a| a.generation.wrapping_add(1));
        self.anim = Some(ChevronAnim {
            module: i,
            opening: *open,
            generation,
        });
        true
    }

    /// Writes the title and grid of every module, for the page's copy text.
    pub fn copy_into(&self, out: &mut String) {
        for (title, model) in &self.modules {
            out.push_str(title);
            out.push('\n');
            let from = out.len();
            model.copy_into(out);
            if out.len() > from {
                out.push('\n');
            }
        }
    }

    /// Translates the words of every grid when the strings changed. True when they did.
    pub fn refresh_strings(&mut self, cx: &App) -> bool {
        let changed = self.words.refresh_strings(cx);
        let mut grids = false;
        for (_, model) in &mut self.modules {
            grids |= model.refresh_strings(cx);
        }
        if changed {
            self.write_words();
        }
        changed || grids
    }

    /// Writes the slots line and the module titles from the current texts.
    fn write_words(&mut self) {
        let mut line = String::new();
        if self.not_reported || self.hidden {
            // No line.
        } else if let Some(total) = self.slots_total {
            let used = self.used;
            self.words
                .text(KEY_SLOTS_LINE)
                .write_each(&mut line, |i, o| {
                    let _ = match i {
                        0 => write!(o, "{used}"),
                        _ => write!(o, "{total}"),
                    };
                });
        } else {
            let used = self.used;
            self.words
                .text(KEY_SLOTS_LINE_USED)
                .write_each(&mut line, |_, o| {
                    let _ = write!(o, "{used}");
                });
        }
        self.slots_line = SharedString::from(line);
        let template = self.words.text(KEY_MODULE_TITLE);
        for (n, (title, _)) in self.modules.iter_mut().enumerate() {
            let mut text = String::new();
            template.write_each(&mut text, |_, o| {
                let _ = write!(o, "{}", n + 1);
            });
            *title = SharedString::from(text);
        }
    }
}

fn inv_shown(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

fn not_reported(model: &mut KeyValueModel, label: &'static str) -> bool {
    model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None)
}

fn module_text(model: &mut KeyValueModel, label: &'static str, f: &InvField<Arc<str>>) -> bool {
    if inv_shown(f.validity) && !f.value.is_empty() {
        model.set_text(label, &f.value)
    } else {
        not_reported(model, label)
    }
}

/// A speed row with the "{$mts} MT/s" text. 0 reads Not reported.
fn module_speed(model: &mut KeyValueModel, label: &'static str, f: &InvField<u32>) -> bool {
    if !(inv_shown(f.validity) && f.value > 0) {
        return not_reported(model, label);
    }
    let mts = f.value;
    let template = model.text(KEY_SPEED_VALUE);
    model.set_valid(label, u64::from(mts), |o| {
        template.write_each(o, |_, o| {
            let _ = write!(o, "{mts}");
        });
    })
}

/// Fills one module's grid. True when any row changed.
fn update_module_model(model: &mut KeyValueModel, m: &MemoryModule) -> bool {
    let mut changed = module_text(model, ROW_MOD_SLOT, &m.slot);
    let cap = &m.capacity_bytes;
    if inv_shown(cap.validity) && cap.value > 0 {
        let bytes = cap.value;
        changed |= model.set_valid(ROW_MOD_CAPACITY, bytes, |o| units::bytes(o, bytes));
    } else {
        changed |= not_reported(model, ROW_MOD_CAPACITY);
    }
    changed |= module_speed(model, ROW_MOD_SPEED, &m.speed_mts);

    // Configured speed shows only when it differs from the rated speed.
    let (speed, configured) = (&m.speed_mts, &m.configured_speed_mts);
    let differs = inv_shown(configured.validity)
        && configured.value > 0
        && !(inv_shown(speed.validity) && speed.value == configured.value);
    changed |= set_hidden(model, ROW_MOD_CONFIGURED, !differs);
    if differs {
        changed |= module_speed(model, ROW_MOD_CONFIGURED, configured);
    }

    changed |= module_text(model, ROW_MOD_TYPE, &m.kind);
    changed |= module_text(model, ROW_MOD_FORM_FACTOR, &m.form_factor);
    changed |= module_text(model, ROW_MOD_MANUFACTURER, &m.manufacturer);
    changed |= module_text(model, ROW_MOD_PART, &m.part_number);

    let rank = &m.rank;
    if inv_shown(rank.validity) && rank.value > 0 {
        let n = rank.value;
        changed |= model.set_valid(ROW_MOD_RANK, u64::from(n), |o| {
            let _ = write!(o, "{n}");
        });
    } else {
        changed |= not_reported(model, ROW_MOD_RANK);
    }

    // The inventory holds millivolts; the grid shows volts with two decimals.
    let mv = &m.configured_voltage_mv;
    if inv_shown(mv.validity) && mv.value > 0 {
        let mv = mv.value;
        let template = model.text(KEY_VOLTAGE_VALUE);
        changed |= model.set_valid(ROW_MOD_VOLTAGE, u64::from(mv), |o| {
            template.write_each(o, |_, o| {
                let _ = write!(o, "{:.2}", f64::from(mv) / 1000.);
            });
        });
    } else {
        changed |= not_reported(model, ROW_MOD_VOLTAGE);
    }
    changed
}

/// Brings the block up to date with the module inventory (PERF-046). Empty slots are not in the
/// inventory. The block starts collapsed from more than [`MODULES_EXPANDED_MAX`] modules, and a
/// later update keeps what the user opened. True when anything shown changed.
pub fn update_modules_block(block: &mut ModulesBlock, inv: &InventorySnapshot) -> bool {
    let validity = inv.header.validity;
    if validity == Validity::WarmingUp {
        let changed = !block.hidden;
        block.hidden = true;
        return changed;
    }
    let mut changed = block.hidden;
    block.hidden = false;

    let slots = &inv.slots_total;
    let slots_total = inv_shown(slots.validity).then_some(slots.value);
    let missing = validity == Validity::Failed || (inv.modules.is_empty() && slots_total.is_none());
    if missing {
        changed |= !block.not_reported;
        block.not_reported = true;
        changed |= !block.modules.is_empty();
        block.modules.clear();
        block.expanded.clear();
        block.used = 0;
        block.slots_total = None;
        changed |= not_reported(&mut block.words, ROW_MOD_DETAILS);
        block.write_words();
        return changed;
    }
    changed |= block.not_reported;
    block.not_reported = false;

    if block.modules.len() != inv.modules.len() {
        block.modules = inv
            .modules
            .iter()
            .map(|_| (SharedString::default(), module_model()))
            .collect();
        block.expanded = vec![inv.modules.len() <= MODULES_EXPANDED_MAX; inv.modules.len()];
        block.anim = None;
        changed = true;
    }
    for ((_, model), m) in block.modules.iter_mut().zip(inv.modules.iter()) {
        changed |= update_module_model(model, m);
    }
    if block.used != inv.modules.len() || block.slots_total != slots_total || changed {
        block.used = inv.modules.len();
        block.slots_total = slots_total;
        block.write_words();
        changed = true;
    }
    changed
}

// ---- Swap devices table ----

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SwapColumn {
    Name,
    Type,
    Size,
    Used,
    Priority,
}

impl SwapColumn {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Type,
        Self::Size,
        Self::Used,
        Self::Priority,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    /// Logical px at 100%. Name is the flex column; this is its width before the first fit.
    fn width(self) -> f32 {
        match self {
            Self::Name => 240.,
            Self::Type => 80.,
            Self::Size | Self::Used => 90.,
            Self::Priority => 70.,
        }
    }
}

impl TableColumn for SwapColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "swap-name",
            Self::Type => "swap-type",
            Self::Size => "swap-size",
            Self::Used => "swap-used",
            Self::Priority => "swap-priority",
        }
    }
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }
    fn pinned_first(self) -> bool {
        false
    }
    fn min_width(self) -> Option<f32> {
        match self {
            Self::Name => Some(120.),
            _ => None,
        }
    }
}

/// The table has no sort; this state only names the first column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SwapSort;

impl TableSort<SwapColumn> for SwapSort {
    fn column(&self) -> SwapColumn {
        SwapColumn::Name
    }
    fn ascending(&self) -> bool {
        true
    }
}

fn swap_specs(words: &SwapWords) -> Vec<ColumnSpec<SwapColumn>> {
    SwapColumn::ALL
        .iter()
        .map(|&id| {
            let label = words.columns[id.index()].clone();
            ColumnSpec {
                id,
                label: label.clone(),
                full_name: label.clone(),
                group: label,
                align: match id {
                    SwapColumn::Name | SwapColumn::Type => ColumnAlign::Left,
                    _ => ColumnAlign::Right,
                },
                kind: match id {
                    SwapColumn::Name => ColumnKind::Path,
                    SwapColumn::Type => ColumnKind::Text,
                    _ => ColumnKind::Numeric,
                },
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

/// The flat, static table of `/proc/swaps` entries (E25-014).
pub struct SwapTableDelegate {
    rows: Arc<[SwapDevice]>,
    version: u64,
    specs: Vec<ColumnSpec<SwapColumn>>,
    layout: ColumnLayout<SwapColumn>,
    words: SwapWords,
}

impl SwapTableDelegate {
    fn new(words: SwapWords, ui_scale: f32) -> Self {
        let specs = swap_specs(&words);
        let mut layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &SwapColumn::ALL,
            &SwapColumn::ALL,
        );
        layout.rebuild(ui_scale);
        Self {
            rows: Arc::from([]),
            version: 0,
            specs,
            layout,
            words,
        }
    }

    /// Takes new rows. False, with the version unchanged, when they equal the current ones.
    fn set_rows(&mut self, rows: Arc<[SwapDevice]>) -> bool {
        if Arc::ptr_eq(&self.rows, &rows) || self.rows == rows {
            return false;
        }
        self.rows = rows;
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

    fn set_words(&mut self, words: SwapWords) {
        self.specs = swap_specs(&words);
        self.words = words;
        self.version += 1;
    }

    /// Gives the Name column the width the other columns leave in a body `body_px` wide
    /// (logical px at the layout's scale, scrollbar excluded), never below its minimum. True
    /// when the width changed.
    fn fit_flex_column(&mut self, body_px: f32) -> bool {
        let scale = self.layout.ui_scale().max(0.1);
        let flex = SwapColumn::Name;
        let others: f32 = SwapColumn::ALL
            .iter()
            .filter(|&&c| c != flex)
            .map(|c| self.layout.widths.get(c).map_or(0.0, |w| w.as_f32()))
            .sum();
        let target = (body_px / scale - SCROLLBAR_W - others).max(flex.min_width().unwrap_or(0.));
        let current = self.layout.widths.get(&flex).map_or(0.0, |w| w.as_f32());
        if (target - current).abs() < 0.5 {
            return false;
        }
        self.layout.widths.insert(flex, px(target));
        self.layout.rebuild(scale);
        self.version += 1;
        true
    }

    /// The table scrolls, so it is a Tab stop (UI-PRC-016).
    pub fn scrolls(&self) -> bool {
        self.rows.len() > SWAP_TABLE_MAX_ROWS
    }
}

/// The type word of a swap device.
fn swap_type_word(words: &SwapWords, kind: SwapKindModel) -> &str {
    match kind {
        SwapKindModel::Partition => &words.types[0],
        SwapKindModel::File => &words.types[1],
        SwapKindModel::Zram => &words.types[2],
        SwapKindModel::Other => "-",
    }
}

/// The text of one swap-table cell, shared by the table and the copy text.
fn swap_cell_text(row: &SwapDevice, col: SwapColumn, words: &SwapWords, out: &mut String) {
    match col {
        SwapColumn::Name => out.push_str(&row.name),
        SwapColumn::Type => out.push_str(swap_type_word(words, row.kind)),
        SwapColumn::Size => units::bytes(out, row.size_bytes),
        SwapColumn::Used => units::bytes(out, row.used_bytes),
        SwapColumn::Priority => {
            let _ = write!(out, "{}", row.priority);
        }
    }
}

impl TreeTableDelegate for SwapTableDelegate {
    type RowId = u32;
    type Column = SwapColumn;
    type Sort = SwapSort;

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
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<SwapColumn> {
        &self.layout
    }
    fn column_spec(&self, id: SwapColumn) -> &ColumnSpec<SwapColumn> {
        &self.specs[id.index()]
    }
    fn cell(&self, ix: usize, col: SwapColumn, out: &mut CellOut) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        out.validity = Validity::Valid;
        if !out.want_text {
            return;
        }
        swap_cell_text(row, col, &self.words, &mut out.text);
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: self.rows.get(ix).map_or_else(SharedString::default, |r| {
                SharedString::from(r.name.clone())
            }),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn name_column(&self) -> Option<SwapColumn> {
        Some(SwapColumn::Name)
    }
    fn header_total(&self, _: SwapColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> SwapSort {
        SwapSort
    }
    fn selectable(&self, _: usize) -> bool {
        false
    }
    fn context_menu(
        &mut self,
        _: &[u32],
        _: Option<SwapColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }
    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_sort(&mut self, _: SwapColumn, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<SwapColumn>,
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

/// A flat table box: the one-line header plus `rows` rows, at most `max_rows` (more rows
/// scroll). The partitions and SMART tables use it too.
pub(super) fn flat_static_table<D: TreeTableDelegate>(
    table: &Entity<TlTreeTable<D>>,
    rows: usize,
    max_rows: usize,
    m: &UiMetrics,
) -> Div {
    div()
        .flex_none()
        .w_full()
        .h(px(HEADER_H_ONE * m.ui_scale) + m.row_height * rows.min(max_rows) as f32)
        .child(table.clone())
}

/// The swap list of a snapshot: the value when it is `Valid` or `Stale`, else none.
fn swap_rows_of(details: &MemoryDetails) -> Arc<[SwapDevice]> {
    match details.swaps.validity {
        Validity::Valid | Validity::Stale => Arc::clone(&details.swaps.value),
        _ => Arc::from([]),
    }
}

/// The Swap chart's height animation (UI-PRF-026).
#[derive(Clone, Copy, Debug)]
struct SwapAnim {
    /// True while the chart opens, false while it closes.
    showing: bool,
    /// Bumped on each change, so a new change restarts the animation.
    generation: u32,
}

/// The Memory device page.
pub struct MemoryPage {
    key: DeviceKey,
    row: DeviceRow,
    physical: Entity<ChartView>,
    commit: Entity<ChartView>,
    swap: Entity<ChartView>,
    /// The composition bar view, which keeps the `CompositionBarState`. Created the first time
    /// the bar is shown; a cached view, so a memory sample repaints only the bar.
    bar: Option<Entity<CompositionBarView>>,
    /// The composition of the newest memory sample.
    composition: Option<Composition>,
    /// E25-003 fills the rows.
    details: KeyValueModel,
    /// The "Compressed memory" block under the Details grid.
    compressed: CompressedBlock,
    /// The "Memory modules" block under the swap table.
    modules: ModulesBlock,
    /// One focus handle per module chevron, in module order.
    module_focus: Vec<FocusHandle>,
    /// This page, for the chevron click closures.
    this: WeakEntity<Self>,
    /// The `/proc/swaps` entries of the "Swap devices" block.
    swap_rows: Arc<[SwapDevice]>,
    /// Created in the first render, which has the `Window` the table needs.
    swap_table: Option<Entity<TlTreeTable<SwapTableDelegate>>>,
    /// The table body width the Name column was last fitted to.
    swap_body: f32,
    /// `performance.memory_composition`.
    show_composition: bool,
    /// `performance.memory_swap_graph`.
    show_swap: bool,
    swap_total: u64,
    totals: Totals,
    /// The Swap chart is in the graph area (its end state while it animates).
    swap_shown: bool,
    swap_anim: Option<SwapAnim>,
    anim_generation: u32,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    /// The page root; the menu items dispatch their actions here.
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl MemoryPage {
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
        let totals = Totals::of(&snap);
        let mut words = Words::default();
        words.refresh(cx);

        let idx = row.idx;
        let physical = {
            let (spec, labels) = (
                physical_spec(idx, totals.mem, &settings, &theme),
                physical_labels(&words),
            );
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };
        let commit = {
            let (spec, labels) = (
                commit_spec(idx, totals.commit_limit, &settings, &theme),
                commit_labels(&words, totals.commit_limit),
            );
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
        };
        let swap = {
            let (spec, labels) = (
                swap_spec(idx, totals.swap, &settings, &theme),
                swap_labels(&words),
            );
            cx.new(|cx| ChartView::new(spec, labels, &hub, cx))
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

        let header = (page_words(cx).memory.clone(), SharedString::default());
        let composition = Composition::of(&snap.system.memory);
        let mut this = Self {
            key,
            row,
            physical,
            commit,
            swap,
            bar: None,
            composition,
            details: memory_details_model(),
            compressed: CompressedBlock::new(),
            modules: ModulesBlock::new(),
            module_focus: Vec::new(),
            this: cx.weak_entity(),
            swap_rows: swap_rows_of(&snap.device_details.memory),
            swap_table: None,
            swap_body: 0.,
            show_composition: settings.composition,
            show_swap: settings.swap,
            swap_total: totals.swap,
            totals,
            swap_shown: settings.swap && totals.swap > 0,
            swap_anim: None,
            anim_generation: 0,
            header,
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            demand,
            id,
            active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        let state = health_state(&snap);
        this.set_chart_state(state, cx);
        this.sync_bar(cx);
        this.sync_details(&snap, cx);
        this.sync_modules(&snap, cx);
        this
    }

    /// Brings Details rows 7 to 11 up to date from the inventory; notifies only on change.
    fn sync_modules(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let mem = &snap.system.memory;
        let total = match mem.validity {
            Validity::Valid | Validity::Stale => mem.total,
            _ => 0,
        };
        if update_memory_details_modules(&mut self.details, &snap.inventory, total) {
            cx.notify();
        }
        if update_modules_block(&mut self.modules, &snap.inventory) {
            let n = self.modules.modules.len();
            self.module_focus.truncate(n);
            while self.module_focus.len() < n {
                self.module_focus.push(cx.focus_handle());
            }
            cx.notify();
        }
    }

    /// Brings the Details rows up to date; notifies only when one changed.
    fn sync_details(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let inputs = MemoryDetailInputs {
            mem: &snap.system.memory,
            now_boot_ns: now_boot_ns(),
            observed_end_ns: snap.system.header.observed_end,
            error: snap.health.get(SourceId::Memory).last_error.as_deref(),
        };
        if update_memory_details(&mut self.details, &inputs) {
            cx.notify();
        }
        let inputs = CompressedInputs {
            mem: &snap.system.memory,
            details: &snap.device_details.memory,
        };
        let changed = update_compressed_block(&mut self.compressed, &inputs);
        if self.compressed.refresh_strings(cx) || changed {
            cx.notify();
        }
        let rows = swap_rows_of(&snap.device_details.memory);
        if !Arc::ptr_eq(&self.swap_rows, &rows) && *self.swap_rows != *rows {
            self.swap_rows = rows;
            cx.notify();
        }
        if let Some(table) = &self.swap_table {
            let rows = Arc::clone(&self.swap_rows);
            table.update(cx, |t, cx| {
                if t.delegate_mut().set_rows(rows) {
                    t.on_new_version(cx);
                }
            });
        }
    }

    /// The "Swap devices" table, once the page has rendered.
    pub fn swap_table(&self) -> Option<&Entity<TlTreeTable<SwapTableDelegate>>> {
        self.swap_table.as_ref()
    }

    /// The "Memory modules" block's models.
    pub fn modules(&self) -> &ModulesBlock {
        &self.modules
    }

    /// Opens or closes module `i`; the chevron turns over `m_fast`.
    pub fn toggle_module(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.modules.toggle(i) {
            cx.notify();
        }
    }

    /// The "Compressed memory" block's models.
    pub fn compressed(&self) -> &CompressedBlock {
        &self.compressed
    }

    /// The composition bar view, once it has been shown.
    pub fn bar(&self) -> Option<&Entity<CompositionBarView>> {
        self.bar.as_ref()
    }

    /// Creates the bar view when the bar is shown for the first time, and hands it the newest
    /// composition. The bar view notifies itself only when the composition changed.
    fn sync_bar(&mut self, cx: &mut Context<Self>) {
        if !self.show_composition {
            return;
        }
        let c = self.composition;
        match &self.bar {
            Some(bar) => bar.update(cx, |b, cx| b.set_composition(c, cx)),
            None => self.bar = Some(cx.new(|cx| CompositionBarView::new(c, cx))),
        }
    }

    pub fn physical(&self) -> &Entity<ChartView> {
        &self.physical
    }

    pub fn commit(&self) -> &Entity<ChartView> {
        &self.commit
    }

    pub fn swap(&self) -> &Entity<ChartView> {
        &self.swap
    }

    pub fn show_composition(&self) -> bool {
        self.show_composition
    }

    pub fn show_swap(&self) -> bool {
        self.show_swap
    }

    /// The Swap chart is in the graph area, or will be when its animation ends.
    pub fn swap_shown(&self) -> bool {
        self.swap_shown
    }

    /// The Swap chart's height is animating.
    pub fn swap_animating(&self) -> bool {
        self.swap_anim.is_some()
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// This page's demand part while it is active: memory, and the inventory for Details rows 7
    /// to 11.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            memory: true,
            inventory: true,
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

    fn set_chart_state(&self, state: ChartState, cx: &mut App) {
        for chart in [&self.physical, &self.commit, &self.swap] {
            let state = state.clone();
            chart.update(cx, |v, cx| v.set_state(state, cx));
        }
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let snap = Arc::clone(hub.read(cx).current());
        if ev.mask.intersects(
            ComponentMask::SYSTEM | ComponentMask::HEALTH | ComponentMask::DEVICE_DETAILS,
        ) {
            self.sync_details(&snap, cx);
        }
        if ev
            .mask
            .intersects(ComponentMask::SYSTEM | ComponentMask::INVENTORY)
        {
            self.sync_modules(&snap, cx);
        }
        if ev.mask.contains(ComponentMask::SYSTEM) {
            self.composition = Composition::of(&snap.system.memory);
            self.sync_bar(cx);
            let totals = Totals::of(&snap);
            if totals != self.totals {
                let old = self.totals;
                self.totals = totals;
                self.swap_total = totals.swap;
                self.rebuild_specs(old, cx);
                self.sync_swap_shown(cx);
            }
        }
        if ev.mask.contains(ComponentMask::HEALTH) {
            self.set_chart_state(health_state(&snap), cx);
        }
    }

    /// Hands each chart whose total changed since `old` its new spec, and the Commit chart its
    /// new value format.
    fn rebuild_specs(&self, old: Totals, cx: &mut App) {
        let (idx, s, t) = (self.row.idx, self.settings, self.totals);
        let theme = current(cx).clone();
        if t.mem != old.mem {
            let spec = physical_spec(idx, t.mem, &s, &theme);
            self.physical.update(cx, |v, cx| v.set_spec(spec, cx));
        }
        if t.commit_limit != old.commit_limit {
            let spec = commit_spec(idx, t.commit_limit, &s, &theme);
            let labels = commit_labels(&self.words, t.commit_limit);
            self.commit.update(cx, |v, cx| {
                v.set_spec(spec, cx);
                v.set_labels(labels, cx);
            });
        }
        if t.swap != old.swap {
            let spec = swap_spec(idx, t.swap, &s, &theme);
            self.swap.update(cx, |v, cx| v.set_spec(spec, cx));
        }
    }

    /// Every chart's spec from the current settings, totals and theme.
    fn rebuild_all_specs(&self, cx: &mut App) {
        let (idx, s, t) = (self.row.idx, self.settings, self.totals);
        let theme = current(cx).clone();
        let physical = physical_spec(idx, t.mem, &s, &theme);
        let commit = commit_spec(idx, t.commit_limit, &s, &theme);
        let swap = swap_spec(idx, t.swap, &s, &theme);
        self.physical.update(cx, |v, cx| v.set_spec(physical, cx));
        self.commit.update(cx, |v, cx| v.set_spec(commit, cx));
        self.swap.update(cx, |v, cx| v.set_spec(swap, cx));
    }

    /// Shows or hides the Swap chart when `show_swap` or `SwapTotal` moved. The height animates
    /// over `m_base` unless animations are off.
    fn sync_swap_shown(&mut self, cx: &mut Context<Self>) {
        let wanted = self.show_swap && self.swap_total > 0;
        if wanted == self.swap_shown {
            return;
        }
        self.swap_shown = wanted;
        self.anim_generation = self.anim_generation.wrapping_add(1);
        if animations_on(cx) {
            let generation = self.anim_generation;
            self.swap_anim = Some(SwapAnim {
                showing: wanted,
                generation,
            });
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(M_BASE).await;
                let _ = this.update(cx, |this, cx| {
                    if this.anim_generation == generation {
                        this.swap_anim = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.swap_anim = None;
        }
        cx.notify();
    }

    /// Applies the page's settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        let old = self.settings;
        if new == old {
            return;
        }
        self.settings = new;
        if (new.range_ns, new.smooth, new.rounded) != (old.range_ns, old.smooth, old.rounded) {
            self.rebuild_all_specs(cx);
        }
        if new.composition != old.composition {
            self.show_composition = new.composition;
            self.sync_bar(cx);
            cx.notify();
        }
        if new.swap != old.swap {
            self.show_swap = new.swap;
            self.sync_swap_shown(cx);
        }
    }

    /// A new theme: the charts take the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_all_specs(cx);
        cx.notify();
    }

    /// New strings: the chart labels, the header and the menu words.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        let (physical, commit, swap) = (
            physical_labels(&self.words),
            commit_labels(&self.words, self.totals.commit_limit),
            swap_labels(&self.words),
        );
        self.physical.update(cx, |v, cx| v.set_labels(physical, cx));
        self.commit.update(cx, |v, cx| v.set_labels(commit, cx));
        self.swap.update(cx, |v, cx| v.set_labels(swap, cx));
        self.header.0 = page_words(cx).memory.clone();
        if let Some(table) = &self.swap_table {
            let words = self.words.swap_table.clone();
            table.update(cx, |t, cx| {
                t.delegate_mut().set_words(words);
                t.on_new_version(cx);
            });
        }
        cx.notify();
    }

    fn on_toggle_composition(
        &mut self,
        _: &PerfToggleMemoryComposition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::update(cx, |d| {
            let p = &mut d.settings.performance;
            p.memory_composition = !p.memory_composition;
        });
        self.sync_settings(cx);
    }

    fn on_toggle_swap(
        &mut self,
        _: &PerfToggleMemorySwapGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::update(cx, |d| {
            let p = &mut d.settings.performance;
            p.memory_swap_graph = !p.memory_swap_graph;
        });
        self.sync_settings(cx);
    }

    /// Creates the "Swap devices" table on the first render. The Name column then follows the
    /// body width of the previous frame.
    fn sync_swap_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let table = match &self.swap_table {
            Some(table) => table.clone(),
            None => {
                let mut delegate = SwapTableDelegate::new(self.words.swap_table.clone(), scale);
                delegate.set_rows(Arc::clone(&self.swap_rows));
                let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
                self.swap_table = Some(table.clone());
                table
            }
        };
        let rescaled = table.update(cx, |t, cx| {
            let changed = t.delegate_mut().set_scale(scale);
            if changed {
                t.on_new_version(cx);
            }
            changed
        });
        if rescaled {
            // The Name column must be fitted again at the new scale.
            self.swap_body = 0.;
        }
        let body = ScrollbarHandle::viewport_bounds(&table.read(cx).scroll_handle())
            .size
            .width
            .as_f32();
        if body > 0. && body != self.swap_body {
            self.swap_body = body;
            table.update(cx, |t, cx| {
                if t.delegate_mut().fit_flex_column(body) {
                    t.on_new_version(cx);
                }
            });
        }
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

impl MemoryPage {
    /// The "Memory modules" block: heading, slots line, then a header row and grid per module.
    /// `None` while the inventory is warming up.
    fn modules_element(
        &self,
        heading: &dyn Fn(SharedString, gpui_kit::TextStyleRefinement) -> Div,
        md: &gpui_kit::TextStyleRefinement,
        sm: &gpui_kit::TextStyleRefinement,
        m: UiMetrics,
        cx: &App,
    ) -> Option<AnyElement> {
        let block = &self.modules;
        if block.is_hidden() {
            return None;
        }
        let theme = current(cx);
        let mut out = v_flex()
            .gap(m.sp[4])
            .child(heading(self.words.block_modules.clone(), md.clone()));
        if block.not_reported {
            return Some(
                out.child(
                    div()
                        .id(MODULES_NOT_REPORTED_ID)
                        .debug_selector(|| MODULES_NOT_REPORTED_ID.into())
                        .child(render_details(block.details_model(), &m, cx)),
                )
                .into_any_element(),
            );
        }
        if !block.slots_line.is_empty() {
            let mut line = div()
                .text_color(theme.text_secondary)
                .child(block.slots_line.clone());
            *line.text_style() = label_style(&m, m.text.sm, FontWeight::NORMAL);
            out = out.child(line);
        }
        let animate = animations_on(cx);
        for (n, (title, model)) in block.modules.iter().enumerate() {
            let open = block.expanded[n];
            let weak = self.this.clone();
            let mut chevron = TlButton::new((MODULE_CHEVRON_ID, n), TlButtonKind::Ghost)
                .size(TlButtonSize::Sm)
                .icon(TlIcon::ChevronRight)
                .icon_size(m.icon.sm)
                .accessible_name(title.clone())
                .on_click(move |_, _, cx| {
                    weak.update(cx, |page, cx| page.toggle_module(n, cx)).ok();
                });
            if let Some(focus) = self.module_focus.get(n) {
                chevron = chevron.focus_handle(focus.clone());
            }
            let turn = |t: f32| radians(std::f32::consts::FRAC_PI_2 * t);
            let chevron = match block.anim.filter(|a| a.module == n && animate) {
                Some(ChevronAnim {
                    opening,
                    generation,
                    ..
                }) => chevron
                    .with_animation(
                        (
                            "perf-mem-module-chevron-anim",
                            generation as usize * 1024 + n,
                        ),
                        Animation::new(M_FAST).with_easing(ease_out_cubic),
                        move |b, t| b.icon_rotation(turn(if opening { t } else { 1. - t })),
                    )
                    .into_any_element(),
                None => chevron
                    .icon_rotation(turn(if open { 1. } else { 0. }))
                    .into_any_element(),
            };
            let mut header = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(m.sp[3])
                .h(px(28. * m.ui_scale))
                .child(
                    div()
                        .debug_selector(move || format!("{MODULE_CHEVRON_ID}-{n}"))
                        .flex_none()
                        .child(chevron),
                );
            header = header.child(heading(title.clone(), sm.clone()));
            out = out.child(header);
            if open {
                out = out.child(
                    div()
                        .id((MODULE_GRID_ID, n))
                        .debug_selector(move || format!("{MODULE_GRID_ID}-{n}"))
                        .child(render_details(model, &m, cx)),
                );
            }
        }
        Some(out.into_any_element())
    }
}

impl DevicePage for MemoryPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let h = GRAPH_H_STACKED * m.ui_scale;

        // The heading tooltip covers the Commit chart box.
        let commit = div()
            .id(ElementId::NamedChild(
                Arc::new(ElementId::Name(SharedString::from(self.key.0.clone()))),
                COMMIT_ID.into(),
            ))
            .w_full()
            .child(Self::chart_box(COMMIT_ID, &self.commit, h));
        let tip = HoverTooltip::state((COMMIT_ID, self.id.as_u64() as usize), window, cx);
        let commit = HoverTooltip::attach(
            &tip,
            commit,
            DELAY_CONTROL,
            TlTooltip::text(self.words.commit_tip.clone()).build(),
            window,
            cx,
        );

        let swap = match self.swap_anim {
            None => self
                .swap_shown
                .then(|| Self::chart_box(SWAP_ID, &self.swap, h).into_any_element()),
            Some(SwapAnim {
                showing,
                generation,
            }) => Some(
                div()
                    .flex_none()
                    .w_full()
                    .overflow_hidden()
                    .child(Self::chart_box(SWAP_ID, &self.swap, h))
                    .with_animation(
                        ("perf-mem-swap-anim", generation as usize),
                        Animation::new(M_BASE).with_easing(ease_out_cubic),
                        move |el, t| el.h(px(h * if showing { t } else { 1. - t })),
                    )
                    .into_any_element(),
            ),
        };

        // The composition bar after `sp6` when `show_composition`.
        let bar = self
            .bar
            .as_ref()
            .filter(|_| self.show_composition)
            .map(|bar| {
                div()
                    .w_full()
                    .pt(m.sp[6] - m.sp[4])
                    .child(bar.clone().cached(StyleRefinement::default().w_full()))
            });
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(Self::chart_box(PHYSICAL_ID, &self.physical, h))
            .child(commit)
            .children(swap)
            .children(bar)
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 3]> {
        let mut blocks: SmallVec<[AnyElement; 3]> = SmallVec::new();
        let (m, theme) = (metrics(cx), current(cx));
        let heading = |text: SharedString, style| {
            let mut el = div().text_color(theme.text_primary).child(text);
            *el.text_style() = style;
            el
        };
        let md = label_style(m, m.text.md, FontWeight::MEDIUM);
        let sm = label_style(m, m.text.sm, FontWeight::MEDIUM);
        if let Some(table) = self
            .swap_table
            .as_ref()
            .filter(|_| !self.swap_rows.is_empty())
        {
            blocks.push(
                v_flex()
                    .gap(m.sp[4])
                    .child(heading(self.words.block_swap.clone(), md.clone()))
                    .child(
                        flat_static_table(table, self.swap_rows.len(), SWAP_TABLE_MAX_ROWS, m)
                            .debug_selector(|| SWAP_TABLE_ID.into()),
                    )
                    .into_any_element(),
            );
        }
        if let Some(block) = self.modules_element(&heading, &md, &sm, *m, cx) {
            blocks.push(block);
        }
        if self.compressed.is_hidden() {
            return blocks;
        }
        let mut block = v_flex()
            .gap(m.sp[4])
            .child(heading(self.words.block_compressed.clone(), md));
        let grid = |id: ElementId, selector: String, model: &KeyValueModel| {
            div()
                .id(id)
                .debug_selector(move || selector)
                .child(render_details(model, m, cx))
        };
        for (n, (name, model)) in self
            .compressed
            .names
            .iter()
            .zip(&self.compressed.zram)
            .enumerate()
        {
            block = block.child(heading(name.clone(), sm.clone())).child(grid(
                (ZRAM_GRID_ID, n).into(),
                format!("{ZRAM_GRID_ID}-{n}"),
                model,
            ));
        }
        block = block
            .child(heading(self.words.zswap.clone(), sm))
            .child(grid(
                ZSWAP_GRID_ID.into(),
                ZSWAP_GRID_ID.to_owned(),
                &self.compressed.zswap,
            ));
        // The compressed block comes first, then the swap table.
        blocks.insert(0, block.into_any_element());
        blocks
    }

    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        let w = &self.words;
        menu.check_item(
            w.menu_composition.clone(),
            self.show_composition,
            Box::new(PerfToggleMemoryComposition),
        )
        .check_item(
            w.menu_swap.clone(),
            self.show_swap,
            Box::new(PerfToggleMemorySwapGraph),
        )
    }

    /// The Memory copy text (PERF-011, UI-PRF-022): the header word, the Details rows, then the
    /// blocks in screen order: Compressed memory, Swap devices, Memory modules. No module serial
    /// is read (D-007).
    fn copy_text(&self, out: &mut String) {
        out.push_str(&self.header.0);
        out.push('\n');
        let rows_from = out.len();
        self.details.copy_into(out);
        if out.len() > rows_from {
            out.push('\n');
        }
        let grid = |out: &mut String, model: &KeyValueModel| {
            let from = out.len();
            model.copy_into(out);
            if out.len() > from {
                out.push('\n');
            }
        };
        if !self.compressed.is_hidden() {
            out.push('\n');
            out.push_str(&self.words.block_compressed);
            out.push('\n');
            for (name, model) in self.compressed.names.iter().zip(&self.compressed.zram) {
                out.push_str(name);
                out.push('\n');
                grid(out, model);
            }
            out.push_str(&self.words.zswap);
            out.push('\n');
            grid(out, &self.compressed.zswap);
        }
        if !self.swap_rows.is_empty() {
            out.push('\n');
            out.push_str(&self.words.block_swap);
            out.push('\n');
            let words = &self.words.swap_table;
            for (i, row) in self.swap_rows.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                for (col, label) in SwapColumn::ALL.into_iter().zip(&words.columns) {
                    out.push_str("    ");
                    out.push_str(label);
                    out.push_str(": ");
                    swap_cell_text(row, col, words, out);
                    out.push('\n');
                }
            }
        }
        if !self.modules.is_hidden() {
            out.push('\n');
            out.push_str(&self.words.block_modules);
            out.push('\n');
            if self.modules.not_reported {
                grid(out, self.modules.details_model());
            } else {
                self.modules.copy_into(out);
            }
        }
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        self.module_focus.iter().cloned().collect()
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

impl Focusable for MemoryPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MemoryPage {
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
        self.compressed.refresh_strings(cx);
        self.modules.refresh_strings(cx);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.sync_swap_table(window, cx);
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle_composition))
            .on_action(cx.listener(Self::on_toggle_swap))
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

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn swap_columns_follow_ui_scale() {
        let mut d = SwapTableDelegate::new(SwapWords::default(), 2.0);
        assert_eq!(
            d.layout.width(SwapColumn::Size),
            px(SwapColumn::Size.width() * 2.0)
        );
        let v = d.version;
        assert!(!d.set_scale(2.0));
        assert!(d.set_scale(1.0));
        assert!(d.version > v);
        assert_eq!(
            d.layout.width(SwapColumn::Size),
            px(SwapColumn::Size.width())
        );
    }

    fn mem() -> MemoryTotals {
        MemoryTotals {
            validity: Validity::Valid,
            total: 64 * GIB,
            available: 40 * GIB,
            free: 20 * GIB,
            cached: 12 * GIB,
            committed: 30 * GIB,
            commit_limit: 80 * GIB,
            slab_reclaimable: GIB,
            slab_unreclaimable: GIB / 2,
            swap_total: 32 * GIB,
            swap_free: 31 * GIB,
            ..MemoryTotals::default()
        }
    }

    #[test]
    fn memory_details_no_realloc_when_unchanged() {
        let mut model = memory_details_model();
        let m = mem();
        let inp = MemoryDetailInputs {
            mem: &m,
            now_boot_ns: 0,
            observed_end_ns: 0,
            error: None,
        };
        assert!(update_memory_details(&mut model, &inp));
        let ptrs: Vec<*const u8> = model.rows.iter().map(|r| r.value.as_ptr()).collect();
        assert!(
            !update_memory_details(&mut model, &inp),
            "second call changes nothing"
        );
        let again: Vec<*const u8> = model.rows.iter().map(|r| r.value.as_ptr()).collect();
        assert_eq!(ptrs, again, "no value text was reallocated");
    }
}
