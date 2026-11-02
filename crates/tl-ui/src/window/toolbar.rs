//! The toolbar row inside the title bar (E20-003, UI-SHL-010, UI-SHL-003 part 7, PROC-016).
//!
//! Four slots, left to right: slot 1 the shield (E20-026, P3), slots 2 and 3 the page buttons
//! of the current tab, slot 4 the window picker (E20-027, P3). An absent slot takes no space, so
//! the title bar drag area grows. Each page epic that has page buttons implements
//! [`ToolbarContent`] and registers it in `window/main.rs` (`toolbar_for`).
//!
//! Toolbar buttons are not Tab stops (R-108): every command they run also has a key or a menu
//! path. A mouse-down on any slot stops there, so it never starts a title bar drag (UI-SHL-004).

use gpui_kit::{
    AnyElement, App, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;

pub use crate::components::toolbar_button::{toolbar_icon, toolbar_toggle};
use crate::selector::TlSelector;
pub use crate::shell::{ToolbarContent, ToolbarModel};
use crate::theme::metrics;

/// Toolbar button side, × ui_scale.
pub const TOOLBAR_BUTTON: f32 = 24.0;
/// Gap between toolbar buttons, × ui_scale. With 24 px buttons the pitch is 31 px (measured).
pub const TOOLBAR_GAP: f32 = 7.0;
/// Toolbar icon size: `i_md`.
pub const TOOLBAR_ICON: f32 = 16.0;

/// No page buttons. Used for every tab without a registration.
pub struct DefaultToolbar;

impl ToolbarContent for DefaultToolbar {
    fn buttons(&self, _: &ToolbarModel, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }
}

/// The toolbar row: `[shield?][page..][picker?]`, centred, gap 7 × ui_scale.
#[derive(IntoElement)]
pub struct Toolbar {
    model: ToolbarModel,
    page: SmallVec<[AnyElement; 2]>,
    shield: Option<AnyElement>,
    picker: Option<AnyElement>,
}

impl Toolbar {
    pub fn new(model: ToolbarModel, page: SmallVec<[AnyElement; 2]>) -> Self {
        debug_assert!(page.len() <= 2, "at most 2 page buttons");
        Self {
            model,
            page,
            shield: None,
            picker: None,
        }
    }

    /// Slot 1, set by E20-026.
    pub fn shield(mut self, el: Option<AnyElement>) -> Self {
        self.shield = el;
        self
    }

    /// Slot 4, set by E20-027.
    pub fn picker(mut self, el: Option<AnyElement>) -> Self {
        self.picker = el;
        self
    }

    pub fn model(&self) -> &ToolbarModel {
        &self.model
    }
}

/// Wraps one slot so a mouse-down on it never reaches the title bar drag handler.
fn slot(el: AnyElement) -> impl IntoElement {
    div()
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            cx.stop_propagation();
            window.prevent_default();
        })
        .child(el)
}

impl RenderOnce for Toolbar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let s = metrics(cx).ui_scale;
        div()
            .tl_selector(|| "Toolbar".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(TOOLBAR_BUTTON * s))
            .gap(px(TOOLBAR_GAP * s))
            .when_some(self.shield, |row, el| row.child(slot(el)))
            .children(self.page.into_iter().map(slot))
            .when_some(self.picker, |row, el| row.child(slot(el)))
    }
}
