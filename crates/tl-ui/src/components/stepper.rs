//! `TlStepper`: a numeric field with minus and plus buttons for ranged integers (UI-FND-015,
//! APP-023, PROC-066, PROC-067). Used for `processes.interval_ms`, `history.retention_h`,
//! `history.max_disk_mb`, `motion.tab_switch_ms`, the Set Priority custom nice and the Set
//! Affinity count.
//!
//! The owner creates the `Entity<InputState>` once, passes its current value with `.value(..)` on
//! every render and stores the value it gets in `on_change`. `on_change` fires when a value is
//! applied: a button click, Up or Down, the wheel while the field is focused, Enter, or blur with
//! valid text. It never fires per keystroke, and never for a value equal to the current one.
//! Every applied value is clamped to the range.
//!
//! Typed text that is not an integer in the range turns the field border `status.danger` and
//! gives it the hover tooltip "Enter a number between {min} and {max}". Enter then does nothing,
//! and blur puts back the last valid value.
//!
//! The kit `NumberInput` is not used: its look does not match, and its wheel steps even when the
//! field is not focused. The minus and plus buttons step through `TlButton::on_click`, not an
//! action: the stepper's value is local to the control, and a button dispatches its action on
//! the focused handle, which is usually not the stepper.

use std::rc::Rc;

use gpui_kit::base::actions::{SelectDown, SelectUp};
use gpui_kit::component::input::{InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::{
    App, Context, ElementId, Entity, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, ScrollWheelEvent, SharedString, Styled, Subscription,
    TextStyleRefinement, Window, div, prelude::FluentBuilder as _, px,
};

use super::button::{TlButton, TlButtonKind};
use super::icons::TlIcon;
use super::input::TlTextInput;
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::style::{TNUM, t_base};
use crate::theme::{current, metrics};

/// Group height and button size, × ui_scale.
const HEIGHT: f32 = 28.;
/// Field width, × ui_scale.
const FIELD_WIDTH: f32 = 72.;
/// Steps per Shift+Up or Shift+Down.
const BIG_STEPS: i64 = 10;

type ChangeHandler = Rc<dyn Fn(i64, &mut Window, &mut App)>;

/// The tooltip of a field with invalid text.
pub fn range_hint(min: i64, max: i64) -> SharedString {
    format!("Enter a number between {min} and {max}").into()
}

/// `text` as a value in `min..=max`, or `None` when it is not an integer or is out of range.
/// Spaces around the digits are ignored; a leading `+` is not accepted.
fn parse_in_range(text: &str, min: i64, max: i64) -> Option<i64> {
    let text = text.trim();
    if text.starts_with('+') {
        return None;
    }
    text.parse::<i64>().ok().filter(|v| (min..=max).contains(v))
}

/// Per-stepper state kept across frames: the range and handler from the last render, the last
/// valid value and the subscription to the field's events.
struct StepperBridge {
    state: Entity<InputState>,
    min: i64,
    max: i64,
    step: i64,
    /// The last valid value, which blur with invalid text goes back to.
    value: i64,
    /// The owner's value the field text was last written for.
    shown: Option<i64>,
    on_change: Option<ChangeHandler>,
    _events: Subscription,
}

impl StepperBridge {
    fn new(state: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events =
            cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        if let Some(v) = this.typed(cx) {
                            this.apply(v, window, cx);
                        }
                    }
                    InputEvent::Blur => match this.typed(cx) {
                        Some(v) => this.apply(v, window, cx),
                        None => this.write_text(this.value, window, cx),
                    },
                    InputEvent::Change | InputEvent::Focus => {}
                },
            );
        Self {
            state: state.clone(),
            min: 0,
            max: 0,
            step: 1,
            value: 0,
            shown: None,
            on_change: None,
            _events: events,
        }
    }

    /// The field text as a value in range, if it is one.
    fn typed(&self, cx: &App) -> Option<i64> {
        let text = self.state.read(cx).text().to_string();
        parse_in_range(&text, self.min, self.max)
    }

    fn write_text(&self, value: i64, window: &mut Window, cx: &mut App) {
        let text = value.to_string();
        if self.state.read(cx).text().to_string() != text {
            self.state.update(cx, |s, cx| s.set_value(text, window, cx));
        }
    }

    /// Clamps `value`, shows it and calls `on_change` when it differs from the current value.
    fn apply(&mut self, value: i64, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.clamp(self.min, self.max);
        self.write_text(value, window, cx);
        self.shown = Some(value);
        if value == self.value {
            return;
        }
        self.value = value;
        cx.notify();
        if let Some(on_change) = self.on_change.clone() {
            on_change(value, window, cx);
        }
    }

    /// Moves by `steps` × step from the typed value when it is valid, else from the last valid
    /// value.
    fn step_by(&mut self, steps: i64, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.typed(cx).unwrap_or(self.value);
        let target = from.saturating_add(steps.saturating_mul(self.step));
        self.apply(target, window, cx);
    }
}

