//! `TlDialog`: the confirmation window every "are you sure" prompt uses (UI-FND-021, R-104,
//! UI-SHL-091, APP-045, R-083).
//!
//! `TlDialog::open` opens a `TlSecondaryWindow` of kind Confirmation, modal to the parent. The
//! body shows an icon, an optional heading, the text, the owner's extra lines and an optional
//! checkbox; the footer shows Cancel left of the action button, with an optional second action
//! between them.
//!
//! One focus and Enter rule for every confirmation (R-104): focus starts on Cancel in a
//! destructive confirmation and on the action button otherwise. Enter activates the focused
//! button, so a stray Enter in a destructive confirmation cancels. With focus off the buttons,
//! in a text field or on the checkbox, Enter runs the button that has focus on open. Esc, Ctrl+W
//! and the window's close button cancel. `on_result` runs exactly once, in the parent window.
//!
//! A text-input dialog ([`TlDialog::text_input`]) adds a `TlTextInput` under the text. Focus
//! starts in the field with its text selected. Enter in the field confirms, also in a destructive
//! dialog, and the result is [`DialogResult::ConfirmedText`] with the field's text.
//!
//! Enter runs once. The `TlDialog` binding handles Enter on a footer button and stops it, so the
//! button's own key-up click never comes. A `TlButton` in the owner's lines gets the key and
//! clicks itself; the binding does not run the default then.
//!
//! The footer buttons are drawn here on the kit's base `Button` with the `TlButton` look, because
//! `TlButton` keeps its focus handle in element state and the dialog must focus a button on open
//! and know which one has focus when Enter is pressed.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, ElementExt as _};
use gpui_kit::component::input::InputState;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, ClickEvent, Context, ElementId, Entity,
    FocusHandle, Focusable as _, FontWeight, Global, Hsla, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rgb, size,
};

use super::checkbox::TlCheckbox;
use super::focus::TlFocusRing;
use super::icons::{TlIcon, icon};
use super::input::TlTextInput;
use super::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec, frame_bottom,
};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The key context of the dialog content.
pub const CONTEXT: &str = "TlDialog";

/// The default window title (APP-045, R-083).
pub const DEFAULT_TITLE: &str = "tasklens";

/// The Cancel button label.
pub const CANCEL_LABEL: &str = "Cancel";

/// Window width, × ui_scale.
const WIDTH: f32 = 420.;
/// The window height follows the content between these, × ui_scale.
const MIN_HEIGHT: f32 = 140.;
const MAX_HEIGHT: f32 = 320.;
/// The text wraps to at most this many lines.
const MAX_TEXT_LINES: usize = 6;
/// Footer button height and minimum width, × ui_scale (`TlButton` md).
const BUTTON_HEIGHT: f32 = 28.;
const BUTTON_MIN_WIDTH: f32 = 72.;
/// Hover lightens Primary and Danger by this much; pressed darkens by the same (`TlButton`).
const STATE_STEP: f32 = 0.08;

gpui_kit::actions!(
    tasklens,
    [
        /// Enter in a confirmation: activates the focused footer button.
        ActivateFocusedButton
    ]
);

/// Binds Enter in `TlDialog`. `TlDialog::open` calls it once.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "enter",
        ActivateFocusedButton,
        Some(CONTEXT),
    )]);
}

/// Set once the dialog keys are bound.
struct DialogKeysBound;

impl Global for DialogKeysBound {}

/// The leading icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogIcon {
    /// Lucide `triangle-alert` in `status.warning`, for destructive actions.
    Warning,
    /// Lucide `info` in `status.info`.
    Info,
    None,
}

/// How a confirmation ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogResult {
    /// The action button ran. `checkbox` is the checkbox value, `false` without one.
    Confirmed { checkbox: bool },
    /// The action button of a text-input dialog ran (see [`TlDialog::text_input`]): the text
    /// in the field.
    ConfirmedText(String),
    /// The second action button ran (see [`TlDialog::second_action`]).
    Second,
    /// Cancel, Esc, Ctrl+W or the window's close button.
    Cancelled,
}

