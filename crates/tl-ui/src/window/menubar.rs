//! The main window's menu bar (E21-A1, UI-SHL-030, UI-SHL-031, UI-SHL-032, UI-SHL-003 part 4,
//! APP-012, APP-002, R-128).
//!
//! `MenuBar` opens, closes and switches the menu popups, handles the menu keys and gives focus
//! back. tasklens draws its own bar instead of the kit `AppMenuBar`, which reads static
//! `OwnedMenu`s and cannot show live checks, sliders or painted theme rows.
//!
//! A menu is built fresh each time it opens, from the settings and app state at that moment;
//! nothing runs while the menus are closed. [`build_menu`] is the registry: one `match` arm per
//! menu. [`has_menu`] says which menus have a button, so the bar never builds a menu to draw
//! itself. A later bead that adds a menu fills both arms.
//!
//! Keys: `OpenMenu` (F10 and the Alt letters, bound by E20) opens a menu with its first item
//! highlighted. Inside an open menu, Left at the top level and Right on an item without a
//! submenu move to the previous or next menu, wrapping. Esc and the other menu keys are
//! `TlMenu`'s. Alt pressed and released alone does nothing. The buttons are not Tab stops
//! (UI-SHL-121).
//!
//! The buttons are plain elements, not `TlButton`: a ghost `TlButton` is 24 or 28 px high with
//! 8 px padding and a `tab_hover` fill, and the menu buttons need 26 px, 16 px padding and the
//! `menu_hover` fill.

#[cfg(any(test, feature = "test-hooks"))]
use gpui_kit::Hsla;
use gpui_kit::base::POPUP_PRIORITY;
use gpui_kit::base::actions::{SelectLeft, SelectRight};
use gpui_kit::{
    App, AppContext as _, Context, DismissEvent, Entity, FocusHandle, Focusable as _, FontWeight,
    Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Render, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled, Subscription,
    WeakEntity, Window, WindowId, anchored, deferred, div, prelude::FluentBuilder as _, px,
};

use super::titlebar::{MenuBarFactory, MenuRowHeight, bar_height};
use super::{menus_file, menus_view};
use crate::actions::{MenuId, OpenMenu};
use crate::components::menu::TlMenuPopup;
use crate::i18n::{Strings, tr, tr_en};
use crate::text::style::label_style;
use crate::theme::{current, metrics};

/// The menus in bar order.
const ORDER: [MenuId; 5] = [
    MenuId::File,
    MenuId::View,
    MenuId::Settings,
    MenuId::Help,
    MenuId::Debug,
];

/// Button height outside the menu row, × ui_scale. The menu row sets `MenuRowHeight` instead.
const BUTTON_H: f32 = 26.0;
/// Button padding on each side, × ui_scale.
const BUTTON_PAD_X: f32 = 16.0;
/// Separator height, × ui_scale. Its width is one physical pixel.
const SEPARATOR_H: f32 = 16.0;

/// Key context of the menu bar. The open popup sits inside it, so the Left and Right that the
/// popup passes on reach the bar.
pub const KEY_CONTEXT: &str = "TlMenuBar";

fn position(id: MenuId) -> usize {
    match id {
        MenuId::File => 0,
        MenuId::View => 1,
        MenuId::Settings => 2,
        MenuId::Help => 3,
        MenuId::Debug => 4,
    }
}

fn label_key(id: MenuId) -> &'static str {
    match id {
        MenuId::File => "shell-menubar-file",
        MenuId::View => "shell-menubar-view",
        MenuId::Settings => "shell-menubar-settings",
        MenuId::Help => "shell-menubar-help",
        MenuId::Debug => "shell-menubar-debug",
    }
}

/// Whether `id` has a button. True exactly when [`build_menu`] builds a menu with items for it
/// (the "no dead items" rule): in P1 File and View. Called on every frame, so an arm reads only
/// settings or state, never builds anything.
pub fn has_menu(id: MenuId, cx: &App) -> bool {
    let _ = cx;
    match id {
        // View always has "Columns", disabled on a tab without a table.
        MenuId::File | MenuId::View => true,
        // Settings: E21-004. Help: E21-014. Debug: E21-038.
        MenuId::Settings | MenuId::Help | MenuId::Debug => false,
    }
}

/// The menu registry: builds the menu `id` with its item actions going to `return_focus`, or
/// `None` when it has no items now.
fn build_menu(
    id: MenuId,
    window: &mut Window,
    cx: &mut App,
    return_focus: &FocusHandle,
) -> Option<Entity<TlMenuPopup>> {
    match id {
        MenuId::File => Some(menus_file::build(window, cx, return_focus)),
        MenuId::View => Some(menus_view::build(window, cx, return_focus)),
        MenuId::Settings | MenuId::Help | MenuId::Debug => None,
    }
}

