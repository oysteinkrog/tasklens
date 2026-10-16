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
//! The slider rows of the menus (View > Appearance, E21-009) use the slider states in
//! [`MenuSliders`], made once per bar. A drag previews the value in memory; the release, or a
//! key press on a highlighted slider row, saves it once.
//!
//! The buttons are plain elements, not `TlButton`: a ghost `TlButton` is 24 or 28 px high with
//! 8 px padding and a `tab_hover` fill, and the menu buttons need 26 px, 16 px padding and the
//! `menu_hover` fill.
//!
//! [`graph_color_menu`] (E21-032, UI-SHL-085, APP-039) is the shared Graph Color submenu: the
//! minigraph card menu and any in-app graph menu fill a submenu with it.

use fluent_bundle::FluentValue;
#[cfg(any(test, feature = "test-hooks"))]
use gpui_kit::Hsla;
use gpui_kit::base::POPUP_PRIORITY;
use gpui_kit::base::actions::{SelectLeft, SelectRight};
use gpui_kit::base::slider::SliderState;
use gpui_kit::{
    App, AppContext as _, Context, DismissEvent, Entity, FocusHandle, Focusable as _, FontWeight,
    Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Render, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled, Subscription,
    WeakEntity, Window, WindowId, anchored, deferred, div, prelude::FluentBuilder as _, px,
};

use tl_model::settings::Settings;

use super::main::{UI_SCALE_TOAST, push_main_toast};
use super::titlebar::{MenuBarFactory, MenuRowHeight, bar_height};
use super::{menus_debug, menus_file, menus_settings, menus_view};
use crate::actions::{MenuId, OpenMenu};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::components::slider::{TlSliderEvent, set_from_elsewhere};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::pages::minigraph::GraphColor;
use crate::settings_global::TlSettings;
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
/// (the "no dead items" rule): in P1 File, View, Settings and, in debug mode, Debug. Called on every frame, so an arm
/// reads only settings or state, never builds anything.
pub fn has_menu(id: MenuId, cx: &App) -> bool {
    match id {
        // View always has "Columns", disabled on a tab without a table.
        MenuId::File | MenuId::View | MenuId::Settings => true,
        // Help: E21-014.
        MenuId::Help => false,
        MenuId::Debug => menus_debug::debug_enabled(cx),
    }
}

/// The menu registry: builds the menu `id` with its item actions going to `return_focus`, or
/// `None` when it has no items now. Slider rows use the bar's `sliders`.
fn build_menu(
    id: MenuId,
    window: &mut Window,
    cx: &mut App,
    return_focus: &FocusHandle,
    sliders: &MenuSliders,
) -> Option<Entity<TlMenuPopup>> {
    match id {
        MenuId::File => Some(menus_file::build(window, cx, return_focus)),
        MenuId::View => Some(menus_view::build(window, cx, return_focus, sliders)),
        MenuId::Settings => Some(menus_settings::build(window, cx, return_focus, sliders)),
        MenuId::Help => None,
        MenuId::Debug => menus_debug::build(window, cx, return_focus),
    }
}

/// One menu slider's setting: the range and step, and how to read and write the value.
pub(crate) struct SliderSetting {
    /// Min, max and step, as `TlMenu::slider_row` takes them.
    pub range: (f32, f32, f32),
    get: fn(&Settings) -> f32,
    set: fn(&mut Settings, f32),
}

/// `appearance.ui_scale_pct`, 75..=200 step 5 (APP-040).
pub(crate) static UI_SCALE: SliderSetting = SliderSetting {
    range: (75.0, 200.0, 5.0),
    get: |s| f32::from(s.appearance.ui_scale_pct),
    set: |s, v| s.appearance.ui_scale_pct = v.round() as u16,
};
/// `appearance.row_height_pct`, 80..=150 step 5.
pub(crate) static ROW_HEIGHT: SliderSetting = SliderSetting {
    range: (80.0, 150.0, 5.0),
    get: |s| f32::from(s.appearance.row_height_pct),
    set: |s, v| s.appearance.row_height_pct = v.round() as u16,
};
/// `appearance.heat_intensity_pct`, 0..=100 step 5.
pub(crate) static HEAT_INTENSITY: SliderSetting = SliderSetting {
    range: (0.0, 100.0, 5.0),
    get: |s| f32::from(s.appearance.heat_intensity_pct),
    set: |s, v| s.appearance.heat_intensity_pct = v.round() as u8,
};
/// `appearance.shadow_intensity_pct`, 0..=100 step 5.
pub(crate) static SHADOW_INTENSITY: SliderSetting = SliderSetting {
    range: (0.0, 100.0, 5.0),
    get: |s| f32::from(s.appearance.shadow_intensity_pct),
    set: |s, v| s.appearance.shadow_intensity_pct = v.round() as u8,
};
/// `motion.tab_switch_ms`, 0..=500 step 10. Its row is Settings > Tab Switch (E21-010).
pub(crate) static TAB_SWITCH: SliderSetting = SliderSetting {
    range: (0.0, 500.0, 10.0),
    get: |s| f32::from(s.motion.tab_switch_ms),
    set: |s, v| s.motion.tab_switch_ms = v.round() as u16,
};

/// The slider states of the menu slider rows. Made once in `MenuBar::new` and reused by every
/// open of the menus, so no entity is made in render. Each one is subscribed once there.
#[derive(Clone)]
pub struct MenuSliders {
    pub ui_scale: Entity<SliderState>,
    pub row_height: Entity<SliderState>,
    pub heat_intensity: Entity<SliderState>,
    pub shadow_intensity: Entity<SliderState>,
    pub tab_switch: Entity<SliderState>,
}