/// The footer buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogButton {
    Cancel,
    Action,
    Second,
}

/// The focus handles of the footer buttons.
pub struct DialogFocus {
    pub cancel: FocusHandle,
    pub action: FocusHandle,
    pub second: FocusHandle,
}

impl DialogFocus {
    pub fn new(cx: &mut App) -> Self {
        Self {
            cancel: cx.focus_handle(),
            action: cx.focus_handle(),
            second: cx.focus_handle(),
        }
    }

    /// The button that has focus when the dialog opens (R-104).
    pub fn initial(&self, destructive: bool) -> &FocusHandle {
        if destructive {
            &self.cancel
        } else {
            &self.action
        }
    }

    /// The footer button that has focus now.
    pub fn focused(&self, window: &Window) -> Option<DialogButton> {
        if self.cancel.is_focused(window) {
            Some(DialogButton::Cancel)
        } else if self.action.is_focused(window) {
            Some(DialogButton::Action)
        } else if self.second.is_focused(window) {
            Some(DialogButton::Second)
        } else {
            None
        }
    }
}

/// The text field of a text-input dialog.
struct DialogInput {
    initial: SharedString,
    placeholder: SharedString,
    /// The accessible name. Without it the window title is the name.
    label: Option<SharedString>,
    /// The field's state, made when the window opens.
    state: Option<Entity<InputState>>,
}

type ExtraBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type ResultHandler = Box<dyn FnOnce(DialogResult, &mut Window, &mut App)>;

/// A confirmation: body and footer. Open it with [`TlDialog::open`].
pub struct TlDialog {
    text: SharedString,
    heading: Option<SharedString>,
    icon: DialogIcon,
    action_label: SharedString,
    destructive: bool,
    /// A second action button right of the action: label and whether it is Danger.
    second: Option<(SharedString, bool)>,
    extra: Option<ExtraBuilder>,
    checkbox: Option<(SharedString, bool)>,
    title: SharedString,
    /// False for a notice with only the action button (see [`TlDialog::without_cancel`]).
    cancel: bool,
    cancel_label: SharedString,
    input: Option<DialogInput>,
}

