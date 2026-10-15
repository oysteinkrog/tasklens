//! "tasklens Theme Editor": the window frame, the name input, the 23-row slot list and the
//! Cancel and Done buttons (UI-SHL-081, APP-037, APP-035, APP-045, R-083, UI-FND-043).
//!
//! The window is a floating Tool window. It starts a live preview with `ThemeStore::begin_preview`
//! and holds the token. Cancel, the close button, Esc and Ctrl+W end the preview through the
//! window-closed hook, so every way of closing cancels. Done checks the name first and then
//! commits. The picker column holds a saturation and value plane, a hue strip, RGB and HSV
//! readouts and a hex input, all editing the selected slot (E21-028). E21-029 fills the preview
//! column (a mini tab strip, a mini table and a static graph, all painted from the live theme) and
//! the Random, Share and Import buttons.

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, BorderStyle, Bounds, Context, Corners, Edges,
    Entity, EventEmitter, FocusHandle, Focusable, Hitbox, HitboxBehavior, Hsla, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathBuilder, Pixels, Point, Render, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, WindowHandle, black, canvas, div,
    fill, gpui, hsla, linear_color_stop, linear_gradient, point, prelude::FluentBuilder as _, px,
    quad, size, transparent_black, white,
};
use serde::Deserialize;
use tracing::warn;

use super::main::{ShellState, push_main_toast};
use super::menus_view::import_from_clipboard;
use super::registry::{GlobalRegistry, WindowKey};
use super::share_theme::open_share_window;
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::input::TlTextInput;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::TlSettings;
use crate::text::style::{TNUM, t_base, t_sm, t_xs};
use crate::theme::preview::PreviewToken;
use crate::theme::{ThemeDef, ThemeError, ThemeSlot, ThemeStore, builtin, current, metrics};

/// The key context of the window content.
pub const CONTEXT: &str = "TlThemeEditor";

/// Window size and minimum size, × ui_scale.
const SIZE: (f32, f32) = (820., 560.);
const MIN_SIZE: (f32, f32) = (720., 480.);
/// Column widths and row heights, × ui_scale.
const SLOT_COLUMN_W: f32 = 240.;
const PICKER_MIN_W: f32 = 280.;
/// The colour plane edge, the hue strip width and the plane cursor ring, × ui_scale.
const PLANE: f32 = 200.;
const STRIP_W: f32 = 16.;
const RING: f32 = 10.;
const PREVIEW_COLUMN_W: f32 = 260.;
const TOP_ROW_H: f32 = 32.;
const SLOT_ROW_H: f32 = 26.;
const SWATCH: f32 = 14.;
const FOOTER_H: f32 = 48.;
/// The longest theme name.
const NAME_MAX: usize = 64;

const TITLE: &str = "shell-theme-editor-title";
const NAME_LABEL: &str = "shell-theme-editor-name";
const RANDOM: &str = "shell-theme-editor-random";
const SHARE: &str = "shell-theme-editor-share";
const IMPORT: &str = "shell-theme-editor-import";
const CANCEL: &str = "shell-theme-editor-cancel";
const DONE: &str = "shell-theme-editor-done";
const NAME_ERROR: &str = "shell-theme-editor-name-error";
const HEX_HINT: &str = "shell-theme-editor-hex-hint";
const READOUT_RGB: &str = "shell-theme-editor-readout-rgb";
const READOUT_HSV: &str = "shell-theme-editor-readout-hsv";
const TOAST_SAVED: &str = "shell-toast-theme-saved";
const TOAST_COPY_FIRST: &str = "shell-toast-theme-copy-first";
const TOAST_RANDOM: &str = "shell-toast-theme-random";
const TOAST_NO_CODE: &str = "shell-toast-theme-no-code";
const PREVIEW_TAB_PROCESSES: &str = "shell-theme-editor-preview-tab-processes";
const PREVIEW_TAB_PERFORMANCE: &str = "shell-theme-editor-preview-tab-performance";
const PREVIEW_COL_NAME: &str = "shell-theme-editor-preview-col-name";
const PREVIEW_COL_CPU: &str = "shell-theme-editor-preview-col-cpu";
const PREVIEW_COL_MEMORY: &str = "shell-theme-editor-preview-col-memory";
const PREVIEW_ROW_NORMAL: &str = "shell-theme-editor-preview-row-normal";
const PREVIEW_ROW_ALTERNATE: &str = "shell-theme-editor-preview-row-alternate";
const PREVIEW_ROW_HOVER: &str = "shell-theme-editor-preview-row-hover";
const PREVIEW_ROW_SELECTED: &str = "shell-theme-editor-preview-row-selected";
/// The preview graph size, × ui_scale, and its fixed sample values (0..=1).
const GRAPH_W: f32 = 236.;
const GRAPH_H: f32 = 60.;
const GRAPH_SAMPLES: [f32; 12] = [
    0.15, 0.25, 0.2, 0.45, 0.4, 0.7, 0.55, 0.6, 0.35, 0.5, 0.3, 0.4,
];

/// Opens "tasklens Theme Editor" for the named custom theme (View > Theme > Edit colors, E21-031).
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct EditTheme {
    pub name: SharedString,
}

gpui_kit::actions!(
    tasklens,
    [
        /// Enter in the editor: Done when the name is valid.
        ThemeEditorDone,
        /// The Random button.
        ThemeEditorRandom,
        /// The Share button.
        ThemeEditorShare,
        /// The Import button.
        ThemeEditorImport
    ]
);

/// Binds Enter in `TlThemeEditor` and handles `EditTheme`. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", ThemeEditorDone, Some(CONTEXT))]);
    // Deferred: from a menu in the main window, that window is mid-update and its bounds cannot
    // be read to centre the new window.
    cx.on_action(|a: &EditTheme, cx| {
        let name = a.name.clone();
        cx.defer(move |cx| edit_named(&name, cx));
    });
}

