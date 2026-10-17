//! The Help menu of the menu bar (E21-014, UI-SHL-041, APP-024, APP-030, APP-041, APP-053,
//! UI-SYS-096).
//!
//! In P2 the menu reads "Keyboard Shortcuts", "System Information...", a separator, "About
//! tasklens...". "Keyboard Shortcuts" dispatches `OpenShortcuts` and shows the live hint
//! "Ctrl+?". "System Information..." dispatches `ShowSystemInformation`, which the main window
//! handles by selecting the Information tab. "About tasklens..." dispatches `OpenAbout`.
//! "Additional setup..." (P3) and "Documentation" (P4) go between them, added by their own beads.

use gpui_kit::{App, Entity, FocusHandle, Window};

use crate::actions::{OpenShortcuts, ShowSystemInformation};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::i18n::tr;
use crate::window::about::OpenAbout;

/// Builds the Help menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    TlMenu::build(window, cx, return_focus.clone(), |menu, _, cx| {
        menu.item(tr(cx, "shell-menu-help-shortcuts"), Box::new(OpenShortcuts))
            .item(
                tr(cx, "shell-menu-help-sysinfo"),
                Box::new(ShowSystemInformation),
            )
            .separator()
            .item(tr(cx, "shell-menu-help-about"), Box::new(OpenAbout))
    })
}
