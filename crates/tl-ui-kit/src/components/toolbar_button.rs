//! The toolbar button builders page toolbars use (E20-003, UI-SHL-010, R-108, tasklens-zljr5.15).
//!
//! The toolbar row itself is `window/toolbar.rs`. These builders sit in the kit so a page's
//! [`ToolbarContent`](crate::shell::ToolbarContent) can build its buttons without naming the
//! shell.

use gpui_kit::{Action, ElementId, SharedString};

use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::shell::ToolbarModel;

/// The rules every toolbar button shares: 24 × 24, `i_md` icon, not a Tab stop, a tooltip with
/// the Action's key hint, and a `text_secondary` icon while the window is inactive.
fn toolbar_button(
    id: impl Into<ElementId>,
    kind: TlButtonKind,
    icon: TlIcon,
    tooltip: SharedString,
    action: Box<dyn Action>,
    model: &ToolbarModel,
) -> TlButton {
    TlButton::new(id, kind)
        .size(TlButtonSize::Sm)
        .icon(icon)
        .tab_stop(false)
        .tooltip(tooltip)
        .action(action)
        .icon_muted(!model.window_active)
}

/// A toolbar Toggle button: `TlButton::Toggle { on }`, 24 × 24, icon `i_md`, not a Tab stop,
/// tooltip with the Action's key hint, icon colour `text_secondary` when the window is inactive.
pub fn toolbar_toggle(
    id: impl Into<ElementId>,
    icon: TlIcon,
    on: bool,
    tooltip: SharedString,
    action: Box<dyn Action>,
    model: &ToolbarModel,
) -> TlButton {
    toolbar_button(
        id,
        TlButtonKind::Toggle { on },
        icon,
        tooltip,
        action,
        model,
    )
}

/// A toolbar Icon button: `TlButton::Icon`, same sizes and rules as [`toolbar_toggle`]. A
/// disabled button keeps its tooltip and dispatches nothing.
pub fn toolbar_icon(
    id: impl Into<ElementId>,
    icon: TlIcon,
    tooltip: SharedString,
    action: Box<dyn Action>,
    disabled: bool,
    model: &ToolbarModel,
) -> TlButton {
    toolbar_button(id, TlButtonKind::Icon, icon, tooltip, action, model).disabled(disabled)
}
