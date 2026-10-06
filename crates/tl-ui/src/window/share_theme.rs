//! "tasklens Share Theme": a theme's `TL1.` code in a read-only box with a Copy button
//! (UI-SHL-082, APP-038, APP-045, UI-FND-043 rule 8).
//!
//! The window is a Confirmation-kind `TlSecondaryWindow` (GPUI `WindowKind::Dialog`), modal to
//! its parent: the main window, or the theme editor, which passes its preview definition. Focus
//! starts on Copy, so Enter copies; the focused footer button takes Enter through the base
//! `Button`'s own keyboard click. The code is selected on open, and Ctrl+C copies the whole code
//! anywhere in the window. Esc and Ctrl+W close it through the frame.
//!
//! The box is the kit `Textarea` with the `TlTextInput` border and fill, because `TlTextInput` is
//! single-line.

use gpui_kit::base::Root;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle,
    Focusable as _, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render,
    SharedString, Styled, Subscription, Window, div, gpui, px, size,
};
use serde::Deserialize;
use tracing::warn;

use super::main::ShellState;
use super::registry::{GlobalRegistry, OpenOutcome, WindowKey};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::i18n::{Strings, tr};
use crate::text::style::{t_base, t_sm};
use crate::theme::share_code::encode_tl1;
use crate::theme::{ThemeDef, ThemeStore, current, metrics};

/// The key context of the window content.
pub const CONTEXT: &str = "TlShareTheme";

/// Window size, × ui_scale.
const SIZE: (f32, f32) = (520., 220.);
/// Lines of the code box.
const CODE_ROWS: usize = 3;

const TITLE: &str = "shell-share-theme-title";
const TEXT: &str = "shell-share-theme-text";
const COPY: &str = "shell-share-theme-copy";
const CLOSE: &str = "shell-share-theme-close";
const COPIED: &str = "shell-toast-theme-code-copied";

/// Opens "tasklens Share Theme" for the named theme, user themes first (View > Theme > Share
/// theme, E21-031).
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ShareTheme {
    pub name: SharedString,
}

gpui_kit::actions!(
    tasklens,
    [
        /// Writes the window's code to the clipboard and shows "Theme code copied" (Copy, and
        /// Ctrl+C outside the code box).
        CopyThemeCode
    ]
);

/// Binds Ctrl+C in `TlShareTheme` and handles `ShareTheme`. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("ctrl-c", CopyThemeCode, Some(CONTEXT))]);
    // Deferred: from a menu in the main window, that window is mid-update and its bounds cannot
    // be read to centre the new window.
    cx.on_action(|a: &ShareTheme, cx| {
        let name = a.name.clone();
        cx.defer(move |cx| share_named(&name, cx));
    });
}

fn share_named(name: &str, cx: &mut App) {
    let Some(def) = cx
        .try_global::<ThemeStore>()
        .and_then(|s| s.resolve(name))
        .map(|(def, _)| def.clone())
    else {
        warn!(target: "tasklens::ui", name, "share: no such theme");
        return;
    };
    let Some(parent) = main_or_active(cx) else {
        warn!(target: "tasklens::ui", "share: no parent window");
        return;
    };
    open_share_window(&def, parent, cx);
}

/// The open main window, else the active window.
fn main_or_active(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<ShellState>()
        .and_then(|s| s.main_window)
        .map(AnyWindowHandle::from)
        .filter(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
        .or_else(|| cx.active_window())
}

/// Opens the share window for `def`, modal to `parent`, or shows `def`'s code in the open one.
/// Returns the window, or `None` when it could not open. Without a window registry (tests) it
/// always opens a window.
pub fn open_share_window(
    def: &ThemeDef,
    parent: AnyWindowHandle,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Confirmation,
        tr(cx, TITLE),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .parent(parent)
    .key(WindowKey::ShareTheme);
    let code: SharedString = encode_tl1(def).into();
    let open_code = code.clone();
    let open = move |cx: &mut App| {
        TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| ShareThemeView::new(open_code, window, cx))
        })
        .map(Into::into)
    };
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
        return open(cx).ok();
    };
    let outcome = registry.update(cx, |r, cx| r.open_or_focus(WindowKey::ShareTheme, cx, open));
    let handle = registry.read(cx).get(&WindowKey::ShareTheme)?;
    if outcome == OpenOutcome::Focused
        && let Some(frame) = handle
            .downcast::<Root>()
            .and_then(|h| TlSecondaryWindow::<ShareThemeView>::find(h, cx))
    {
        let view = frame.read(cx).content().clone();
        let _ = handle.update(cx, |_, window, cx| {
            view.update(cx, |v, cx| v.set_code(code, window, cx));
        });
    }
    Some(handle)
}

