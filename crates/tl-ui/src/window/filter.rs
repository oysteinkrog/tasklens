//! The shell filter box at the right end of the tab strip (E20-009, UI-SHL-024, UI-SHL-020,
//! UI-SHL-122 step 4, APP-028, PROC-044, R-043, ADR-009, UI-FND-016).
//!
//! One input whose text is kept per tab for the session. A tab switch swaps the text in with
//! `set_value`, which emits no `Change`. Typing restarts a 100 ms debounce; when it fires,
//! `FilterChanged { tab, text }` goes out on `GlobalShellEvents`. Esc, `ClearFilter` and the
//! clear button send the empty text at once.
//!
//! The box draws the `TlFilterBox` look itself: the kit `TlFilterBox` always draws the input's
//! 1 px border and fills it with `input`, and here the accent outline is the border and the fill
//! is `tab_bg` (UI-SHL-024). The width animation is the same `transition`.
//!
//! Esc, Enter and Down: the kit input binds them in its "Input" context, which is deeper than
//! "ShellFilter" and so wins the key match. The "ShellFilter" wrapper catches the kit actions in
//! the capture phase, before the input sees them, and runs the filter rule. `FilterEscape` and
//! `FilterAccept` reach the same code when dispatched directly.
//!
//! Moving focus to the active page is the main window's job: the box emits [`ReturnFocus`] and
//! `MainWindow` focuses the page's `primary_focus`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::accesskit::Role;
use gpui_kit::base::{Transition, transition};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::command::Tab;

use super::main::{FilterChanged, GlobalShellEvents};
use crate::actions::{FilterAccept, FilterEscape, SHELL_FILTER_CONTEXT};
use crate::components::icons::{TlIcon, icon};
use crate::components::input::{FILTER_GROW, FILTER_WIDTH, FILTER_WIDTH_WIDE};
use crate::components::motion::ease_out_cubic;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::{Strings, tr};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};

/// Quiet time after the last keystroke before `FilterChanged` goes out (ADR-009).
pub const FILTER_DEBOUNCE: Duration = Duration::from_millis(100);
/// Outlined region height, × ui_scale: the strip's 30 minus its 2 px line.
const BOX_H: f32 = 28.;
/// Accent outline thickness, × ui_scale.
const OUTLINE_W: f32 = 2.;
/// Left padding before the label, × ui_scale.
const PAD_LEFT: f32 = 10.;
/// Right padding after the input, × ui_scale.
const PAD_RIGHT: f32 = 8.;
/// Input height (`TlInputSize::Sm`), × ui_scale.
const INPUT_H: f32 = 24.;
/// Clear button hit area, × ui_scale.
const CLEAR_HIT: f32 = 20.;

/// Asks the main window to focus the active page's `primary_focus`.
#[derive(Clone, Copy, Debug)]
pub struct ReturnFocus;

/// The filter box is shown on every tab but Performance.
pub fn shown_on(tab: Tab) -> bool {
    tab != Tab::Performance
}

/// Translated text, read once per language.
struct Labels {
    label: SharedString,
    clear: SharedString,
    name: SharedString,
    strings_version: u64,
}

impl Labels {
    fn read(cx: &App) -> Self {
        let has_strings = cx.has_global::<Strings>();
        let text = |key| {
            if has_strings {
                tr(cx, key)
            } else {
                SharedString::default()
            }
        };
        Self {
            label: text("shell-filter-label"),
            clear: text("shell-filter-clear"),
            name: text("shell-filter-name"),
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
        }
    }
}

/// The shell filter box. Embedded in the tab strip's filter slot.
pub struct FilterBox {
    input: Entity<InputState>,
    /// Index = `Tab::ALL` position. Session only, never persisted or logged.
    per_tab: [SharedString; 6],
    tab: Tab,
    debounce: Option<Task<()>>,
    window_active: bool,
    labels: Labels,
    /// The outlined width at the last prepaint.
    width: Rc<Cell<Pixels>>,
    _subs: SmallVec<[Subscription; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    outline: gpui_kit::Hsla,
}

impl EventEmitter<ReturnFocus> for FilterBox {}

impl FilterBox {
    pub fn new(tab: Tab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.on_change(cx),
                // Width and fill follow focus.
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
                InputEvent::PressEnter { .. } => {}
            },
        ));
        subs.push(
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                let active = window.is_window_active();
                if active != this.window_active {
                    this.window_active = active;
                    cx.notify();
                }
            }),
        );
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = cx.global::<Strings>().strings_version;
            if version != this.labels.strings_version {
                this.labels = Labels::read(cx);
                cx.notify();
            }
        }));
        Self {
            input,
            per_tab: Default::default(),
            tab,
            debounce: None,
            window_active: window.is_window_active(),
            labels: Labels::read(cx),
            width: Rc::new(Cell::new(px(0.))),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            outline: gpui_kit::Hsla::default(),
        }
    }

    /// Swaps in `tab`'s text with `set_value`, which emits no `Change`. A debounce still pending
    /// for the old tab is sent at once, so that tab's page gets its last text.
    pub fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        if self.debounce.take().is_some() {
            self.emit(self.tab, cx);
        }
        self.tab = tab;
        let text = self.per_tab[tab as usize].clone();
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    /// `tab`'s filter text.
    pub fn text(&self, tab: Tab) -> &SharedString {
        &self.per_tab[tab as usize]
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Ctrl+F: focuses the input and selects all its text.
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    /// `ClearFilter`: empties the text, sends the empty text at once and returns focus to the
    /// page.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_now(window, cx);
        cx.emit(ReturnFocus);
    }

    /// Type-to-filter (E20-A2): focuses the input and inserts `ch` at the caret. The insert emits
    /// `Change`, so the debounce runs as for typing.
    pub fn begin_typing(&mut self, ch: char, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.insert(ch.to_string(), window, cx);
        });
    }

    /// The outlined width at the last layout, for the tab strip.
    pub fn width(&self) -> Pixels {
        self.width.get()
    }

    /// The outline colour of the last render.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn outline_color(&self) -> gpui_kit::Hsla {
        self.outline
    }

    /// Stores the text and restarts the debounce. The text is the one allocation per key.
    fn on_change(&mut self, cx: &mut Context<Self>) {
        let text = SharedString::from(self.input.read(cx).text().to_string());
        let tab = self.tab;
        self.per_tab[tab as usize] = text;
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FILTER_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                this.debounce = None;
                this.emit(tab, cx);
            });
        }));
        cx.notify();
    }

    /// Sends `FilterChanged` with `tab`'s stored text.
    fn emit(&self, tab: Tab, cx: &mut App) {
        let Some(events) = cx.try_global::<GlobalShellEvents>().map(|e| e.0.clone()) else {
            return;
        };
        let text = self.per_tab[tab as usize].clone();
        events.update(cx, |_, cx| cx.emit(FilterChanged { tab, text }));
    }

    /// Empties the text without a `Change`, drops the debounce and sends the empty text at once.
    fn clear_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = None;
        self.per_tab[self.tab as usize] = SharedString::default();
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.emit(self.tab, cx);
        cx.notify();
    }

    /// Esc: with text, clear it and send the empty text at once; then focus the page. An Esc on
    /// empty text only moves focus; the Esc chain runs on a further Esc from the page.
    fn on_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.per_tab[self.tab as usize].is_empty() || self.input.read(cx).text().len() > 0 {
            self.clear_now(window, cx);
        }
        cx.emit(ReturnFocus);
    }

    /// Enter or Down: focus the page and keep the filter.
    fn on_accept(&mut self, cx: &mut Context<Self>) {
        cx.emit(ReturnFocus);
    }
}