fn edit_named(name: &str, cx: &mut App) {
    let Some((def, builtin)) = cx
        .try_global::<ThemeStore>()
        .and_then(|s| s.resolve(name))
        .map(|(def, builtin)| (def.clone(), builtin))
    else {
        warn!(target: "tasklens::ui", name, "edit: no such theme");
        return;
    };
    if builtin {
        let text = tr(cx, TOAST_COPY_FIRST);
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, cx| {
                push_toast(ToastKind::Warning, text, window, cx);
            });
        }
        return;
    }
    let Some(parent) = main_or_active(cx) else {
        warn!(target: "tasklens::ui", "edit: no parent window");
        return;
    };
    open_editor(def, parent, cx);
}

/// The open main window, else the active window.
fn main_or_active(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<ShellState>()
        .and_then(|s| s.main_window)
        .map(AnyWindowHandle::from)
        .filter(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
        .or_else(|| cx.active_window())
}

fn push_toast(kind: ToastKind, text: SharedString, window: &mut Window, cx: &mut App) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
    }
}

/// Opens the editor for `def`. An editor open for the same theme is focused. An editor open for
/// another theme is cancelled and closed first. Without a window registry (tests) it always opens
/// a window.
pub fn open_editor(def: ThemeDef, parent: AnyWindowHandle, cx: &mut App) {
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, TITLE),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(WindowKey::ThemeEditor);
    let same = ThemeStore::preview_name(cx).is_some_and(|n| *n == *def.name);
    let registry = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone());
    if !same && let Some(registry) = &registry {
        close_open_editor(registry, cx);
    }
    let open = move |cx: &mut App| {
        let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| ThemeEditor::new(&def, parent, window, cx))
        })?;
        follow_title(handle, cx);
        Ok(handle.into())
    };
    match registry {
        Some(registry) => {
            registry.update(cx, |r, cx| {
                r.open_or_focus(WindowKey::ThemeEditor, cx, open)
            });
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// Cancels the open editor's preview and closes its window.
fn close_open_editor(registry: &Entity<super::registry::WindowRegistry>, cx: &mut App) {
    let Some(handle) = registry.read(cx).get(&WindowKey::ThemeEditor) else {
        return;
    };
    if let Some(root) = handle.downcast::<Root>()
        && let Some(frame) = TlSecondaryWindow::<ThemeEditor>::find(root, cx)
    {
        let view = frame.read(cx).content().clone();
        view.update(cx, |v, cx| v.cancel(cx));
    }
    registry.update(cx, |r, cx| r.close(&WindowKey::ThemeEditor, cx));
}

/// Sets the frame title again when the strings version changes, one `tr` call per version
/// (R-132).
fn follow_title(handle: WindowHandle<Root>, cx: &mut App) {
    let Some(frame) = TlSecondaryWindow::<ThemeEditor>::find(handle, cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        frame.update(cx, |_, cx| {
            let mut version = strings_version(cx);
            cx.observe_global_in::<Strings>(window, move |frame, window, cx| {
                let now = strings_version(cx);
                if now != version {
                    version = now;
                    frame.set_title(tr(cx, TITLE), window, cx);
                }
            })
            .detach();
        });
    });
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

/// The string key of a slot's label (APP-035).
fn slot_key(slot: ThemeSlot) -> &'static str {
    match slot {
        ThemeSlot::WindowBg => "shell-theme-editor-slot-window-bg",
        ThemeSlot::Titlebar => "shell-theme-editor-slot-titlebar",
        ThemeSlot::Toolbar => "shell-theme-editor-slot-toolbar",
        ThemeSlot::Input => "shell-theme-editor-slot-input",
        ThemeSlot::InputFocus => "shell-theme-editor-slot-input-focus",
        ThemeSlot::Header => "shell-theme-editor-slot-header",
        ThemeSlot::ListBg => "shell-theme-editor-slot-list-bg",
        ThemeSlot::RowAlt => "shell-theme-editor-slot-row-alt",
        ThemeSlot::RowHover => "shell-theme-editor-slot-row-hover",
        ThemeSlot::RowSelected => "shell-theme-editor-slot-row-selected",
        ThemeSlot::Divider => "shell-theme-editor-slot-divider",
        ThemeSlot::MenuBg => "shell-theme-editor-slot-menu-bg",
        ThemeSlot::MenuHover => "shell-theme-editor-slot-menu-hover",
        ThemeSlot::TabBg => "shell-theme-editor-slot-tab-bg",
        ThemeSlot::TabHover => "shell-theme-editor-slot-tab-hover",
        ThemeSlot::ScrollTrack => "shell-theme-editor-slot-scroll-track",
        ThemeSlot::ScrollThumb => "shell-theme-editor-slot-scroll-thumb",
        ThemeSlot::TextPrimary => "shell-theme-editor-slot-text-primary",
        ThemeSlot::TextSecondary => "shell-theme-editor-slot-text-secondary",
        ThemeSlot::TextSelectedPrimary => "shell-theme-editor-slot-text-selected-primary",
        ThemeSlot::TextSelectedSecondary => "shell-theme-editor-slot-text-selected-secondary",
        ThemeSlot::ValueHeat => "shell-theme-editor-slot-value-heat",
        ThemeSlot::Accent => "shell-theme-editor-slot-accent",
    }
}

/// Upper-case `#RRGGBB`. The alpha is dropped.
fn rgb_hex(c: Hsla) -> String {
    let c: Rgba = c.into();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", b(c.r), b(c.g), b(c.b))
}

/// The colour of `slot` in `def`.
fn def_slot(def: &ThemeDef, slot: ThemeSlot) -> Hsla {
    match slot {
        ThemeSlot::Accent => def.accent,
        s => def.slots[s as usize],
    }
}

/// Parses 6 hex digits with or without a leading `#`. Eight digits and anything else are refused.
fn hex6(text: &str) -> Option<Hsla> {
    let text = text.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    builtin::parse_hex(&format!("#{digits}")).ok()
}

fn to_bytes(c: Hsla) -> [u8; 3] {
    let c: Rgba = c.into();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [b(c.r), b(c.g), b(c.b)]
}

/// RGB to (hue in degrees 0..360, saturation 0..=1, value 0..=1). A grey has hue 0.
fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb.map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// HSV (hue in degrees, 360 counts as 0) to RGB bytes.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|p| ((p + m).clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn rgb_to_hsla(rgb: [u8; 3]) -> Hsla {
    let [r, g, b] = rgb.map(|v| f32::from(v) / 255.0);
    Rgba { r, g, b, a: 1.0 }.into()
}

