//! Performance tab menus (docs/ui/performance.md UI-PRF-014, UI-PRF-015; PERF-002, PERF-007,
//! PERF-010, PERF-011, PERF-016): the card menu of a sidebar card, and the page menu of a device
//! page, whose common part a summary tile also opens.

use std::sync::Arc;

use gpui_kit::{App, Global, SharedString, Window};

use super::actions::{
    PerfAddTrayGraph, PerfCopyDeviceInfo, PerfCopyValue, PerfEnterEditMode, PerfJumpToType,
    PerfSetGraphRange, PerfSetPageVisible, PerfToggleGpuEngine, PerfToggleSummaryMode,
};
use super::page::DevicePage;
use super::{DeviceKey, DeviceKind};
use crate::components::menu::TlMenu;
use crate::i18n::tr;

/// The card menu of `key`. `visible_count` is the number of visible cards, `hidden` whether
/// `key` is hidden (only edit mode shows hidden cards). `tray_available` is `None` before P3,
/// which leaves the tray item out.
pub fn card_menu(
    menu: TlMenu,
    key: &DeviceKey,
    visible_count: usize,
    hidden: bool,
    edit_mode: bool,
    tray_available: Option<bool>,
    cx: &App,
) -> TlMenu {
    let mut menu = menu.item(
        tr(cx, "perf-menu-select-pages"),
        Box::new(PerfEnterEditMode),
    );
    let hide = tr(cx, "perf-menu-hide-page");
    menu = if visible_count <= 1 {
        menu.item_disabled(hide, Some(tr(cx, "perf-toast-last-page")))
    } else {
        menu.item(
            hide,
            Box::new(PerfSetPageVisible {
                key: key.clone(),
                visible: false,
            }),
        )
    };
    if edit_mode && hidden {
        menu = menu.item(
            tr(cx, "perf-menu-show-page"),
            Box::new(PerfSetPageVisible {
                key: key.clone(),
                visible: true,
            }),
        );
    }
    menu = menu.separator();
    tray_item(menu, key, tray_available, cx)
}

/// "Add tray icon graph": enabled, disabled with a tooltip, or left out (PERF-088).
fn tray_item(menu: TlMenu, key: &DeviceKey, tray: Option<bool>, cx: &App) -> TlMenu {
    // Overview, fan and battery cards have no tray source (wox.2).
    let tray = tray.filter(|_| crate::tray_bridge::tray_source_of_card(key).is_some());
    match tray {
        Some(true) => menu.item(
            tr(cx, "perf-menu-add-tray"),
            Box::new(PerfAddTrayGraph {
                source: key.clone(),
            }),
        ),
        Some(false) => menu.item_disabled(
            tr(cx, "perf-menu-add-tray"),
            Some(tr(cx, "perf-tip-no-tray")),
        ),
        None => menu,
    }
}

/// What the page menu needs besides the page (UI-PRF-015).
pub struct PageMenuCtx<'a> {
    /// `performance.graph_range_s`.
    pub range_s: u16,
    pub summary_on: bool,
    /// The 5 and 10 minute ranges.
    pub ranges_long_enabled: bool,
    /// Device kinds for the "View >" submenu in [`VIEW_KINDS`] order, with whether a device of
    /// that kind is visible. `None` leaves the submenu out (a summary tile's menu).
    pub view_types: Option<&'a [(DeviceKind, bool)]>,
    /// The right-click landed on a Details value: "Copy value" comes first (UI-PRF-020).
    pub clicked_value: bool,
    /// The tray item after "View": `None` leaves it out, `Some(false)` disables it.
    pub tray_available: Option<bool>,
    /// The right-click landed on a GPU engine cell: "Hide this engine graph" comes first. The
    /// value is the engine's `performance.hidden_gpu_engines` key.
    pub cell_engine: Option<Arc<str>>,
}

/// The GPU engine cell under the last right-click. The Performance tab clears it when a right
/// press starts and takes it once the press is handled, like `ClickedValue`.
#[derive(Default)]
pub struct ClickedEngine(pub Option<Arc<str>>);

impl Global for ClickedEngine {}

