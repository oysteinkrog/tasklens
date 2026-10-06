//! `TlSecondaryWindow`: the frame every dialog, tool window and Properties window opens through
//! (UI-FND-021, UI-SHL-090, APP-045, APP-009, R-073, R-176, ADR-010, D-006).
//!
//! The frame draws a 34 px title bar with the icon, the title, the pin and the caption buttons,
//! then the owner's content. It has its own kit Root, so it has its own toasts and menus.
//!
//! The title bar is drawn here on GPUI parts, not with the kit `TitleBar`, because the kit bar
//! cannot meet the spec: it always maximizes on a title double-click, fills with a gradient
//! instead of `titlebar`, uses the kit's own hover colours, and hides its caption buttons
//! whenever the platform reports server decorations. The pin is drawn here on the kit's base
//! `Button` with the `TlButton::Toggle` look, because `TlButton::Toggle` is always 24 px and the
//! title-bar pin is 34 × 34.
//!
//! Esc and Ctrl+W dispatch `CloseSecondaryWindow` in the `TlWindow` context. The frame closes the
//! window. A Run or Confirmation owner that must run its Cancel first handles
//! `CloseSecondaryWindow` on its own content element; the frame then never sees it.

use gpui_kit::accesskit::Toggled;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Button as BaseButton, Root};
use gpui_kit::component::Icon;
use gpui_kit::{
    Action, AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, ClickEvent, Context, Entity,
    FocusHandle, FontWeight, Global, Hsla, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription, TextLayout,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowHandle, WindowId, WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, rgb,
    size,
};
use rustc_hash::FxHashMap;
use tl_model::settings::Decorations;
use tl_model::validity::Validity;

use super::focus::TlFocusRing;
use super::icons::{TlIcon, icon};
use super::menu::context_menu_layer;
use super::toast::ToastQueue;
use super::tooltip::{DELAY_CONTROL, FocusTooltip, HoverTooltip, TlTooltip};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::settings_global::TlSettings;
use crate::text::style::label_style;
use crate::theme::{TlTheme, current, metrics};
use crate::window::registry::WindowKey;

/// The Wayland app id and X11 class of every tasklens window.
pub const APP_ID: &str = "io.github.oysteinkrog.Tasklens";

/// The key context of every secondary window's root.
pub const CONTEXT: &str = "TlWindow";

/// Title bar height and caption button width, × ui_scale.
const TITLE_BAR: f32 = 34.;
/// Left padding of the title bar on Linux, × ui_scale.
const TITLE_LEFT_PAD: f32 = 12.;
/// The pin under server decorations, × ui_scale.
const SERVER_PIN: f32 = 24.;
/// Fill alpha of the pin when on, as `TlButton::Toggle`.
const PIN_ON_ALPHA: f32 = 0.25;
/// Toast stack top in P1: the main-window value, × ui_scale (R-195). 34 + 30 + `sp4`.
const TOAST_TOP: f32 = 72.;
/// The same under server decorations, × ui_scale.
const TOAST_TOP_SERVER: f32 = 66.;

/// The Run window's fixed size (APP-014), × ui_scale.
pub const RUN_SIZE: (f32, f32) = (480., 248.);

/// Pin tooltip when off.
pub const PIN_TOOLTIP_OFF: &str = "Always on top";
/// Pin tooltip when on.
pub const PIN_TOOLTIP_ON: &str = "Always on top (on)";
/// Pin tooltip without a keep-above mechanism (R-073).
pub const PIN_TOOLTIP_UNSUPPORTED: &str = "Not supported by this desktop";

/// Flips keep-above for one window. The shell handles it: the main window in E20-022, secondary
/// windows in E20-029, through the KWin bridge only (R-176, R-177). `caption` is the exact window
/// title at dispatch time. Dispatched from code only, so it has no JSON form.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ToggleAlwaysOnTop {
    pub window: WindowKey,
    pub caption: SharedString,
}

gpui_kit::actions!(
    tasklens,
    [
        /// Closes the secondary window that has focus (Ctrl+W and Esc in `TlWindow`). Separate
        /// from the shell's `CloseWindow`.
        CloseSecondaryWindow
    ]
);

/// Binds Esc and Ctrl+W in `TlWindow`. `actions::bind_keys_global` calls it.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseSecondaryWindow, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", CloseSecondaryWindow, Some(CONTEXT)),
    ]);
}

