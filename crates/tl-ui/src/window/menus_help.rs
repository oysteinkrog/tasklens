//! The Help menu of the menu bar (E21-014, UI-SHL-041, APP-024, APP-030, APP-041, APP-053,
//! UI-SYS-096).
//!
//! The menu reads "Keyboard Shortcuts", "System Information...", "Additional setup..." (E21-034),
//! a separator, "About tasklens...". "Keyboard Shortcuts" dispatches `OpenShortcuts` and shows the live hint
//! "Ctrl+?". "System Information..." dispatches `ShowSystemInformation`, which the main window
//! handles by selecting the Information tab. "About tasklens..." dispatches `OpenAbout`.
//! "Additional setup..." dispatches `OpenAdditionalSetup`. "Documentation" (P4) follows
//! "Additional setup..." and dispatches `OpenDocumentation`, which opens the installed README.

use gpui_kit::{App, Entity, FocusHandle, Window};

use crate::action_tracker::ToastTexts;
use crate::actions::{OpenAdditionalSetup, OpenShortcuts, ShowSystemInformation};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::i18n::tr;
use crate::window::about::{OpenAbout, send_open_uri};

gpui_kit::actions!(
    tasklens,
    [
        /// Help > Documentation: opens the installed README.
        OpenDocumentation
    ]
);

/// Where the package installs the README.
const DOCUMENTATION_URI: &str = "file:///usr/share/doc/tasklens/README.md";

/// Registers the Help menu actions.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenDocumentation, cx| open_documentation(cx));
}

/// Opens the README. A failure shows a toast in the active window.
fn open_documentation(cx: &mut App) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            let texts = ToastTexts::new("", tr(cx, "shell-toast-docs-failed"));
            send_open_uri(DOCUMENTATION_URI.into(), texts, window, cx);
        });
    });
}

/// Builds the Help menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    TlMenu::build(window, cx, return_focus.clone(), |menu, _, cx| {
        menu.item(tr(cx, "shell-menu-help-shortcuts"), Box::new(OpenShortcuts))
            .item(
                tr(cx, "shell-menu-help-sysinfo"),
                Box::new(ShowSystemInformation),
            )
            .item(
                tr(cx, "shell-menu-help-setup"),
                Box::new(OpenAdditionalSetup { focus_row: None }),
            )
            .item(tr(cx, "shell-menu-help-docs"), Box::new(OpenDocumentation))
            .separator()
            .item(tr(cx, "shell-menu-help-about"), Box::new(OpenAbout))
    })
}
