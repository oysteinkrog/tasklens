//! The main window's title bar (E20-004, UI-SHL-003, UI-SHL-004, UI-SHL-005, UI-SHL-007,
//! APP-001, APP-002, APP-008, D-006, R-031, R-108, R-125, U-44).
//!
//! Left to right: padding, app icon, the menu bar slot (E21), the toolbar (E20-003), the drag
//! area and the caption buttons. The bar is a plain flex row with its own drag code, not the kit
//! `TitleBar`: gpui-kit 0.7.0's `TitleBar` always appends its own window controls and has no
//! switch to hide them (U-44).
//!
//! Under server decorations ("Use system title bar", UI-SHL-006) KWin draws the title bar, and
//! the bar becomes a 28 px menu row: the same parts up to the toolbar, then a spacer and the pin,
//! with no drag code and no minimize, maximize or close. The layout follows what GPUI reports
//! (`window.window_decorations()`), not the setting, because a compositor may refuse server
//! decorations.
//!
//! The pin ("Always on top", E20-022, UI-SHL-005, APP-008, R-073) sits left of minimize in the
//! client bar and last in the menu row. It dispatches `ToggleAlwaysOnTop` for the main window and
//! is disabled while the KWin bridge is not up.
//!
//! The bar has no Tab stops (R-108). Every caption command also has a key (Ctrl+W) or a
//! compositor shortcut. Each caption button's tooltip text is its accessible name.

use gpui_kit::accesskit::Role;
use gpui_kit::component::Icon;
use gpui_kit::{
    Action, AnyElement, AnyView, App, Context, Div, Global, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Subscription, Window, div,
    img, prelude::FluentBuilder as _, px, rgb,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::health::HelperState;
use tl_model::table::TableMode;
use tl_model::validity::Validity;

use super::main::WINDOW_TITLE;
use super::registry::WindowKey;
use super::toolbar::{DefaultToolbar, Toolbar, ToolbarContent, ToolbarModel};
use crate::actions::{CloseWindow, MinimizeWindow, ZoomWindow};
use crate::components::icons::{AppIcon, TlIcon};
use crate::components::shield::TlShield;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::window_frame::ToggleAlwaysOnTop;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::tr;
use crate::settings_global::{Section, TlSettings};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Title bar height, × ui_scale.
pub const TITLEBAR_H: f32 = 34.0;
/// Menu row height under server decorations, × ui_scale (UI-SHL-006).
pub const MENU_ROW_H: f32 = 28.0;
/// Main window caption button width, × ui_scale. Secondary windows keep the kit's 34.
pub const CAPTION_W: f32 = 36.0;

/// Left padding, × ui_scale.
const LEFT_PAD: f32 = 12.0;
/// App icon side, × ui_scale.
const APP_ICON: f32 = 16.0;
/// Gap between the app icon and the menu bar slot, × ui_scale.
const GAP_ICON: f32 = 8.0;
/// Gap between the menu bar slot and the toolbar, × ui_scale.
const GAP_MENU: f32 = 4.0;
/// Minimum width of the drag area, × ui_scale.
const DRAG_MIN_W: f32 = 24.0;
/// Menu button height in the menu row, × ui_scale.
const MENU_ROW_BUTTON_H: f32 = 24.0;
/// A pressed caption button shows its hover fill at this opacity.
const PRESSED_OPACITY: f32 = 0.8;
/// Fill alpha of the pin when on, as `TlButton::Toggle` (UI-FND-021).
const PIN_ON_ALPHA: f32 = 0.25;
/// Opacity of the disabled pin (UI-FND-021).
const PIN_DISABLED_OPACITY: f32 = 0.5;

/// Slot for the menu bar. E21-A1 sets it in `menubar::init` before the main window opens.
pub struct MenuBarFactory(pub fn(&mut Window, &mut App) -> AnyView);

impl Global for MenuBarFactory {}

/// The height of the menu bar's buttons, in px with ui_scale applied. Contract for E21-A1: the
/// menu bar reads this Global when it renders and uses 26 × ui_scale when it is absent. The title
/// bar sets it to 24 × ui_scale before the slot renders in the menu row, and removes it in the
/// client bar. It changes only when the decoration kind or the scale changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuRowHeight(pub Pixels);

impl Global for MenuRowHeight {}

/// Sets or removes `MenuRowHeight`. Nothing happens when it already holds `height`, so the
/// Global's observers do not run on every frame.
fn set_menu_row_height(height: Option<Pixels>, cx: &mut App) {
    if cx.try_global::<MenuRowHeight>().map(|h| h.0) == height {
        return;
    }
    match height {
        Some(h) => cx.set_global(MenuRowHeight(h)),
        None => {
            cx.remove_global::<MenuRowHeight>();
        }
    }
}

/// The left button went down on a drag part; the next mouse move starts a window move.
#[derive(Default)]
struct DragState {
    should_move: bool,
}

/// Caption tooltips, made once in `new`.
struct CaptionStrings {
    minimize: SharedString,
    maximize: SharedString,
    restore: SharedString,
    close: SharedString,
    pin_off: SharedString,
    pin_on: SharedString,
    pin_unsupported: SharedString,
}

/// One caption button as last drawn. Tests read it through `TitleBarView::drawn_captions`.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnCaption {
    /// "minimize", "maximize" or "close".
    pub id: &'static str,
    pub icon: TlIcon,
    pub tooltip: SharedString,
    /// The icon colour when not hovered.
    pub color: Hsla,
}