/// The time range items: seconds and string key.
const RANGES: [(u16, &str); 4] = [
    (30, "perf-range-30s"),
    (60, "perf-range-1m"),
    (300, "perf-range-5m"),
    (600, "perf-range-10m"),
];

/// The "View >" items in order (UI-PRF-015 item 7): the device kind and the `en-US.ftl` key of
/// its type name, the group names of edit mode.
pub const VIEW_KINDS: [(DeviceKind, &str); 7] = [
    (DeviceKind::Cpu, "perf-group-processor"),
    (DeviceKind::Memory, "perf-group-memory"),
    (DeviceKind::Disk, "perf-group-disks"),
    (DeviceKind::Gpu, "perf-group-gpus"),
    (DeviceKind::Net, "perf-group-nets"),
    (DeviceKind::Fan, "perf-group-fans"),
    (DeviceKind::Battery, "perf-group-batteries"),
];

/// The page menu of `page`: "Copy value" on a Details value, the common items, then the page's
/// own items, then "View >" and "Add tray icon graph".
pub fn page_menu(
    menu: TlMenu,
    page: &dyn DevicePage,
    ctx: &PageMenuCtx,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let menu = if ctx.clicked_value {
        menu.item(tr(cx, "perf-menu-copy-value"), Box::new(PerfCopyValue))
    } else {
        menu
    };
    let menu = match &ctx.cell_engine {
        Some(engine) => menu.item(
            tr(cx, "perf-gpu-menu-hide-engine"),
            Box::new(PerfToggleGpuEngine {
                key: page.key().clone(),
                engine: engine.clone(),
            }),
        ),
        None => menu,
    };
    let menu = page_menu_common(menu, ctx, window, cx).separator();
    // `TlMenu` drops a doubled or trailing separator, so a page with no items leaves one.
    let menu = page.menu_items(menu, window, cx).separator();
    let menu = match ctx.view_types {
        Some(types) => view_menu(menu, types, window, cx),
        None => menu,
    };
    tray_item(menu, page.key(), ctx.tray_available, cx)
}

/// "View >": one item per type that jumps to its first visible device; a type with no visible
/// device is disabled (PERF-007).
fn view_menu(
    menu: TlMenu,
    types: &[(DeviceKind, bool)],
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let items: Vec<(DeviceKind, SharedString, bool)> = VIEW_KINDS
        .iter()
        .map(|&(kind, key)| {
            let visible = types.iter().any(|&(k, v)| k == kind && v);
            (kind, tr(cx, key), visible)
        })
        .collect();
    menu.submenu(
        tr(cx, "perf-menu-view"),
        window,
        cx,
        move |mut menu, _, _| {
            for (kind, word, visible) in &items {
                menu = if *visible {
                    menu.item(word.clone(), Box::new(PerfJumpToType { kind: *kind }))
                } else {
                    menu.item_disabled(word.clone(), None)
                };
            }
            menu
        },
    )
}

/// Copy, Graph Summary View and Time range (UI-PRF-015 items 1 to 3). A
/// summary tile's menu is these alone: no device page is drawn in summary mode, so a page's
/// own items would reach no handler.
pub fn page_menu_common(
    menu: TlMenu,
    ctx: &PageMenuCtx,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let (range_s, long) = (ctx.range_s, ctx.ranges_long_enabled);
    let later = tr(cx, "perf-later-version");
    let words: Vec<SharedString> = RANGES.iter().map(|(_, k)| tr(cx, k)).collect();
    menu.item(tr(cx, "perf-menu-copy"), Box::new(PerfCopyDeviceInfo))
        .check_item(
            tr(cx, "perf-menu-summary"),
            ctx.summary_on,
            Box::new(PerfToggleSummaryMode),
        )
        .submenu(
            tr(cx, "perf-menu-time-range"),
            window,
            cx,
            move |mut menu, _, _| {
                for (&(secs, _), word) in RANGES.iter().zip(&words) {
                    menu = if secs > 60 && !long {
                        menu.item_disabled(word.clone(), Some(later.clone()))
                    } else {
                        menu.radio_item(
                            word.clone(),
                            secs == range_s,
                            Box::new(PerfSetGraphRange { secs }),
                        )
                    };
                }
                menu
            },
        )
}
