//! "tasklens Theme Editor": the window frame, the name input, the 23-row slot list and the
//! Cancel and Done buttons (UI-SHL-081, APP-037, APP-035, APP-045, R-083, UI-FND-043).
//!
//! The window is a floating Tool window. It starts a live preview with `ThemeStore::begin_preview`
//! and holds the token. Cancel, the close button, Esc and Ctrl+W end the preview through the
//! window-closed hook, so every way of closing cancels. Done checks the name first and then
//! commits. E21-028 fills the picker column and E21-029 the preview column and the Random, Share
//! and Import buttons.

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, ParentElement,
    Render, Rgba, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowHandle, div, gpui, prelude::FluentBuilder as _, px, size,
};
use serde::Deserialize;
use tracing::warn;

use super::main::{ShellState, push_main_toast};
use super::registry::{GlobalRegistry, WindowKey};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::input::TlTextInput;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::text::style::{TNUM, t_base, t_xs};
use crate::theme::preview::PreviewToken;
use crate::theme::{ThemeDef, ThemeError, ThemeSlot, ThemeStore, current, metrics};

/// The key context of the window content.
pub const CONTEXT: &str = "TlThemeEditor";

/// Window size and minimum size, × ui_scale.
const SIZE: (f32, f32) = (820., 560.);
const MIN_SIZE: (f32, f32) = (720., 480.);
/// Column widths and row heights, × ui_scale.
const SLOT_COLUMN_W: f32 = 240.;
const PICKER_MIN_W: f32 = 280.;
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
const TOAST_SAVED: &str = "shell-toast-theme-saved";
const TOAST_COPY_FIRST: &str = "shell-toast-theme-copy-first";

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
        ThemeEditorDone
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
    slots: Vec<SharedString>,
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
    // E21-028: plane, hue, hex
    parent: AnyWindowHandle,
    list_focus: FocusHandle,
    cancel_focus: FocusHandle,
    words: EditorWords,
    strings_version: u64,
    /// The 23 hex readouts of the theme with this `version`.
    hexes: (u64, Vec<SharedString>),
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
        Self {
            token: Some(token),
            base_name: def.name.clone(),
            selected_slot: ThemeSlot::WindowBg,
            name,
            name_error: None,
            parent,
            list_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            words: EditorWords::new(cx),
            strings_version: strings_version(cx),
            hexes: (u64::MAX, Vec::new()),
            _subs: subs,
        }
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

    fn select(&mut self, slot: ThemeSlot, cx: &mut Context<Self>) {
        if self.selected_slot != slot {
            self.selected_slot = slot;
            cx.notify();
        }
    }

    fn on_list_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let i = self.selected_slot as usize;
        let to = match ev.keystroke.key.as_str() {
            "up" if i > 0 => i - 1,
            "down" if i + 1 < ThemeSlot::ALL.len() => i + 1,
            _ => return,
        };
        self.select(ThemeSlot::ALL[to], cx);
        cx.stop_propagation();
    }

    /// The hex readouts of the current theme, rebuilt only when its version changes.
    fn hexes(&mut self, cx: &App) -> &[SharedString] {
        let theme = current(cx);
        if self.hexes.0 != theme.version {
            let hexes = ThemeSlot::ALL
                .iter()
                .map(|s| SharedString::from(rgb_hex(s.get(theme))))
                .collect();
            self.hexes = (theme.version, hexes);
        }
        &self.hexes.1
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
                            this.select(slot, cx);
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
            .on_key_down(cx.listener(|this, ev, _, cx| this.on_list_key(ev, cx)))
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
                // E21-028 fills this column with the picker.
                div().flex_1().min_w(px(PICKER_MIN_W * s)),
            )
            .child(
                // E21-029 fills this column with the preview.
                div().w(px(PREVIEW_COLUMN_W * s)).flex_none(),
            );

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
                    .disabled(true),
            )
            .child(
                TlButton::new("ThemeEditor-share", TlButtonKind::Secondary)
                    .label(self.words.share.clone())
                    .disabled(true),
            )
            .child(
                TlButton::new("ThemeEditor-import", TlButtonKind::Secondary)
                    .label(self.words.import.clone())
                    .disabled(true),
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