/// The pin as last drawn. Tests read it through `TitleBarView::drawn_pin`.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnPin {
    pub on: bool,
    pub enabled: bool,
    pub icon: TlIcon,
    pub tooltip: SharedString,
}

/// Test stand-in for the platform window. The GPUI test window panics on `start_window_move`,
/// `zoom_window`, `minimize_window` and `show_window_menu`, always reports server decorations
/// and is never maximized. When this Global is installed the shell records those four calls here
/// instead of making them, and reads the overrides below.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
pub struct TitleBarProbe {
    /// Overrides `window.window_decorations()`: true for client decorations.
    pub client: Option<bool>,
    /// Overrides `window.is_maximized()`.
    pub maximized: Option<bool>,
    /// Overrides `window.window_controls()`.
    pub controls: Option<gpui_kit::WindowControls>,
    /// `start_window_move` calls.
    pub moves: u32,
    /// `zoom_window` calls.
    pub zooms: u32,
    /// `minimize_window` calls.
    pub minimizes: u32,
    /// `show_window_menu` positions.
    pub window_menus: Vec<Point<Pixels>>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl Global for TitleBarProbe {}

fn probe_installed(cx: &App) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    return cx.has_global::<TitleBarProbe>();
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = cx;
        false
    }
}

/// True when the window reports client decorations (the client bar), false for server
/// decorations (the menu row).
pub(crate) fn is_client(window: &Window, cx: &App) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(client) = cx.try_global::<TitleBarProbe>().and_then(|p| p.client) {
        return client;
    }
    let _ = cx;
    matches!(
        window.window_decorations(),
        gpui_kit::Decorations::Client { .. }
    )
}

/// The bar height for a decoration kind at ui_scale `s`: 34 × s for client, 28 × s for server.
pub(crate) fn bar_height_of(client: bool, s: f32) -> Pixels {
    px(if client { TITLEBAR_H } else { MENU_ROW_H } * s)
}

/// The bar height the window's reported decorations give: 34 × ui_scale for client, 28 ×
/// ui_scale for server.
pub fn bar_height(window: &Window, cx: &App) -> Pixels {
    bar_height_of(is_client(window, cx), metrics(cx).ui_scale)
}

fn is_maximized(window: &Window, cx: &App) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(maximized) = cx.try_global::<TitleBarProbe>().and_then(|p| p.maximized) {
        return maximized;
    }
    let _ = cx;
    window.is_maximized()
}