/// What the plane tells the editor: its saturation or value changed.
pub struct PlaneChanged;

/// What the hue strip tells the editor: the new hue in degrees.
pub struct HueChanged(pub f32);

/// The step of an arrow key: `unit`, or ten times that with Shift.
fn step(ev: &KeyDownEvent, unit: f32) -> f32 {
    if ev.keystroke.modifiers.shift {
        unit * 10.0
    } else {
        unit
    }
}

/// The saturation and value plane: white to the pure hue across, transparent to black down. The
/// cursor is a ring at (s, 1 - v).
pub struct ColorPlane {
    hue: f32,
    s: f32,
    v: f32,
    focus: FocusHandle,
    dragging: bool,
}

impl EventEmitter<PlaneChanged> for ColorPlane {}

impl Focusable for ColorPlane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ColorPlane {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            hue: 0.0,
            s: 0.0,
            v: 0.0,
            focus: cx.focus_handle(),
            dragging: false,
        }
    }

    pub fn hue(&self) -> f32 {
        self.hue
    }

    pub fn s(&self) -> f32 {
        self.s
    }

    pub fn v(&self) -> f32 {
        self.v
    }

    fn set_sv(&mut self, s: f32, v: f32, cx: &mut Context<Self>) {
        let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
        if s != self.s || v != self.v {
            self.s = s;
            self.v = v;
            cx.emit(PlaneChanged);
            cx.notify();
        }
    }

    fn set_from_point(&mut self, p: Point<Pixels>, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let w = f32::from(bounds.size.width).max(1.0);
        let h = f32::from(bounds.size.height).max(1.0);
        let s = f32::from(p.x - bounds.left()) / w;
        let v = 1.0 - f32::from(p.y - bounds.top()) / h;
        self.set_sv(s, v, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let d = step(ev, 0.01);
        let (s, v) = match ev.keystroke.key.as_str() {
            "left" => (self.s - d, self.v),
            "right" => (self.s + d, self.v),
            "up" => (self.s, self.v + d),
            "down" => (self.s, self.v - d),
            _ => return,
        };
        self.set_sv(s, v, cx);
        cx.stop_propagation();
    }
}

impl Render for ColorPlane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let edge = px(PLANE * m.ui_scale);
        let ring = px(RING * m.ui_scale);
        let (hue, s, v) = (self.hue, self.s, self.v);
        let me = cx.entity();
        div()
            .id("ThemeEditor-plane")
            .debug_selector(|| "ThemeEditor-plane".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev, _, cx| this.on_key(ev, cx)))
            .size(edge)
            .flex_none()
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, _| {
                        paint_plane(bounds, hue, s, v, ring, window);
                        plane_events(&me, bounds, &hitbox, window);
                    },
                )
                .size_full(),
            )
    }
}

fn paint_plane(
    bounds: Bounds<Pixels>,
    hue: f32,
    s: f32,
    v: f32,
    ring: Pixels,
    window: &mut Window,
) {
    let pure = hsla(hue / 360.0, 1.0, 0.5, 1.0);
    window.paint_quad(fill(
        bounds,
        linear_gradient(
            90.,
            linear_color_stop(white(), 0.),
            linear_color_stop(pure, 1.),
        ),
    ));
    window.paint_quad(fill(
        bounds,
        linear_gradient(
            180.,
            linear_color_stop(transparent_black(), 0.),
            linear_color_stop(black(), 1.),
        ),
    ));
    let centre = point(
        bounds.left() + bounds.size.width * s,
        bounds.top() + bounds.size.height * (1.0 - v),
    );
    let at = Bounds::new(
        point(centre.x - ring / 2.0, centre.y - ring / 2.0),
        size(ring, ring),
    );
    window.paint_quad(quad(
        at,
        Corners::all(ring / 2.0),
        transparent_black(),
        Edges::all(px(2.)),
        white(),
        BorderStyle::Solid,
    ));
}

/// Registers the plane's pointer listeners for this frame. A drag goes on outside the plane.
fn plane_events(
    me: &Entity<ColorPlane>,
    bounds: Bounds<Pixels>,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    let (e, hb) = (me.clone(), hitbox.clone());
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase.bubble() && ev.button == MouseButton::Left && hb.is_hovered(window) {
            e.update(cx, |p, cx| {
                p.focus.focus(window, cx);
                p.dragging = true;
                p.set_from_point(ev.position, bounds, cx);
            });
            cx.stop_propagation();
        }
    });
    let e = me.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
        if phase.bubble() {
            e.update(cx, |p, cx| {
                if !p.dragging {
                    return;
                }
                if ev.pressed_button == Some(MouseButton::Left) {
                    p.set_from_point(ev.position, bounds, cx);
                } else {
                    p.dragging = false;
                }
            });
        }
    });
    let e = me.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase.bubble() {
            e.update(cx, |p, _| p.dragging = false);
        }
    });
}

/// The vertical hue strip, 0 degrees at the top. The cursor is a 2 px bar.
pub struct HueStrip {
    hue: f32,
    focus: FocusHandle,
    dragging: bool,
}

impl EventEmitter<HueChanged> for HueStrip {}

impl Focusable for HueStrip {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl HueStrip {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            hue: 0.0,
            focus: cx.focus_handle(),
            dragging: false,
        }
    }

    pub fn hue(&self) -> f32 {
        self.hue
    }

    fn set_hue(&mut self, hue: f32, cx: &mut Context<Self>) {
        let hue = hue.clamp(0.0, 360.0);
        if hue != self.hue {
            self.hue = hue;
            cx.emit(HueChanged(hue));
            cx.notify();
        }
    }

    fn set_from_point(&mut self, p: Point<Pixels>, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let h = f32::from(bounds.size.height).max(1.0);
        self.set_hue(f32::from(p.y - bounds.top()) / h * 360.0, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let d = step(ev, 1.0);
        let hue = match ev.keystroke.key.as_str() {
            "up" => self.hue - d,
            "down" => self.hue + d,
            _ => return,
        };
        self.set_hue(hue, cx);
        cx.stop_propagation();
    }
}