/// The menu labels, translated once per strings version (R-132).
struct MenuWords {
    version: Option<u64>,
    labels: [SharedString; 5],
}

impl MenuWords {
    fn new(cx: &App) -> Self {
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        let labels = match version {
            Some(_) => ORDER.map(|id| tr(cx, label_key(id))),
            None => ORDER.map(|id| tr_en(label_key(id), &[])),
        };
        Self { version, labels }
    }
}

/// The menu bar of each window that has one, so `OpenMenu` finds the bar of the active window.
#[derive(Default)]
struct MenuBars(Vec<(WindowId, WeakEntity<MenuBar>)>);

impl Global for MenuBars {}

/// The menu bar in `window`, if it has one.
pub fn menu_bar_for(window: WindowId, cx: &App) -> Option<Entity<MenuBar>> {
    cx.try_global::<MenuBars>()?
        .0
        .iter()
        .find(|(w, _)| *w == window)
        .and_then(|(_, bar)| bar.upgrade())
}

/// Installs the menu bar into the title bar slot and registers the `OpenMenu` handler. Runs once,
/// before the main window opens.
pub fn init(cx: &mut App) {
    cx.set_global(MenuBarFactory(|window, cx| new(window, cx).into()));
    cx.on_action(|action: &OpenMenu, cx| {
        let id = action.0;
        let Some(handle) = cx.active_window() else {
            return;
        };
        let Some(bar) = menu_bar_for(handle.window_id(), cx) else {
            return;
        };
        // The window is busy dispatching this action, so open the menu right after.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                bar.update(cx, |bar, cx| bar.open_menu(id, true, window, cx));
            });
        });
    });
}

/// Creates the menu bar of `window`.
pub fn new(window: &mut Window, cx: &mut App) -> Entity<MenuBar> {
    let bar = cx.new(|cx| MenuBar::new(window, cx));
    let window_id = window.window_handle().window_id();
    let bars = cx.default_global::<MenuBars>();
    bars.0
        .retain(|(w, bar)| *w != window_id && bar.upgrade().is_some());
    bars.0.push((window_id, bar.downgrade()));
    bar
}

/// The menu bar of one main window.
pub struct MenuBar {
    open: Option<MenuId>,
    popup: Option<Entity<TlMenuPopup>>,
    /// The element focused before the first menu opened. Items run there and focus goes back
    /// there when the last menu closes.
    return_focus: Option<FocusHandle>,
    opened_by_key: bool,
    /// The action context when nothing was focused before the menu opened.
    fallback_focus: FocusHandle,
    words: MenuWords,
    dismiss: Option<Subscription>,
    _activation: Subscription,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn_label_color: Option<Hsla>,
}

