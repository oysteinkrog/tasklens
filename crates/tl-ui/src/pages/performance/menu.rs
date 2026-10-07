//! Performance tab menus (docs/ui/performance.md UI-PRF-014; PERF-002). Part 1: the card menu
//! of a sidebar card, also opened by a right-click on a summary tile. The page menu (E24-011,
//! UI-PRF-015) is a later bead.

use gpui_kit::App;

use super::DeviceKey;
use super::actions::{PerfAddTrayGraph, PerfEnterEditMode, PerfSetPageVisible};
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
    match tray_available {
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