impl TlDialog {
    /// A non-destructive confirmation titled "tasklens" with the Info icon. `action_label` names
    /// the result ("Delete", "Reset"), never "Yes".
    pub fn confirm(text: impl Into<SharedString>, action_label: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            heading: None,
            icon: DialogIcon::Info,
            action_label: action_label.into(),
            destructive: false,
            second: None,
            extra: None,
            checkbox: None,
            title: DEFAULT_TITLE.into(),
            cancel: true,
            cancel_label: CANCEL_LABEL.into(),
            input: None,
        }
    }

    /// A Danger action button, the Warning icon and focus on Cancel.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self.icon = DialogIcon::Warning;
        self
    }

    /// A second action button between Cancel and the action button, Danger when `danger` is
    /// set. It ends the dialog with [`DialogResult::Second`] (UI-PRC-014, the window picker's
    /// "Terminate"). It never has focus on open, so Enter never runs it by default.
    pub fn second_action(mut self, label: impl Into<SharedString>, danger: bool) -> Self {
        self.second = Some((label.into(), danger));
        self
    }

    /// A one-line `t_md` 500 heading above the text.
    pub fn heading(mut self, text: impl Into<SharedString>) -> Self {
        self.heading = Some(text.into());
        self
    }

    /// The window title. Default "tasklens" (APP-045, R-083).
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    /// Replaces the leading icon.
    pub fn icon(mut self, icon: DialogIcon) -> Self {
        self.icon = icon;
        self
    }

    /// The owner's lines under the text (an entry name, a path, a target list), in `t_base` or
    /// `t_sm`. `build` runs on every render, because an element can be drawn only once.
    pub fn extra(mut self, build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        self.extra = Some(Rc::new(build));
        self
    }

    /// A `TlCheckbox` under the text, such as "Do not ask again" where a spec allows it.
    pub fn checkbox(mut self, label: impl Into<SharedString>, initial: bool) -> Self {
        self.checkbox = Some((label.into(), initial));
        self
    }

    /// A text field under the text, holding `initial` with `placeholder` shown while it is
    /// empty. Focus starts in the field with the text selected, Enter in the field confirms and
    /// the action button ends the dialog with [`DialogResult::ConfirmedText`].
    pub fn text_input(
        mut self,
        initial: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        self.input = Some(DialogInput {
            initial: initial.into(),
            placeholder: placeholder.into(),
            label: None,
            state: None,
        });
        self
    }

    /// The accessible name of the text field. Default: the window title. Without
    /// [`text_input`](Self::text_input) this does nothing.
    pub fn input_label(mut self, label: impl Into<SharedString>) -> Self {
        if let Some(input) = self.input.as_mut() {
            input.label = Some(label.into());
        }
        self
    }

    /// The text field's state while the dialog is open, `None` without a field.
    pub fn input_state(&self) -> Option<&Entity<InputState>> {
        self.input.as_ref().and_then(|input| input.state.as_ref())
    }

    /// Drops the Cancel button, for a notice whose one button closes it (the action tracker's
    /// Details list, R-197). Esc, Ctrl+W and the close button still end it as Cancelled.
    pub fn without_cancel(mut self) -> Self {
        self.cancel = false;
        self
    }

    /// The Cancel button's label. Default "Cancel"; the owner passes its translated text.
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    pub fn cancel_text(&self) -> &SharedString {
        &self.cancel_label
    }

    pub fn has_cancel(&self) -> bool {
        self.cancel
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }

    pub fn is_destructive(&self) -> bool {
        self.destructive
    }

    pub fn icon_kind(&self) -> DialogIcon {
        self.icon
    }

    pub fn window_title(&self) -> &SharedString {
        &self.title
    }

    /// Opens the confirmation as a modal window of `parent`. `on_result` runs once, in the
    /// parent window, when the dialog ends; it does not run if the parent window is gone by
    /// then. If the window cannot open this returns the error and never calls `on_result`; the
    /// owner treats that as Cancelled.
    pub fn open(
        self,
        parent: &mut Window,
        cx: &mut App,
        on_result: impl FnOnce(DialogResult, &mut Window, &mut App) + 'static,
    ) -> gpui_kit::Result<()> {
        if !cx.has_global::<DialogKeysBound>() {
            bind_keys(cx);
            cx.set_global(DialogKeysBound);
        }
        let s = metrics(cx).ui_scale;
        let parent_handle = parent.window_handle();
        let spec = TlWindowSpec::new(
            TlWindowKind::Confirmation,
            self.title.clone(),
            size(px(WIDTH * s), px(MIN_HEIGHT * s)),
        )
        .parent(parent_handle);
        let mut dialog = self;
        TlSecondaryWindow::open(cx, spec, move |window, cx| {
            let focus = DialogFocus::new(cx);
            match dialog.input.as_mut() {
                Some(input) => {
                    let state = cx.new(|cx| InputState::new(window, cx));
                    let initial = input.initial.clone();
                    state.update(cx, |s, cx| {
                        s.set_value(initial, window, cx);
                        s.select_all(window, cx);
                        s.focus(window, cx);
                    });
                    input.state = Some(state);
                }
                None => focus.initial(dialog.destructive).focus(window, cx),
            }
            cx.new(|cx| {
                TlDialogView::new(
                    dialog,
                    focus,
                    parent_handle,
                    Box::new(on_result),
                    window,
                    cx,
                )
            })
        })?;
        // The frame cannot refresh the parent while the parent is being updated.
        parent.refresh();
        Ok(())
    }

    /// The body and footer. The confirmation window renders it; an in-window kit `Dialog` for a
    /// sub-prompt inside a secondary window reuses it. `checked` is the checkbox value now,
    /// `on_check` gets each new value, and `on_result` runs when a footer button is clicked.
    pub fn render_body(
        &self,
        focus: &DialogFocus,
        checked: bool,
        on_check: impl Fn(bool, &mut Window, &mut App) + 'static,
        on_result: impl Fn(DialogResult, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);

        let lead = match self.icon {
            DialogIcon::Warning => Some((TlIcon::TriangleAlert, theme.status.warning)),
            DialogIcon::Info => Some((TlIcon::Info, theme.status.info)),
            DialogIcon::None => None,
        };
        let lead = lead.map(|(name, color)| {
            div()
                .debug_selector(|| "TlDialogIcon".into())
                .flex_none()
                .child(icon(name, m.icon.lg, color))
        });

        let heading = self.heading.clone().map(|text| {
            let mut el = div();
            *el.text_style() = label_style(&m, m.text.md, FontWeight::MEDIUM);
            el.debug_selector(|| "TlDialogHeading".into())
                .text_color(theme.text_primary)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text)
        });
        let mut text = div();
        *text.text_style() = label_style(&m, m.text.base, FontWeight::NORMAL);
        let text = text
            .debug_selector(|| "TlDialogText".into())
            .text_color(theme.text_primary)
            .line_clamp(MAX_TEXT_LINES)
            .text_ellipsis()
            .child(self.text.clone());
        let extra = self.extra.as_ref().map(|build| build(window, cx));
        let input = self.input.as_ref().and_then(|input| {
            let state = input.state.as_ref()?;
            Some(
                TlTextInput::new("TlDialogInput", state)
                    .placeholder(input.placeholder.clone())
                    .aria_label(input.label.clone().unwrap_or_else(|| self.title.clone())),
            )
        });
        let checkbox = self.checkbox.as_ref().map(|(label, _)| {
            TlCheckbox::new("TlDialogCheckbox")
                .label(label.clone())
                .checked(Some(checked))
                .on_change(on_check)
        });

        let on_result = Rc::new(on_result);
        let cancel = {
            let on_result = on_result.clone();
            footer_button(
                "TlDialogCancel",
                self.cancel_label.clone(),
                FooterKind::Secondary,
                &focus.cancel,
                move |_, window, cx| on_result(DialogResult::Cancelled, window, cx),
                &theme,
                &m,
                window,
            )
        };
        let second = self.second.as_ref().map(|(label, danger)| {
            let on_result = on_result.clone();
            footer_button(
                "TlDialogSecond",
                label.clone(),
                if *danger {
                    FooterKind::Danger
                } else {
                    FooterKind::Primary
                },
                &focus.second,
                move |_, window, cx| on_result(DialogResult::Second, window, cx),
                &theme,
                &m,
                window,
            )
        });
        let input_state = self.input_state().cloned();
        let action = footer_button(
            "TlDialogAction",
            self.action_label.clone(),
            if self.destructive {
                FooterKind::Danger
            } else {
                FooterKind::Primary
            },
            &focus.action,
            move |_, window, cx| {
                let result = confirmed(checked, input_state.as_ref(), cx);
                on_result(result, window, cx)
            },
            &theme,
            &m,
            window,
        );

        div()
            .debug_selector(|| "TlDialogBody".into())
            .flex()
            .flex_col()
            .w_full()
            .p(m.sp[6])
            .gap(m.sp[6])
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(m.sp[5])
                    .children(lead)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(m.sp[3])
                            .children(heading)
                            .child(text)
                            .children(extra)
                            .children(input)
                            .children(checkbox),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "TlDialogFooter".into())
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(m.sp[4])
                    .when(self.cancel, |footer| footer.child(cancel))
                    .children(second)
                    .child(action),
            )
            .into_any_element()
    }
}