impl MenuBar {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let activation = cx.observe_window_activation(window, |_, _, cx| cx.notify());
        Self {
            open: None,
            popup: None,
            return_focus: None,
            opened_by_key: false,
            fallback_focus: cx.focus_handle(),
            words: MenuWords::new(cx),
            dismiss: None,
            _activation: activation,
            #[cfg(any(test, feature = "test-hooks"))]
            drawn_label_color: None,
        }
    }

    /// The open menu, if any.
    pub fn open_menu_id(&self) -> Option<MenuId> {
        self.open
    }

    /// The open menu's popup, if any.
    pub fn popup(&self) -> Option<&Entity<TlMenuPopup>> {
        self.popup.as_ref()
    }

    /// Whether the open menu was opened from the keyboard.
    pub fn opened_by_key(&self) -> bool {
        self.opened_by_key
    }

    /// The label colour of the last frame.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn_label_color(&self) -> Option<Hsla> {
        self.drawn_label_color
    }

    /// Opens menu `id` in place of the open one. From the keyboard its first enabled item is
    /// highlighted. A menu that is absent or has no items does nothing.
    pub fn open_menu(
        &mut self,
        id: MenuId,
        by_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !has_menu(id, cx) {
            return;
        }
        if self.open == Some(id) {
            if by_key && let Some(popup) = self.popup.clone() {
                self.opened_by_key = true;
                popup.update(cx, |popup, cx| popup.highlight_first(window, cx));
            }
            return;
        }
        if self.open.is_none() {
            self.return_focus = window.focused(cx);
        }
        let context = self
            .return_focus
            .clone()
            .unwrap_or_else(|| self.fallback_focus.clone());
        self.dismiss = None;
        self.popup = None;
        let Some(popup) = build_menu(id, window, cx, &context) else {
            self.close(window, cx);
            return;
        };
        self.dismiss = Some(cx.subscribe_in(
            &popup,
            window,
            |this, popup, _: &DismissEvent, window, cx| this.on_dismiss(popup, window, cx),
        ));
        popup.focus_handle(cx).focus(window, cx);
        if by_key {
            popup.update(cx, |popup, cx| popup.highlight_first(window, cx));
        }
        self.open = Some(id);
        self.popup = Some(popup);
        self.opened_by_key = by_key;
        cx.notify();
    }

    /// Closes the open menu without running an item and gives focus back.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss = None;
        self.popup = None;
        self.open = None;
        self.opened_by_key = false;
        match self.return_focus.take() {
            Some(focus) => focus.focus(window, cx),
            None => window.blur(cx),
        }
        cx.notify();
    }

    /// The popup closed itself (Esc, an item ran, a click outside). It has already given focus
    /// back, unless an item moved focus elsewhere.
    fn on_dismiss(
        &mut self,
        popup: &Entity<TlMenuPopup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.popup.as_ref() != Some(popup) {
            return;
        }
        self.dismiss = None;
        self.popup = None;
        self.open = None;
        self.opened_by_key = false;
        if self.return_focus.take().is_none() && self.fallback_focus.is_focused(window) {
            window.blur(cx);
        }
        cx.notify();
    }

    fn toggle(&mut self, id: MenuId, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == Some(id) {
            self.close(window, cx);
        } else {
            self.open_menu(id, false, window, cx);
        }
    }

    fn on_button_hover(
        &mut self,
        id: MenuId,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered && self.open.is_some_and(|open| open != id) {
            self.open_menu(id, false, window, cx);
        }
    }

    /// Moves to the next (`forward`) or previous present menu, wrapping.
    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open else {
            return;
        };
        let start = position(open);
        let n = ORDER.len();
        let next = (1..n)
            .map(|k| {
                let ix = if forward {
                    (start + k) % n
                } else {
                    (start + n - k) % n
                };
                ORDER[ix]
            })
            .find(|&id| has_menu(id, cx));
        if let Some(id) = next {
            self.open_menu(id, true, window, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }

    /// One menu button with the open popup below it. `drop_y` is the bar bottom measured from
    /// the button top, so the popup's top edge meets the title bar's bottom edge.
    fn button(
        &self,
        id: MenuId,
        h: Pixels,
        drop_y: Pixels,
        pad_x: Pixels,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui_kit::Div> {
        let t = current(cx);
        let hover = t.menu_hover;
        let radius = metrics(cx).radius_small;
        let open = self.open == Some(id);
        let label = self.words.labels[position(id)].clone();
        let stop = |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            window.prevent_default();
        };
        div()
            .id(("TlMenuBarButton", position(id)))
            .debug_selector(move || format!("TlMenuBarButton-{}", label_key(id)))
            .role(Role::MenuItem)
            .aria_label(label.clone())
            .aria_expanded(open)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(h)
            .px(pad_x)
            .rounded(radius)
            .when(open, |d| d.bg(hover))
            .hover(move |s| s.bg(hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    this.toggle(id, window, cx);
                }),
            )
            .on_mouse_down(MouseButton::Right, stop)
            .on_mouse_down(MouseButton::Middle, stop)
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.on_button_hover(id, *hovered, window, cx)
            }))
            .child(label)
            .when_some(self.popup.clone().filter(|_| open), |d, popup| {
                d.child(
                    div().absolute().left_0().top(drop_y).child(
                        deferred(
                            anchored()
                                .anchor(gpui_kit::Anchor::TopLeft)
                                .snap_to_window_with_margin(px(8.))
                                .child(popup),
                        )
                        .with_priority(POPUP_PRIORITY),
                    ),
                )
            })
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        if version != self.words.version {
            self.words = MenuWords::new(cx);
        }
        let m = *metrics(cx);
        let s = m.ui_scale;
        let t = current(cx);
        let fg = if window.is_window_active() {
            t.text_primary
        } else {
            t.text_secondary
        };
        let divider = t.divider;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.drawn_label_color = Some(fg);
        }
        let h = cx
            .try_global::<MenuRowHeight>()
            .map_or(px(BUTTON_H * s), |r| r.0);
        let bar_h = bar_height(window, cx);
        let drop_y = (bar_h + h) / 2.;
        let pad_x = px(BUTTON_PAD_X * s);
        let separator_w = px(1. / window.scale_factor());

        let mut bar = div()
            .id("TlMenuBar")
            .debug_selector(|| "TlMenuBar".into())
            .role(Role::MenuBar)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(bar_h)
            .text_color(fg)
            // The fallback action context: zero size, so a click never focuses it.
            .child(div().size_0().track_focus(&self.fallback_focus));
        *bar.text_style() = label_style(&m, m.text.md, FontWeight::NORMAL);
        for id in ORDER {
            if !has_menu(id, cx) {
                continue;
            }
            bar = bar.child(self.button(id, h, drop_y, pad_x, cx)).child(
                div()
                    .flex_none()
                    .w(separator_w)
                    .h(px(SEPARATOR_H * s))
                    .bg(divider),
            );
        }
        bar
    }
}