/// The kinds of secondary window (UI-FND-021 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlWindowKind {
    /// A `TlDialog` confirmation, modal to its parent.
    Confirmation,
    /// Settings, Theme Editor, Shortcuts, Handle Search, About, Additional Setup, Crash Report.
    /// About and Crash Report are not resizable.
    Tool { resizable: bool },
    /// "tasklens Run", fixed 480 × 248 × ui_scale.
    Run,
    /// Process, service, startup and connection Properties.
    Properties,
}

impl TlWindowKind {
    /// The GPUI window kind.
    pub fn window_kind(self) -> WindowKind {
        match self {
            Self::Confirmation => WindowKind::Dialog,
            _ => WindowKind::Floating,
        }
    }

    /// The minimum size from the kind table. An owner may set another one in its spec.
    pub fn default_min_size(self) -> gpui_kit::Size<Pixels> {
        let (w, h) = match self {
            Self::Confirmation => (360., 120.),
            Self::Tool { .. } => (480., 320.),
            Self::Run => RUN_SIZE,
            Self::Properties => (640., 400.),
        };
        size(px(w), px(h))
    }

    pub fn resizable(self) -> bool {
        match self {
            Self::Confirmation | Self::Run => false,
            Self::Tool { resizable } => resizable,
            Self::Properties => true,
        }
    }

    /// Modal to its parent: the parent's content is covered and ignores input.
    pub fn modal(self) -> bool {
        self == Self::Confirmation
    }

    /// A title double-click maximizes Tool and Properties windows that can be resized.
    fn maximizes(self) -> bool {
        self.resizable()
    }
}

/// What `TlSecondaryWindow::open` needs from the owner.
#[derive(Clone, Debug)]
pub struct TlWindowSpec {
    pub kind: TlWindowKind,
    /// The exact title from the APP-045 table or the owning spec.
    pub title: SharedString,
    pub size: gpui_kit::Size<Pixels>,
    pub min_size: gpui_kit::Size<Pixels>,
    /// The window to centre on. A Confirmation is modal to it.
    pub parent: Option<AnyWindowHandle>,
    /// `None` for confirmations and other keyless modal children.
    pub key: Option<WindowKey>,
}

impl TlWindowSpec {
    /// A spec with the kind's minimum size, no parent and no key.
    pub fn new(
        kind: TlWindowKind,
        title: impl Into<SharedString>,
        size: gpui_kit::Size<Pixels>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            size,
            min_size: kind.default_min_size(),
            parent: None,
            key: None,
        }
    }

    pub fn parent(mut self, parent: AnyWindowHandle) -> Self {
        self.parent = Some(parent);
        self
    }

    pub fn key(mut self, key: WindowKey) -> Self {
        self.key = Some(key);
        self
    }

    pub fn min_size(mut self, min_size: gpui_kit::Size<Pixels>) -> Self {
        self.min_size = min_size;
        self
    }

    /// The size the window opens at. Run is always `RUN_SIZE` × `ui_scale`; other kinds come
    /// already scaled from the owner.
    fn open_size(&self, ui_scale: f32) -> gpui_kit::Size<Pixels> {
        match self.kind {
            TlWindowKind::Run => size(px(RUN_SIZE.0 * ui_scale), px(RUN_SIZE.1 * ui_scale)),
            _ => self.size,
        }
    }
}

/// The window options for `spec`. Pure apart from reading the parent's bounds and the display.
pub fn window_options(
    spec: &TlWindowSpec,
    decorations: Decorations,
    cx: &mut App,
) -> WindowOptions {
    let open_size = spec.open_size(metrics(cx).ui_scale);
    let min_size = match spec.kind {
        TlWindowKind::Run => open_size,
        _ => spec.min_size,
    };
    let parent_bounds = spec
        .parent
        .and_then(|p| p.update(cx, |_, window, _| window.bounds()).ok());
    let bounds = match parent_bounds {
        Some(parent) => Bounds::centered_at(parent.center(), open_size),
        None => Bounds::centered(None, open_size, cx),
    };
    let base = gpui_kit::component::TitleBar::window_options();
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(spec.title.clone()),
            ..gpui_kit::component::TitleBar::title_bar_options()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_decorations: Some(match decorations {
            Decorations::Client => WindowDecorations::Client,
            Decorations::Server => WindowDecorations::Server,
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        kind: spec.kind.window_kind(),
        is_resizable: spec.kind.resizable(),
        window_min_size: Some(min_size),
        app_id: Some(APP_ID.into()),
        ..base
    }
}

/// How the pin draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinLook {
    pub on: bool,
    pub enabled: bool,
    pub tooltip: &'static str,
}