/// The look of a footer button, as the matching `TlButtonKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FooterKind {
    Secondary,
    Primary,
    Danger,
}

/// Colours of a footer button.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FooterLook {
    bg: Hsla,
    hover: Hsla,
    pressed: Hsla,
    border: Option<Hsla>,
    fg: Hsla,
    weight: FontWeight,
}

/// `c` with lightness moved by `step`, clamped to 0..=1.
fn lightness(c: Hsla, step: f32) -> Hsla {
    Hsla {
        l: (c.l + step).clamp(0., 1.),
        ..c
    }
}

/// The `TlButton` colours of `kind` in `theme`.
fn footer_look(kind: FooterKind, theme: &TlTheme) -> FooterLook {
    let filled = |fill: Hsla| FooterLook {
        bg: fill,
        hover: lightness(fill, STATE_STEP),
        pressed: lightness(fill, -STATE_STEP),
        border: None,
        fg: rgb(0xFFFFFF).into(),
        weight: FontWeight::MEDIUM,
    };
    match kind {
        FooterKind::Primary => filled(theme.accent),
        FooterKind::Danger => filled(theme.status.danger),
        FooterKind::Secondary => FooterLook {
            bg: theme.toolbar,
            hover: theme.tab_hover,
            pressed: theme.header,
            border: Some(theme.divider),
            fg: theme.text_primary,
            weight: FontWeight::NORMAL,
        },
    }
}

