//! `TlTextInput` and `TlFilterBox`: the single-line text field and the "Filter:" box (UI-FND-016,
//! APP-028, PROC-077, APP-014, APP-037, PROC-024, R-043).
//!
//! `TlTextInput` wraps the styled kit `Input` (`.appearance(true)`, kit size `small` for sm and
//! the default for md). The kit keeps the editing: select all, undo, word moves, clipboard, the
//! caret and the selection (`caret` and `selection` come from the tasklens theme through
//! `theme::map`). Right-click opens the kit edit menu: Cut, Copy, Paste and Select all. That is
//! the kit's own menu for a plain input, so no `Input::context_menu` builder is set.
//!
//! What tasklens draws itself, because the kit cannot meet the spec:
//! - The border. The kit hover and focus styles cannot be set from outside (`Input` is only
//!   `Styled`), and its focus look is a ring. A wrapper `div` draws the 1 px border: `divider`,
//!   hover `text_secondary`, focused `accent`, invalid `status.danger`. The kit border is off.
//! - The clear button and the spinner. The kit clear button is a kit `xsmall` button with no
//!   tooltip, and the kit spinner is not `TlSpinner`. They go in the kit suffix slot.
//! - Esc. The kit `clean_on_escape` clears even an empty field and never lets the key go on.
//!   `TlTextInput` catches `Escape` first: with text it clears it (one `Change`) and stops the
//!   key; with no text it does nothing and the key goes on.
//!
//! The owner creates the `Entity<InputState>` once and reads the text with
//! `state.read(cx).text()`, never `value()`, which allocates.

use std::time::Duration;

use gpui_kit::base::{Transition, transition};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Escape, Input, InputState};
use gpui_kit::{
    App, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled, TextAlign,
    Window, div, prelude::FluentBuilder as _, px,
};

use super::icons::{TlIcon, icon};
use super::motion::ease_out_cubic;
use super::progress::TlSpinner;
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};

/// The filter box width at rest and while focused or holding text, × ui_scale.
pub const FILTER_WIDTH: f32 = 160.;
pub const FILTER_WIDTH_WIDE: f32 = 280.;
/// How long the filter box takes to grow or shrink.
pub const FILTER_GROW: Duration = Duration::from_millis(150);
/// The clear button hit area, × ui_scale.
const CLEAR_HIT: f32 = 20.;

/// Field height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlInputSize {
    /// 24 px at 100% scale.
    Sm,
    /// 28 px at 100% scale.
    #[default]
    Md,
}

impl TlInputSize {
    fn height(self) -> f32 {
        match self {
            Self::Sm => 24.,
            Self::Md => 28.,
        }
    }

    /// The right padding the kit sets on a field with a suffix (`Size::input_px`).
    fn kit_suffix_pad(self) -> Pixels {
        match self {
            Self::Sm => px(8.),
            Self::Md => px(10.),
        }
    }
}

/// A single-line text field bound to an owner's `Entity<InputState>`.
#[derive(IntoElement)]
pub struct TlTextInput {
    id: ElementId,
    state: Entity<InputState>,
    size: TlInputSize,
    prefix_icon: Option<TlIcon>,
    clear_button: bool,
    clean_on_escape: bool,
    invalid: bool,
    loading: bool,
    disabled: bool,
    placeholder: Option<SharedString>,
    width: Option<Pixels>,
    align_right: bool,
    suffix_text: Option<SharedString>,
    square_corners: bool,
}