/// The window's own text, translated once per strings version (R-132), so a frame makes no `tr`
/// call. The title is set once, at open.
struct ShareWords {
    text: SharedString,
    copy: SharedString,
    close: SharedString,
    copied: SharedString,
}

impl ShareWords {
    fn new(cx: &App) -> Self {
        Self {
            text: tr(cx, TEXT),
            copy: tr(cx, COPY),
            close: tr(cx, CLOSE),
            copied: tr(cx, COPIED),
        }
    }
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

/// The content of the share window.
pub struct ShareThemeView {
    code: SharedString,
    input: Entity<TextareaState>,
    copy_focus: FocusHandle,
    words: ShareWords,
    strings_version: u64,
    _strings: Subscription,
}

impl ShareThemeView {
    fn new(code: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).rows(CODE_ROWS));
        let copy_focus = cx.focus_handle();
        copy_focus.focus(window, cx);
        let strings = cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.words = ShareWords::new(cx);
                cx.notify();
            }
        });
        let mut this = Self {
            code: SharedString::default(),
            input,
            copy_focus,
            words: ShareWords::new(cx),
            strings_version: strings_version(cx),
            _strings: strings,
        };
        this.set_code(code, window, cx);
        this
    }

    /// Shows `code` in the box, selected, so Ctrl+C there copies all of it.
    fn set_code(&mut self, code: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.code = code.clone();
        self.input.update(cx, |s, cx| {
            s.set_value(code, window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    /// The `TL1.` code shown.
    pub fn code(&self) -> &SharedString {
        &self.code
    }

    /// The text selected in the code box.
    pub fn selected_text(&self, cx: &App) -> String {
        self.input.read(cx).selected_value().to_string()
    }

    /// Whether Copy has focus.
    pub fn copy_focused(&self, window: &Window) -> bool {
        self.copy_focus.is_focused(window)
    }

    fn on_copy(&mut self, _: &CopyThemeCode, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.code.to_string()));
        let text = self.words.copied.clone();
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
        }
    }
}

impl Render for ShareThemeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);

        let mut text = div();
        *text.text_style() = t_base(&m);
        let text = text
            .debug_selector(|| "ShareTheme-text".into())
            .text_color(theme.text_primary)
            .child(self.words.text.clone());

        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let mut code = Textarea::new(&self.input)
            .readonly(true)
            .appearance(true)
            .bordered(false)
            .aria_label(self.words.text.clone());
        *code.text_style() = t_sm(&m);
        let code = code
            .px(m.sp[4])
            .bg(if focused {
                theme.input_focus
            } else {
                theme.input
            })
            .text_color(theme.text_primary);
        let code = div()
            .debug_selector(|| "ShareTheme-code".into())
            .w_full()
            .rounded(m.radius_small)
            .border_1()
            .border_color(if focused { theme.accent } else { theme.divider })
            .overflow_hidden()
            .child(code);

        let footer = div()
            .debug_selector(|| "ShareTheme-footer".into())
            .flex()
            .flex_row()
            .justify_end()
            .gap(m.sp[4])
            .child(
                TlButton::new("ShareTheme-close", TlButtonKind::Secondary)
                    .label(self.words.close.clone())
                    .action(Box::new(CloseSecondaryWindow)),
            )
            .child(
                TlButton::new("ShareTheme-copy", TlButtonKind::Primary)
                    .label(self.words.copy.clone())
                    .focus_handle(self.copy_focus.clone())
                    .action(Box::new(CopyThemeCode)),
            );

        div()
            .id("ShareTheme")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_copy))
            .size_full()
            .flex()
            .flex_col()
            .p(m.sp[6])
            .gap(m.sp[5])
            .child(text)
            .child(code)
            .child(div().flex_1())
            .child(footer)
    }
}