/// A numeric field with minus and plus buttons over an owner-held `Entity<InputState>`.
#[derive(IntoElement)]
pub struct TlStepper {
    id: ElementId,
    state: Entity<InputState>,
    min: i64,
    max: i64,
    step: i64,
    value: i64,
    suffix: Option<SharedString>,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl TlStepper {
    pub fn new(id: impl Into<ElementId>, state: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            min: 0,
            max: i64::MAX,
            step: 1,
            value: 0,
            suffix: None,
            disabled: false,
            on_change: None,
        }
    }

    /// The allowed values `min..=max` and the amount one step moves. A `step` below 1 counts
    /// as 1.
    pub fn range(mut self, min: i64, max: i64, step: i64) -> Self {
        self.min = min;
        self.max = max.max(min);
        self.step = step.max(1);
        self
    }

    /// The owner's current value. It is clamped to the range for display.
    pub fn value(mut self, value: i64) -> Self {
        self.value = value;
        self
    }

    /// A unit such as "ms", "h" or "MB", drawn inside the field in `text_secondary`.
    pub fn suffix(mut self, suffix: impl Into<SharedString>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new value when one is applied, never per keystroke.
    pub fn on_change(mut self, handler: impl Fn(i64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TlStepper {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let state = self.state;
        let value = self.value.clamp(self.min, self.max);

        let bridge = window.use_keyed_state((self.id.clone(), "stepper"), cx, |window, cx| {
            StepperBridge::new(&state, window, cx)
        });
        bridge.update(cx, |b, cx| {
            b.min = self.min;
            b.max = self.max;
            b.step = self.step;
            b.on_change = self.on_change;
            if b.shown != Some(value) {
                b.write_text(value, window, cx);
                b.shown = Some(value);
                b.value = value;
            }
        });

        let invalid = {
            let text = state.read(cx).text().to_string();
            text != value.to_string() && parse_in_range(&text, self.min, self.max).is_none()
        };
        let h = px(HEIGHT * m.ui_scale);
        let radius = m.radius_small;
        let border = theme.divider;

        let step_button = |name: &'static str, icon: TlIcon, tip: &'static str, steps: i64| {
            let bridge = bridge.clone();
            TlButton::new((self.id.clone(), name), TlButtonKind::Icon)
                .icon(icon)
                .tooltip(tip)
                .disabled(
                    self.disabled
                        || (steps < 0 && value <= self.min)
                        || (steps > 0 && value >= self.max),
                )
                .on_click(move |_, window, cx| {
                    bridge.update(cx, |b, cx| b.step_by(steps, window, cx));
                })
        };
        // The buttons draw their 1 px border on the three sides away from the field, so the
        // field's own border (which shows focus and errors) forms the joins.
        let minus = div()
            .relative()
            .flex_none()
            .size(h)
            .child(step_button("minus", TlIcon::Minus, "Decrease", -1))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .border_l_1()
                    .border_t_1()
                    .border_b_1()
                    .border_color(border)
                    .rounded_tl(radius)
                    .rounded_bl(radius),
            );
        let plus = div()
            .relative()
            .flex_none()
            .size(h)
            .child(step_button("plus", TlIcon::Plus, "Increase", 1))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .border_r_1()
                    .border_t_1()
                    .border_b_1()
                    .border_color(border)
                    .rounded_tr(radius)
                    .rounded_br(radius),
            );

        let mut field = div()
            .id((self.id.clone(), "field"))
            .debug_selector({
                let id = self.id.clone();
                move || format!("TlStepper-field-{id}")
            })
            .relative()
            .flex_none()
            .when(!self.disabled, |d| {
                let up = bridge.clone();
                let down = bridge.clone();
                let big_up = bridge.clone();
                let big_down = bridge.clone();
                let wheel = bridge.clone();
                let wheel_state = state.clone();
                d.capture_action(move |_: &MoveUp, window, cx| {
                    up.update(cx, |b, cx| b.step_by(1, window, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &MoveDown, window, cx| {
                    down.update(cx, |b, cx| b.step_by(-1, window, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &SelectUp, window, cx| {
                    big_up.update(cx, |b, cx| b.step_by(BIG_STEPS, window, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &SelectDown, window, cx| {
                    big_down.update(cx, |b, cx| b.step_by(-BIG_STEPS, window, cx));
                    cx.stop_propagation();
                })
                .on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                    if !wheel_state.read(cx).focus_handle(cx).is_focused(window) {
                        return;
                    }
                    let dy = event.delta.pixel_delta(px(1.)).y;
                    if dy == px(0.) {
                        return;
                    }
                    let steps = if dy > px(0.) { 1 } else { -1 };
                    wheel.update(cx, |b, cx| b.step_by(steps, window, cx));
                    cx.stop_propagation();
                })
            })
            .child(
                TlTextInput::new((self.id.clone(), "input"), &state)
                    .invalid(invalid)
                    .disabled(self.disabled)
                    .width(px(FIELD_WIDTH * m.ui_scale)),
            )
            .when_some(self.suffix, |d, suffix| {
                let mut unit = div()
                    .debug_selector(|| "TlStepper-suffix".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(m.sp[4])
                    .flex()
                    .items_center()
                    .text_color(theme.text_secondary);
                *unit.text_style() = t_base(&m);
                d.child(unit.child(suffix))
            });
        *field.text_style() = TextStyleRefinement {
            font_features: Some(TNUM.clone()),
            ..Default::default()
        };
        if invalid {
            let tip = HoverTooltip::state((self.id.clone(), "invalid-tooltip"), window, cx);
            field = HoverTooltip::attach(
                &tip,
                field,
                DELAY_CONTROL,
                TlTooltip::text(range_hint(self.min, self.max)).build(),
                window,
                cx,
            );
        }

        div()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("TlStepper-{id}")
            })
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(h)
            .child(minus)
            .child(field)
            .child(plus)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use gpui_kit::{
        AppContext as _, Bounds, FocusHandle, Modifiers, Pixels, Render, ScrollDelta,
        TestAppContext, TouchPhase, VisualTestContext, point,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    /// Renders one stepper over `value` and records every `on_change` value.
    struct Host {
        state: Entity<InputState>,
        other: FocusHandle,
        range: (i64, i64, i64),
        value: Rc<Cell<i64>>,
        changes: Rc<RefCell<Vec<i64>>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (min, max, step) = self.range;
            let value = self.value.clone();
            let changes = self.changes.clone();
            div()
                .size(px(600.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .child(
                    TlStepper::new("s", &self.state)
                        .range(min, max, step)
                        .value(self.value.get())
                        .suffix("ms")
                        .on_change(move |v, _, _| {
                            value.set(v);
                            changes.borrow_mut().push(v);
                        }),
                )
                .child(div().id("other").track_focus(&self.other).size(px(20.)))
        }
    }

    fn host(
        cx: &mut TestAppContext,
        range: (i64, i64, i64),
        value: i64,
    ) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (view, cx) = cx.add_window_view(|window, cx| Host {
            state: cx.new(|cx| InputState::new(window, cx)),
            other: cx.focus_handle(),
            range,
            value: Rc::new(Cell::new(value)),
            changes: Rc::new(RefCell::new(Vec::new())),
        });
        // Focus events and HoverTooltip need an active window.
        cx.update(|window, _| window.activate_window());
        draw(cx);
        // Keep the pointer off the stepper, so nothing shows a hover look.
        cx.simulate_mouse_move(point(px(500.), px(500.)), None, Modifiers::none());
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        draw(cx);
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let b = bounds(cx, selector);
        cx.simulate_click(b.center(), Modifiers::none());
        draw(cx);
    }

    fn text(view: &Entity<Host>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| view.read(cx).state.read(cx).text().to_string())
    }

    fn changes(view: &Entity<Host>, cx: &mut VisualTestContext) -> Vec<i64> {
        cx.update(|_, cx| view.read(cx).changes.borrow().clone())
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

    /// Counts painted quads with a 1 px `status.danger` border.
    fn danger_quads(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let color = current(cx).status.danger;
            let one = window.scale_factor();
            window
                .painted_quads()
                .iter()
                .filter(|q| q.border_color == color && q.border_widths.top.0 == one)
                .count()
        })
    }

    fn scroll_up(cx: &mut VisualTestContext, at: gpui_kit::Point<Pixels>) {
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., 1.)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        draw(cx);
    }

    #[test]
    fn parse_rules() {
        assert_eq!(parse_in_range("12", 1, 12), Some(12));
        assert_eq!(parse_in_range(" 5 ", 1, 12), Some(5));
        assert_eq!(parse_in_range("-20", -20, 19), Some(-20));
        assert_eq!(parse_in_range("13", 1, 12), None);
        assert_eq!(parse_in_range("abc", 1, 12), None);
        assert_eq!(parse_in_range("", 1, 12), None);
        assert_eq!(parse_in_range("+5", 1, 12), None);
        assert_eq!(parse_in_range("5.0", 1, 12), None);
    }

    #[gpui_kit::test]
    fn clamps_and_disables_ends(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, (1, 12, 1), 12);
        assert_eq!(text(&view, cx), "12");

        click(cx, "TlButton-s-plus");
        assert!(changes(&view, cx).is_empty(), "plus is disabled at max");

        click(cx, "TlButton-s-minus");
        assert_eq!(changes(&view, cx), vec![11], "minus is enabled");
        assert_eq!(text(&view, cx), "11");

        // An owner value out of range shows clamped.
        cx.update(|_, cx| {
            view.update(cx, |h, cx| {
                h.value.set(99);
                cx.notify();
            })
        });
        draw(cx);
        assert_eq!(text(&view, cx), "12");

        // Down at min does nothing.
        cx.update(|_, cx| {
            view.update(cx, |h, cx| {
                h.value.set(1);
                cx.notify();
            })
        });
        draw(cx);
        click(cx, "TlButton-s-minus");
        assert_eq!(changes(&view, cx), vec![11], "minus is disabled at min");
    }

    #[gpui_kit::test]
    fn plus_click_steps_once(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, (250, 10000, 250), 1000);
        click(cx, "TlButton-s-plus");
        assert_eq!(changes(&view, cx), vec![1250]);
        assert_eq!(text(&view, cx), "1250");
    }

    #[gpui_kit::test]
    fn layout_and_suffix(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, (250, 10000, 250), 1000);
        let group = bounds(cx, "TlStepper-s");
        assert_eq!(group.size.height, px(28.));
        assert_eq!(group.size.width, px(28. + 72. + 28.));
        let field = bounds(cx, "TlStepper-field-s");
        let unit = bounds(cx, "TlStepper-suffix");
        assert!(
            field.contains(&unit.center()) && unit.right() < field.right(),
            "the unit sits inside the field"
        );
    }

    #[gpui_kit::test]
    fn invalid_reverts_on_blur(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, (250, 10000, 250), 1000);
        focus_input(&view, cx);
        set_text(&view, cx, "abc");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(text(&view, cx), "abc", "Enter does not apply invalid text");
        blur(&view, cx);
        assert_eq!(text(&view, cx), "1000");
        assert!(changes(&view, cx).is_empty());
        assert_eq!(danger_quads(cx), 0, "valid again after the revert");
    }

    #[gpui_kit::test]
    fn enter_applies_typed_value(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, (250, 10000, 250), 1000);
        focus_input(&view, cx);
        set_text(&view, cx, "2000");
        assert!(changes(&view, cx).is_empty(), "no change per keystroke");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(changes(&view, cx), vec![2000]);
    }

    #[gpui_kit::test]
    fn invalid_tooltip_text(cx: &mut TestAppContext) {
        assert_eq!(
            range_hint(250, 10000).as_ref(),
            "Enter a number between 250 and 10000"
        );
        let (view, cx) = host(cx, (250, 10000, 250), 1000);
        assert_eq!(danger_quads(cx), 0);
        focus_input(&view, cx);
        set_text(&view, cx, "99999");
        assert!(danger_quads(cx) > 0, "out of range: danger border");
        assert!(changes(&view, cx).is_empty(), "not applied");

        let field = bounds(cx, "TlStepper-field-s");
        cx.simulate_mouse_move(field.center(), None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        draw(cx);
        assert!(
            cx.debug_bounds("TlTooltip").is_some(),
            "the range tooltip shows on hover"
        );
    }

    #[gpui_kit::test]
    fn wheel_only_when_focused(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, (250, 10000, 250), 1000);
        let field = bounds(cx, "TlStepper-field-s");
        scroll_up(cx, field.center());
        assert!(changes(&view, cx).is_empty(), "unfocused: no step");
        assert_eq!(text(&view, cx), "1000");

        focus_input(&view, cx);
        scroll_up(cx, field.center());
        assert_eq!(changes(&view, cx), vec![1250], "focused: one step");
    }

    #[gpui_kit::test]
    fn shift_steps_ten(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, (0, 100000, 250), 1000);
        focus_input(&view, cx);
        cx.simulate_keystrokes("up");
        draw(cx);
        assert_eq!(changes(&view, cx), vec![1250]);
        cx.simulate_keystrokes("shift-up");
        draw(cx);
        assert_eq!(changes(&view, cx), vec![1250, 3750]);
        cx.simulate_keystrokes("shift-down");
        draw(cx);
        cx.simulate_keystrokes("down");
        draw(cx);
        assert_eq!(changes(&view, cx), vec![1250, 3750, 1250, 1000]);
    }
}