fn window_controls(window: &Window, cx: &App) -> gpui_kit::WindowControls {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(controls) = cx.try_global::<TitleBarProbe>().and_then(|p| p.controls) {
        return controls;
    }
    let _ = cx;
    window.window_controls()
}

fn start_move(window: &Window, cx: &mut App) {
    if probe_installed(cx) {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            cx.global_mut::<TitleBarProbe>().moves += 1;
        }
        return;
    }
    window.start_window_move();
}

pub(crate) fn zoom(window: &Window, cx: &mut App) {
    if probe_installed(cx) {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            cx.global_mut::<TitleBarProbe>().zooms += 1;
        }
        return;
    }
    window.zoom_window();
}

fn window_menu(window: &Window, position: Point<Pixels>, cx: &mut App) {
    if probe_installed(cx) {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            cx.global_mut::<TitleBarProbe>().window_menus.push(position);
        }
        return;
    }
    window.show_window_menu(position);
}

/// True when the pin can work: `Snapshot.windows.header.validity` is Valid or Stale (KWin bridge
/// up). False without a hub.
pub fn keep_above_supported(cx: &App) -> bool {
    cx.try_global::<GlobalHub>().is_some_and(|hub| {
        matches!(
            hub.0.read(cx).current().windows.header.validity,
            Validity::Valid | Validity::Stale
        )
    })
}

/// The Minimize caption button's behaviour. E20-A1's `MinimizeWindow` handler calls it.
pub fn minimize_main(window: &mut Window, cx: &mut App) {
    // minimize rule (E31-009): the minimize-to-tray check goes here, before minimize_window
    if probe_installed(cx) {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            cx.global_mut::<TitleBarProbe>().minimizes += 1;
        }
        return;
    }
    window.minimize_window();
}

/// The `window` and `debug` section versions; zero when settings are not loaded.
fn settings_versions(cx: &App) -> (u64, u64) {
    cx.try_global::<TlSettings>().map_or((0, 0), |s| {
        (s.version(Section::Window), s.version(Section::Debug))
    })
}

fn stop(_: &MouseDownEvent, window: &mut Window, cx: &mut App) {
    cx.stop_propagation();
    window.prevent_default();
}

/// A mouse-down of any button on `el` stops there: a left press never drags, and right or middle
/// presses do nothing.
fn no_drag<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, stop)
        .on_mouse_down(MouseButton::Right, stop)
        .on_mouse_down(MouseButton::Middle, stop)
}

/// The drag behaviour of the bar (UI-SHL-004). Only presses that reach the bar itself count:
/// the padding, icon, gaps and drag area. Buttons stop their presses with `no_drag`.
fn drag_handlers(bar: Stateful<Div>, cx: &mut Context<TitleBarView>) -> Stateful<Div> {
    bar.on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
            if ev.click_count == 2 {
                this.drag.should_move = false;
                zoom(window, cx);
            } else {
                this.drag.should_move = true;
            }
        }),
    )
    .on_mouse_up(
        MouseButton::Left,
        cx.listener(|this, _: &MouseUpEvent, _, _| this.drag.should_move = false),
    )
    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, _| {
        this.drag.should_move = false;
    }))
    .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, cx| {
        if this.drag.should_move {
            this.drag.should_move = false;
            start_move(window, cx);
        }
    }))
    .on_mouse_down(MouseButton::Right, |ev: &MouseDownEvent, window, cx| {
        window_menu(window, ev.position, cx);
    })
}

/// The main window's title bar view.
pub struct TitleBarView {
    /// Created once in `new` from `MenuBarFactory`; `None` when the Global is absent.
    menu_bar: Option<AnyView>,
    /// Graph summary mode hides the menus (PERF-010).
    menus_hidden: bool,
    model: ToolbarModel,
    /// The active tab's page buttons.
    content: Box<dyn ToolbarContent>,
    drag: DragState,
    strings: CaptionStrings,
    maximized: bool,
    keep_above: bool,
    settings_versions: (u64, u64),
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn: Vec<DrawnCaption>,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn_pin: Option<DrawnPin>,
}