impl Render for HueStrip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let hue = self.hue;
        let me = cx.entity();
        div()
            .id("ThemeEditor-hue")
            .debug_selector(|| "ThemeEditor-hue".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev, _, cx| this.on_key(ev, cx)))
            .w(px(STRIP_W * m.ui_scale))
            .h(px(PLANE * m.ui_scale))
            .flex_none()
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, _| {
                        paint_strip(bounds, hue, window);
                        strip_events(&me, bounds, &hitbox, window);
                    },
                )
                .size_full(),
            )
    }
}

fn paint_strip(bounds: Bounds<Pixels>, hue: f32, window: &mut Window) {
    let seg = bounds.size.height / 6.0;
    for k in 0..6 {
        let at = Bounds::new(
            point(bounds.left(), bounds.top() + seg * k as f32),
            size(bounds.size.width, seg),
        );
        let (a, b) = (k as f32 / 6.0, (k + 1) as f32 / 6.0);
        window.paint_quad(fill(
            at,
            linear_gradient(
                180.,
                linear_color_stop(hsla(a, 1.0, 0.5, 1.0), 0.),
                linear_color_stop(hsla(b, 1.0, 0.5, 1.0), 1.),
            ),
        ));
    }
    let y = bounds.top() + bounds.size.height * (hue / 360.0);
    let bar = px(2.);
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.left(), y - bar / 2.0),
            size(bounds.size.width, bar),
        ),
        white(),
    ));
}

fn strip_events(
    me: &Entity<HueStrip>,
    bounds: Bounds<Pixels>,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    let (e, hb) = (me.clone(), hitbox.clone());
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase.bubble() && ev.button == MouseButton::Left && hb.is_hovered(window) {
            e.update(cx, |p, cx| {
                p.focus.focus(window, cx);
                p.dragging = true;
                p.set_from_point(ev.position, bounds, cx);
            });
            cx.stop_propagation();
        }
    });
    let e = me.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
        if phase.bubble() {
            e.update(cx, |p, cx| {
                if !p.dragging {
                    return;
                }
                if ev.pressed_button == Some(MouseButton::Left) {
                    p.set_from_point(ev.position, bounds, cx);
                } else {
                    p.dragging = false;
                }
            });
        }
    });
    let e = me.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase.bubble() {
            e.update(cx, |p, _| p.dragging = false);
        }
    });
}

/// The window's own text, translated once per strings version (R-132).
struct EditorWords {
    name: SharedString,
    random: SharedString,
    share: SharedString,
    import: SharedString,
    cancel: SharedString,
    done: SharedString,
    name_error: SharedString,
    saved: SharedString,
    random_toast: SharedString,
    no_code: SharedString,
    /// The preview pane's texts: two tabs, three column headers, four row names.
    preview_tabs: [SharedString; 2],
    preview_cols: [SharedString; 3],
    preview_rows: [SharedString; 4],
    hex_hint: SharedString,
    /// "R \u{1} G \u{2} B \u{3}" and "H \u{1} S \u{2} V \u{3}": the numbers go in at the
    /// marker characters, without `cx`.
    rgb_tpl: SharedString,
    hsv_tpl: SharedString,
    slots: Vec<SharedString>,
}

/// The text of `key` with the three arguments replaced by marker characters 1, 2 and 3.
fn marked(cx: &App, key: &str, names: [&str; 3]) -> SharedString {
    let args = [
        (names[0], FluentValue::from("\u{1}")),
        (names[1], FluentValue::from("\u{2}")),
        (names[2], FluentValue::from("\u{3}")),
    ];
    tr_args(cx, key, &args)
}

fn fill_marks(tpl: &str, v: [String; 3]) -> SharedString {
    let [a, b, c] = v;
    tpl.replace('\u{1}', &a)
        .replace('\u{2}', &b)
        .replace('\u{3}', &c)
        .into()
}

impl EditorWords {
    fn new(cx: &App) -> Self {
        Self {
            name: tr(cx, NAME_LABEL),
            random: tr(cx, RANDOM),
            share: tr(cx, SHARE),
            import: tr(cx, IMPORT),
            cancel: tr(cx, CANCEL),
            done: tr(cx, DONE),
            name_error: tr(cx, NAME_ERROR),
            saved: tr(cx, TOAST_SAVED),
            random_toast: tr(cx, TOAST_RANDOM),
            no_code: tr(cx, TOAST_NO_CODE),
            preview_tabs: [PREVIEW_TAB_PROCESSES, PREVIEW_TAB_PERFORMANCE].map(|k| tr(cx, k)),
            preview_cols: [PREVIEW_COL_NAME, PREVIEW_COL_CPU, PREVIEW_COL_MEMORY]
                .map(|k| tr(cx, k)),
            preview_rows: [
                PREVIEW_ROW_NORMAL,
                PREVIEW_ROW_ALTERNATE,
                PREVIEW_ROW_HOVER,
                PREVIEW_ROW_SELECTED,
            ]
            .map(|k| tr(cx, k)),
            hex_hint: tr(cx, HEX_HINT),
            rgb_tpl: marked(cx, READOUT_RGB, ["r", "g", "b"]),
            hsv_tpl: marked(cx, READOUT_HSV, ["h", "s", "v"]),
            slots: ThemeSlot::ALL
                .iter()
                .map(|s| tr(cx, slot_key(*s)))
                .collect(),
        }
    }
}

/// The content of the Theme Editor window.
pub struct ThemeEditor {
    token: Option<PreviewToken>,
    base_name: Arc<str>,
    selected_slot: ThemeSlot,
    name: Entity<InputState>,
    name_error: Option<SharedString>,
    // E21-028
    plane: Option<Entity<ColorPlane>>,
    hue: Option<Entity<HueStrip>>,
    hex: Option<Entity<InputState>>,
    hex_error: bool,
    /// The RGB and the HSV readout texts.
    readouts: (SharedString, SharedString),
    parent: AnyWindowHandle,
    list_focus: FocusHandle,
    cancel_focus: FocusHandle,
    words: EditorWords,
    strings_version: u64,
    /// The 23 hex readouts of the theme with this `version`.
    hexes: (u64, Vec<SharedString>),
    /// The 4 row backgrounds the preview table painted last (test probe).
    painted_rows: [Hsla; 4],
    _subs: Vec<Subscription>,
}

