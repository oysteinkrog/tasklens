//! The Memory page: the Physical memory, Commit and Swap charts, the composition bar, the
//! Details grid, the Compressed memory, Swap devices and Memory modules blocks, the page-menu
//! check items and the page's demand (docs/ui/performance.md UI-PRF-026 to UI-PRF-028,
//! UI-PRF-020, UI-PRF-060; PERF-018, PERF-026, PERF-041; ADR-008).
//!
//! The page body is one widget container (ADR-019 phase 3, `tasklens-hzire.6`). By default it
//! holds the 1.3.0 parts in their 1.3.0 places (`page_flow::memory`): the Physical memory,
//! Commit and Swap charts as chart widgets, the composition bar ([`CompositionBarView`] in a
//! [`MemoryPartView`], fixed widget `memory_bar`), Details, and the three blocks as the fixed
//! widgets `memory_compressed`, `swap_devices` and `memory_modules`. The Usage and By app switch
//! above the first part picks the page's preset: `usage` (Physical memory) or `apps` (Memory by
//! app, the fixed widget `memory_by_app`, whose legend opens a memory group in Processes).
//! "Show swap usage" and "Show memory composition" add or remove the Swap chart and the bar.
//! Edit Widgets adds in use, available, cached, swap, commit, pressure and the other memory
//! metrics as chart, bar, value, stats or any mix.
//!
//! `page.memory` converts `memory_view`, `memory_swap_graph` and `memory_composition` once, at
//! load (`Settings::clamp`). The switch and the page menu edit the page's flow layout and never
//! write those keys; a page without an entry gets one. With a grid layout the switch moves to
//! the page header and asks "Replace your widgets?" first, and the two check items are hidden.
//! "Groups shown" stays in the page menu while the page shows Memory by app.
//!
//! Render contract: the page renders on a header change, a range change, a layout change and an
//! edit mode change, never because of a sample. The charts, the bar, Details and the blocks are
//! views of their own that notify only for their own changes.
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::ScrollbarHandle;
use gpui_kit::component::v_flex;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyView, AnyWindowHandle, App, AppContext as _,
    Context, Div, ElementId, Entity, EntityId, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, WeakEntity, Window, div, px, radians,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::SourceId;
use tl_model::snapshot::{
    InvField, InventorySnapshot, MemoryDetails, MemoryModule, Snapshot, SwapDevice, SwapKindModel,
};
use tl_model::system::{MemoryTotals, ZramTotals};
use tl_model::units;
use tl_model::validity::Validity;
use tl_ui_settings::layout::{
    ContainerLayout, FixedWidget, WidgetContent, WidgetOptions, WidgetSetting, container_layout,
};
use tl_ui_settings::layout_boxes::METRIC_MEM_SWAP;
use tl_ui_settings::layout_pages::{
    METRIC_COMMIT_PCT, METRIC_PHYSICAL, PRESET_APPS, PRESET_USAGE, convert_memory_page,
    memory_preset, page_container,
};
use tl_ui_settings::settings::{MemoryView, PerformanceSettings};

use super::actions::{
    PerfSetMemoryGroupsShown, PerfToggleMemoryComposition, PerfToggleMemorySwapGraph,
    PerfWidgetEditStart, PerfWidgetResetPage,
};
use super::aggregate::{add_demand, page_layout, reset_page, start_page_edit, strings_version};
use super::details::{Fallback, KeyValueModel, render_details};
use super::gpu_views::PartResized;
use super::memory_bar::{Composition, CompositionBarView};
use super::memory_views::{
    MemoryPartView, PART_BAR, PART_COMPRESSED, PART_MODULES, PART_SWAP_DEVICES, PART_SWITCH,
    PartDraw, PartShown,
};
use super::overview::memory_apps::{AppsChart, AppsMetric, GROUPS_SHOWN};
use super::page::{
    DevicePage, DevicePageFrame, details_draw, page_history_request, page_range_ns, page_words,
};
use super::page_flow::memory::{BLOCKS, MemoryParts, block_index, memory_flow};
use super::widget::container::{FlowDecor, WidgetContainer};
use super::widget::details_widget::{DetailsResized, DetailsShown, DetailsView};
use super::widget::edit::{WidgetEditor, default_layout};
use super::widget::fixed_widget::FixedContent;
use super::widget::menu::confirm_preset_page;
use super::widget::source::SourceStore;
use super::{DeviceKey, DeviceRow};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use tl_ui_base::i18n::{Strings, tr};
use tl_ui_base::selector::TlSelector;
use tl_ui_base::settings_global::{Section, SettingKey, SettingValue, TlSettings, animations_on};
use tl_ui_base::text::style::label_style;
use tl_ui_base::theme::{TlTheme, UiMetrics, current, metrics};
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::button::{TlButton, TlButtonKind, TlButtonSize};
use tl_ui_kit::components::icons::TlIcon;
use tl_ui_kit::components::menu::TlMenu;
use tl_ui_kit::components::motion::{M_FAST, ease_out_cubic};
use tl_ui_table::table::cells::CellOut;
use tl_ui_table::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use tl_ui_table::table::tree_table::{
    HEADER_H_ONE, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, SCROLLBAR_W,
    TableSort, TlTreeTable, TreeTableDelegate,
};