impl TitleBarView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let menu_bar = cx
            .try_global::<MenuBarFactory>()
            .map(|f| f.0)
            .map(|build| build(window, cx));
        let model = ToolbarModel {
            tab: Tab::Processes,
            history_mode: false,
            hide_system: false,
            mode: TableMode::default(),
            helper: HelperState::Missing,
            helper_installed: false,
            picker_available: false,
            picking: false,
            window_active: window.is_window_active(),
        };
        let strings = CaptionStrings {
            minimize: tr(cx, "shell-titlebar-minimize"),
            maximize: tr(cx, "shell-titlebar-maximize"),
            restore: tr(cx, "shell-titlebar-restore"),
            close: tr(cx, "shell-titlebar-close"),
            pin_off: tr(cx, "fnd-pin-off"),
            pin_on: tr(cx, "fnd-pin-on"),
            pin_unsupported: tr(cx, "fnd-pin-unsupported"),
        };

        let mut subs = SmallVec::new();
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            this.model.window_active = window.is_window_active();
            cx.notify();
        }));
        subs.push(cx.observe_window_bounds(window, |this, window, cx| {
            let maximized = is_maximized(window, cx);
            if maximized != this.maximized {
                this.maximized = maximized;
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<TlSettings>(|this, cx| {
            let versions = settings_versions(cx);
            if versions != this.settings_versions {
                this.settings_versions = versions;
                cx.notify();
            }
        }));
        if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
            subs.push(cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
                if !ev.mask.intersects(ComponentMask::WINDOWS) {
                    return;
                }
                let supported = keep_above_supported(cx);
                if supported != this.keep_above {
                    this.keep_above = supported;
                    cx.notify();
                }
            }));
        }

        Self {
            menu_bar,
            menus_hidden: false,
            model,
            content: Box::new(DefaultToolbar),
            drag: DragState::default(),
            strings,
            maximized: is_maximized(window, cx),
            keep_above: keep_above_supported(cx),
            settings_versions: settings_versions(cx),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            drawn: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            drawn_pin: None,
        }
    }

    /// Sets the toolbar for the active tab. Notifies only when the model (and with it the tab)
    /// changed. The bar keeps `window_active` itself.
    pub fn set_toolbar(
        &mut self,
        mut model: ToolbarModel,
        content: Box<dyn ToolbarContent>,
        cx: &mut Context<Self>,
    ) {
        self.content = content;
        model.window_active = self.model.window_active;
        if model != self.model {
            self.model = model;
            cx.notify();
        }
    }

    /// Updates the toolbar model. Notifies only when it changed.
    pub fn set_model(&mut self, mut model: ToolbarModel, cx: &mut Context<Self>) {
        model.window_active = self.model.window_active;
        if model != self.model {
            self.model = model;
            cx.notify();
        }
    }

    /// Hides or shows the menus. Notifies only when it changed.
    pub fn set_menus_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if self.menus_hidden != hidden {
            self.menus_hidden = hidden;
            cx.notify();
        }
    }

    pub fn model(&self) -> &ToolbarModel {
        &self.model
    }

    /// The caption buttons as last drawn, left to right.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn_captions(&self) -> &[DrawnCaption] {
        &self.drawn
    }

    /// The pin as last drawn; `None` before the first render.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn_pin(&self) -> Option<&DrawnPin> {
        self.drawn_pin.as_ref()
    }

    /// The pin cell, `CAPTION_W` × `h` (UI-SHL-005). On: `pin` in `accent` on an `accent` fill
    /// at 0.25 with a 1 px `accent` border. Off: `pin-off`. Without the KWin bridge it is drawn
    /// at 0.5 opacity with no hover, and a click does nothing. No Tab stop; every mouse-down
    /// stops at it, so it never starts a drag.
    fn pin(&mut self, h: Pixels, theme: &TlTheme, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let s = m.ui_scale;
        let on = cx.has_global::<TlSettings>() && TlSettings::settings(cx).window.always_on_top;
        let enabled = self.keep_above;
        let icon = if on { TlIcon::Pin } else { TlIcon::PinOff };
        let tooltip = match (enabled, on) {
            (false, _) => self.strings.pin_unsupported.clone(),
            (true, true) => self.strings.pin_on.clone(),
            (true, false) => self.strings.pin_off.clone(),
        };
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.drawn_pin = Some(DrawnPin {
                on,
                enabled,
                icon,
                tooltip: tooltip.clone(),
            });
        }
        let fg = if on { theme.accent } else { theme.text_primary };
        let hover = theme.tab_hover;
        let accent = theme.accent;
        let button = no_drag(
            div()
                .id("TitleBarPin")
                .debug_selector(|| "TitleBarPin".into())
                .role(Role::Button)
                .aria_label(tooltip.clone())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(CAPTION_W * s))
                .h(h)
                .text_color(fg)
                .when(on, |b| {
                    b.bg(accent.alpha(PIN_ON_ALPHA))
                        .border_1()
                        .border_color(accent)
                })
                .when(!enabled, |b| b.opacity(PIN_DISABLED_OPACITY))
                .when(enabled, |b| {
                    b.hover(move |st| st.bg(hover))
                        .active(move |st| st.bg(hover.opacity(PRESSED_OPACITY)))
                        .on_click(|_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(
                                Box::new(ToggleAlwaysOnTop {
                                    window: WindowKey::Main,
                                    caption: WINDOW_TITLE.into(),
                                }),
                                cx,
                            );
                        })
                })
                .child(Icon::new(icon).size(m.icon.md)),
        );
        let tip = HoverTooltip::state("TitleBarPin-tooltip", window, cx);
        HoverTooltip::attach(
            &tip,
            button,
            DELAY_CONTROL,
            TlTooltip::text(tooltip).build(),
            window,
            cx,
        )
        .into_any_element()
    }

    /// One caption button: `CAPTION_W` × `TITLEBAR_H`, no Tab stop, a hover tooltip.
    #[allow(clippy::too_many_arguments)]
    fn caption(
        &mut self,
        id: &'static str,
        icon: TlIcon,
        tooltip: SharedString,
        action: fn() -> Box<dyn Action>,
        hover: Hsla,
        hover_fg: Hsla,
        fg: Hsla,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.drawn.push(DrawnCaption {
            id,
            icon,
            tooltip: tooltip.clone(),
            color: fg,
        });
        let s = m.ui_scale;
        let button = no_drag(
            div()
                .id(id)
                .debug_selector(move || format!("TitleBarCaption-{id}"))
                .role(Role::Button)
                .aria_label(tooltip.clone())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(CAPTION_W * s))
                .h(px(TITLEBAR_H * s))
                .text_color(fg)
                .hover(move |st| st.bg(hover).text_color(hover_fg))
                .active(move |st| st.bg(hover.opacity(PRESSED_OPACITY)))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(action(), cx);
                })
                .child(Icon::new(icon).size(m.icon.md)),
        );
        let tip = HoverTooltip::state(format!("TitleBarCaption-{id}-tooltip"), window, cx);
        HoverTooltip::attach(
            &tip,
            button,
            DELAY_CONTROL,
            TlTooltip::text(tooltip).build(),
            window,
            cx,
        )
        .into_any_element()
    }

    fn captions(&mut self, theme: &TlTheme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let m = *metrics(cx);
        let controls = window_controls(window, cx);
        let fg = if window.is_window_active() {
            theme.text_primary
        } else {
            theme.text_secondary
        };
        let hover = theme.tab_hover;
        let white: Hsla = rgb(0xFFFFFF).into();
        let mut row = div().flex().flex_row().flex_none().h_full();
        if controls.minimize {
            let tip = self.strings.minimize.clone();
            row = row.child(self.caption(
                "minimize",
                TlIcon::Minus,
                tip,
                || Box::new(MinimizeWindow),
                hover,
                fg,
                fg,
                &m,
                window,
                cx,
            ));
        }
        if controls.maximize {
            let (icon, tip) = if is_maximized(window, cx) {
                (TlIcon::Copy, self.strings.restore.clone())
            } else {
                (TlIcon::Square, self.strings.maximize.clone())
            };
            row = row.child(self.caption(
                "maximize",
                icon,
                tip,
                || Box::new(ZoomWindow),
                hover,
                fg,
                fg,
                &m,
                window,
                cx,
            ));
        }
        let tip = self.strings.close.clone();
        row.child(self.caption(
            "close",
            TlIcon::X,
            tip,
            || Box::new(CloseWindow),
            theme.status.danger,
            white,
            fg,
            &m,
            window,
            cx,
        ))
    }
}