/// A md footer button with the `TlButton` look, tracking `focus`.
#[allow(clippy::too_many_arguments)]
fn footer_button(
    id: &'static str,
    label: SharedString,
    kind: FooterKind,
    focus: &FocusHandle,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    theme: &TlTheme,
    m: &UiMetrics,
    window: &Window,
) -> AnyElement {
    let look = footer_look(kind, theme);
    let mut text = div();
    *text.text_style() = label_style(m, m.text.base, look.weight);
    let text = text.whitespace_nowrap().child(label.clone());
    let button = BaseButton::new(ElementId::Name(id.into()))
        .track_focus(focus)
        .debug_selector(move || id.into())
        .relative()
        .flex_none()
        .h(px(BUTTON_HEIGHT * m.ui_scale))
        .min_w(px(BUTTON_MIN_WIDTH * m.ui_scale))
        .px(m.sp[5])
        .rounded(m.radius_small)
        .bg(look.bg)
        .text_color(look.fg)
        .when_some(look.border, |b, border| b.border_1().border_color(border))
        .accessibility_label(label)
        .hover(move |s| s.bg(look.hover))
        .active(move |s| s.bg(look.pressed))
        .on_click(on_click)
        .child(text);
    if TlFocusRing::show(focus.is_focused(window), window) {
        TlFocusRing::ring(button, m.radius_small, theme).into_any_element()
    } else {
        button.into_any_element()
    }
}

/// The result of the action button: the field's text in a text-input dialog.
fn confirmed(checked: bool, input: Option<&Entity<InputState>>, cx: &App) -> DialogResult {
    match input {
        Some(state) => DialogResult::ConfirmedText(state.read(cx).text().to_string()),
        None => DialogResult::Confirmed { checkbox: checked },
    }
}

/// The result of Enter when `focused` has focus. With no footer button focused, Enter runs the
/// button that has focus on open: Cancel in a destructive confirmation, the action otherwise.
fn enter_result(focused: Option<DialogButton>, destructive: bool, checked: bool) -> DialogResult {
    let button = focused.unwrap_or(if destructive {
        DialogButton::Cancel
    } else {
        DialogButton::Action
    });
    match button {
        DialogButton::Cancel => DialogResult::Cancelled,
        DialogButton::Action => DialogResult::Confirmed { checkbox: checked },
        DialogButton::Second => DialogResult::Second,
    }
}

/// Whether focus is on a `TlButton`, which activates itself on Enter.
fn tl_button_focused(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|c| c.contains("TlButton"))
}

/// The content view of a confirmation window.
pub struct TlDialogView {
    dialog: TlDialog,
    focus: DialogFocus,
    checked: bool,
    parent: AnyWindowHandle,
    on_result: Option<ResultHandler>,
    /// The window height last asked for, so the fit runs once per change.
    fitted: Rc<Cell<Option<Pixels>>>,
    _closed: Subscription,
}

impl TlDialogView {
    /// The window this confirmation is modal to.
    pub fn parent(&self) -> AnyWindowHandle {
        self.parent
    }