impl Render for FilterBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let state = self.input.read(cx);
        let focused = state.focus_handle(cx).is_focused(window);
        let has_text = state.text().len() > 0;

        let outline = if self.window_active {
            theme.accent
        } else {
            theme.divider
        };
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.outline = outline;
        }

        let target = if focused || has_text {
            FILTER_WIDTH_WIDE
        } else {
            FILTER_WIDTH
        };
        let input_w = transition(
            ("shell-filter", "width"),
            px(target * s),
            Transition::new(FILTER_GROW).ease(ease_out_cubic),
            window,
            cx,
        );

        let mut label = div()
            .debug_selector(|| "FilterBox-label".into())
            .flex_none()
            .whitespace_nowrap()
            .text_color(theme.text_secondary);
        *label.text_style() = t_sm(&m);
        let label = label.child(self.labels.label.clone());

        let clear = has_text.then(|| {
            let button = div()
                .id("shell-filter-clear")
                .debug_selector(|| "FilterBox-clear".into())
                .role(Role::Button)
                .aria_label(self.labels.clear.clone())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(CLEAR_HIT * s))
                .rounded(m.radius_small)
                .hover(|st| st.bg(theme.tab_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.clear_now(window, cx);
                    this.input.update(cx, |s, cx| s.focus(window, cx));
                }))
                .child(icon(TlIcon::X, m.icon.sm, theme.text_secondary));
            let tip = HoverTooltip::state("shell-filter-clear-tooltip", window, cx);
            HoverTooltip::attach(
                &tip,
                button,
                DELAY_CONTROL,
                TlTooltip::text(self.labels.clear.clone()).build(),
                window,
                cx,
            )
        });

        let mut input = Input::new(&self.input)
            .appearance(true)
            .bordered(false)
            .focus_bordered(false)
            .small()
            .aria_label(self.labels.name.clone())
            .when_some(clear, |i, clear| i.suffix(clear));
        *input.text_style() = t_base(&m);
        let input = Styled::size_full(input)
            .px(m.sp[4])
            .gap(m.sp[3])
            .rounded(m.radius_small)
            .bg(if focused {
                theme.input_focus
            } else {
                theme.tab_bg
            })
            .text_color(theme.text_primary);

        let field = div()
            .id("shell-filter-input")
            .debug_selector(|| "FilterBox-input".into())
            .key_context(SHELL_FILTER_CONTEXT)
            .flex_none()
            .h(px(INPUT_H * s))
            .w(input_w)
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.on_escape(window, cx);
            }))
            .capture_action(cx.listener(|this, _: &Enter, _, cx| {
                cx.stop_propagation();
                this.on_accept(cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.on_accept(cx);
            }))
            .on_action(cx.listener(|this, _: &FilterEscape, window, cx| {
                this.on_escape(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FilterAccept, _, cx| this.on_accept(cx)))
            .child(input);

        let width = self.width.clone();
        div()
            .id("FilterBox")
            .debug_selector(|| "FilterBox".into())
            .flex()
            .flex_none()
            .h(px(BOX_H * s))
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(b) = bounds.first() {
                    width.set(b.size.width);
                }
            })
            .child(
                div()
                    .debug_selector(|| "FilterBox-outline".into())
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .h_full()
                    .gap(m.sp[3])
                    .pl(px(PAD_LEFT * s))
                    .pr(px(PAD_RIGHT * s))
                    .bg(theme.tab_bg)
                    .border_t(px(OUTLINE_W * s))
                    .border_l(px(OUTLINE_W * s))
                    .border_color(outline)
                    .rounded_tl(m.radius_card)
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.input.update(cx, |s, cx| s.focus(window, cx));
                        }),
                    )
                    .child(label)
                    .child(field),
            )
    }
}
