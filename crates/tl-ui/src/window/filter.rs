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
//!
//! On Processes the box also draws the "Include children" checkbox to the left of the outline
//! (E20-010, UI-SHL-025). It toggles `processes.filter_include_children` through
//! `ToggleSetting`; the process page owns the toasts and the filtering.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::accesskit::Role;
use gpui_kit::base::{Transition, transition};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{
    Enter, Escape, Input, InputEvent, InputState, MoveDown, Position,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::table::TableMode;

use super::main::{FilterChanged, GlobalShellEvents};
use crate::actions::{FilterAccept, FilterEscape, SHELL_FILTER_CONTEXT, ToggleSetting};
use crate::components::checkbox::TlCheckbox;
use crate::components::icons::{TlIcon, icon};
use crate::components::input::{FILTER_GROW, FILTER_WIDTH, FILTER_WIDTH_WIDE};
use crate::components::motion::ease_out_cubic;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, SettingKey, TlSettings};
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

/// "Include children" shows on Processes while the filter has text or the mode is Tree. The
/// same rule as E22's `include_children_visible`; E20 cannot depend on E22.
pub fn include_children_visible(tab: Tab, filter_text: &str, mode: TableMode) -> bool {
    tab == Tab::Processes && (!filter_text.is_empty() || mode == TableMode::Tree)
}

/// The `processes` settings the checkbox follows, read once per section version.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct ProcessesView {
    mode: TableMode,
    include_children: bool,
    version: u64,
}

impl ProcessesView {
    fn read(cx: &App) -> Self {
        let Some(settings) = cx.try_global::<TlSettings>() else {
            return Self::default();
        };
        let p = &TlSettings::settings(cx).processes;
        Self {
            mode: p.mode,
            include_children: p.filter_include_children,
            version: settings.version(Section::Processes),
        }
    }
}

/// Translated text, read once per language.
struct Labels {
    label: SharedString,
    clear: SharedString,
    name: SharedString,
    include_children: SharedString,
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
            include_children: text("shell-filter-include-children"),
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
    processes: ProcessesView,
    /// The drawn width at the last prepaint: the outline plus the checkbox part when shown.
    width: Rc<Cell<Pixels>>,
    /// The outline's left edge minus the box's left edge at the last prepaint: the checkbox part
    /// and its gap when shown, else 0. The tab strip ends its accent line there.
    outline_inset: Rc<Cell<Pixels>>,
    /// The input's target width at the last render.
    input_target: Pixels,
    /// The input's drawn width at the last prepaint. Differs from `input_target` only while the
    /// width animates.
    input_drawn: Rc<Cell<Pixels>>,
    _subs: SmallVec<[Subscription; 4]>,
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
        subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| {
            let version = cx.global::<TlSettings>().version(Section::Processes);
            if version != this.processes.version {
                this.processes = ProcessesView::read(cx);
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
            processes: ProcessesView::read(cx),
            width: Rc::new(Cell::new(px(0.))),
            outline_inset: Rc::new(Cell::new(px(0.))),
            input_target: px(0.),
            input_drawn: Rc::new(Cell::new(px(0.))),
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

    /// Type-to-filter (E20-A2): focuses the input, moves the caret to the end of the text and
    /// inserts `ch` there. The insert emits `Change`, so the debounce runs as for typing.
    pub fn begin_typing(&mut self, ch: char, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            // Also focuses. The column is clamped to the line length: the end of the text.
            s.set_cursor_position(Position::new(0, u32::MAX), window, cx);
            s.insert(ch.to_string(), window, cx);
        });
    }

    /// The drawn width at the last layout, checkbox part included, for the tab strip.
    pub fn width(&self) -> Pixels {
        self.width.get()
    }

    /// The drawn width at the last layout with the input at its target width, so it does not
    /// change while the width animates. The tab strip fits its tabs to it (E20-015).
    pub fn target_width(&self) -> Pixels {
        self.width.get() + self.input_target - self.input_drawn.get()
    }

    /// The outline's left edge minus the box's left edge at the last layout, for the tab strip.
    pub fn outline_inset(&self) -> Pixels {
        self.outline_inset.get()
    }

    /// Whether the "Include children" checkbox shows for the current tab, text and mode.
    pub fn include_children_shown(&self) -> bool {
        include_children_visible(
            self.tab,
            &self.per_tab[self.tab as usize],
            self.processes.mode,
        )
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
        self.input_target = px(target * s);

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

        let include = self.include_children_shown().then(|| {
            let toggle = |window: &mut Window, cx: &mut App| {
                window.dispatch_action(
                    Box::new(ToggleSetting(SettingKey::ProcessesFilterIncludeChildren)),
                    cx,
                );
            };
            let checked = self.processes.include_children;
            let mut text = div()
                .id("shell-filter-include-children-label")
                .debug_selector(|| "FilterBox-include-children-label".into())
                .flex_none()
                .whitespace_nowrap()
                .text_color(theme.text_primary)
                .on_click(move |_, window, cx| toggle(window, cx));
            *text.text_style() = t_sm(&m);
            div()
                .id("shell-filter-include-children")
                .debug_selector(|| "FilterBox-include-children".into())
                .role(Role::CheckBox)
                .aria_label(self.labels.include_children.clone())
                .aria_toggled(checked.into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .h_full()
                .gap(m.sp[3])
                .child(text.child(self.labels.include_children.clone()))
                .child(
                    TlCheckbox::new("shell-filter-include-children-box")
                        .checked(Some(checked))
                        .inline(true)
                        .on_change(move |_, window, cx| toggle(window, cx)),
                )
        });

        let width = self.width.clone();
        let outline_inset = self.outline_inset.clone();
        let input_drawn = self.input_drawn.clone();
        // Reversed so the outline comes first in element order: the checkbox is the Tab stop
        // after the filter input (UI-SHL-121) while it is drawn to the left.
        div()
            .debug_selector(|| "FilterBox".into())
            .flex()
            .flex_row_reverse()
            .flex_none()
            .h(px(BOX_H * s))
            .gap(m.sp[6])
            .on_children_prepainted(move |bounds, _, _| {
                let left = bounds.iter().map(|b| b.left()).reduce(Pixels::min);
                let right = bounds.iter().map(|b| b.right()).reduce(Pixels::max);
                if let (Some(left), Some(right)) = (left, right) {
                    width.set(right - left);
                    // The outline is the first child.
                    outline_inset.set(bounds[0].left() - left);
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
                    // The input is the second child, after the label.
                    .on_children_prepainted(move |bounds, _, _| {
                        if let Some(b) = bounds.get(1) {
                            input_drawn.set(b.size.width);
                        }
                    })
                    .child(label)
                    .child(field),
            )
            .children(include)
    }
}