impl MenuSliders {
    fn new(window: &mut Window, cx: &mut Context<MenuBar>) -> (Self, Vec<Subscription>) {
        let mut subs = Vec::new();
        let mut slider = |setting: &'static SliderSetting, cx: &mut Context<MenuBar>| {
            let (min, max, step) = setting.range;
            let value = setting_value(setting, cx);
            let state = cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(value)
            });
            subs.push(cx.subscribe_in(
                &state,
                window,
                move |_, _, event: &TlSliderEvent, window, cx| {
                    on_slider(setting, *event, window, cx)
                },
            ));
            state
        };
        let sliders = Self {
            ui_scale: slider(&UI_SCALE, cx),
            row_height: slider(&ROW_HEIGHT, cx),
            heat_intensity: slider(&HEAT_INTENSITY, cx),
            shadow_intensity: slider(&SHADOW_INTENSITY, cx),
            tab_switch: slider(&TAB_SWITCH, cx),
        };
        (sliders, subs)
    }

    fn each(&self) -> [(&Entity<SliderState>, &'static SliderSetting); 5] {
        [
            (&self.ui_scale, &UI_SCALE),
            (&self.row_height, &ROW_HEIGHT),
            (&self.heat_intensity, &HEAT_INTENSITY),
            (&self.shadow_intensity, &SHADOW_INTENSITY),
            (&self.tab_switch, &TAB_SWITCH),
        ]
    }

    /// Moves each slider to its setting, which Ctrl+Plus or the Settings window may have
    /// changed while the menus were closed. Emits nothing.
    fn sync(&self, cx: &mut App) {
        for (state, setting) in self.each() {
            let value = setting_value(setting, cx);
            set_from_elsewhere(state, value, cx);
        }
    }
}

/// The setting's value now, or its default before `TlSettings` is installed.
fn setting_value(setting: &SliderSetting, cx: &App) -> f32 {
    if cx.has_global::<TlSettings>() {
        (setting.get)(TlSettings::settings(cx))
    } else {
        (setting.get)(&Settings::default())
    }
}

/// A drag applies the value in memory and the settings observers re-lay out the UI. The release
/// or a key press saves it once. A saved UI scale shows "UI scale changed to N%" in the one
/// keyed scale toast, which Ctrl+Plus and Ctrl+Minus also use.
fn on_slider(setting: &SliderSetting, event: TlSliderEvent, window: &mut Window, cx: &mut App) {
    let set = setting.set;
    match event {
        TlSliderEvent::Preview(v) => TlSettings::preview(cx, |d| set(&mut d.settings, v)),
        TlSliderEvent::Commit(v) => {
            TlSettings::update(cx, |d| set(&mut d.settings, v));
            if std::ptr::eq(setting, &UI_SCALE) {
                let pct = TlSettings::settings(cx).appearance.ui_scale_pct;
                let text = scale_toast_text(pct, cx);
                match ToastQueue::of_window(window, cx) {
                    Some(queue) => queue.update(cx, |queue, cx| {
                        queue.push(ToastKind::Info, text, Some(UI_SCALE_TOAST), None, cx)
                    }),
                    None => push_main_toast(cx, ToastKind::Info, text, Some(UI_SCALE_TOAST), None),
                }
            }
        }
    }
}

pub(crate) fn scale_toast_text(pct: u16, cx: &App) -> SharedString {
    let args = [("pct", FluentValue::from(pct))];
    if cx.has_global::<Strings>() {
        tr_args(cx, "shell-toast-ui-scale", &args)
    } else {
        tr_en("shell-toast-ui-scale", &args)
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
/// Fills a Graph Color submenu: "Accent (theme default)" and the 17 named colours in
/// `GraphColor::ALL` order, each a radio row with a colour square (the Accent row shows the
/// current theme's accent). The mark sits on `current`; a row dispatches `action(color)`.
pub fn graph_color_menu(
    mut menu: TlMenu,
    current: GraphColor,
    action: impl Fn(GraphColor) -> Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    let theme = crate::theme::current(cx).clone();
    for color in GraphColor::ALL {
        let key = format!("shell-graph-color-{}", color.as_str());
        menu = menu.swatch_radio_item(
            tr(cx, &key),
            color.hsla(&theme),
            color == current,
            action(color),
        );
    }
    menu
}

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
    menus_settings::init(cx);
    menus_debug::init(cx);
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
    sliders: MenuSliders,
    dismiss: Option<Subscription>,
    _activation: Subscription,
    _sliders: Vec<Subscription>,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn_label_color: Option<Hsla>,
}

impl MenuBar {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let activation = cx.observe_window_activation(window, |_, _, cx| cx.notify());
        let (sliders, slider_subs) = MenuSliders::new(window, cx);
        Self {
            open: None,
            popup: None,
            return_focus: None,
            opened_by_key: false,
            fallback_focus: cx.focus_handle(),
            words: MenuWords::new(cx),
            sliders,
            dismiss: None,
            _activation: activation,
            _sliders: slider_subs,
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

    /// The menu slider states (tests check they are reused across opens).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn sliders(&self) -> &MenuSliders {
        &self.sliders
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
        self.sliders.sync(cx);
        let sliders = self.sliders.clone();
        let Some(popup) = build_menu(id, window, cx, &context, &sliders) else {
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
