//! The Run window "tasklens Run": the command combo with its history popup, Run as
//! administrator, Browse and Clear history, launching through the engine's shared launcher
//! (UI-PWN-028, UI-PWN-029, UI-SHL-097, PROC-095, APP-014, R-061, R-134).
//!
//! Security: the text runs as `/bin/sh -c <text>` in the engine, as root through pkexec when
//! asked. The window refuses text with a control character before it sends anything, and it
//! never logs the command text (ADR-015).
//!
//! The words are `en-US.ftl` keys, translated once per strings version into [`RunWords`].

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Root;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Enter, Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::{
    App, AppContext as _, BoxShadow, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowHandle,
    deferred, div, point, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::actions::{
    ActionError, ActionId, ActionKind, ActionRequest, ActionState, LaunchSpec, LaunchWhat,
};
use tracing::warn;

use super::actions::{
    BrowseRunTarget, ClearRunHistory, RUN_DIALOG_CONTEXT, RunCommand, RunCommandElevated,
    RunHistoryDown, RunHistoryUp,
};
use super::frame::{PwnWindow, TITLE_RUN, pwn_window_spec};
use super::run_history;
use crate::action_tracker::{ActionTrackerEvent, GlobalActionTracker, ToastTexts};
use crate::actions::OpenRunDialog;
use crate::app::{AppEnv, RunHistoryEntry, RunHistoryStore};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::checkbox::TlCheckbox;
use crate::components::icons::TlIcon;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::TlTooltip;
use crate::components::window_frame::{CloseSecondaryWindow, TlSecondaryWindow};
use crate::i18n::{Strings, tr};
use crate::text::style::{t_base, t_xs};
use crate::theme::{current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, WindowKey};

/// Popup row height, × ui_scale.
pub const ROW_H: f32 = 26.;
/// Rows the popup shows before it scrolls.
pub const VISIBLE_ROWS: usize = 5;
/// Combo row height, × ui_scale.
const COMBO_ROW_H: f32 = 32.;
/// Width of the "Open:" label, × ui_scale.
const LABEL_W: f32 = 48.;
/// Checkbox row and note row heights, × ui_scale.
const CHECK_ROW_H: f32 = 24.;
const NOTE_ROW_H: f32 = 18.;
/// Footer height, × ui_scale.
const FOOTER_H: f32 = 52.;
/// The checkbox box and its gap: the note starts under the label.
const NOTE_INDENT: f32 = 24.;
/// Icon sizes, × ui_scale.
const PROMPT_ICON: f32 = 32.;
const CHEVRON_ICON: f32 = 14.;
const ADMIN_ICON: f32 = 12.;

/// Debug selectors.
pub const POPUP_ID: &str = "RunHistoryPopup";
pub const POPUP_ROW_ID: &str = "RunHistoryRow";

/// Characters that make Browse wrap a path in single quotes.
const SHELL_CHARS: &str = " `~!#$&*()|\\;'\"<>?[]{}";

const KEYS: [&str; 16] = [
    "pwn-run-prompt",
    "pwn-run-open",
    "pwn-run-history-button",
    "pwn-run-as-admin",
    "pwn-run-wayland-note",
    "pwn-run-browse",
    "pwn-run-clear-history",
    "pwn-run-ok",
    "pwn-run-cancel",
    "pwn-run-ran-as-admin",
    "pwn-run-toast-empty",
    "pwn-run-toast-invalid",
    "pwn-run-toast-launched",
    "pwn-run-toast-failed",
    "pwn-run-toast-auth-cancelled",
    "pwn-run-toast-history-cleared",
];

/// The window's words, translated once per strings version.
pub struct RunWords {
    strings_version: Option<u64>,
    pub prompt: SharedString,
    pub open: SharedString,
    pub history_button: SharedString,
    pub as_admin: SharedString,
    pub wayland_note: SharedString,
    pub browse: SharedString,
    pub clear_history: SharedString,
    pub ok: SharedString,
    pub cancel: SharedString,
    pub ran_as_admin: SharedString,
    pub toast_empty: SharedString,
    pub toast_invalid: SharedString,
    pub toast_launched: SharedString,
    pub toast_failed: SharedString,
    pub toast_auth_cancelled: SharedString,
    pub toast_history_cleared: SharedString,
}

impl RunWords {
    fn new(cx: &App) -> Self {
        let [
            prompt,
            open,
            history_button,
            as_admin,
            wayland_note,
            browse,
            clear_history,
            ok,
            cancel,
            ran_as_admin,
            toast_empty,
            toast_invalid,
            toast_launched,
            toast_failed,
            toast_auth_cancelled,
            toast_history_cleared,
        ] = KEYS.map(|k| tr(cx, k));
        Self {
            strings_version: strings_version(cx),
            prompt,
            open,
            history_button,
            as_admin,
            wayland_note,
            browse,
            clear_history,
            ok,
            cancel,
            ran_as_admin,
            toast_empty,
            toast_invalid,
            toast_launched,
            toast_failed,
            toast_auth_cancelled,
            toast_history_cleared,
        }
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

/// Registers the `OpenRunDialog` handler (File > Run..., Ctrl+N). `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: from a menu the main window is mid-update.
    cx.on_action(|_: &OpenRunDialog, cx| cx.defer(open_run_window));
}

/// Activates the open Run window, or opens one. Without a window registry (tests) it always
/// opens one.
pub fn open_run_window(cx: &mut App) {
    let open = |cx: &mut App| {
        let spec = pwn_window_spec(cx, PwnWindow::Run, TITLE_RUN.into(), None).key(WindowKey::Run);
        let handle = TlSecondaryWindow::open(cx, spec, |window, cx| {
            cx.new(|cx| RunWindow::new(window, cx))
        })?;
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| r.open_or_focus(WindowKey::Run, cx, open));
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// `path` as Browse puts it in the combo: wrapped in single quotes when it holds a space or a
/// shell character, with each `'` written as `'\''`.
pub fn quote_path(path: &str) -> String {
    if !path.chars().any(|c| SHELL_CHARS.contains(c)) {
        return path.to_owned();
    }
    let mut out = String::with_capacity(path.len() + 2);
    out.push('\'');
    for c in path.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Why a command text cannot be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCheck {
    Ok,
    Empty,
    /// NUL or another control character.
    Invalid,
}

/// Checks the combo text before it is sent. Control characters never reach the launcher.
pub fn check_command(text: &str) -> CommandCheck {
    if text.trim().is_empty() {
        CommandCheck::Empty
    } else if text.chars().any(char::is_control) {
        CommandCheck::Invalid
    } else {
        CommandCheck::Ok
    }
}

/// The request in flight.
struct Pending {
    id: ActionId,
    command: String,
    as_admin: bool,
}

/// The content of the Run window.
pub struct RunWindow {
    input: Entity<InputState>,
    history: Arc<[RunHistoryEntry]>,
    /// "Run as administrator".
    elevated: bool,
    pending: Option<Pending>,
    popup_open: bool,
    /// Index into `matches`.
    highlight: Option<usize>,
    /// History indexes the popup lists, best match first.
    matches: SmallVec<[u32; 16]>,
    words: Rc<RunWords>,
    focus: FocusHandle,
    check_focus: FocusHandle,
    _subs: SmallVec<[Subscription; 3]>,
}

impl RunWindow {
    /// The content of the Run window `handle`, while it is open.
    pub fn find(handle: WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let words = Rc::new(RunWords::new(cx));
        let history = run_history::load(store(cx));
        let input = cx.new(|cx| InputState::new(window, cx));
        if let Some(first) = history.first() {
            let command = first.command.clone();
            input.update(cx, |s, cx| {
                s.set_value(command, window, cx);
                s.select_all(window, cx);
            });
        }
        input.update(cx, |s, cx| s.focus(window, cx));

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.refilter(cx);
            }
        }));
        if let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) {
            subs.push(cx.subscribe_in(
                &tracker,
                window,
                |this, tracker, ev: &ActionTrackerEvent, window, cx| {
                    if let ActionTrackerEvent::Finished { id, ok, total, .. } = *ev {
                        let state = tracker
                            .read(cx)
                            .results(id)
                            .and_then(|r| r.first())
                            .map(|(_, s)| s.clone());
                        this.on_finished(id, ok == total && ok > 0, state, window, cx);
                    }
                },
            ));
        }
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            if this.words.strings_version != strings_version(cx) {
                this.words = Rc::new(RunWords::new(cx));
                cx.notify();
            }
        }));
        Self {
            input,
            history,
            elevated: false,
            pending: None,
            popup_open: false,
            highlight: None,
            matches: SmallVec::new(),
            words,
            focus: cx.focus_handle(),
            check_focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn history(&self) -> &Arc<[RunHistoryEntry]> {
        &self.history
    }

    pub fn popup_open(&self) -> bool {
        self.popup_open && !self.matches.is_empty()
    }

    /// The rows the popup lists, as history indexes.
    pub fn popup_rows(&self) -> &[u32] {
        &self.matches
    }

    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// Clear history is enabled: something to clear and nothing in flight.
    pub fn can_clear(&self) -> bool {
        !self.history.is_empty() && self.pending.is_none()
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn elevated(&self) -> bool {
        self.elevated
    }

    pub fn set_elevated(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.elevated != on {
            self.elevated = on;
            cx.notify();
        }
    }

    pub fn words(&self) -> &Rc<RunWords> {
        &self.words
    }

    fn text(&self, cx: &App) -> String {
        self.input.read(cx).text().to_string()
    }

    /// Filters the history for the typed text. The popup opens when something matches and the
    /// text is not empty, and closes when nothing matches.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.matches = run_history::matches(&self.history, &text);
        self.highlight = None;
        self.popup_open = !text.is_empty() && !self.matches.is_empty();
        cx.notify();
    }

    /// Down (`forward`) or Up: opens the popup on the matches of the text (all history when
    /// empty), moves the highlight and copies its command into the input.
    pub fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        if !self.popup_open() {
            let text = self.text(cx);
            self.matches = run_history::matches(&self.history, &text);
            if self.matches.is_empty() {
                // The text is not in the history: show all of it.
                self.matches = run_history::matches(&self.history, "");
            }
            if self.matches.is_empty() {
                return;
            }
            self.popup_open = true;
            self.highlight = None;
        }
        let last = self.matches.len() - 1;
        let next = match (self.highlight, forward) {
            (None, true) => 0,
            (None, false) => last,
            (Some(h), true) => (h + 1).min(last),
            (Some(h), false) => h.saturating_sub(1),
        };
        self.highlight = Some(next);
        let command = self.history[self.matches[next] as usize].command.clone();
        self.input
            .update(cx, |s, cx| s.set_value(command, window, cx));
        cx.notify();
    }

    fn close_popup(&mut self, cx: &mut Context<Self>) {
        if self.popup_open {
            self.popup_open = false;
            self.highlight = None;
            cx.notify();
        }
    }

    /// The chevron: shows the whole history, or closes the popup.
    fn toggle_popup(&mut self, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.close_popup(cx);
            return;
        }
        self.matches = run_history::matches(&self.history, "");
        self.popup_open = !self.matches.is_empty();
        self.highlight = None;
        cx.notify();
    }

    /// A click on popup row `pos`: the command goes in the input; a double-click runs it.
    fn click_row(&mut self, pos: usize, double: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&ix) = self.matches.get(pos) else {
            return;
        };
        let command = self.history[ix as usize].command.clone();
        self.input
            .update(cx, |s, cx| s.set_value(command, window, cx));
        self.close_popup(cx);
        if double {
            let elevated = self.elevated;
            self.run(elevated, window, cx);
        }
    }

    /// Checks the text and sends the launch. `as_admin` runs it through pkexec.
    pub fn run(&mut self, as_admin: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let text = self.text(cx);
        match check_command(&text) {
            CommandCheck::Empty => {
                let t = self.words.toast_empty.clone();
                toast(window, cx, ToastKind::Warning, t);
                return;
            }
            CommandCheck::Invalid => {
                let t = self.words.toast_invalid.clone();
                toast(window, cx, ToastKind::Error, t);
                return;
            }
            CommandCheck::Ok => {}
        }
        let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
            warn!(target: "tasklens::ui", "no action tracker; the command was not sent");
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            warn!(target: "tasklens::ui", "no toast queue; the command was not sent");
            return;
        };
        self.close_popup(cx);
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command(text.as_str().into()),
                as_admin,
                cwd: None,
            }),
            targets: SmallVec::new(),
            privileged: false,
        };
        // The window shows its own result toasts.
        let texts = ToastTexts {
            owner_toasts: true,
            ..ToastTexts::new(SharedString::default(), SharedString::default())
        };
        let sent = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        });
        if let Some(id) = sent {
            self.pending = Some(Pending {
                id,
                command: text,
                as_admin,
            });
            cx.notify();
        }
    }

    fn on_finished(
        &mut self,
        id: ActionId,
        ok: bool,
        state: Option<ActionState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending.as_ref().is_none_or(|p| p.id != id) {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        if ok {
            let history = run_history::record(
                &self.history,
                &pending.command,
                pending.as_admin,
                &run_history::now_rfc3339(),
            );
            if let Err(err) = run_history::save(store(cx), &history) {
                warn!(target: "tasklens::ui", %err, "run history not saved");
            }
            self.history = history.into();
            let text = self.words.toast_launched.clone();
            push_main_toast(cx, ToastKind::Success, text, None, None);
            window.remove_window();
            return;
        }
        let (kind, text) = match state {
            Some(ActionState::Failed(ActionError::AuthCancelled)) => {
                (ToastKind::Info, self.words.toast_auth_cancelled.clone())
            }
            _ => (ToastKind::Error, self.words.toast_failed.clone()),
        };
        toast(window, cx, kind, text);
        cx.notify();
    }

    /// Clears the history file at once, with no confirmation.
    pub fn clear_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_clear() {
            return;
        }
        if let Err(err) = run_history::clear(store(cx)) {
            warn!(target: "tasklens::ui", %err, "run history not cleared");
        }
        self.history = Arc::from([]);
        self.matches.clear();
        self.close_popup(cx);
        let text = self.words.toast_history_cleared.clone();
        toast(window, cx, ToastKind::Success, text);
        cx.notify();
    }

    /// Puts a picked path in the combo, quoted when it needs it.
    pub fn set_browsed(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = quote_path(path);
        self.input.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        self.close_popup(cx);
    }

    /// The portal file chooser. The portal has no file-type filters here, so it lists all
    /// files.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            let _ = this.update_in(cx, |this, window, cx| this.set_browsed(&path, window, cx));
        })
        .detach();
    }

    fn on_run(&mut self, _: &RunCommand, window: &mut Window, cx: &mut Context<Self>) {
        let elevated = self.elevated;
        self.run(elevated, window, cx);
    }

    fn on_run_elevated(
        &mut self,
        _: &RunCommandElevated,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(true, window, cx);
    }

    fn on_clear(&mut self, _: &ClearRunHistory, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_history(window, cx);
    }

    fn on_browse(&mut self, _: &BrowseRunTarget, window: &mut Window, cx: &mut Context<Self>) {
        self.browse(window, cx);
    }

    fn on_up(&mut self, _: &RunHistoryUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    fn on_down(&mut self, _: &RunHistoryDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }

    /// Esc closes the popup first; the frame closes the window after that.
    fn on_close(&mut self, _: &CloseSecondaryWindow, _: &mut Window, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.close_popup(cx);
        } else {
            cx.propagate();
        }
    }

    fn popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.popup_open() {
            return None;
        }
        let m = *metrics(cx);
        let s = m.ui_scale;
        let theme = current(cx).clone();
        let rows = self.matches.iter().enumerate().map(|(pos, &ix)| {
            let entry = &self.history[ix as usize];
            let mut command = div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis()
                .overflow_hidden()
                .text_color(theme.text_primary);
            *command.text_style() = t_base(&m);
            let command = command.child(SharedString::from(entry.command.clone()));
            let admin = entry.elevated.then(|| {
                div()
                    .id(("run-admin", pos))
                    .flex_none()
                    .tooltip(TlTooltip::text(self.words.ran_as_admin.clone()).build())
                    .child(
                        Icon::new(IconName::ShieldCheck)
                            .size(px(ADMIN_ICON * s))
                            .text_color(theme.text_secondary),
                    )
            });
            div()
                .id(("run-row", pos))
                .debug_selector(move || format!("{POPUP_ROW_ID}-{pos}"))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[3])
                .h(px(ROW_H * s))
                .px(m.sp[4])
                .when(self.highlight == Some(pos), |d| d.bg(theme.menu_hover))
                .hover(|st| st.bg(theme.menu_hover))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    this.click_row(pos, ev.click_count() >= 2, window, cx);
                }))
                .child(command)
                .children(admin)
        });
        let n = self.matches.len().min(VISIBLE_ROWS);
        let mut panel = div()
            .id(POPUP_ID)
            .debug_selector(|| POPUP_ID.into())
            .flex()
            .flex_col()
            .w_full()
            .h(px(ROW_H * s * n as f32))
            .overflow_y_scroll()
            .bg(theme.menu_bg)
            .border_1()
            .border_color(theme.divider)
            .rounded(m.radius_small)
            .children(rows);
        panel.style().box_shadow = Some(vec![BoxShadow {
            color: theme.derived.shadow,
            offset: point(px(0.), px(2.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }]);
        Some(
            deferred(
                div()
                    .absolute()
                    .top(px(COMBO_ROW_H * s))
                    .left_0()
                    .w_full()
                    .child(panel),
            )
            .with_priority(1),
        )
    }
}