/// Debug selector of the Usage / By app switch; each segment adds `-usage` or `-apps`.
pub const VIEW_SWITCH_ID: &str = "perf-mem-view";

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
const KEY_VIEW_USAGE: &str = "perf-mem-view-usage";
const KEY_VIEW_APPS: &str = "perf-mem-view-apps";
const KEY_GROUPS_SHOWN: &str = "perf-mem-apps-groups-shown";
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
    pub view_usage: SharedString,
    pub view_apps: SharedString,
    pub groups_shown: SharedString,
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
        self.view_usage = t(KEY_VIEW_USAGE);
        self.view_apps = t(KEY_VIEW_APPS);
        self.groups_shown = t(KEY_GROUPS_SHOWN);
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
    /// `performance.memory_by_app.top_n`.
    top_n: u8,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: page_range_ns(cx),
            top_n: s.performance.memory_by_app.top_n,
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

// ---- The page's flow layout (ADR-019 phase 3 "Header buttons and page menu") ----

/// The view of the Memory page preset `id`; `None` for an id that is not one.
pub fn preset_view(id: &str) -> Option<MemoryView> {
    match id {
        PRESET_USAGE => Some(MemoryView::Usage),
        PRESET_APPS => Some(MemoryView::Apps),
        _ => None,
    }
}

/// The view of `layout`: its preset's, `None` for a grid layout.
pub fn layout_view(layout: &ContainerLayout) -> Option<MemoryView> {
    layout.preset.as_ref().and_then(|p| preset_view(&p.id))
}

/// Whether `w` is a metric widget of `metric`.
fn is_metric(w: &WidgetSetting, metric: &str) -> bool {
    matches!(&w.content, WidgetContent::Metric(s) if s.metric.as_str() == metric)
}

/// Whether `w` is a preset's first part: Physical memory or Memory by app.
fn is_mode_widget(w: &WidgetSetting) -> bool {
    is_metric(w, METRIC_PHYSICAL) || w.content == WidgetContent::Fixed(FixedWidget::MemoryByApp)
}

fn is_bar(w: &WidgetSetting) -> bool {
    w.content == WidgetContent::Fixed(FixedWidget::MemoryBar)
}

/// Whether the flow layout `layout` lists the Swap chart.
pub fn layout_swap(layout: &ContainerLayout) -> bool {
    layout.widgets.iter().any(|w| is_metric(w, METRIC_MEM_SWAP))
}

/// Whether the flow layout `layout` lists the composition bar.
pub fn layout_bar(layout: &ContainerLayout) -> bool {
    layout.widgets.iter().any(is_bar)
}

/// The widget of the built-in `view` preset that `pick` chooses, as conversion lists it.
fn preset_widget(
    container: &str,
    view: MemoryView,
    pick: fn(&WidgetSetting) -> bool,
) -> Option<WidgetSetting> {
    let perf = PerformanceSettings {
        memory_view: view,
        memory_swap_graph: true,
        memory_composition: true,
        ..PerformanceSettings::default()
    };
    convert_memory_page(&perf, container)
        .widgets
        .into_iter()
        .find(pick)
}

/// Inserts `w` into `layout` at `at` with a new id.
fn insert_widget(layout: &mut ContainerLayout, at: usize, w: WidgetSetting) {
    let at = at.min(layout.widgets.len());
    let id = layout.next_id;
    layout.next_id += 1;
    layout.widgets.insert(
        at,
        WidgetSetting {
            id,
            cell: None,
            options: WidgetOptions::default(),
            ..w
        },
    );
}

/// The place of the first widget `pick` chooses in `layout`.
fn position(layout: &ContainerLayout, pick: impl Fn(&WidgetSetting) -> bool) -> Option<usize> {
    layout.widgets.iter().position(pick)
}

/// Sets the preset of the flow layout `layout` to `view`'s and puts the new preset's first part
/// in place of the old one. Commit, Swap, the bar, Details and the blocks keep their ids.
/// Nothing for a grid layout.
pub fn set_memory_preset(layout: &mut ContainerLayout, view: MemoryView) {
    let Some(preset) = layout.preset.as_mut() else {
        return;
    };
    let id = memory_preset(view);
    if preset.id == id {
        return;
    }
    preset.id = id.to_owned();
    let at = position(layout, is_mode_widget).unwrap_or(0);
    layout.widgets.retain(|w| !is_mode_widget(w));
    if let Some(w) = preset_widget(&layout.container, view, is_mode_widget) {
        insert_widget(layout, at, w);
    }
}

/// Adds the Swap chart to the flow layout `layout` (after Commit, as in 1.3.0) or takes it off.
/// Nothing for a grid layout.
pub fn set_memory_swap(layout: &mut ContainerLayout, on: bool) {
    if layout.preset.is_none() || layout_swap(layout) == on {
        return;
    }
    if !on {
        layout.widgets.retain(|w| !is_metric(w, METRIC_MEM_SWAP));
        return;
    }
    let at = position(layout, |w| is_metric(w, METRIC_COMMIT_PCT))
        .map(|i| i + 1)
        .or_else(|| position(layout, is_bar))
        .or_else(|| position(layout, is_details))
        .unwrap_or(layout.widgets.len());
    let view = layout_view(layout).unwrap_or_default();
    if let Some(w) = preset_widget(&layout.container, view, |w| is_metric(w, METRIC_MEM_SWAP)) {
        insert_widget(layout, at, w);
    }
}

/// Adds the composition bar to the flow layout `layout` (before Details, as in 1.3.0) or takes
/// it off. Nothing for a grid layout.
pub fn set_memory_bar(layout: &mut ContainerLayout, on: bool) {
    if layout.preset.is_none() || layout_bar(layout) == on {
        return;
    }
    if !on {
        layout.widgets.retain(|w| !is_bar(w));
        return;
    }
    let at = position(layout, is_details).unwrap_or(layout.widgets.len());
    let view = layout_view(layout).unwrap_or_default();
    if let Some(w) = preset_widget(&layout.container, view, is_bar) {
        insert_widget(layout, at, w);
    }
}

