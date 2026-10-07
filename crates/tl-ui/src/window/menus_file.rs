//! The File menu of the menu bar (E21-A1, UI-SHL-031, UI-SHL-032, APP-012).
//!
//! From P2 the menu reads "Run...", a separator, "Exit tasklens". "Run..." dispatches
//! `OpenRunDialog`; E23 handles it by opening or focusing "tasklens Run". "Exit tasklens"
//! dispatches `Quit`, which quits even when the tray settings are on (UI-SHL-113); E20 owns its
//! handler. Shortcut hints ("Ctrl+N", "Ctrl+Q") come from the live bindings in the menu's action
//! context. Search File Handles... (P3) and Open System Monitor (P4) go after "Run...", each
//! added by its own bead; the separator above "Exit tasklens" is dropped when nothing is above
//! it.

use gpui_kit::{App, Entity, FocusHandle, Window};

use crate::actions::{OpenRunDialog, Quit};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::i18n::tr;

/// Builds the File menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    TlMenu::build(window, cx, return_focus.clone(), |menu, _, cx| {
        menu.item(tr(cx, "shell-menu-file-run"), Box::new(OpenRunDialog))
            .separator()
            .item(tr(cx, "shell-menu-file-exit"), Box::new(Quit))
    })
}