/// The history file, `None` without one (tests).
fn store(cx: &App) -> Option<&dyn RunHistoryStore> {
    cx.try_global::<AppEnv>()
        .and_then(|env| env.run_history.as_deref())
}

/// A toast in the Run window's own stack.
fn toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack in the Run window"),
    }
}

impl Focusable for RunWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RunWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let s = m.ui_scale;
        let theme = current(cx).clone();
        let words = self.words.clone();
        let busy = self.pending.is_some();
        let view = cx.entity();

        let mut prompt = div().flex_1().min_w_0().text_color(theme.text_primary);
        *prompt.text_style() = t_base(&m);
        let header = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_start()
            .gap(m.sp[5])
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(PROMPT_ICON * s))
                    .text_color(theme.text_secondary),
            )
            .child(prompt.child(words.prompt.clone()));

        let mut label = div()
            .flex_none()
            .w(px(LABEL_W * s))
            .text_color(theme.text_primary);
        *label.text_style() = t_base(&m);
        let chevron = {
            let view = view.clone();
            TlButton::new("RunHistoryButton", TlButtonKind::Icon)
                .size(TlButtonSize::Sm)
                .icon(TlIcon::ChevronDown)
                .icon_size(px(CHEVRON_ICON * s))
                .tooltip(words.history_button.clone())
                .accessible_name(words.history_button.clone())
                .tab_stop(false)
                .disabled(busy || self.history.is_empty())
                .on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.toggle_popup(cx));
                })
        };
        let combo = div()
            .debug_selector(|| "RunCombo".into())
            .relative()
            .flex_1()
            .min_w_0()
            .child(
                TlTextInput::new("RunInput", &self.input)
                    .size(TlInputSize::Md)
                    .aria_label(words.open.trim_end_matches(':').to_owned())
                    .disabled(busy),
            )
            .child(
                div()
                    .absolute()
                    .top(px(2. * s))
                    .right(px(2. * s))
                    .child(chevron),
            )
            .children(self.popup(cx));
        let combo_row = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(COMBO_ROW_H * s))
            .child(label.child(words.open.clone()))
            .child(combo);

        let check = {
            let view = view.clone();
            TlCheckbox::new("RunAsAdmin")
                .focus_handle(self.check_focus.clone())
                .checked(Some(self.elevated))
                .label(words.as_admin.clone())
                .disabled(busy)
                .on_change(move |on, _, cx| {
                    view.update(cx, |this, cx| this.set_elevated(on, cx));
                })
        };
        let check_row = div()
            .id("RunAsAdminRow")
            .flex()
            .flex_none()
            .items_center()
            .h(px(CHECK_ROW_H * s))
            .tooltip(TlTooltip::text(words.wayland_note.clone()).build())
            .child(check);
        let mut note = div()
            .debug_selector(|| "RunWaylandNote".into())
            .flex_none()
            .h(px(NOTE_ROW_H * s))
            .pl(px(NOTE_INDENT * s))
            .text_color(theme.text_secondary);
        *note.text_style() = t_xs(&m);
        let note = note.child(words.wayland_note.clone());

        let button = |id: &'static str, kind, label: SharedString| {
            TlButton::new(id, kind).size(TlButtonSize::Sm).label(label)
        };
        let browse = button("RunBrowse", TlButtonKind::Secondary, words.browse.clone())
            .disabled(busy)
            .action(Box::new(BrowseRunTarget));
        let clear = button(
            "RunClearHistory",
            TlButtonKind::Secondary,
            words.clear_history.clone(),
        )
        .disabled(!self.can_clear())
        .action(Box::new(ClearRunHistory));
        let ok = button("RunOk", TlButtonKind::Primary, words.ok.clone())
            .loading(busy)
            .disabled(busy)
            .action(Box::new(RunCommand));
        let cancel = button("RunCancel", TlButtonKind::Secondary, words.cancel.clone())
            .disabled(busy)
            .on_click(|_, window, _| window.remove_window());
        let footer = div()
            .debug_selector(|| "RunFooter".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[4])
            .h(px(FOOTER_H * s))
            .px(m.sp[6])
            .border_t_1()
            .border_color(theme.divider)
            .child(browse)
            .child(clear)
            .child(div().flex_1())
            .child(ok)
            .child(cancel);

        let (up, down, enter, escape) = (view.clone(), view.clone(), view.clone(), view);
        div()
            .id("RunWindow")
            .key_context(RUN_DIALOG_CONTEXT)
            .track_focus(&self.focus)
            // The input binds the arrows, Enter and Esc itself, so the window takes them first.
            .capture_action(move |_: &MoveUp, window, cx| {
                cx.stop_propagation();
                up.update(cx, |this, cx| this.step(false, window, cx));
            })
            .capture_action(move |_: &MoveDown, window, cx| {
                cx.stop_propagation();
                down.update(cx, |this, cx| this.step(true, window, cx));
            })
            .capture_action(move |a: &Enter, window, cx| {
                cx.stop_propagation();
                enter.update(cx, |this, cx| {
                    let elevated = a.shift || this.elevated;
                    this.run(elevated, window, cx);
                });
            })
            .capture_action(move |_: &Escape, _, cx| {
                if escape.read(cx).popup_open() {
                    cx.stop_propagation();
                    escape.update(cx, |this, cx| this.close_popup(cx));
                }
            })
            .on_action(cx.listener(Self::on_run))
            .on_action(cx.listener(Self::on_run_elevated))
            .on_action(cx.listener(Self::on_clear))
            .on_action(cx.listener(Self::on_browse))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_close))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(m.sp[4])
                    .p(m.sp[6])
                    .child(header)
                    .child(combo_row)
                    .child(check_row)
                    .child(note),
            )
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_quoting() {
        assert_eq!(quote_path("/usr/bin/kate"), "/usr/bin/kate");
        assert_eq!(quote_path("/opt/My App/run"), "'/opt/My App/run'");
        assert_eq!(quote_path("/tmp/a$b"), "'/tmp/a$b'");
        assert_eq!(quote_path("/tmp/it's"), "'/tmp/it'\\''s'");
    }

    #[test]
    fn command_check_rejects_controls() {
        assert_eq!(check_command(""), CommandCheck::Empty);
        assert_eq!(check_command("   "), CommandCheck::Empty);
        assert_eq!(check_command("kate\u{1}"), CommandCheck::Invalid);
        assert_eq!(check_command("ka\0te"), CommandCheck::Invalid);
        assert_eq!(check_command("kate\n"), CommandCheck::Invalid);
        assert_eq!(check_command("kate ~/notes.txt"), CommandCheck::Ok);
    }
}