impl ThemeEditor {
    fn new(
        def: &ThemeDef,
        parent: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let token = ThemeStore::begin_preview(def, cx);
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |s, cx| {
            s.set_value(def.name.to_string(), window, cx);
            s.focus(window, cx);
        });
        let mut subs = vec![cx.subscribe_in(
            &name,
            window,
            |this, input, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => this.on_name_change(input, window, cx),
                InputEvent::PressEnter { .. } => this.done(window, cx),
                _ => {}
            },
        )];
        let plane = cx.new(ColorPlane::new);
        let hue = cx.new(HueStrip::new);
        let hex = cx.new(|cx| InputState::new(window, cx));
        subs.push(
            cx.subscribe_in(&plane, window, |this, _, _: &PlaneChanged, window, cx| {
                this.pick(window, cx)
            }),
        );
        subs.push(
            cx.subscribe_in(&hue, window, |this, _, ev: &HueChanged, window, cx| {
                if let Some(plane) = &this.plane {
                    let h = ev.0;
                    plane.update(cx, |p, cx| {
                        p.hue = h;
                        cx.notify();
                    });
                }
                this.pick(window, cx);
            }),
        );
        // Applies on Enter or blur only. A change from code (`set_value`) must not apply.
        subs.push(cx.subscribe_in(
            &hex,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.apply_hex(window, cx),
                InputEvent::Change => {
                    if this.hex_error {
                        this.hex_error = false;
                        cx.notify();
                    }
                }
                InputEvent::Focus => {}
            },
        ));
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.words = EditorWords::new(cx);
                cx.notify();
            }
        }));
        // Closing the window any other way (the close button, Esc, Ctrl+W) cancels.
        let id = window.window_handle().window_id();
        let view = cx.entity().downgrade();
        subs.push(cx.on_window_closed(move |cx, closed| {
            if closed == id {
                view.update(cx, |this, cx| this.cancel(cx)).ok();
            }
        }));
        cx.on_release(|this, cx| this.cancel(cx)).detach();
        let words = EditorWords::new(cx);
        let mut this = Self {
            token: Some(token),
            base_name: def.name.clone(),
            selected_slot: ThemeSlot::WindowBg,
            name,
            name_error: None,
            plane: Some(plane),
            hue: Some(hue),
            hex: Some(hex),
            hex_error: false,
            readouts: (SharedString::default(), SharedString::default()),
            parent,
            list_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            words,
            strings_version: strings_version(cx),
            hexes: (u64::MAX, Vec::new()),
            painted_rows: [Hsla::default(); 4],
            _subs: subs,
        };
        this.load_slot(window, cx);
        this
    }

    pub fn plane(&self) -> Option<&Entity<ColorPlane>> {
        self.plane.as_ref()
    }

    pub fn hue_strip(&self) -> Option<&Entity<HueStrip>> {
        self.hue.as_ref()
    }

    /// The RGB and the HSV readout texts under the plane.
    pub fn readouts(&self) -> (&str, &str) {
        (&self.readouts.0, &self.readouts.1)
    }

    /// The text of the name input.
    pub fn name_text(&self, cx: &App) -> SharedString {
        self.name.read(cx).value()
    }

    /// The text of the hex input.
    pub fn hex_text(&self, cx: &App) -> SharedString {
        self.hex
            .as_ref()
            .map(|h| h.read(cx).value())
            .unwrap_or_default()
    }

    /// Whether the hex input holds text that is not 6 hex digits.
    pub fn hex_error(&self) -> bool {
        self.hex_error
    }

    fn set_readouts(&mut self, rgb: [u8; 3], cx: &App) {
        let Some(plane) = &self.plane else {
            return;
        };
        let p = plane.read(cx);
        let pct = |x: f32| (x * 100.0).round() as u32;
        self.readouts = (
            fill_marks(&self.words.rgb_tpl, rgb.map(|b| b.to_string())),
            fill_marks(
                &self.words.hsv_tpl,
                [
                    format!("{:03}", p.hue.round() as u32),
                    format!("{:03}", pct(p.s)),
                    format!("{:03}", pct(p.v)),
                ],
            ),
        );
    }

    /// Loads the colour of the selected slot, from the live preview, into the plane, the strip,
    /// the readouts and the hex input. A grey keeps the current hue.
    pub fn load_slot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(token), Some(plane), Some(strip)) = (&self.token, &self.plane, &self.hue) else {
            return;
        };
        let c = def_slot(&ThemeStore::preview_def(token, cx), self.selected_slot);
        self.load_color(c, plane.clone(), strip.clone(), window, cx);
    }

    fn load_color(
        &mut self,
        c: Hsla,
        plane: Entity<ColorPlane>,
        strip: Entity<HueStrip>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rgb = to_bytes(c);
        let (h, s, v) = rgb_to_hsv(rgb);
        let hue = if s == 0.0 { plane.read(cx).hue } else { h };
        plane.update(cx, |p, cx| {
            p.hue = hue;
            p.s = s;
            p.v = v;
            cx.notify();
        });
        strip.update(cx, |p, cx| {
            p.hue = hue;
            cx.notify();
        });
        self.set_readouts(rgb, cx);
        self.hex_error = false;
        if let Some(hex) = &self.hex {
            hex.update(cx, |i, cx| i.set_value(rgb_hex(c), window, cx));
        }
        cx.notify();
    }

    /// A drag, key or strip change: sends the plane's colour to the preview.
    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(token), Some(plane)) = (&self.token, &self.plane) else {
            return;
        };
        let p = plane.read(cx);
        let rgb = hsv_to_rgb(p.hue, p.s, p.v);
        let c = rgb_to_hsla(rgb);
        ThemeStore::set_preview_slot(token, self.selected_slot, c, cx);
        self.set_readouts(rgb, cx);
        self.hex_error = false;
        if let Some(hex) = &self.hex {
            hex.update(cx, |i, cx| i.set_value(rgb_hex(c), window, cx));
        }
        cx.notify();
    }

    /// Enter or blur in the hex input. Bad text only sets the error and applies nothing.
    fn apply_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(token), Some(plane), Some(strip), Some(hex)) =
            (&self.token, &self.plane, &self.hue, &self.hex)
        else {
            return;
        };
        let text = hex.read(cx).value();
        let Some(c) = hex6(&text) else {
            self.hex_error = true;
            cx.notify();
            return;
        };
        let c = rgb_to_hsla(to_bytes(c));
        let now = def_slot(&ThemeStore::preview_def(token, cx), self.selected_slot);
        let changed = to_bytes(now) != to_bytes(c);
        let (plane, strip) = (plane.clone(), strip.clone());
        if changed {
            ThemeStore::set_preview_slot(token, self.selected_slot, c, cx);
        }
        self.load_color(c, plane, strip, window, cx);
    }

    /// The slot whose colour the picker edits.
    pub fn selected_slot(&self) -> ThemeSlot {
        self.selected_slot
    }

    /// The name error shown under the name input, if any.
    pub fn name_error(&self) -> Option<&SharedString> {
        self.name_error.as_ref()
    }

    /// Whether Done is enabled.
    pub fn done_enabled(&self) -> bool {
        self.name_error.is_none()
    }

    /// Ends the preview without saving. Safe to call twice.
    fn cancel(&mut self, cx: &mut App) {
        if let Some(token) = self.token.take() {
            ThemeStore::cancel_preview(token, cx);
        }
    }

    fn on_name_change(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut text = input.read(cx).text().to_string();
        if text.chars().count() > NAME_MAX {
            text = text.chars().take(NAME_MAX).collect();
            input.update(cx, |s, cx| s.set_value(text.clone(), window, cx));
        }
        if let Some(token) = &self.token {
            ThemeStore::set_preview_name(token, text.as_str().into(), cx);
        }
        self.name_error = self.check_name(&text, cx);
        cx.notify();
    }

    /// The error for `text` as a theme name: empty after trimming, or equal ignoring case to
    /// another theme's name. The theme being edited is skipped.
    fn check_name(&self, text: &str, cx: &App) -> Option<SharedString> {
        let name = text.trim();
        let taken = name.is_empty() || {
            let store = cx.global::<ThemeStore>();
            store
                .user
                .iter()
                .chain(store.builtin.iter())
                .filter(|d| d.name != self.base_name)
                .any(|d| d.name.to_lowercase() == name.to_lowercase())
        };
        taken.then(|| self.words.name_error.clone())
    }

    fn on_done(&mut self, _: &ThemeEditorDone, window: &mut Window, cx: &mut Context<Self>) {
        // Enter in the hex input applies the hex text only; the input lets the key through.
        let in_hex = self
            .hex
            .as_ref()
            .is_some_and(|h| h.focus_handle(cx).is_focused(window));
        if in_hex {
            return;
        }
        if self.cancel_focus.is_focused(window) {
            window.remove_window();
        } else {
            self.done(window, cx);
        }
    }

    /// Done: commits when the name is valid, writes `theme.active`, shows the toast and closes.
    fn done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.name.read(cx).text().to_string();
        self.name_error = self.check_name(&text, cx);
        if self.name_error.is_some() {
            cx.notify();
            return;
        }
        let Some(token) = self.token.take() else {
            return;
        };
        match ThemeStore::commit_preview(token, cx) {
            Ok(def) => {
                TlSettings::update(cx, |d| d.settings.theme.active = def.name.to_string());
                let text = self.words.saved.clone();
                let parent = self.parent;
                // Deferred: the parent may be the window running this update.
                cx.defer(move |cx| {
                    let pushed = parent
                        .update(cx, |_, window, cx| {
                            ToastQueue::of_window(window, cx).map(|queue| {
                                queue.update(cx, |q, cx| {
                                    q.push(ToastKind::Success, text.clone(), None, None, cx)
                                })
                            })
                        })
                        .ok()
                        .flatten();
                    if pushed.is_none() {
                        push_main_toast(cx, ToastKind::Success, text, None, None);
                    }
                });
            }
            Err(ThemeError::BuiltinCannotBeEdited) => {
                warn!(target: "tasklens::ui", "theme editor: base is a built-in theme");
            }
            Err(e) => warn!(target: "tasklens::ui", ?e, "theme not saved"),
        }
        window.remove_window();
    }

    fn select(&mut self, slot: ThemeSlot, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_slot != slot {
            self.selected_slot = slot;
            self.load_slot(window, cx);
            cx.notify();
        }
    }

    fn on_list_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let i = self.selected_slot as usize;
        let to = match ev.keystroke.key.as_str() {
            "up" if i > 0 => i - 1,
            "down" if i + 1 < ThemeSlot::ALL.len() => i + 1,
            _ => return,
        };
        self.select(ThemeSlot::ALL[to], window, cx);
        cx.stop_propagation();
    }

    /// The 4 row backgrounds the preview table painted last: normal, alternate, hover, selected.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn preview_row_bgs(&self) -> [Hsla; 4] {
        self.painted_rows
    }

    /// Random: new colours for all 23 slots, then the toast.
    fn on_random(&mut self, _: &ThemeEditorRandom, window: &mut Window, cx: &mut Context<Self>) {
        let Some(token) = &self.token else {
            return;
        };
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        ThemeStore::random_preview(token, seed, cx);
        self.load_slot(window, cx);
        push_toast(
            ToastKind::Success,
            self.words.random_toast.clone(),
            window,
            cx,
        );
        cx.notify();
    }

    /// Share: opens the Share Theme window with the preview as it is now, unsaved edits included.
    fn on_share(&mut self, _: &ThemeEditorShare, window: &mut Window, cx: &mut Context<Self>) {
        let Some(token) = &self.token else {
            return;
        };
        let def = ThemeStore::preview_def(token, cx);
        let parent = window.window_handle();
        cx.defer(move |cx| {
            open_share_window(&def, parent, cx);
        });
    }

    /// Import: loads the first theme of the clipboard into the preview (slots and name). The
    /// clipboard text is never logged.
    fn on_import(&mut self, _: &ThemeEditorImport, window: &mut Window, cx: &mut Context<Self>) {
        let defs = match import_from_clipboard(cx) {
            Ok(defs) if !defs.is_empty() => defs,
            result => {
                if let Err(e) = result {
                    tracing::debug!(target: "tasklens::ui", ?e, "clipboard theme not imported");
                }
                push_toast(ToastKind::Info, self.words.no_code.clone(), window, cx);
                return;
            }
        };
        let Some(token) = &self.token else {
            return;
        };
        let d = &defs[0];
        for slot in ThemeSlot::ALL {
            ThemeStore::set_preview_slot(token, slot, def_slot(d, slot), cx);
        }
        ThemeStore::set_preview_name(token, d.name.clone(), cx);
        self.name
            .update(cx, |s, cx| s.set_value(d.name.to_string(), window, cx));
        self.name_error = self.check_name(&d.name, cx);
        self.load_slot(window, cx);
        cx.notify();
    }

    /// The hex readouts of the current theme, rebuilt only when its style version changes.
    fn hexes(&mut self, cx: &App) -> &[SharedString] {
        let theme = current(cx);
        // Preview frames keep the base theme's `version`, so key on the style version.
        let version = metrics(cx).style_version;
        if self.hexes.0 != version {
            let hexes = ThemeSlot::ALL
                .iter()
                .map(|s| SharedString::from(rgb_hex(s.get(theme))))
                .collect();
            self.hexes = (version, hexes);
        }
        &self.hexes.1
    }
}

