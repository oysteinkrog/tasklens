//! The trait every Performance device page implements, so the page frame can lay it out. It
//! moved here from `page.rs` (tasklens-zljr5.22) because the Overview page implements it and
//! the trait names the widget editor (ADR-020: both sit in the overview layer).

use std::rc::Rc;

use gpui_kit::{AnyElement, App, Bounds, Entity, FocusHandle, Pixels, SharedString, Window};
use smallvec::SmallVec;

use crate::components::menu::TlMenu;
use crate::hub::SnapshotHub;
use crate::pages::performance::core::DeviceKey;
use crate::pages::performance::core::details::KeyValueModel;
use crate::pages::performance::widget::edit::WidgetEditor;

/// One device page. The page entity owns its charts, Details model and blocks; the frame lays
/// them out. Every method runs inside the page's own render.
pub trait DevicePage: 'static {
    fn key(&self) -> &DeviceKey;
    /// The hub of the page's host. The range switch reads that host's settings from it.
    fn hub(&self) -> Option<&Entity<SnapshotHub>>;
    /// (type word, model text), from `page::header_text`.
    fn header(&self) -> (SharedString, SharedString);
    /// Header buttons, right-aligned. Each is a `TlButton` with a tooltip.
    fn toggles(&self, w: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 2]>;
    fn graphs(&self, w: &mut Window, cx: &mut App) -> AnyElement;
    fn details(&self) -> &KeyValueModel;
    /// Tables, module blocks and links under the Details grid.
    fn blocks(&self, w: &mut Window, cx: &mut App) -> SmallVec<[AnyElement; 3]>;
    /// Page-specific items for UI-PRF-015 slot 5.
    fn menu_items(&self, menu: TlMenu, w: &mut Window, cx: &mut App) -> TlMenu;
    /// Appends the page's copy text (E24-016, E25).
    fn copy_text(&self, out: &mut String);
    /// Header toggles, then block controls (UI-SHL-121).
    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]>;
    /// Takes the link clicks of the Details grid, with the flat row index and the link's bounds
    /// (`TlKeyValueGrid::on_link_click`). `None` keeps the links dispatching their actions.
    fn details_link_click(&self) -> Option<DetailsLinkClick> {
        None
    }
    /// The page's focus handles for Details link values, by flat row index
    /// (`TlKeyValueGrid::link_focus`), so `focus_order` can name them. `None` by default.
    fn details_link_focus(&self) -> Option<Rc<[(usize, FocusHandle)]>> {
        None
    }
    /// Builds the element drawn before a Details value, by flat row index
    /// (`TlKeyValueGrid::value_prefix`). `None` by default.
    fn details_value_prefix(&self) -> Option<DetailsValuePrefix> {
        None
    }
    /// Builds the element drawn after a Details value, by flat row index
    /// (`TlKeyValueGrid::value_suffix`). `None` by default.
    fn details_value_suffix(&self) -> Option<DetailsValuePrefix> {
        None
    }
    /// The editor around the page's widget container, when the page's body is one (ADR-019
    /// phase 3). `None` keeps the 1.3.0 frame.
    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        None
    }
    /// Called with true when the page becomes the shown page and false when it stops being
    /// shown (R-229; E25-010 wires the calls).
    fn set_active(&mut self, active: bool, cx: &mut App) {
        let _ = (active, cx);
    }
}

/// A Details value prefix builder: flat row index, the element and its width.
pub type DetailsValuePrefix = crate::components::kv_grid::ValuePrefix;

/// A Details link click handler: flat row index and the link's bounds.
pub type DetailsLinkClick = Rc<dyn Fn(usize, Bounds<Pixels>, &mut Window, &mut App)>;