    fn new(
        dialog: TlDialog,
        focus: DialogFocus,
        parent: AnyWindowHandle,
        on_result: ResultHandler,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Closing the window any other way (the close button, the frame's own Esc) cancels.
        let id = window.window_handle().window_id();
        let view = cx.entity().downgrade();
        let closed = cx.on_window_closed(move |cx, closed| {
            if closed == id {
                view.update(cx, |this, cx| this.deliver(DialogResult::Cancelled, cx))
                    .ok();
            }
        });
        cx.on_release(|this, cx| this.deliver(DialogResult::Cancelled, cx))
            .detach();
        let checked = dialog.checkbox.as_ref().is_some_and(|(_, on)| *on);
        Self {
            dialog,
            focus,
            checked,
            parent,
            on_result: Some(on_result),
            fitted: Rc::default(),
            _closed: closed,
        }
    }

    pub fn dialog(&self) -> &TlDialog {
        &self.dialog
    }

    /// The checkbox value now.
    pub fn checked(&self) -> bool {
        self.checked
    }

    /// The footer button that has focus now.
    pub fn focused_button(&self, window: &Window) -> Option<DialogButton> {
        self.focus.focused(window)
    }

    /// Runs `on_result` in the parent window, once.
    fn deliver(&mut self, result: DialogResult, cx: &mut App) {
        let Some(on_result) = self.on_result.take() else {
            return;
        };
        let parent = self.parent;
        cx.defer(move |cx| {
            if parent
                .update(cx, |_, window, cx| on_result(result, window, cx))
                .is_err()
            {
                tracing::debug!(target: "tasklens::ui", "confirmation parent closed first");
            }
        });
    }

    /// Ends the dialog with `result` and closes its window.
    fn finish(&mut self, result: DialogResult, window: &mut Window, cx: &mut App) {
        if self.on_result.is_none() {
            return;
        }
        self.deliver(result, cx);
        window.remove_window();
    }

    fn on_enter(&mut self, _: &ActivateFocusedButton, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focus.focused(window);
        if focused.is_none() && tl_button_focused(window) {
            cx.propagate();
            return;
        }
        // The kit field lets Enter go on to this binding. Enter in the dialog's own field
        // confirms, also in a destructive dialog.
        let input = self.dialog.input_state().cloned();
        let in_field = input
            .as_ref()
            .is_some_and(|state| state.focus_handle(cx).is_focused(window));
        let result = if in_field {
            confirmed(self.checked, input.as_ref(), cx)
        } else {
            match enter_result(focused, self.dialog.destructive, self.checked) {
                DialogResult::Confirmed { .. } => confirmed(self.checked, input.as_ref(), cx),
                other => other,
            }
        };
        self.finish(result, window, cx);
    }

    fn on_close(&mut self, _: &CloseSecondaryWindow, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(DialogResult::Cancelled, window, cx);
    }
}

impl Render for TlDialogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = metrics(cx).ui_scale;
        let view = cx.entity().downgrade();
        let on_check = {
            let view = view.clone();
            move |value: bool, _: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    this.checked = value;
                    cx.notify();
                })
                .ok();
            }
        };
        let on_result = move |result: DialogResult, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| this.finish(result, window, cx))
                .ok();
        };
        let body =
            self.dialog
                .render_body(&self.focus, self.checked, on_check, on_result, window, cx);

        // The window height follows the body: its bottom edge in the window includes the frame's
        // title bar above it.
        let fitted = self.fitted.clone();
        let fit = move |bounds: gpui_kit::Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            let want = bounds
                .bottom()
                .clamp(px(MIN_HEIGHT * s), px(MAX_HEIGHT * s));
            if fitted.get() != Some(want) {
                fitted.set(Some(want));
                window.defer(cx, move |window, cx| {
                    let width = window.bounds().size.width;
                    window.resize(size(width, want + frame_bottom(window, cx)));
                });
            }
        };

        div()
            .id("TlDialog")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_close))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                // Content and footer come before the frame's pin in the tab order.
                div()
                    .tab_group()
                    .tab_index(-1)
                    .tab_stop(false)
                    .flex_none()
                    // A flex column measures the body at the width it draws at. As a block the
                    // wrapper measured about twice the body's height and the window fitted to
                    // that, which left an empty band under the buttons (tasklens-fut.23).
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(body)
                    .on_prepaint(fit),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::{Modifiers, TestAppContext};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;
    use crate::theme::ThemeMode;
    use crate::theme::tests::test_theme;

    /// Draws a dialog body with Cancel, a second action and the action button.
    struct ThreeButtons {
        dialog: TlDialog,
        focus: DialogFocus,
        results: Rc<RefCell<Vec<DialogResult>>>,
    }

    impl Render for ThreeButtons {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let results = self.results.clone();
            div().size_full().child(self.dialog.render_body(
                &self.focus,
                false,
                |_, _, _| {},
                move |result, _, _| results.borrow_mut().push(result),
                window,
                cx,
            ))
        }
    }

    #[gpui_kit::test]
    fn second_action_sits_between_cancel_and_select(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
        });
        let results = Rc::new(RefCell::new(Vec::new()));
        let sink = results.clone();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let dialog = TlDialog::confirm("What do you want to do with this process?", "Select")
                .second_action("Terminate", true);
            let focus = DialogFocus::new(cx);
            focus.initial(dialog.is_destructive()).focus(window, cx);
            ThreeButtons {
                dialog,
                focus,
                results: sink,
            }
        });
        cx.run_until_parked();

        // Focus starts on Select, so Enter confirms; Terminate is never the default.
        let focused = cx.update(|window, cx| view.read(cx).focus.focused(window));
        assert_eq!(focused, Some(DialogButton::Action));
        assert_eq!(
            enter_result(focused, false, false),
            DialogResult::Confirmed { checkbox: false }
        );
        assert_eq!(
            enter_result(Some(DialogButton::Second), false, false),
            DialogResult::Second
        );
        assert_eq!(
            enter_result(None, true, false),
            DialogResult::Cancelled,
            "with no button focused a destructive confirmation cancels"
        );

        let cancel = cx.debug_bounds("TlDialogCancel").expect("cancel");
        let second = cx.debug_bounds("TlDialogSecond").expect("second");
        let action = cx.debug_bounds("TlDialogAction").expect("action");
        assert!(cancel.right() <= second.left(), "Cancel left of Terminate");
        assert!(second.right() <= action.left(), "Select is rightmost");

        cx.simulate_click(second.center(), Modifiers::none());
        assert_eq!(*results.borrow(), [DialogResult::Second]);
    }

    #[test]
    fn confirm_defaults() {
        let d = TlDialog::confirm("Delete this custom theme?", "Delete");
        assert_eq!(d.window_title().as_ref(), "tasklens");
        assert_eq!(d.icon_kind(), DialogIcon::Info);
        assert!(!d.is_destructive());
        assert!(d.heading.is_none());
        assert!(d.checkbox.is_none());
    }

    #[test]
    fn destructive_uses_warning() {
        let d = TlDialog::confirm("Reset all settings?", "Reset")
            .destructive()
            .heading("Reset settings")
            .title("tasklens Reset")
            .checkbox("Do not ask again", true);
        assert!(d.is_destructive());
        assert_eq!(d.icon_kind(), DialogIcon::Warning);
        assert_eq!(d.heading.as_deref(), Some("Reset settings"));
        assert_eq!(d.window_title().as_ref(), "tasklens Reset");
        assert_eq!(d.checkbox, Some(("Do not ask again".into(), true)));
    }

    #[test]
    fn footer_looks_match_tl_button() {
        let theme = test_theme(ThemeMode::Dark);
        let white: Hsla = rgb(0xFFFFFF).into();
        let primary = footer_look(FooterKind::Primary, &theme);
        assert_eq!(primary.bg, theme.accent);
        assert_eq!(primary.fg, white);
        assert_eq!(primary.weight, FontWeight::MEDIUM);
        let danger = footer_look(FooterKind::Danger, &theme);
        assert_eq!(danger.bg, theme.status.danger);
        assert!(danger.hover.l >= danger.bg.l && danger.pressed.l <= danger.bg.l);
        let secondary = footer_look(FooterKind::Secondary, &theme);
        assert_eq!(secondary.bg, theme.toolbar);
        assert_eq!(secondary.border, Some(theme.divider));
        assert_eq!(secondary.fg, theme.text_primary);
    }
}
