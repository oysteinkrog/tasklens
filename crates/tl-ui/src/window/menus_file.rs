//! The File menu of the menu bar (E21-A1, UI-SHL-031, APP-012).
//!
//! P1 holds one item, "Exit tasklens", which dispatches `Quit`. `Quit` quits even when the tray
//! settings are on (UI-SHL-113); E20 owns its handler. Its "Ctrl+Q" hint comes from the menu's
//! action context. Run... (P2), Search File Handles... (P3) and Open System Monitor (P4) go above
//! it with a separator, each added by its own bead.

use gpui_kit::{App, Entity, FocusHandle, Window};

use crate::actions::Quit;
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::i18n::tr;

/// Builds the File menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    TlMenu::build(window, cx, return_focus.clone(), |menu, _, cx| {
        menu.item(tr(cx, "shell-menu-file-exit"), Box::new(Quit))
    })
}
