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
    Action, AnyElement, App, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::health::HelperState;
use tl_model::table::TableMode;

use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::theme::metrics;

/// Toolbar button side, × ui_scale.
pub const TOOLBAR_BUTTON: f32 = 24.0;
/// Gap between toolbar buttons, × ui_scale. With 24 px buttons the pitch is 31 px (measured).
pub const TOOLBAR_GAP: f32 = 7.0;
/// Toolbar icon size: `i_md`.
pub const TOOLBAR_ICON: f32 = 16.0;

/// What the toolbar needs to know to draw its buttons. The title bar keeps it current.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolbarModel {
    /// The open tab.
    pub tab: Tab,
    /// `UiState.processes.history_mode`.
    pub history_mode: bool,
    /// `settings.processes.hide_system`.
    pub hide_system: bool,
    /// `settings.processes.mode`.
    pub mode: TableMode,
    /// `HealthSnapshot.helper`. `Missing` before P3.
    pub helper: HelperState,
    /// `/usr/lib/tasklens/tl-helper` exists. Checked once at startup by E20-026; false until then.
    pub helper_installed: bool,
    /// `Snapshot.windows.header.validity != Unsupported`.
    pub picker_available: bool,
    /// Window picking mode is on.
    pub picking: bool,
    /// The main window is the active window.
    pub window_active: bool,
}

/// The page buttons of one tab. One per page epic that has page buttons. Registered in
/// `window/main.rs` (`toolbar_for`).
pub trait ToolbarContent: 'static {
    /// Page buttons for slots 2 and 3, left to right, at most 2. Build them with
    /// [`toolbar_toggle`] and [`toolbar_icon`].
    fn buttons(
        &self,
        model: &ToolbarModel,
        window: &mut Window,
        cx: &mut App,
    ) -> SmallVec<[AnyElement; 2]>;
}

/// No page buttons. Used for every tab without a registration.
pub struct DefaultToolbar;

impl ToolbarContent for DefaultToolbar {
    fn buttons(&self, _: &ToolbarModel, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }
}

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
            .debug_selector(|| "Toolbar".into())
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