impl TlTextInput {
    pub fn new(id: impl Into<ElementId>, state: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            size: TlInputSize::Md,
            prefix_icon: None,
            clear_button: false,
            clean_on_escape: false,
            invalid: false,
            loading: false,
            disabled: false,
            placeholder: None,
            width: None,
            align_right: false,
            suffix_text: None,
            square_corners: false,
        }
    }

    pub fn size(mut self, size: TlInputSize) -> Self {
        self.size = size;
        self
    }

    /// An `i_sm` icon in `text_secondary` left of the text, such as `TlIcon::Search`.
    pub fn prefix_icon(mut self, icon: TlIcon) -> Self {
        self.prefix_icon = Some(icon);
        self
    }

    /// Shows a clear button (Lucide `x`, tooltip "Clear filter") while the text is not empty.
    pub fn clear_button(mut self, show: bool) -> Self {
        self.clear_button = show;
        self
    }

    /// Esc clears non-empty text and emits one `Change`; the owner then moves focus. Esc on
    /// empty text goes on to the next handler.
    pub fn clean_on_escape(mut self, clean: bool) -> Self {
        self.clean_on_escape = clean;
        self
    }

    /// Draws the border in `status.danger`. The owner's tooltip gives the reason.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Shows a `TlSpinner` in place of the clear button.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Opacity 0.5, text in `text_secondary`, no editing.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Text shown in `text_secondary` while the field is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// A fixed width. Without it the field fills its parent's width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Right-aligns the text, for numbers.
    pub fn align_right(mut self, align_right: bool) -> Self {
        self.align_right = align_right;
        self
    }

    /// A unit such as "ms", drawn after the text in `t_base` `text_secondary`.
    pub fn suffix_text(mut self, text: impl Into<SharedString>) -> Self {
        self.suffix_text = Some(text.into());
        self
    }

    /// Draws all four corners square, for a field joined to other controls (TlStepper).
    pub fn square_corners(mut self, square: bool) -> Self {
        self.square_corners = square;
        self
    }
}

impl RenderOnce for TlTextInput {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let state = self.state;

        if let Some(placeholder) = self.placeholder
            && state.read(cx).presentation().placeholder() != &placeholder
        {
            state.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }

        let focused = !self.disabled && state.read(cx).focus_handle(cx).is_focused(window);
        let has_text = state.read(cx).text().len() > 0;
        let h = px(self.size.height() * m.ui_scale);
        let radius = if self.square_corners {
            px(0.)
        } else {
            m.radius_small
        };

        let border = if self.invalid {
            theme.status.danger
        } else if focused {
            theme.accent
        } else {
            theme.divider
        };
        let show_clear = self.clear_button && has_text && !self.loading && !self.disabled;

        let suffix_text = self.suffix_text.map(|text| {
            let mut unit = div()
                .debug_selector(|| "TlTextInput-suffix".into())
                .flex_none()
                .whitespace_nowrap()
                .text_color(theme.text_secondary);
            *unit.text_style() = t_base(&m);
            unit.child(text)
        });
        let suffix = (show_clear || self.loading || suffix_text.is_some()).then(|| {
            let slot = div()
                .flex()
                .items_center()
                .gap(m.sp[3])
                // The kit pads a field with a suffix by its own amount; bring it back to sp4.
                .mr(m.sp[4] - self.size.kit_suffix_pad())
                .children(suffix_text);
            if !(show_clear || self.loading) {
                slot
            } else if self.loading {
                slot.child(
                    div()
                        .debug_selector(|| "TlTextInput-spinner".into())
                        .child(TlSpinner::new(m.icon.sm).color(theme.text_secondary)),
                )
            } else {
                let clear_state = state.clone();
                let button = div()
                    .id((self.id.clone(), "clear"))
                    .debug_selector(|| "TlTextInput-clear".into())
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(CLEAR_HIT * m.ui_scale))
                    .rounded(radius)
                    .hover(|s| s.bg(theme.tab_hover))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        clear_state.update(cx, |s, cx| {
                            s.clean(window, cx);
                            s.focus(window, cx);
                        });
                    })
                    .child(icon(TlIcon::X, m.icon.sm, theme.text_secondary));
                let tip = HoverTooltip::state((self.id.clone(), "clear-tooltip"), window, cx);
                slot.child(HoverTooltip::attach(
                    &tip,
                    button,
                    DELAY_CONTROL,
                    TlTooltip::text("Clear filter").build(),
                    window,
                    cx,
                ))
            }
        });

        let mut input = Input::new(&state)
            .appearance(true)
            .bordered(false)
            .focus_bordered(false)
            .disabled(self.disabled)
            .when(self.size == TlInputSize::Sm, |i| i.small())
            .when_some(self.prefix_icon, |i, name| {
                i.prefix(icon(name, m.icon.sm, theme.text_secondary))
            })
            .when_some(suffix, |i, suffix| i.suffix(suffix));
        *input.text_style() = t_base(&m);
        if self.align_right {
            input.text_style().text_align = Some(TextAlign::Right);
        }
        // `Input::h` sets the multi-line height only, so the size goes through `Styled`.
        let input = Styled::size_full(input)
            .px(m.sp[4])
            .gap(m.sp[3])
            .rounded((radius - px(1.)).max(px(0.)))
            .bg(if focused {
                theme.input_focus
            } else {
                theme.input
            })
            .text_color(if self.disabled {
                theme.text_secondary
            } else {
                theme.text_primary
            });

        let clean_on_escape = self.clean_on_escape;
        let hover_border = theme.text_secondary;
        div()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("TlTextInput-{id}")
            })
            .key_context("TlInput")
            .flex_none()
            .h(h)
            .map(|d| match self.width {
                Some(w) => d.w(w),
                None => d.w_full(),
            })
            .rounded(radius)
            .border_1()
            .border_color(border)
            .when(self.disabled, |d| d.opacity(0.5))
            .when(!self.disabled && !focused && !self.invalid, |d| {
                d.hover(move |s| s.border_color(hover_border))
            })
            .when(clean_on_escape, |d| {
                d.capture_action(move |_: &Escape, window, cx| {
                    if state.read(cx).text().len() == 0 {
                        return;
                    }
                    state.update(cx, |s, cx| s.clean(window, cx));
                    cx.stop_propagation();
                })
            })
            .child(input)
    }
}