impl ThemeEditor {
    /// The preview column: mini tabs, a mini table and a static graph, painted from `theme`.
    fn preview_pane(
        &mut self,
        theme: &crate::theme::TlTheme,
        m: &crate::theme::UiMetrics,
    ) -> impl IntoElement {
        let s = m.ui_scale;
        let text = |t: SharedString, color: Hsla, xs: bool| {
            let mut d = div();
            *d.text_style() = if xs { t_xs(m) } else { t_sm(m) };
            d.text_color(color).child(t)
        };
        let tabs = div().flex().flex_row().gap(m.sp[2]).children(
            self.words
                .preview_tabs
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, t)| {
                    div()
                        .px(m.sp[4])
                        .py(m.sp[2])
                        .bg(theme.tab_bg)
                        .when(i == 0, |d| d.border_1().border_color(theme.accent))
                        .child(text(t, theme.text_primary, false))
                }),
        );
        let [c_name, c_cpu, c_mem] = self.words.preview_cols.clone();
        let cell = |t: SharedString, color: Hsla, grow: bool| {
            let d = div().overflow_hidden().child(text(t, color, false));
            if grow { d.flex_1() } else { d.w(px(60. * s)) }
        };
        let header = div()
            .flex()
            .flex_row()
            .px(m.sp[4])
            .bg(theme.header)
            .child(cell(c_name, theme.text_secondary, true))
            .child(cell(c_cpu, theme.text_secondary, false))
            .child(cell(c_mem, theme.text_secondary, false));
        let bgs = [
            theme.list_bg,
            theme.row_alt,
            theme.row_hover,
            theme.row_selected,
        ];
        self.painted_rows = bgs;
        let table = div().flex().flex_col().child(header).children(
            self.words
                .preview_rows
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, name)| {
                    let (primary, secondary) = if i == 3 {
                        (theme.text_selected_primary, theme.text_selected_secondary)
                    } else {
                        (theme.text_primary, theme.text_secondary)
                    };
                    div()
                        .flex()
                        .flex_row()
                        .px(m.sp[4])
                        .py(m.sp[2])
                        .bg(bgs[i])
                        .child(cell(name, primary, true))
                        .child(cell("1.2%".into(), secondary, false))
                        .child(cell("180 MB".into(), secondary, false))
                }),
        );
        let g = theme.graph;
        let graph = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.paint_quad(fill(bounds, g.chart_bg));
                let (w, h) = (bounds.size.width, bounds.size.height);
                for k in 1..=3 {
                    let y = bounds.origin.y + h * (k as f32 / 4.0);
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.origin.x, y), size(w, px(1.))),
                        g.grid,
                    ));
                }
                let at = |i: usize, v: f32| {
                    point(
                        bounds.origin.x + w * (i as f32 / (GRAPH_SAMPLES.len() - 1) as f32),
                        bounds.origin.y + h - h * v,
                    )
                };
                let base = |i: usize| at(i, 0.0);
                let last = GRAPH_SAMPLES.len() - 1;
                let mut area = PathBuilder::fill();
                area.move_to(base(0));
                for (i, v) in GRAPH_SAMPLES.iter().enumerate() {
                    area.line_to(at(i, *v));
                }
                area.line_to(base(last));
                area.close();
                if let Ok(path) = area.build() {
                    window.paint_path(path, g.cpu_fill);
                }
                let mut line = PathBuilder::stroke(px(1.));
                for (i, v) in GRAPH_SAMPLES.iter().enumerate() {
                    if i == 0 {
                        line.move_to(at(i, *v));
                    } else {
                        line.line_to(at(i, *v));
                    }
                }
                if let Ok(path) = line.build() {
                    window.paint_path(path, g.cpu_line);
                }
            },
        )
        .w(px(GRAPH_W * s))
        .h(px(GRAPH_H * s));
        div()
            .debug_selector(|| "ThemeEditor-preview".into())
            .w(px(PREVIEW_COLUMN_W * s))
            .flex_none()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .child(tabs)
            .child(table)
            .child(graph)
    }
}

