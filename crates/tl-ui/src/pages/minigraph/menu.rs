//! The minigraph card menu (UI-PRF-042, PERF-081, R-016). P1 has Hide graph, Time range and
//! Reset minigraph layout; the items of later beads are left out while their flag is false.

use gpui_kit::{App, SharedString, Window};

use super::GraphColor;
use super::actions::{
    MinigraphHide, MinigraphRemove, MinigraphSetColor, MinigraphSetRange, ResetMinigraphLayout,
};
use super::sources::{GraphColumn, MinigraphId};
use crate::components::menu::TlMenu;
use crate::i18n::tr;
use crate::tray_bridge::AddTrayGraph;
use crate::window::main::tray_available;

/// What the card menu needs of its card.
pub struct CardMenuCtx {
    pub id: MinigraphId,
    pub is_process: bool,
    /// The card's `[[minigraphs]] source` string, sent to the tray bridge.
    pub source: SharedString,
    /// The card's column; a system card always sends "cpu".
    pub column: GraphColumn,
    /// The card's range in seconds; its radio is checked.
    pub range_s: u16,
    pub color: GraphColor,
    pub detached: bool,
    /// The 5 and 10 minute ranges (E26-009, P2).
    pub long_ranges: bool,
    /// The Graph color submenu; false before E26-010.
    pub colors: bool,
    /// Detach and Attach to main window; false before E26-015.
    pub detach: bool,
}

/// The time range radios: seconds and string key (the E24-012 words).
const RANGES: [(u16, &str); 4] = [
    (30, "perf-range-30s"),
    (60, "perf-range-1m"),
    (300, "perf-range-5m"),
    (600, "perf-range-10m"),
];

/// The card menu items in UI-PRF-042 order. `TlMenu` drops a doubled or trailing separator, so
/// item 8 shows only when item 6 or 7 exists.
pub fn card_menu(menu: TlMenu, ctx: &CardMenuCtx, window: &mut Window, cx: &mut App) -> TlMenu {
    let id = ctx.id;
    // 1, 2: a system card hides, a process card is removed (E26-011).
    let mut menu = if ctx.is_process {
        menu.item(
            tr(cx, "minigraph-menu-remove"),
            Box::new(MinigraphRemove { id }),
        )
    } else {
        menu.item(
            tr(cx, "minigraph-menu-hide"),
            Box::new(MinigraphHide { id }),
        )
    };
    // 3
    let (range_s, long) = (ctx.range_s, ctx.long_ranges);
    let later = tr(cx, "perf-later-version");
    let words: Vec<SharedString> = RANGES.iter().map(|(_, k)| tr(cx, k)).collect();
    menu = menu.submenu(
        tr(cx, "minigraph-menu-range"),
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
                        Box::new(MinigraphSetRange { id, secs }),
                    )
                };
            }
            menu
        },
    );
    // 4: Graph color, the shell's shared builder (E21-032).
    if ctx.colors {
        let color = ctx.color;
        menu = menu.submenu(
            tr(cx, "minigraph-menu-color"),
            window,
            cx,
            move |menu, _, cx| {
                crate::window::menubar::graph_color_menu(
                    menu,
                    color,
                    move |c| Box::new(MinigraphSetColor { id, color: c }),
                    cx,
                )
            },
        );
    }
    // 5
    menu = menu.separator();
    // 6: Detach or Attach to main window, E26-015 adds it while `detach`.
    // 7: Add tray icon graph; the bridge toasts when the graph already exists.
    menu = if tray_available(cx) {
        let column: SharedString = if ctx.is_process {
            ctx.column.as_str()
        } else {
            "cpu"
        }
        .into();
        menu.item(
            tr(cx, "perf-menu-add-tray"),
            Box::new(AddTrayGraph {
                source: ctx.source.clone(),
                column,
            }),
        )
    } else {
        menu.item_disabled(
            tr(cx, "perf-menu-add-tray"),
            Some(tr(cx, "perf-tip-no-tray")),
        )
    };
    // 8
    menu = menu.separator();
    // 9
    menu.item(
        tr(cx, "minigraph-menu-reset"),
        Box::new(ResetMinigraphLayout),
    )
}