/// The pin's look for a pin state and whether keep-above exists on this desktop (R-073).
pub fn pin_look(pinned: bool, supported: bool) -> PinLook {
    let tooltip = match (supported, pinned) {
        (false, _) => PIN_TOOLTIP_UNSUPPORTED,
        (true, true) => PIN_TOOLTIP_ON,
        (true, false) => PIN_TOOLTIP_OFF,
    };
    PinLook {
        on: pinned,
        enabled: supported,
        tooltip,
    }
}

/// Keep-above works unless the windows component is `Unsupported` (no KWin bridge, R-073). With
/// no hub there is no engine, so no bridge either.
fn pin_supported(cx: &App) -> bool {
    cx.try_global::<GlobalHub>().is_some_and(|hub| {
        hub.0.read(cx).current().windows.header.validity != Validity::Unsupported
    })
}

/// The `appearance.decorations` setting; client decorations when settings are not loaded.
fn decorations(cx: &App) -> Decorations {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).appearance.decorations
    } else {
        Decorations::Client
    }
}

/// Open confirmations per parent window.
#[derive(Default)]
struct ModalChildren(FxHashMap<WindowId, u32>);

impl Global for ModalChildren {}

/// Whether `window` has an open Confirmation child. Its content is then covered by
/// `derived.overlay` and ignores input. The main window reads this too.
pub fn has_modal_child(window: AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<ModalChildren>()
        .is_some_and(|m| m.0.get(&window.window_id()).is_some_and(|n| *n > 0))
}

fn add_modal_child(parent: AnyWindowHandle, cx: &mut App) {
    *cx.default_global::<ModalChildren>()
        .0
        .entry(parent.window_id())
        .or_default() += 1;
    let _ = parent.update(cx, |_, window, _| window.refresh());
}

fn remove_modal_child(parent: AnyWindowHandle, cx: &mut App) {
    let map = &mut cx.default_global::<ModalChildren>().0;
    if let Some(n) = map.get_mut(&parent.window_id()) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            map.remove(&parent.window_id());
        }
    }
    let _ = parent.update(cx, |_, window, _| window.refresh());
}

/// The frame view: the root view of every secondary window, inside the kit Root.
pub struct TlSecondaryWindow<V> {
    content: Entity<V>,
    pinned: bool,
    toasts: Entity<ToastQueue>,
    key: Option<WindowKey>,
    kind: TlWindowKind,
    title: SharedString,
    /// `appearance.decorations = "server"`: no title bar, the pin sits on the content.
    server: bool,
    /// Holds the `TlWindow` key context when no content element has focus.
    focus: FocusHandle,
    /// The left button went down on the title bar; the next move starts a window move.
    drag_armed: bool,
    /// The drawn title, for the ellipsis check.
    title_layout: TextLayout,
    /// `title_layout` belongs to a rendered title bar.
    title_rendered: bool,
    _hub: Option<Subscription>,
}

impl<V: Render> TlSecondaryWindow<V> {
    /// Opens a secondary window for `spec` with the content `build` returns. The content should
    /// focus its first focusable element; if it focuses nothing, the frame takes focus so Esc and
    /// Ctrl+W work. Returns the error from `open_window` and opens nothing on failure.
    pub fn open(
        cx: &mut App,
        spec: TlWindowSpec,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> gpui_kit::Result<WindowHandle<Root>> {
        let decorations = decorations(cx);
        let options = window_options(&spec, decorations, cx);
        let modal_parent = spec.parent.filter(|_| spec.kind.modal());
        let server = decorations == Decorations::Server;
        let (handle, _) = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| Self::new(spec, server, build, window, cx))
        })
        .inspect_err(|err| {
            tracing::warn!(target: "tasklens::ui", %err, "could not open a secondary window");
        })?;
        if let Some(parent) = modal_parent {
            add_modal_child(parent, cx);
        }
        // `gpui_kit::open_window` always roots the window in the base Root.
        Ok(WindowHandle::new(handle.window_id()))
    }

    fn new(
        spec: TlWindowSpec,
        server: bool,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        tracing::debug!(target: "tasklens::ui", kind = ?spec.kind, "secondary window opens");
        let ui_scale = metrics(cx).ui_scale;
        let top = px(if server { TOAST_TOP_SERVER } else { TOAST_TOP } * ui_scale);
        let toasts = cx.new(|cx| ToastQueue::new(window, cx, top));
        let content = build(window, cx);
        let focus = cx.focus_handle();
        if window.focused(cx).is_none() {
            focus.focus(window, cx);
        }
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
        let _hub = hub.map(|hub| {
            cx.subscribe(&hub, |_, _, ev: &SnapshotChanged, cx| {
                if ev.mask.intersects(ComponentMask::WINDOWS) {
                    cx.notify();
                }
            })
        });
        if let Some(parent) = spec.parent.filter(|_| spec.kind.modal()) {
            cx.on_release(move |_, cx| remove_modal_child(parent, cx))
                .detach();
        }
        Self {
            content,
            pinned: false,
            toasts,
            key: spec.key,
            kind: spec.kind,
            title: spec.title,
            server,
            focus,
            drag_armed: false,
            title_layout: TextLayout::default(),
            title_rendered: false,
            _hub,
        }
    }

    /// The frame view of a window opened by `open`, while it is open.
    pub fn find(handle: WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        handle.read(cx).ok()?.view().clone().downcast::<Self>().ok()
    }
}