fn is_details(w: &WidgetSetting) -> bool {
    w.content == WidgetContent::Fixed(FixedWidget::Details)
}

/// The memory-by-app chart, made the first time a layout places it.
type AppsSlot = Rc<RefCell<Option<Entity<AppsChart>>>>;

/// The chart of `slot`, made now when it has none.
fn apps_of(slot: &AppsSlot, hub: &Entity<SnapshotHub>, cx: &mut App) -> Entity<AppsChart> {
    if let Some(apps) = &*slot.borrow() {
        return apps.clone();
    }
    let hub = hub.clone();
    let apps = cx.new(|cx| AppsChart::new(hub, AppsMetric::Memory, cx));
    *slot.borrow_mut() = Some(apps.clone());
    apps
}

/// The draw of a part from the page: `f` builds the element. `None` once the page is gone.
fn part_draw(
    page: WeakEntity<MemoryPage>,
    f: fn(&mut MemoryPage, &mut Window, &mut Context<MemoryPage>) -> Option<AnyElement>,
) -> PartDraw {
    Rc::new(move |window: &mut Window, cx: &mut App| {
        page.upgrade()?.update(cx, |p, cx| f(p, window, cx))
    })
}

/// A heading of a block in `style`.
fn heading(text: SharedString, style: gpui_kit::TextStyleRefinement, theme: &TlTheme) -> Div {
    let mut el = div().text_color(theme.text_primary).child(text);
    *el.text_style() = style;
    el
}

/// The Memory device page.
pub struct MemoryPage {
    key: DeviceKey,
    row: DeviceRow,
    /// The container id, `page.memory`.
    container_id: SharedString,
    /// The memory-by-app chart, once a layout placed it.
    apps: AppsSlot,
    hub: Entity<SnapshotHub>,
    /// The composition bar view, which keeps the `CompositionBarState`; a memory sample
    /// repaints only the bar.
    bar: Entity<CompositionBarView>,
    /// E25-003 fills the rows.
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    /// The "Compressed memory" block.
    compressed: CompressedBlock,
    /// The "Memory modules" block.
    modules: ModulesBlock,
    /// One focus handle per module chevron, in module order.
    module_focus: Vec<FocusHandle>,
    /// This page, for the click closures.
    this: WeakEntity<Self>,
    /// The `/proc/swaps` entries of the "Swap devices" block.
    swap_rows: Arc<[SwapDevice]>,
    /// Created the first time the block draws, which has the `Window` the table needs.
    swap_table: Option<Entity<TlTreeTable<SwapTableDelegate>>>,
    /// The table body width the Name column was last fitted to.
    swap_body: f32,
    /// The Usage and By app switch the flow layout draws above its first part.
    switch_view: Entity<MemoryPartView>,
    /// The composition bar part (fixed widget `memory_bar`).
    bar_view: Entity<MemoryPartView>,
    /// The blocks, in `page_flow::memory::BLOCKS` order.
    block_views: [Entity<MemoryPartView>; 3],
    /// The heights and flags the flow placer reads.
    parts: MemoryParts,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
    /// The preset of the layout the page shows; `None` for a grid layout.
    view: Option<MemoryView>,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 3],
    /// `UiMetrics.style_version` the parts were drawn for.
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
    _subs: SmallVec<[Subscription; 12]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
    /// How many times the page asked the container to place its widgets again.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub place_count: Cell<u32>,
}