impl Render for ThemeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let selected = self.selected_slot;
        let hexes: Vec<SharedString> = self.hexes(cx).to_vec();
        let _ = window;
        let (rgb_text, hsv_text) = self.readouts.clone();
        let hex_error = self.hex_error;
        let hint = self.words.hex_hint.clone();

        let invalid = self.name_error.is_some();
        let mut label = div();
        *label.text_style() = t_base(&m);
        let label = label
            .text_color(theme.text_primary)
            .child(self.words.name.clone());
        let top = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(m.sp[4])
            .h(px(TOP_ROW_H * s))
            .child(label)
            .child(
                div().flex_1().child(
                    TlTextInput::new("ThemeEditor-name", &self.name)
                        .invalid(invalid)
                        .aria_label(self.words.name.clone()),
                ),
            );
        let error = self.name_error.clone().map(|text| {
            let mut e = div();
            *e.text_style() = t_xs(&m);
            e.debug_selector(|| "ThemeEditor-name-error".into())
                .text_color(theme.status.danger)
                .child(text)
        });

        let mut rows = Vec::with_capacity(ThemeSlot::ALL.len());
        for (i, slot) in ThemeSlot::ALL.into_iter().enumerate() {
            let chosen = slot == selected;
            let (text_color, hex_color) = if chosen {
                (theme.text_selected_primary, theme.text_selected_primary)
            } else {
                (theme.text_primary, theme.text_secondary)
            };
            let mut name = div();
            *name.text_style() = t_base(&m);
            let mut hex = div();
            *hex.text_style() = t_xs(&m);
            hex.text_style().font_features = Some(TNUM.clone());
            rows.push(
                div()
                    .id(("ThemeEditor-slot", i))
                    .debug_selector(move || format!("ThemeEditor-slot-{i}"))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(m.sp[4])
                    .h(px(SLOT_ROW_H * s))
                    .px(m.sp[4])
                    .when(chosen, |d| d.bg(theme.row_selected))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.list_focus.focus(window, cx);
                            this.select(slot, window, cx);
                        }),
                    )
                    .child(
                        div()
                            .size(px(SWATCH * s))
                            .flex_none()
                            .rounded(m.radius_small)
                            .border_1()
                            .border_color(theme.divider)
                            .bg(slot.get(&theme)),
                    )
                    .child(
                        name.flex_1()
                            .text_color(text_color)
                            .child(self.words.slots[i].clone()),
                    )
                    .child(hex.text_color(hex_color).child(hexes[i].clone())),
            );
        }
        let list = div()
            .id("ThemeEditor-slots")
            .debug_selector(|| "ThemeEditor-slots".into())
            .track_focus(&self.list_focus)
            .on_key_down(cx.listener(|this, ev, window, cx| this.on_list_key(ev, window, cx)))
            .w(px(SLOT_COLUMN_W * s))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(theme.list_bg)
            .border_1()
            .border_color(theme.divider)
            .children(rows);

        let middle = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .gap(m.sp[4])
            .child(list)
            .child(
                div()
                    .flex_1()
                    .min_w(px(PICKER_MIN_W * s))
                    .flex()
                    .flex_col()
                    .gap(m.sp[4])
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(m.sp[4])
                            .children(self.plane.clone())
                            .children(self.hue.clone()),
                    )
                    .child(readout(rgb_text, &m, theme.text_primary))
                    .child(readout(hsv_text, &m, theme.text_primary))
                    .child(div().debug_selector(|| "ThemeEditor-hex".into()).children(
                        self.hex.as_ref().map(|hex| {
                            TlTextInput::new("ThemeEditor-hex-input", hex)
                                .placeholder(hint.clone())
                                .invalid(hex_error)
                                .aria_label(hint.clone())
                        }),
                    ))
                    .children(hex_error.then(|| {
                        let mut e = div();
                        *e.text_style() = t_xs(&m);
                        e.debug_selector(|| "ThemeEditor-hex-error".into())
                            .text_color(theme.status.danger)
                            .child(hint.clone())
                    })),
            )
            .child(self.preview_pane(&theme, &m));

        let footer = div()
            .debug_selector(|| "ThemeEditor-footer".into())
            .flex()
            .flex_row()
            .items_center()
            .h(px(FOOTER_H * s))
            .gap(m.sp[4])
            .child(
                TlButton::new("ThemeEditor-random", TlButtonKind::Secondary)
                    .label(self.words.random.clone())
                    .action(Box::new(ThemeEditorRandom)),
            )
            .child(
                TlButton::new("ThemeEditor-share", TlButtonKind::Secondary)
                    .label(self.words.share.clone())
                    .action(Box::new(ThemeEditorShare)),
            )
            .child(
                TlButton::new("ThemeEditor-import", TlButtonKind::Secondary)
                    .label(self.words.import.clone())
                    .action(Box::new(ThemeEditorImport)),
            )
            .child(div().flex_1())
            .child(
                TlButton::new("ThemeEditor-cancel", TlButtonKind::Secondary)
                    .label(self.words.cancel.clone())
                    .focus_handle(self.cancel_focus.clone())
                    .action(Box::new(CloseSecondaryWindow)),
            )
            .child(
                TlButton::new("ThemeEditor-done", TlButtonKind::Primary)
                    .label(self.words.done.clone())
                    .disabled(invalid)
                    .action(Box::new(ThemeEditorDone)),
            );

        div()
            .id("ThemeEditor")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_done))
            .on_action(cx.listener(Self::on_random))
            .on_action(cx.listener(Self::on_share))
            .on_action(cx.listener(Self::on_import))
            .size_full()
            .flex()
            .flex_col()
            .p(m.sp[6])
            .gap(m.sp[4])
            .child(top)
            .children(error)
            .child(middle)
            .child(footer)
    }
}

/// A readout line in small tabular text.
fn readout(text: SharedString, m: &crate::theme::UiMetrics, color: Hsla) -> impl IntoElement {
    let mut d = div();
    *d.text_style() = t_sm(m);
    d.text_style().font_features = Some(TNUM.clone());
    d.text_color(color).child(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_rgb_roundtrip() {
        for r in (0..=255u16).step_by(15) {
            for g in (0..=255u16).step_by(15) {
                for b in (0..=255u16).step_by(15) {
                    let rgb = [r as u8, g as u8, b as u8];
                    let (h, s, v) = rgb_to_hsv(rgb);
                    assert_eq!(hsv_to_rgb(h, s, v), rgb, "{rgb:?}");
                }
            }
        }
    }

    #[test]
    fn hex6_accepts_six_digits_only() {
        assert!(hex6("1E1F22").is_some());
        assert!(hex6(" #1e1f22 ").is_some());
        assert!(hex6("12345").is_none());
        assert!(hex6("#1E1F22FF").is_none());
        assert!(hex6("#GGGGGG").is_none());
    }
}