/// The "Filter:" label and a small `TlTextInput` with a clear button and Esc to clear. 160 wide
/// at rest; 280 while focused or holding text. The width changes over 150 ms `ease_out_cubic`,
/// at once under reduced motion. The box does not debounce: the owner waits 100 ms after the
/// last `Change` before it sends the query (ADR-009).
#[derive(IntoElement)]
pub struct TlFilterBox {
    id: ElementId,
    state: Entity<InputState>,
    disabled: bool,
}

impl TlFilterBox {
    pub fn new(id: impl Into<ElementId>, state: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Focuses the box and inserts `ch` at the caret, emitting `Change`. The shell calls this
    /// for Ctrl+F and for a printable key typed while a table has focus (E20).
    pub fn focus_and_insert(
        state: &Entity<InputState>,
        ch: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        state.update(cx, |s, cx| {
            s.focus(window, cx);
            if !ch.is_empty() {
                s.insert(ch.to_owned(), window, cx);
            }
        });
    }
}

impl RenderOnce for TlFilterBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = self.state.read(cx);
        let wide = s.focus_handle(cx).is_focused(window) || s.text().len() > 0;
        let target = if wide {
            FILTER_WIDTH_WIDE
        } else {
            FILTER_WIDTH
        };
        let width = transition(
            (self.id.clone(), "width"),
            px(target * m.ui_scale),
            Transition::new(FILTER_GROW).ease(ease_out_cubic),
            window,
            cx,
        );