impl MemoryPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let container_id = SharedString::from(page_container(&key.0));
        let settings = PageSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let mut words = Words::default();
        words.refresh(cx);
        let this = cx.weak_entity();

        let composition = Composition::of(&snap.system.memory);
        let bar = cx.new(|cx| CompositionBarView::new(composition, cx));
        let mut details = memory_details_model();
        details.refresh_strings(cx);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(this.clone());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let mut part = |part: &'static str, draw: PartDraw| {
            let id = container_id.clone();
            cx.new(move |_| MemoryPartView::new(id, part, PartShown::default(), draw))
        };
        let switch_view = part(
            PART_SWITCH,
            part_draw(this.clone(), |p, _, cx| Some(p.view_switch(cx))),
        );
        let bar_view = part(
            PART_BAR,
            part_draw(this.clone(), |p, _, _| {
                // Not cached: a cached view with only a width has no height of its own.
                Some(div().w_full().child(p.bar.clone()).into_any_element())
            }),
        );
        let block_views = [
            part(
                PART_COMPRESSED,
                part_draw(this.clone(), |p, _, cx| p.compressed_element(cx)),
            ),
            part(
                PART_SWAP_DEVICES,
                part_draw(this.clone(), |p, window, cx| {
                    p.sync_swap_table(window, cx);
                    p.swap_element(cx)
                }),
            ),
            part(
                PART_MODULES,
                part_draw(this.clone(), |p, _, cx| p.modules_element(cx)),
            ),
        ];
        let natural = |v: &Entity<MemoryPartView>, cx: &App| v.read(cx).natural_height();
        let parts = MemoryParts {
            switch: natural(&switch_view, cx),
            switch_at: Rc::default(),
            swap: Rc::new(Cell::new(snap.system.memory.swap_total > 0)),
            bar: natural(&bar_view, cx),
            details: details_view.read(cx).natural_height(),
            blocks: [
                natural(&block_views[0], cx),
                natural(&block_views[1], cx),
                natural(&block_views[2], cx),
            ],
            shown: [
                Rc::new(Cell::new(false)),
                Rc::new(Cell::new(false)),
                Rc::new(Cell::new(false)),
            ],
        };
        let apps: AppsSlot = Rc::default();
        let content: FixedContent = {
            let details: AnyView = details_view.clone().into();
            let bar: AnyView = bar_view.clone().into();
            let blocks: [AnyView; 3] = block_views.clone().map(Into::into);
            let shown = parts.shown.clone();
            let (slot, hub) = (Rc::clone(&apps), hub.clone());
            Rc::new(move |kind: &FixedWidget, cx: &mut App| match kind {
                FixedWidget::Details => Some(details.clone()),
                FixedWidget::MemoryBar => Some(bar.clone()),
                FixedWidget::MemoryByApp => Some(apps_of(&slot, &hub, cx).into()),
                // A block 1.3.0 hides shows "Nothing to show on this device" in a grid layout.
                kind => {
                    let i = block_index(kind)?;
                    shown[i].get().then(|| blocks[i].clone())
                }
            })
        };
        let layout = page_layout(&container_id, cx);
        let view = layout_view(&layout);
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::for_page(&hub)));
            let id = container_id.clone();
            let hub = hub.clone();
            let decor = FlowDecor {
                view: switch_view.clone().into(),
                bounds: Rc::clone(&parts.switch_at),
            };
            let flow = memory_flow(parts.clone());
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                c.set_flow_placer(Some(flow), cx);
                c.set_fixed_content(Some(content), cx);
                c.set_flow_decor(Some(decor), cx);
                c
            })
        };
        let editor = {
            let container = container.clone();
            let hub = hub.clone();
            cx.new(move |cx| WidgetEditor::new(container, &hub, cx))
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        // A part got taller or shorter: the parts under it move.
        subs.push(
            cx.subscribe(&details_view, |this, _, _: &DetailsResized, cx| {
                this.place_again(cx);
            }),
        );
        for view in [&switch_view, &bar_view]
            .into_iter()
            .chain(block_views.iter())
        {
            subs.push(cx.subscribe(view, |this, _, _: &PartResized, cx| {
                this.place_again(cx);
            }));
        }
        // A layout change can change what the page asks for, its menu and its focus order.
        subs.push(cx.observe(&container, |this, _, cx| this.on_container_changed(cx)));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let header = (page_words(cx).memory.clone(), SharedString::default());
        editor.update(cx, |e, _| e.set_page_name(header.0.clone()));
        let mut this = Self {
            key,
            row,
            container_id,
            apps,
            hub,
            bar,
            details,
            details_view,
            compressed: CompressedBlock::new(),
            modules: ModulesBlock::new(),
            module_focus: Vec::new(),
            this,
            swap_rows: swap_rows_of(&snap.device_details.memory),
            swap_table: None,
            swap_body: 0.,
            switch_view,
            bar_view,
            block_views,
            parts,
            container,
            editor,
            view,
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
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            place_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this.sync_parts(cx);
        this
    }

    /// The memory-by-app chart, once a layout has placed it.
    pub fn apps(&self) -> Option<Entity<AppsChart>> {
        self.apps.borrow().clone()
    }

    /// The preset of the layout the page shows: Usage or By app, `None` with a grid layout.
    pub fn view(&self) -> Option<MemoryView> {
        self.view
    }

    /// The page's widget container.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The editor around the container (Edit Widgets).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The Details widget's view.
    pub fn details_view(&self) -> &Entity<DetailsView> {
        &self.details_view
    }

    /// The container id, `page.memory`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The Usage and By app switch.
    pub fn switch_view(&self) -> &Entity<MemoryPartView> {
        &self.switch_view
    }

    /// The composition bar part (fixed widget `memory_bar`).
    pub fn bar_view(&self) -> &Entity<MemoryPartView> {
        &self.bar_view
    }

    /// The block parts, in `page_flow::memory::BLOCKS` order: Compressed memory, Swap devices,
    /// Memory modules.
    pub fn block_views(&self) -> &[Entity<MemoryPartView>; 3] {
        &self.block_views
    }

    /// Whether the block `kind` has something to show (1.3.0 hides it otherwise).
    pub fn block_shown(&self, kind: &FixedWidget) -> bool {
        block_index(kind).is_some_and(|i| self.parts.shown[i].get())
    }

    /// The id of the first widget of `metric` in the layout the page shows.
    pub fn widget_of(&self, metric: &str, cx: &App) -> Option<u32> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .find(|w| is_metric(w, metric))
            .map(|w| w.id)
    }

    /// The chart of the first placed widget of `metric` that draws one.
    pub fn chart_of(&self, metric: &str, cx: &App) -> Option<Entity<ChartView>> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .filter(|w| is_metric(w, metric))
            .find_map(|w| c.widget(w.id)?.read(cx).chart().cloned())
    }

    /// The Physical memory chart, while the page shows it.
    pub fn physical(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_PHYSICAL, cx)
    }

    /// The Commit chart, while the page shows it.
    pub fn commit(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_COMMIT_PCT, cx)
    }

    /// The Swap chart, while the page shows it.
    pub fn swap(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_MEM_SWAP, cx)
    }

    /// Whether the page places a fixed widget of `kind`.
    fn placed(&self, kind: &FixedWidget, cx: &App) -> bool {
        let c = self.container.read(cx);
        let placed = c.placed_ids();
        c.layout().widgets.iter().any(|w| {
            placed.contains(&w.id) && matches!(&w.content, WidgetContent::Fixed(k) if k == kind)
        })
    }

    /// The flow layout lists the composition bar ("Show memory composition").
    pub fn show_composition(&self, cx: &App) -> bool {
        layout_bar(self.container.read(cx).layout())
    }

    /// The flow layout lists the Swap chart ("Show swap usage").
    pub fn show_swap(&self, cx: &App) -> bool {
        layout_swap(self.container.read(cx).layout())
    }

    /// Whether the Swap chart takes space: the layout lists it and the system has swap.
    pub fn swap_shown(&self, cx: &App) -> bool {
        self.swap(cx).is_some()
    }

    /// The composition bar view.
    pub fn bar(&self) -> &Entity<CompositionBarView> {
        &self.bar
    }

    /// The "Swap devices" table, once the block has drawn.
    pub fn swap_table(&self) -> Option<&Entity<TlTreeTable<SwapTableDelegate>>> {
        self.swap_table.as_ref()
    }

    /// The "Memory modules" block's models.
    pub fn modules(&self) -> &ModulesBlock {
        &self.modules
    }

    /// The "Compressed memory" block's models.
    pub fn compressed(&self) -> &CompressedBlock {
        &self.compressed
    }

    /// Opens or closes module `i`; the chevron turns over `m_fast`.
    pub fn toggle_module(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.modules.toggle(i) {
            self.sync_parts(cx);
        }
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// This page's demand part while it is active: memory, the inventory for Details rows 7 to
    /// 11 and the modules block, the device metric history at 1 h, 24 h and 7 d, and what the
    /// placed widgets read. Memory by app asks for the process sweep (`process_top`): the engine
    /// sums the memory groups on each sweep (UI-PRF-060). With the default layout it equals
    /// 1.3.0's.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            memory: true,
            inventory: true,
            device_history_range: page_history_request(self.settings.range_ns),
            ..Demand::default()
        };
        add_demand(&mut demand, &self.container.read(cx).demand_part());
        demand
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part(cx));
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A part's height, the swap or a block's shown state changed: the flow layout places the
    /// parts again.
    fn place_again(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.place_count.set(self.place_count.get() + 1);
        self.container.update(cx, |c, cx| {
            c.place_again(cx);
        });
    }

    /// The container placed its widgets again or took a new layout: the demand follows.
    fn on_container_changed(&mut self, cx: &mut Context<Self>) {
        self.send_demand(cx);
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

    /// Brings Details, the blocks, the bar and the swap state up to date with the components in
    /// `mask`. The page itself does not render; each part notifies for its own change. A sample
    /// that changes what exists (SwapTotal crossing 0, a block starting or stopping to show)
    /// places the widgets once, after both are up to date.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut parts = false;
        let mut moved = false;
        if mask.intersects(
            ComponentMask::SYSTEM | ComponentMask::HEALTH | ComponentMask::DEVICE_DETAILS,
        ) {
            parts |= self.sync_details(snap, cx);
        }
        if mask.intersects(ComponentMask::SYSTEM | ComponentMask::INVENTORY) {
            parts |= self.sync_modules(snap, cx);
        }
        if mask.contains(ComponentMask::SYSTEM) {
            let c = Composition::of(&snap.system.memory);
            self.bar.update(cx, |b, cx| b.set_composition(c, cx));
            let swap = snap.system.memory.swap_total > 0;
            if swap != self.parts.swap.get() {
                self.parts.swap.set(swap);
                moved = true;
            }
        }
        if parts {
            moved |= self.update_parts(cx);
        }
        if moved {
            self.place_again(cx);
        }
        // Memory went away: an edit ends as Cancel does.
        if mask.contains(ComponentMask::DEVICES)
            && !snap
                .devices
                .entries
                .iter()
                .any(|e| e.idx == self.row.idx && e.present)
        {
            self.editor.update(cx, |e, cx| e.cancel(cx));
        }
    }

    /// Brings Details rows 7 to 11 and the modules block up to date from the inventory. True
    /// when the block changed.
    fn sync_modules(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let mem = &snap.system.memory;
        let total = match mem.validity {
            Validity::Valid | Validity::Stale => mem.total,
            _ => 0,
        };
        if update_memory_details_modules(&mut self.details, &snap.inventory, total) {
            self.push_details(cx);
        }
        if !update_modules_block(&mut self.modules, &snap.inventory) {
            return false;
        }
        let n = self.modules.modules.len();
        self.module_focus.truncate(n);
        while self.module_focus.len() < n {
            self.module_focus.push(cx.focus_handle());
        }
        true
    }

    /// Brings the Details rows, the compressed block and the swap rows up to date. True when a
    /// block changed.
    fn sync_details(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let inputs = MemoryDetailInputs {
            mem: &snap.system.memory,
            now_boot_ns: self.hub.read(cx).now_ns(),
            observed_end_ns: snap.system.header.observed_end,
            error: snap.health.get(SourceId::Memory).last_error.as_deref(),
        };
        if update_memory_details(&mut self.details, &inputs) {
            self.push_details(cx);
        }
        let inputs = CompressedInputs {
            mem: &snap.system.memory,
            details: &snap.device_details.memory,
        };
        let mut changed = update_compressed_block(&mut self.compressed, &inputs);
        changed |= self.compressed.refresh_strings(cx);
        let rows = swap_rows_of(&snap.device_details.memory);
        if !Arc::ptr_eq(&self.swap_rows, &rows) && *self.swap_rows != *rows {
            self.swap_rows = rows;
            changed = true;
        }
        if let Some(table) = &self.swap_table {
            let rows = Arc::clone(&self.swap_rows);
            table.update(cx, |t, cx| {
                if t.delegate_mut().set_rows(rows) {
                    t.on_new_version(cx);
                }
            });
        }
        changed
    }

    /// Hands the Details view what the grid shows; it notifies only when that changed.
    fn push_details(&mut self, cx: &mut Context<Self>) {
        let shown = DetailsShown::of(&self.details, strings_version(cx));
        self.details_view.update(cx, |v, cx| v.set_shown(shown, cx));
    }

    /// Hands every part what it shows (see `update_parts`) and places the parts again when a
    /// block starts or stops having something to show.
    fn sync_parts(&mut self, cx: &mut Context<Self>) {
        if self.update_parts(cx) {
            self.place_again(cx);
        }
    }

    /// Hands every part what it shows; each renders only when that changed. Add Widget lists a
    /// block or not by whether it has something to show. True when a block started or stopped
    /// having something to show, so the flow must place the parts again; the caller places.
    fn update_parts(&mut self, cx: &mut Context<Self>) -> bool {
        let strings = strings_version(cx);
        let shown = [
            !self.compressed.is_hidden(),
            !self.swap_rows.is_empty(),
            !self.modules.is_hidden(),
        ];
        let mut moved = false;
        for (cell, on) in self.parts.shown.iter().zip(shown) {
            if cell.get() != on {
                cell.set(on);
                moved = true;
            }
        }
        let blocks = [
            self.compressed_shown(strings),
            self.swap_devices_shown(),
            self.modules_shown(strings),
        ];
        for (view, s) in self.block_views.iter().zip(blocks) {
            view.update(cx, |v, cx| v.set_shown(s, cx));
        }
        let switch = PartShown {
            texts: vec![self.words.view_usage.clone(), self.words.view_apps.clone()],
            flags: vec![self.view == Some(MemoryView::Apps)],
            ..PartShown::default()
        };
        self.switch_view.update(cx, |v, cx| v.set_shown(switch, cx));
        let parts = page_parts(shown);
        self.editor.update(cx, |e, _| e.set_page_parts(parts));
        moved
    }

    /// What the Compressed memory block draws.
    fn compressed_shown(&self, strings: Option<u64>) -> PartShown {
        let c = &self.compressed;
        let mut texts = vec![
            self.words.block_compressed.clone(),
            self.words.zswap.clone(),
        ];
        texts.extend(c.names.iter().cloned());
        let mut grids: Vec<DetailsShown> = c
            .zram
            .iter()
            .map(|m| DetailsShown::of(m, strings))
            .collect();
        grids.push(DetailsShown::of(&c.zswap, strings));
        PartShown {
            texts,
            grids,
            flags: vec![c.is_hidden()],
            ..PartShown::default()
        }
    }

    /// What the Swap devices block draws besides the table, which renders its rows itself.
    fn swap_devices_shown(&self) -> PartShown {
        let mut texts = vec![self.words.block_swap.clone()];
        texts.extend(self.words.swap_table.columns.iter().cloned());
        texts.extend(self.words.swap_table.types.iter().cloned());
        PartShown {
            texts,
            numbers: vec![self.swap_rows.len() as u64],
            ..PartShown::default()
        }
    }

    /// What the Memory modules block draws.
    fn modules_shown(&self, strings: Option<u64>) -> PartShown {
        let b = &self.modules;
        let mut texts = vec![self.words.block_modules.clone(), b.slots_line.clone()];
        texts.extend(b.modules.iter().map(|(t, _)| t.clone()));
        let mut grids: Vec<DetailsShown> = b
            .modules
            .iter()
            .map(|(_, m)| DetailsShown::of(m, strings))
            .collect();
        grids.push(DetailsShown::of(b.details_model(), strings));
        let mut flags = b.expanded.clone();
        flags.extend([b.not_reported, b.is_hidden()]);
        let numbers = match b.anim {
            Some(a) => vec![
                1,
                a.module as u64,
                u64::from(a.opening),
                u64::from(a.generation),
            ],
            None => Vec::new(),
        };
        PartShown {
            texts,
            grids,
            flags,
            numbers,
        }
    }

    /// The layout the page shows now: its saved entry, else its built-in default.
    fn shown_layout(&self, cx: &App) -> ContainerLayout {
        page_layout(&self.container_id, cx)
    }

    /// Shows the saved layout (Apply, Reset Page, a menu item) when the page is not in Edit
    /// Widgets. The switch and the header follow a new preset.
    fn sync_layout(&mut self, cx: &mut Context<Self>) {
        if self.editor.read(cx).is_editing() {
            return;
        }
        let layout = self.shown_layout(cx);
        let view = layout_view(&layout);
        self.container.update(cx, |c, cx| c.set_layout(layout, cx));
        if view != self.view {
            // A grid layout draws the switch in the header.
            let header = view.is_none() || self.view.is_none();
            self.view = view;
            self.sync_parts(cx);
            if header {
                self.notify(cx);
            }
        }
    }

    /// Applies the page's settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        self.sync_layout(cx);
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
        if new.range_ns != old.range_ns {
            // The header switch shows the new range.
            self.send_demand(cx);
            self.notify(cx);
        }
    }

    /// A new theme or UI scale: the parts draw again in it.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        for view in [&self.switch_view, &self.bar_view]
            .into_iter()
            .chain(self.block_views.iter())
        {
            view.update(cx, |_, cx| cx.notify());
        }
        self.notify(cx);
    }

    /// New strings: the page's words, Details, the blocks, the swap table and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        let words = self.words.refresh(cx);
        let details = self.details.refresh_strings(cx);
        let compressed = self.compressed.refresh_strings(cx);
        let modules = self.modules.refresh_strings(cx);
        if !(words || details || compressed || modules) {
            return;
        }
        self.push_details(cx);
        self.header.0 = page_words(cx).memory.clone();
        let word = self.header.0.clone();
        self.editor.update(cx, |e, _| e.set_page_name(word));
        if let Some(table) = &self.swap_table {
            let words = self.words.swap_table.clone();
            table.update(cx, |t, cx| {
                t.delegate_mut().set_words(words);
                t.on_new_version(cx);
            });
        }
        self.sync_parts(cx);
        self.notify(cx);
    }

    /// Writes `edit` to the page's flow layout; a page without an entry gets one. Nothing with
    /// a grid layout. The old keys are not written.
    fn edit_flow(&mut self, edit: impl FnOnce(&mut ContainerLayout), cx: &mut Context<Self>) {
        if !cx.has_global::<TlSettings>() {
            return;
        }
        let container = self.container_id.to_string();
        TlSettings::update(cx, |d| {
            let perf = &mut d.settings.performance;
            let mut layout =
                container_layout(perf, &container).unwrap_or_else(|| default_layout(&container));
            if layout.preset.is_none() {
                return;
            }
            edit(&mut layout);
            perf.layouts.set(layout);
        });
        self.sync_settings(cx);
    }

    /// Shows `view` (Usage or By app) with a flow layout: the page takes the view's preset and
    /// its first part; a page without an entry gets one. Nothing with a grid layout, where the
    /// switch asks first ([`Self::pick_view`]). `memory_view` is not written.
    pub fn set_view(&mut self, view: MemoryView, cx: &mut Context<Self>) {
        self.edit_flow(|l| set_memory_preset(l, view), cx);
    }

    /// The Usage / By app switch: [`Self::set_view`] with a flow layout. With a grid layout it
    /// asks "Replace your widgets?" first, then saves the view's flow layout with the built-in
    /// default's widgets.
    fn pick_view(&mut self, view: MemoryView, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.has_global::<TlSettings>() {
            return;
        }
        if self.view.is_some() {
            self.set_view(view, cx);
            return;
        }
        let container = self.container_id.to_string();
        let page = cx.weak_entity();
        let preset = match view {
            MemoryView::Usage => self.words.view_usage.clone(),
            MemoryView::Apps => self.words.view_apps.clone(),
        };
        confirm_preset_page(self.header.0.clone(), preset, window, cx, move |_, cx| {
            let perf = PerformanceSettings {
                memory_view: view,
                ..PerformanceSettings::default()
            };
            let layout = convert_memory_page(&perf, &container);
            TlSettings::update(cx, |d| d.settings.performance.layouts.set(layout));
            let _ = page.update(cx, |p, cx| p.sync_settings(cx));
        });
    }

    /// "Show memory composition": adds or removes the bar in the page's flow layout.
    fn on_toggle_composition(
        &mut self,
        _: &PerfToggleMemoryComposition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let on = !self.show_composition(cx);
        self.edit_flow(|l| set_memory_bar(l, on), cx);
    }

    /// "Show swap usage": adds or removes the Swap chart in the page's flow layout.
    fn on_toggle_swap(
        &mut self,
        _: &PerfToggleMemorySwapGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let on = !self.show_swap(cx);
        self.edit_flow(|l| set_memory_swap(l, on), cx);
    }

    fn on_groups_shown(
        &mut self,
        a: &PerfSetMemoryGroupsShown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::set(
            cx,
            SettingKey::PerformanceMemoryByAppTopN,
            SettingValue::Int(i64::from(a.count)),
        );
        self.sync_settings(cx);
    }

    /// "Edit Widgets…" in the page menu.
    fn on_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        start_page_edit(&self.editor, &self.hub, window, cx);
    }

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset"): the entry goes and
    /// `memory_view`, `memory_swap_graph` and `memory_composition` go back to their defaults.
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
    }

    /// The Usage / By app switch, right-aligned: above the first part of a flow layout, in the
    /// header with a grid layout.
    fn view_switch(&self, cx: &App) -> AnyElement {
        let m = metrics(cx);
        let mut row = div().flex().flex_none();
        for (id, label, view) in [
            ("usage", self.words.view_usage.clone(), MemoryView::Usage),
            ("apps", self.words.view_apps.clone(), MemoryView::Apps),
        ] {
            let weak = self.this.clone();
            let selector = format!("{VIEW_SWITCH_ID}-{id}");
            row = row.child(
                div().tl_selector(move || selector).child(
                    TlButton::new(
                        ElementId::Name(format!("{VIEW_SWITCH_ID}-{id}").into()),
                        TlButtonKind::Segment {
                            on: self.view == Some(view),
                        },
                    )
                    .size(TlButtonSize::Sm)
                    .label(label)
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |page, cx| page.pick_view(view, window, cx))
                            .ok();
                    }),
                ),
            );
        }
        div()
            .tl_selector(|| VIEW_SWITCH_ID.into())
            .flex()
            .flex_row()
            .justify_end()
            .w_full()
            .pb(m.sp[1])
            .child(row)
            .into_any_element()
    }

    /// Creates the "Swap devices" table on the first draw. The Name column then follows the
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

    /// The "Swap devices" block: its heading and the table. `None` without a swap device.
    fn swap_element(&self, cx: &App) -> Option<AnyElement> {
        let table = self
            .swap_table
            .as_ref()
            .filter(|_| !self.swap_rows.is_empty())?;
        let (m, theme) = (metrics(cx), current(cx));
        let md = label_style(m, m.text.md, FontWeight::MEDIUM);
        Some(
            v_flex()
                .gap(m.sp[4])
                .child(heading(self.words.block_swap.clone(), md, theme))
                .child(
                    flat_static_table(table, self.swap_rows.len(), SWAP_TABLE_MAX_ROWS, m)
                        .tl_selector(|| SWAP_TABLE_ID.into()),
                )
                .into_any_element(),
        )
    }

    /// The "Compressed memory" block: one grid per zram device, then zswap. `None` while there
    /// is no zram device and zswap is off.
    fn compressed_element(&self, cx: &App) -> Option<AnyElement> {
        if self.compressed.is_hidden() {
            return None;
        }
        let (m, theme) = (metrics(cx), current(cx));
        let md = label_style(m, m.text.md, FontWeight::MEDIUM);
        let sm = label_style(m, m.text.sm, FontWeight::MEDIUM);
        let mut block =
            v_flex()
                .gap(m.sp[4])
                .child(heading(self.words.block_compressed.clone(), md, theme));
        let grid = |id: ElementId, selector: String, model: &KeyValueModel| {
            div()
                .id(id)
                .tl_selector(move || selector)
                .child(render_details(model, m, cx))
        };
        for (n, (name, model)) in self
            .compressed
            .names
            .iter()
            .zip(&self.compressed.zram)
            .enumerate()
        {
            block = block
                .child(heading(name.clone(), sm.clone(), theme))
                .child(grid(
                    (ZRAM_GRID_ID, n).into(),
                    format!("{ZRAM_GRID_ID}-{n}"),
                    model,
                ));
        }
        block = block
            .child(heading(self.words.zswap.clone(), sm, theme))
            .child(grid(
                ZSWAP_GRID_ID.into(),
                ZSWAP_GRID_ID.to_owned(),
                &self.compressed.zswap,
            ));
        Some(block.into_any_element())
    }

    /// The "Memory modules" block: heading, slots line, then a header row and grid per module.
    /// `None` while the inventory is warming up.
    fn modules_element(&self, cx: &App) -> Option<AnyElement> {
        let block = &self.modules;
        if block.is_hidden() {
            return None;
        }
        let m = *metrics(cx);
        let theme = current(cx);
        let md = label_style(&m, m.text.md, FontWeight::MEDIUM);
        let sm = label_style(&m, m.text.sm, FontWeight::MEDIUM);
        let mut out =
            v_flex()
                .gap(m.sp[4])
                .child(heading(self.words.block_modules.clone(), md, theme));
        if block.not_reported {
            return Some(
                out.child(
                    div()
                        .id(MODULES_NOT_REPORTED_ID)
                        .tl_selector(|| MODULES_NOT_REPORTED_ID.into())
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
            let header = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(m.sp[3])
                .h(px(28. * m.ui_scale))
                .child(
                    div()
                        .tl_selector(move || format!("{MODULE_CHEVRON_ID}-{n}"))
                        .flex_none()
                        .child(chevron),
                )
                .child(heading(title.clone(), sm.clone(), theme));
            out = out.child(header);
            if open {
                out = out.child(
                    div()
                        .id((MODULE_GRID_ID, n))
                        .tl_selector(move || format!("{MODULE_GRID_ID}-{n}"))
                        .child(render_details(model, &m, cx)),
                );
            }
        }
        Some(out.into_any_element())
    }
}

/// The page parts Add Widget lists: Details, the bar, Memory by app, and each block that has
/// something to show (`shown`, in `BLOCKS` order).
fn page_parts(shown: [bool; 3]) -> Vec<FixedWidget> {
    let mut parts = vec![
        FixedWidget::Details,
        FixedWidget::MemoryBar,
        FixedWidget::MemoryByApp,
    ];
    parts.extend(
        BLOCKS
            .into_iter()
            .zip(shown)
            .filter_map(|(kind, on)| on.then_some(kind)),
    );
    parts
}

impl DevicePage for MemoryPage {
    fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        Some(&self.hub)
    }

    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    /// With a grid layout the Usage / By app switch sits in the header; a flow layout draws it
    /// above its first part.
    fn toggles(&self, _: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 2]> {
        if self.view.is_some() {
            return SmallVec::new();
        }
        smallvec![self.view_switch(cx)]
    }

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        // The charts, the bar and Memory by app are widgets of the container.
        div().into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        // The blocks are widgets of the container.
        SmallVec::new()
    }

    /// "Groups shown" while the page shows Memory by app (UI-PRF-060), then "Show memory
    /// composition" and "Show swap usage" with a flow layout, checked from it.
    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let w = &self.words;
        let menu = if self.placed(&FixedWidget::MemoryByApp, cx) {
            let top_n = self.settings.top_n;
            menu.submenu(w.groups_shown.clone(), window, cx, move |sub, _, _| {
                GROUPS_SHOWN.iter().fold(sub, |sub, &n| {
                    sub.radio_item(
                        n.to_string(),
                        n == top_n,
                        Box::new(PerfSetMemoryGroupsShown { count: n }),
                    )
                })
            })
            .separator()
        } else {
            menu
        };
        if self.view.is_none() {
            return menu;
        }
        menu.check_item(
            w.menu_composition.clone(),
            self.show_composition(cx),
            Box::new(PerfToggleMemoryComposition),
        )
        .check_item(
            w.menu_swap.clone(),
            self.show_swap(cx),
            Box::new(PerfToggleMemorySwapGraph),
        )
    }

    /// The Memory copy text (PERF-011, UI-PRF-022): the header word, the Details rows, then the
    /// blocks in 1.3.0 order: Compressed memory, Swap devices, Memory modules, whatever the
    /// layout holds. No module serial is read (D-007).
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

    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        Some(&self.editor)
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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle_composition))
            .on_action(cx.listener(Self::on_toggle_swap))
            .on_action(cx.listener(Self::on_groups_shown))
            .on_action(cx.listener(Self::on_edit_start))
            .on_action(cx.listener(Self::on_reset_page))
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
