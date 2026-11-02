//! Focus rules of the main window (E20-007, UI-SHL-121, UI-SHL-122, canonical table (b) Esc row,
//! R-043, R-108, APP-058, widgets rule 16).
//!
//! This file is the keyboard focus contract of the main window:
//! - Title bar, toolbar, tabs and caption buttons are never Tab stops (R-108). The Tab order in
//!   the main window is the element tree order of tab stops: filter input, "Include children"
//!   checkbox (when shown), the active page's targets in the page's order, clickable status chip
//!   segments. GPUI wraps from the last to the first. No `tab_index` values are set by the shell.
//! - Menus, popovers and in-window dialogs return focus to the element that had it (kit
//!   behaviour, widgets rule 16). Secondary windows are separate windows; GPUI keeps each
//!   window's last focused element.
//! - The focus ring is drawn on keyboard focus only (`TlFocusRing`, E15).
//!
//! The Esc chain handler that acts on [`esc_outcome`] is E20-011.

use gpui_kit::{App, FocusHandle, Window};

/// Initial focus rule (UI-SHL-121): focus the page's primary target when it has one,
/// else the main window's root focus handle (key context "Global"). Runs when the window opens
/// and after every tab switch.
pub fn focus_initial(
    primary: Option<&FocusHandle>,
    root: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    window.focus(primary.unwrap_or(root), cx);
}

/// What the Esc chain sees after the kit has handled menus, popovers and dialogs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EscState {
    /// Window picking is active (E20-027).
    pub picking: bool,
    /// The shell filter input has focus.
    pub filter_focused: bool,
    /// The active tab's filter text is not empty.
    pub filter_has_text: bool,
    /// `settings.window.close_with_esc`.
    pub close_with_esc: bool,
}

/// What one Esc press does in the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscOutcome {
    /// Dispatch `CancelWindowPick`.
    CancelPick,
    /// The filter's own rule (UI-SHL-024): handled by `FilterEscape`, stop here.
    FilterRule,
    /// Clear the active tab's filter text and focus the table.
    ClearFilterAndFocusTable,
    /// Dispatch `CloseWindow` (close decision of UI-SHL-113).
    CloseWindow,
    /// Esc does nothing.
    Nothing,
}

/// The Esc chain after the kit has handled open menus, popovers and in-window dialogs
/// (steps 1 and 2 never reach this function). First match wins:
/// 3 picking -> CancelPick; 4 filter_focused -> FilterRule; 5 filter_has_text ->
/// ClearFilterAndFocusTable; 6 close_with_esc -> CloseWindow; 7 -> Nothing.
pub fn esc_outcome(s: EscState) -> EscOutcome {
    if s.picking {
        EscOutcome::CancelPick
    } else if s.filter_focused {
        EscOutcome::FilterRule
    } else if s.filter_has_text {
        EscOutcome::ClearFilterAndFocusTable
    } else if s.close_with_esc {
        EscOutcome::CloseWindow
    } else {
        EscOutcome::Nothing
    }
}