impl<V: 'static> TlSecondaryWindow<V> {
    pub fn content(&self) -> &Entity<V> {
        &self.content
    }

    /// This window's own toast queue.
    pub fn toasts(&self) -> &Entity<ToastQueue> {
        &self.toasts
    }

    pub fn key(&self) -> Option<&WindowKey> {
        self.key.as_ref()
    }

    pub fn kind(&self) -> TlWindowKind {
        self.kind
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Changes the title in the bar and at the compositor.
    pub fn set_title(
        &mut self,
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = title.into();
        window.set_window_title(&self.title);
        cx.notify();
    }

    /// The drawn pin state. Kept in memory only (R-176).
    pub fn pinned(&self) -> bool {
        self.pinned
    }

    /// The pin's look now.
    pub fn pin_look(&self, cx: &App) -> PinLook {
        pin_look(self.pinned, pin_supported(cx))
    }

    /// Whether this window draws its own title bar.
    pub fn has_title_bar(&self) -> bool {
        !self.server
    }

    /// The title text as last drawn, with "…" when it was cut. `None` under server
    /// decorations or before the first render. Call it after a draw: GPUI panics on a title
    /// that was rendered but not laid out.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn_title(&self) -> Option<String> {
        self.title_rendered.then(|| self.title_layout.text())
    }

    /// Flips the drawn pin and dispatches `ToggleAlwaysOnTop` with the current title.
    fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pinned = !self.pinned;
        if let Some(key) = self.key.clone() {
            window.dispatch_action(
                Box::new(ToggleAlwaysOnTop {
                    window: key,
                    caption: self.title.clone(),
                }),
                cx,
            );
        }
        cx.notify();
    }

    fn on_close(&mut self, _: &CloseSecondaryWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    /// The pin: a `TlButton::Toggle` look, `side` square.
    fn pin(
        &self,
        side: Pixels,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *metrics(cx);
        let look = pin_look(self.pinned, pin_supported(cx));
        let id = "TlWindowPin";
        let focus: FocusHandle = window
            .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let show_ring = look.enabled && TlFocusRing::show(focus.is_focused(window), window);
        let (bg, border, fg) = if look.on {
            (
                theme.accent.alpha(PIN_ON_ALPHA),
                Some(theme.accent),
                theme.accent,
            )
        } else {
            (gpui_kit::transparent_black(), None, theme.text_primary)
        };
        let hover = theme.tab_hover;
        let pressed = theme.header;
        let mut button = BaseButton::new(id)
            .disabled(!look.enabled)
            .track_focus(&focus)
            .debug_selector(|| "TlWindowPin".into())
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(side)
            .rounded(m.radius_small)
            .bg(bg)
            .text_color(fg)
            .when_some(border, |b, c| b.border_1().border_color(c))
            .when(!look.enabled, |b| b.opacity(0.5))
            .accessibility_label(look.tooltip)
            .aria_toggled(if look.on {
                Toggled::True
            } else {
                Toggled::False
            })
            .when(look.enabled, |b| {
                b.hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.toggle_pin(window, cx);
                    }))
            })
            .child(icon(TlIcon::Pin, m.icon.md, fg));
        let tip = TlTooltip::text(look.tooltip);
        let hover_tip = HoverTooltip::state("TlWindowPin-hover-tooltip", window, cx);
        button = HoverTooltip::attach(
            &hover_tip,
            button,
            DELAY_CONTROL,
            tip.clone().build(),
            window,
            cx,
        );
        let focus_tip = FocusTooltip::state("TlWindowPin-focus-tooltip", window, cx);
        button = FocusTooltip::attach(
            &focus_tip,
            button,
            &focus,
            DELAY_CONTROL,
            tip.build(),
            window,
            cx,
        );
        if show_ring {
            TlFocusRing::ring(button, m.radius_small, theme).into_any_element()
        } else {
            button.into_any_element()
        }
    }

    fn title_bar(
        &mut self,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *metrics(cx);
        let s = m.ui_scale;
        let bar_h = px(TITLE_BAR * s);
        let controls = window.window_controls();
        let maximizes = self.kind.maximizes();

        let text = StyledText::new(self.title.clone());
        self.title_layout = text.layout().clone();
        self.title_rendered = true;
        // The label style replaces the whole text refinement, so set it before the text
        // properties below.
        let mut title = div();
        *title.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
        let title = title
            .debug_selector(|| "TlWindowTitle".into())
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme.text_primary)
            .child(text);

        let white: Hsla = rgb(0xFFFFFF).into();
        let caption = |id: &'static str, name: IconName, hover: Hsla, hover_fg: Hsla| {
            div()
                .id(id)
                .debug_selector(move || format!("TlWindowCaption-{id}"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(bar_h)
                .h_full()
                .text_color(theme.text_primary)
                .hover(move |st| st.bg(hover).text_color(hover_fg))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .child(Icon::new(name).size(m.icon.sm))
        };
        let pin = self.pin(bar_h, theme, window, cx);

        div()
            .id("TlWindowTitleBar")
            .debug_selector(|| "TlWindowTitleBar".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(bar_h)
            .pl(px(TITLE_LEFT_PAD * s))
            .gap(m.sp[3])
            .bg(theme.titlebar)
            .border_b_1()
            .border_color(theme.divider)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, _| this.drag_armed = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.drag_armed = false),
            )
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _, _| this.drag_armed = false),
            )
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, _| {
                if this.drag_armed {
                    this.drag_armed = false;
                    window.start_window_move();
                }
            }))
            .on_mouse_down(MouseButton::Right, |ev: &MouseDownEvent, window, _| {
                window.show_window_menu(ev.position)
            })
            .on_click(move |ev, window, _| {
                if maximizes && ev.click_count() == 2 {
                    window.zoom_window();
                }
            })
            .child(
                div()
                    .flex_none()
                    .size(m.icon.md)
                    .debug_selector(|| "TlWindowIcon".into()),
            )
            .child(title)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .h_full()
                    .items_center()
                    .child(pin)
                    .when(controls.minimize, |d| {
                        d.child(
                            caption(
                                "minimize",
                                IconName::WindowMinimize,
                                theme.tab_hover,
                                theme.text_primary,
                            )
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.minimize_window();
                            }),
                        )
                    })
                    .when(controls.maximize && maximizes, |d| {
                        let name = if window.is_maximized() {
                            IconName::WindowRestore
                        } else {
                            IconName::WindowMaximize
                        };
                        d.child(
                            caption("maximize", name, theme.tab_hover, theme.text_primary)
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.zoom_window();
                                }),
                        )
                    })
                    .child(
                        caption("close", IconName::WindowClose, theme.status.danger, white)
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl<V: Render> Render for TlSecondaryWindow<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let blocked = has_modal_child(window.window_handle(), cx);
        let title_bar = (!self.server).then(|| self.title_bar(&theme, window, cx));
        let server_pin = self
            .server
            .then(|| self.pin(px(SERVER_PIN * m.ui_scale), &theme, window, cx));

        div()
            .id("TlWindow")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_close))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .text_color(theme.text_primary)
            .children(title_bar)
            .child(
                div()
                    .debug_selector(|| "TlWindowContent".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(self.content.clone())
                    .when_some(server_pin, |d, pin| {
                        d.child(div().absolute().top(m.sp[4]).right(m.sp[4]).child(pin))
                    })
                    .when(blocked, |d| {
                        d.child(
                            div()
                                .id("TlWindowModalOverlay")
                                .debug_selector(|| "TlWindowModalOverlay".into())
                                .absolute()
                                .inset_0()
                                .bg(theme.derived.overlay)
                                .occlude(),
                        )
                    }),
            )
            .children(context_menu_layer(window, cx))
    }
}