impl TitleBarView {
    /// The parts both bars share, at height `h`: padding, app icon, menu bar slot and toolbar.
    fn leading(
        &mut self,
        h: Pixels,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        let s = metrics(cx).ui_scale;
        let page = self.content.buttons(&self.model, window, cx);
        // Slot 1 (E20-026): the shield on every tab while the helper is installed.
        let shield = self
            .model
            .helper_installed
            .then(|| TlShield::new(self.model.helper).into_any_element());
        let toolbar = no_drag(
            div()
                .flex_none()
                .child(Toolbar::new(self.model.clone(), page).shield(shield)),
        );
        div()
            .id("TitleBar")
            .debug_selector(|| "TitleBar".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .w_full()
            .h(h)
            .pl(px(LEFT_PAD * s))
            .bg(theme.titlebar)
            .cursor_default()
            .child(
                // Keeps its 16 px when the asset is missing (E15-022).
                div()
                    .debug_selector(|| "TitleBarIcon".into())
                    .flex_none()
                    .size(px(APP_ICON * s))
                    .mr(px(GAP_ICON * s))
                    .child(img(AppIcon::Logo.path_str()).size_full()),
            )
            .when_some(
                self.menu_bar.clone().filter(|_| !self.menus_hidden),
                |bar, menu| {
                    bar.child(no_drag(
                        div()
                            .debug_selector(|| "TitleBarMenu".into())
                            .flex_none()
                            .mr(px(GAP_MENU * s))
                            .child(menu),
                    ))
                },
            )
            .child(toolbar)
    }

    /// The menu row under server decorations (UI-SHL-006): the shared parts, a spacer, then the
    /// pin cell (36 × 28). KWin draws the caption buttons and handles drag,
    /// so the row has no drag, double-click or window-menu handlers.
    fn menu_row(&mut self, theme: &TlTheme, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let s = metrics(cx).ui_scale;
        set_menu_row_height(Some(px(MENU_ROW_BUTTON_H * s)), cx);
        let h = bar_height_of(false, s);
        let pin = self.pin(h, theme, window, cx);
        self.leading(h, theme, window, cx)
            .child(
                div()
                    .debug_selector(|| "TitleBarSpacer".into())
                    .flex_1()
                    .h_full(),
            )
            .child(pin)
    }

    /// The client bar (E20-004): the shared parts, the drag area and the caption buttons.
    fn client_bar(
        &mut self,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let s = metrics(cx).ui_scale;
        set_menu_row_height(None, cx);
        let h = bar_height_of(true, s);
        let bar = self.leading(h, theme, window, cx);
        let pin = self.pin(h, theme, window, cx);
        let captions = self.captions(theme, window, cx);
        let bar = bar
            .child(
                div()
                    .debug_selector(|| "TitleBarDrag".into())
                    .flex_1()
                    .min_w(px(DRAG_MIN_W * s))
                    .h_full(),
            )
            .child(pin)
            .child(captions);
        drag_handlers(bar, cx)
    }
}

impl Render for TitleBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.drawn.clear();
            self.drawn_pin = None;
        }
        if is_client(window, cx) {
            self.client_bar(&theme, window, cx)
        } else {
            self.menu_row(&theme, window, cx)
        }
    }
}