        let mut label = div()
            .flex_none()
            .whitespace_nowrap()
            .text_color(theme.text_secondary);
        *label.text_style() = t_sm(&m);

        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[3])
            .child(label.child("Filter:"))
            .child(
                TlTextInput::new(self.id, &self.state)
                    .size(TlInputSize::Sm)
                    .clear_button(true)
                    .clean_on_escape(true)
                    .disabled(self.disabled)
                    .width(width),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui_kit::component::input::InputEvent;
    use gpui_kit::{
        AnyElement, AppContext as _, Context, FocusHandle, KeyBinding, Render, Subscription,
        TestAppContext, VisualTestContext,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_input_test, [RootEscape]);

    type Build = Box<dyn Fn(&Entity<InputState>) -> AnyElement>;

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        cx.bind_keys([KeyBinding::new("escape", RootEscape, Some("TlInputTest"))]);
    }

    /// Renders `build(&state)`, counts `Change` events and `RootEscape` actions at the root.
    struct Host {
        state: Entity<InputState>,
        other: FocusHandle,
        build: Build,
        changes: Rc<Cell<usize>>,
        root_escapes: Rc<Cell<usize>>,
        _sub: Subscription,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let escapes = self.root_escapes.clone();
            div()
                .id("root")
                .key_context("TlInputTest")
                .on_action(move |_: &RootEscape, _, _| escapes.set(escapes.get() + 1))
                .size(px(600.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .child((self.build)(&self.state))
                .child(div().id("other").track_focus(&self.other).size(px(20.)))
        }
    }

    fn host(
        cx: &mut TestAppContext,
        build: impl Fn(&Entity<InputState>) -> AnyElement + 'static,
    ) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|cx| InputState::new(window, cx));
            let changes = Rc::new(Cell::new(0));
            let sub = cx.subscribe(&state, {
                let changes = changes.clone();
                move |_, _, event: &InputEvent, _| {
                    if matches!(event, InputEvent::Change) {
                        changes.set(changes.get() + 1);
                    }
                }
            });
            Host {
                state,
                other: cx.focus_handle(),
                build: Box::new(build),
                changes,
                root_escapes: Rc::new(Cell::new(0)),
                _sub: sub,
            }
        });
        draw(cx);
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn set_text(view: &Entity<Host>, cx: &mut VisualTestContext, text: &'static str) {
        cx.update(|window, cx| {
            let state = view.read(cx).state.clone();
            state.update(cx, |s, cx| s.set_value(text, window, cx));
        });
        draw(cx);
    }

    fn focus_input(view: &Entity<Host>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let state = view.read(cx).state.clone();
            state.update(cx, |s, cx| s.focus(window, cx));
        });
        draw(cx);
    }

    fn blur(view: &Entity<Host>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let other = view.read(cx).other.clone();
            window.focus(&other, cx);
        });
        draw(cx);
    }

    fn text(view: &Entity<Host>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| view.read(cx).state.read(cx).text().to_string())
    }

    fn width(cx: &mut VisualTestContext, selector: &'static str) -> Pixels {
        draw(cx);
        cx.debug_bounds(selector)
            .expect("the input is drawn")
            .size
            .width
    }

    fn filter_box(state: &Entity<InputState>) -> AnyElement {
        TlFilterBox::new("filter", state).into_any_element()
    }

    #[gpui_kit::test]
    fn escape_clears_then_emits_change(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, filter_box);
        set_text(&view, cx, "fire");
        focus_input(&view, cx);
        let (changes, escapes) = cx.update(|_, cx| {
            let h = view.read(cx);
            (h.changes.clone(), h.root_escapes.clone())
        });
        assert_eq!(changes.get(), 0, "set_value emits no Change");

        cx.simulate_keystrokes("escape");
        assert_eq!(text(&view, cx), "");
        assert_eq!(changes.get(), 1, "one Change for the clear");
        assert_eq!(escapes.get(), 0, "the first Esc stops at the box");

        cx.simulate_keystrokes("escape");
        assert_eq!(changes.get(), 1, "no Change on empty text");
        assert_eq!(escapes.get(), 1, "Esc on empty text goes on to the root");
    }

    #[gpui_kit::test]
    fn filter_box_width_follows_focus(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = host(cx, filter_box);
        assert_eq!(width(cx, "TlTextInput-filter"), px(FILTER_WIDTH));

        focus_input(&view, cx);
        assert_eq!(
            width(cx, "TlTextInput-filter"),
            px(FILTER_WIDTH_WIDE),
            "wide in the same frame"
        );

        blur(&view, cx);
        assert_eq!(
            width(cx, "TlTextInput-filter"),
            px(FILTER_WIDTH),
            "narrow again with no text"
        );
    }

    #[gpui_kit::test]
    fn stays_wide_when_non_empty(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = host(cx, filter_box);
        focus_input(&view, cx);
        cx.simulate_input("abc");
        assert_eq!(text(&view, cx), "abc");
        blur(&view, cx);
        assert_eq!(width(cx, "TlTextInput-filter"), px(FILTER_WIDTH_WIDE));
    }

    #[gpui_kit::test]
    fn focus_and_insert_emits_change(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, filter_box);
        cx.update(|window, cx| {
            let state = view.read(cx).state.clone();
            TlFilterBox::focus_and_insert(&state, "f", window, cx);
        });
        draw(cx);
        assert_eq!(text(&view, cx), "f");
        let (changes, focused) = cx.update(|window, cx| {
            let h = view.read(cx);
            (
                h.changes.get(),
                h.state.read(cx).focus_handle(cx).is_focused(window),
            )
        });
        assert_eq!(changes, 1);
        assert!(focused);
    }

    /// Counts painted quads with a 1 px border in `color`.
    fn border_quads(
        cx: &mut VisualTestContext,
        pick: fn(&crate::theme::TlTheme) -> gpui_kit::Hsla,
    ) -> usize {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let color = pick(current(cx));
            let one = window.scale_factor();
            window
                .painted_quads()
                .iter()
                .filter(|q| q.border_color == color && q.border_widths.top.0 == one)
                .count()
        })
    }

    #[gpui_kit::test]
    fn invalid_and_loading_states(cx: &mut TestAppContext) {
        let flags = Rc::new(RefCell::new((false, false)));
        let (view, cx) = host(cx, {
            let flags = flags.clone();
            move |state| {
                let (invalid, loading) = *flags.borrow();
                TlTextInput::new("field", state)
                    .clear_button(true)
                    .invalid(invalid)
                    .loading(loading)
                    .width(px(200.))
                    .into_any_element()
            }
        });
        let redraw = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            draw(cx);
        };

        // Keep the pointer off the field, so it shows no hover border.
        cx.simulate_mouse_move(
            gpui_kit::point(px(500.), px(500.)),
            None,
            gpui_kit::Modifiers::none(),
        );
        assert_eq!(border_quads(cx, |t| t.status.danger), 0);
        assert!(
            border_quads(cx, |t| t.divider) > 0,
            "the rest border is divider"
        );
        assert!(
            cx.debug_bounds("TlTextInput-clear").is_none(),
            "no clear button when empty"
        );

        *flags.borrow_mut() = (true, false);
        redraw(cx);
        assert!(
            border_quads(cx, |t| t.status.danger) > 0,
            "invalid: danger border"
        );

        *flags.borrow_mut() = (false, false);
        set_text(&view, cx, "fire");
        redraw(cx);
        assert!(
            cx.debug_bounds("TlTextInput-clear").is_some(),
            "clear button with text"
        );
        assert!(cx.debug_bounds("TlTextInput-spinner").is_none());

        *flags.borrow_mut() = (false, true);
        redraw(cx);
        assert!(
            cx.debug_bounds("TlTextInput-spinner").is_some(),
            "loading: spinner"
        );
        assert!(
            cx.debug_bounds("TlTextInput-clear").is_none(),
            "loading: no clear button"
        );
    }

    #[gpui_kit::test]
    fn align_right_sets_text_align(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, |state| {
            TlTextInput::new("field", state)
                .align_right(true)
                .width(px(72.))
                .into_any_element()
        });
        let align = cx.update(|_, cx| view.read(cx).state.read(cx).presentation().text_align());
        assert_eq!(align, gpui_kit::TextAlign::Right);
    }

    #[gpui_kit::test]
    fn suffix_text_inside_field(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |state| {
            TlTextInput::new("field", state)
                .suffix_text("ms")
                .width(px(72.))
                .into_any_element()
        });
        draw(cx);
        let field = cx.debug_bounds("TlTextInput-field").expect("field drawn");
        let unit = cx.debug_bounds("TlTextInput-suffix").expect("suffix drawn");
        assert!(field.contains(&unit.center()));
        assert!(
            unit.right() < field.right(),
            "the suffix sits inside the border"
        );
    }

    #[gpui_kit::test]
    fn square_corners_drop_radius(cx: &mut TestAppContext) {
        let square = Rc::new(Cell::new(false));
        let (view, cx) = host(cx, {
            let square = square.clone();
            move |state| {
                TlTextInput::new("field", state)
                    .square_corners(square.get())
                    .width(px(72.))
                    .into_any_element()
            }
        });
        cx.simulate_mouse_move(
            gpui_kit::point(px(500.), px(500.)),
            None,
            gpui_kit::Modifiers::none(),
        );
        let max_radius = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let color = current(cx).divider;
                window
                    .painted_quads()
                    .iter()
                    .filter(|q| q.border_color == color)
                    .map(|q| q.corner_radii.top_left.0)
                    .fold(0., f32::max)
            })
        };
        assert!(max_radius(cx) > 0., "rounded by default");
        square.set(true);
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        assert_eq!(max_radius(cx), 0., "square corners");
    }

    #[gpui_kit::test]
    fn clear_button_clears(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, |state| {
            TlTextInput::new("field", state)
                .clear_button(true)
                .width(px(200.))
                .into_any_element()
        });
        set_text(&view, cx, "fire");
        let clear = cx.debug_bounds("TlTextInput-clear").expect("clear button");
        assert_eq!(clear.size.width, px(CLEAR_HIT));
        cx.simulate_click(clear.center(), gpui_kit::Modifiers::none());
        assert_eq!(text(&view, cx), "");
        let changes = cx.update(|_, cx| view.read(cx).changes.get());
        assert_eq!(changes, 1);
    }
}
