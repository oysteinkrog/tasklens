//! Slider for ranged settings: UI Scale, Row Height, Value Color Coding Intensity, Drop Shadow
//! Intensity and `motion.smooth_scroll_amount` (UI-FND-014, APP-040, APP-023).
//!
//! The view owns a kit `SliderState` entity and renders `TlSlider::new(&state)`. It subscribes to
//! `TlSliderEvent` on that same entity:
//! - `Preview(v)` fires on every pointer move of a drag (kit `SliderEvent::Change`). Apply the
//!   value live, do not save it.
//! - `Commit(v)` fires once when the pointer is released (kit `SliderEvent::Release`), and on
//!   each keyboard step. Save the setting here only.
//!
//! `Preview` can fire at pointer rate. For UI Scale each preview rebuilds the metrics, so the
//! owner (the shell) applies at most one per frame: keep the latest value and apply it in
//! `on_next_frame`. The slider does not coalesce.
//!
//! When the setting changes from elsewhere (Ctrl+Plus), call `set_from_elsewhere`: it moves the
//! thumb and emits nothing, so a `Commit` handler that saves the setting cannot loop back.
//!
//! The kit's styled `Slider` fixes its thumb at 16 px with no border, so this composes the kit's
//! unstyled `Slider`, `SliderTrack`, `SliderIndicator` and `SliderThumb` and styles them here. The
//! kit slider has no keyboard handling; the `TlSlider` key context adds it.

use std::sync::{Arc, LazyLock};

use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::base::{Slider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, FontFeatures, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Pixels, RenderOnce, SharedString, Styled, Subscription,
    TextOverflow, Window, div, px, relative, white,
};

use crate::components::focus::TlFocusRing;
use crate::theme::{current, metrics};

gpui_kit::actions!(
    tasklens,
    [
        SliderStepDown,
        SliderStepUp,
        SliderBigStepDown,
        SliderBigStepUp,
        SliderPageDown,
        SliderPageUp,
        SliderMin,
        SliderMax,
    ]
);

/// Key context of the slider row.
pub const KEY_CONTEXT: &str = "TlSlider";

/// Steps per Shift+arrow press.
const BIG_STEPS: f32 = 5.0;
/// Steps per PageUp or PageDown press.
const PAGE_STEPS: f32 = 10.0;

/// Sizes before the UI scale.
const TRACK_HEIGHT: f32 = 4.0;
const TRACK_HIT_HEIGHT: f32 = 16.0;
const LENGTH_MENU: f32 = 120.0;
const LENGTH_PAGE: f32 = 200.0;
const THUMB: f32 = 12.0;
const THUMB_ACTIVE: f32 = 14.0;
pub const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_LABEL_MIN_WIDTH: f32 = 140.0;
const VALUE_MIN_WIDTH: f32 = 40.0;

/// Tabular figures for the value text. `text/style.rs::TNUM` replaces this when it lands.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// Binds the slider keys in the `TlSlider` context.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", SliderStepDown, ctx),
        KeyBinding::new("right", SliderStepUp, ctx),
        KeyBinding::new("shift-left", SliderBigStepDown, ctx),
        KeyBinding::new("shift-right", SliderBigStepUp, ctx),
        KeyBinding::new("pagedown", SliderPageDown, ctx),
        KeyBinding::new("pageup", SliderPageUp, ctx),
        KeyBinding::new("home", SliderMin, ctx),
        KeyBinding::new("end", SliderMax, ctx),
    ]);
}

/// What the slider tells its owner. Emitted by the `SliderState` entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TlSliderEvent {
    /// The value while dragging. Apply it, do not save it.
    Preview(f32),
    /// The final value after a release or a keyboard step. Save it.
    Commit(f32),
}

impl EventEmitter<TlSliderEvent> for SliderState {}

/// Sets the value without emitting an event, for a setting changed elsewhere (Ctrl+Plus). The
/// value is clamped to the range. Does nothing when the value is already there.
pub fn set_from_elsewhere(state: &Entity<SliderState>, value: f32, cx: &mut App) {
    state.update(cx, |s, cx| {
        let value = value.clamp(s.min_value(), s.max_value());
        if s.value() == SliderValue::Single(value) {
            return;
        }
        rebuild(s, s.min_value(), s.max_value(), s.step_value(), value);
        cx.notify();
    });
}

/// Replaces the kit state with one of the same scale (always linear here) and the given range
/// and value. The kit only sets the range through its builder, and `set_value` needs a window.
/// Bounds come back on the next prepaint, and the kit's drag flag comes back on the next drag
/// move.
fn rebuild(s: &mut SliderState, min: f32, max: f32, step: f32, value: f32) {
    *s = SliderState::new()
        .min(min)
        .max(max)
        .step(step)
        .default_value(value);
}

/// A keyboard move.
#[derive(Clone, Copy, Debug)]
pub(crate) enum KeyMove {
    Steps(f32),
    Min,
    Max,
}

/// The value a keyboard move lands on: on the step grid from `min`, clamped to the range.
fn key_target(value: f32, min: f32, max: f32, step: f32, mv: KeyMove) -> f32 {
    match mv {
        KeyMove::Steps(n) => {
            let raw = value + n * step;
            let snapped = if step > 0.0 {
                min + ((raw - min) / step).round() * step
            } else {
                raw
            };
            snapped.clamp(min, max)
        }
        KeyMove::Min => min,
        KeyMove::Max => max,
    }
}

/// Applies a keyboard move and emits `Commit` when the value changed. A press at a limit emits
/// nothing.
pub(crate) fn key_move(
    state: &Entity<SliderState>,
    mv: KeyMove,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |s, cx| {
        let value = s.value().end();
        let next = key_target(value, s.min_value(), s.max_value(), s.step_value(), mv);
        if next == value {
            return;
        }
        s.set_value(next, window, cx);
        cx.emit(TlSliderEvent::Commit(next));
    });
}

/// Per-slider element state: the focus handle, the drag flag and the subscription that maps kit
/// events to `TlSliderEvent`. Lives while the slider renders.
struct SliderBridge {
    focus: FocusHandle,
    dragging: bool,
    _events: Subscription,
}

impl SliderBridge {
    fn new(state: &Entity<SliderState>, cx: &mut Context<Self>) -> Self {
        let events = cx.subscribe(state, |this, state, event: &SliderEvent, cx| {
            let (dragging, out) = match event {
                SliderEvent::Change(v) => (true, TlSliderEvent::Preview(v.end())),
                SliderEvent::Release(v) => (false, TlSliderEvent::Commit(v.end())),
            };
            if this.dragging != dragging {
                this.dragging = dragging;
                cx.notify();
            }
            state.update(cx, |_, cx| cx.emit(out));
        });
        Self {
            focus: cx.focus_handle().tab_stop(true),
            dragging: false,
            _events: events,
        }
    }
}

/// A horizontal slider with an optional label on the left and the value text on the right.
#[derive(IntoElement)]
pub struct TlSlider {
    state: Entity<SliderState>,
    label: Option<SharedString>,
    range: Option<(f32, f32, f32)>,
    value_text: Option<SharedString>,
    in_menu: bool,
    disabled: bool,
}

impl TlSlider {
    pub fn new(state: &Entity<SliderState>) -> Self {
        Self {
            state: state.clone(),
            label: None,
            range: None,
            value_text: None,
            in_menu: false,
            disabled: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets the range and step on the state. Without it the state keeps its own range.
    pub fn range(mut self, min: f32, max: f32, step: f32) -> Self {
        self.range = Some((min, max, step));
        self
    }

    /// The text right of the track, formatted by the owner (for example "100%"). Without it the
    /// bare number shows.
    pub fn value_text(mut self, text: impl Into<SharedString>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// In a menu row: track 120 long, row 28 high, label at least 140 wide. Otherwise the track is
    /// 200 long, for a settings page.
    pub fn in_menu(mut self, in_menu: bool) -> Self {
        self.in_menu = in_menu;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for TlSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state;
        let id = state.entity_id();
        if let Some((min, max, step)) = self.range {
            state.update(cx, |s, _| {
                if (s.min_value(), s.max_value(), s.step_value()) != (min, max, step) {
                    let value = s.value().end().clamp(min, max);
                    rebuild(s, min, max, step, value);
                }
            });
        }

        let bridge = window.use_keyed_state(("tl-slider-bridge", id), cx, |_, cx| {
            SliderBridge::new(&state, cx)
        });
        let (focus, dragging) = {
            let b = bridge.read(cx);
            (b.focus.clone().tab_stop(!self.disabled), b.dragging)
        };

        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = |v: f32| px(v * m.ui_scale);
        let disabled = self.disabled;

        let (value, percentage) = {
            let st = state.read(cx);
            (st.value().end(), st.percentage().end)
        };
        let value_text = self
            .value_text
            .unwrap_or_else(|| SharedString::from(value.to_string()));

        let thumb_size = if dragging { THUMB_ACTIVE } else { THUMB };
        let mut thumb_dot = div()
            .relative()
            .flex_none()
            .size(s(thumb_size))
            .rounded_full()
            .bg(white())
            .border_1()
            .border_color(theme.divider)
            .when(!disabled, |d| {
                d.group_hover("tl-slider-thumb", |st| st.size(s(THUMB_ACTIVE)))
            });
        if TlFocusRing::show(focus.is_focused(window), window) {
            let radius: Pixels = s(thumb_size) / 2.0;
            thumb_dot = TlFocusRing::ring(thumb_dot, radius, &theme);
        }

        let thumb = SliderThumb::new(&state)
            .disabled(disabled)
            .group("tl-slider-thumb")
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .size(s(THUMB_ACTIVE))
            .top(s((TRACK_HEIGHT - THUMB_ACTIVE) / 2.0))
            .left(relative(percentage))
            .ml(-s(THUMB_ACTIVE / 2.0))
            .child(thumb_dot);

        let indicator = SliderIndicator::new(&state)
            .relative()
            .w_full()
            .h(s(TRACK_HEIGHT))
            .rounded_full()
            .bg(theme.scroll_thumb)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(relative(percentage))
                    .rounded_full()
                    .bg(theme.accent),
            )
            .child(thumb);

        let slider = Slider::new(&state)
            .horizontal()
            .disabled(disabled)
            .flex_none()
            .w(s(if self.in_menu {
                LENGTH_MENU
            } else {
                LENGTH_PAGE
            }))
            // The thumb overhangs the track end by half its width: keep the value text clear.
            .when(!self.in_menu, |d| d.mr(s(THUMB_ACTIVE / 2.0)))
            .child(
                SliderTrack::new(&state)
                    .disabled(disabled)
                    .flex()
                    .items_center()
                    .w_full()
                    .h(s(TRACK_HIT_HEIGHT))
                    .child(indicator),
            );

        let value_el = div()
            .debug_selector(|| "TlSliderValue".into())
            .flex_none()
            .min_w(s(VALUE_MIN_WIDTH))
            .text_size(m.text.sm)
            .font_features(TNUM.clone())
            .text_color(if dragging {
                theme.text_primary
            } else {
                theme.text_secondary
            })
            .child(value_text);

        div()
            .id(("tl-slider", id))
            .key_context(KEY_CONTEXT)
            .track_focus(&focus)
            .flex()
            .items_center()
            .gap(m.sp[4])
            .when(self.in_menu, |d| d.h(s(MENU_ROW_HEIGHT)).flex_1().min_w_0())
            .when_some(self.label, |d, label| {
                // In a menu the label takes the row's spare width, 140 if the menu has room,
                // and ends in an ellipsis when the menu is at its widest.
                let label_el = if self.in_menu {
                    div()
                        .flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(s(MENU_LABEL_MIN_WIDTH))
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_overflow(TextOverflow::Truncate("\u{2026}".into()))
                } else {
                    div().flex_none()
                };
                d.child(
                    label_el
                        .debug_selector(|| "TlSliderLabel".into())
                        .text_size(m.text.base)
                        .text_color(theme.text_primary)
                        .child(label),
                )
            })
            .child(slider)
            .child(value_el)
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| {
                let on = |mv: KeyMove| {
                    let state = state.clone();
                    move |window: &mut Window, cx: &mut App| key_move(&state, mv, window, cx)
                };
                let (down, up) = (on(KeyMove::Steps(-1.0)), on(KeyMove::Steps(1.0)));
                let (big_down, big_up) = (
                    on(KeyMove::Steps(-BIG_STEPS)),
                    on(KeyMove::Steps(BIG_STEPS)),
                );
                let (page_down, page_up) = (
                    on(KeyMove::Steps(-PAGE_STEPS)),
                    on(KeyMove::Steps(PAGE_STEPS)),
                );
                let (min, max) = (on(KeyMove::Min), on(KeyMove::Max));
                let focus = focus.clone();
                // A press on the thumb or track focuses the slider. The kit thumb stops the
                // mouse down from bubbling, so this listens in the capture phase.
                d.capture_any_mouse_down(move |_, window, cx| window.focus(&focus, cx))
                    .on_action(move |_: &SliderStepDown, w, cx| down(w, cx))
                    .on_action(move |_: &SliderStepUp, w, cx| up(w, cx))
                    .on_action(move |_: &SliderBigStepDown, w, cx| big_down(w, cx))
                    .on_action(move |_: &SliderBigStepUp, w, cx| big_up(w, cx))
                    .on_action(move |_: &SliderPageDown, w, cx| page_down(w, cx))
                    .on_action(move |_: &SliderPageUp, w, cx| page_up(w, cx))
                    .on_action(move |_: &SliderMin, w, cx| min(w, cx))
                    .on_action(move |_: &SliderMax, w, cx| max(w, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        AppContext as _, Bounds, Modifiers, MouseButton, Point, Render, TestAppContext,
        VisualTestContext, point,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    struct Harness {
        state: Entity<SliderState>,
        _events: Subscription,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(TlSlider::new(&self.state).range(0.0, 200.0, 5.0))
        }
    }

    type Events = Rc<RefCell<Vec<TlSliderEvent>>>;

    fn setup(
        cx: &mut TestAppContext,
        value: f32,
    ) -> (&mut VisualTestContext, Entity<SliderState>, Events) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            bind_keys(cx);
        });
        let events: Events = Rc::default();
        let state = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(200.0)
                .step(5.0)
                .default_value(value)
        });
        let (s, log) = (state.clone(), events.clone());
        let (_, vcx) = cx.add_window_view(move |_, cx| Harness {
            _events: cx.subscribe(&s, move |_, _, e: &TlSliderEvent, _| {
                log.borrow_mut().push(*e)
            }),
            state: s,
        });
        vcx.run_until_parked();
        (vcx, state, events)
    }

    /// The track bounds from the last prepaint.
    fn track(state: &Entity<SliderState>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.update(|_, cx| state.read(cx).bounds())
    }

    fn thumb_center(state: &Entity<SliderState>, cx: &mut VisualTestContext) -> Point<Pixels> {
        let b = track(state, cx);
        let pct = cx.update(|_, cx| state.read(cx).percentage().end);
        point(b.left() + b.size.width * pct, b.center().y)
    }

    fn value(state: &Entity<SliderState>, cx: &mut VisualTestContext) -> f32 {
        cx.update(|_, cx| state.read(cx).value().end())
    }

    /// Clicks the thumb to focus the slider. A click without a move emits no event.
    fn focus_slider(state: &Entity<SliderState>, cx: &mut VisualTestContext) {
        let at = thumb_center(state, cx);
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn release_saves_once(cx: &mut TestAppContext) {
        let (cx, state, events) = setup(cx, 100.0);
        let start = thumb_center(&state, cx);
        let none = Modifiers::none();
        cx.simulate_mouse_down(start, MouseButton::Left, none);
        // The first move only starts the kit drag; it reports no value.
        cx.simulate_mouse_move(start + point(px(4.), px(0.)), MouseButton::Left, none);
        for i in 1..=5 {
            let at = start + point(px(4. + 10. * i as f32), px(0.));
            cx.simulate_mouse_move(at, MouseButton::Left, none);
        }
        let end = start + point(px(54.), px(0.));
        cx.simulate_mouse_up(end, MouseButton::Left, none);
        cx.run_until_parked();

        let ev = events.borrow();
        let previews = ev
            .iter()
            .filter(|e| matches!(e, TlSliderEvent::Preview(_)))
            .count();
        let commits: Vec<_> = ev
            .iter()
            .filter(|e| matches!(e, TlSliderEvent::Commit(_)))
            .collect();
        assert_eq!(previews, 5, "events: {ev:?}");
        assert_eq!(commits.len(), 1, "events: {ev:?}");
        assert_eq!(ev.last(), Some(&TlSliderEvent::Commit(value(&state, cx))));
        assert!(value(&state, cx) > 100.0);
    }

    #[gpui_kit::test]
    fn arrow_steps_and_clamps(cx: &mut TestAppContext) {
        let (cx, state, events) = setup(cx, 195.0);
        focus_slider(&state, cx);
        assert!(events.borrow().is_empty(), "focusing emits nothing");

        cx.simulate_keystrokes("right right");
        assert_eq!(value(&state, cx), 200.0);
        assert_eq!(*events.borrow(), vec![TlSliderEvent::Commit(200.0)]);

        cx.simulate_keystrokes("left");
        assert_eq!(value(&state, cx), 195.0);
        assert_eq!(events.borrow().last(), Some(&TlSliderEvent::Commit(195.0)));
    }

    #[gpui_kit::test]
    fn shift_and_page_steps(cx: &mut TestAppContext) {
        let (cx, state, events) = setup(cx, 100.0);
        focus_slider(&state, cx);

        cx.simulate_keystrokes("shift-left");
        assert_eq!(value(&state, cx), 75.0);
        cx.simulate_keystrokes("shift-right shift-right");
        assert_eq!(value(&state, cx), 125.0);
        cx.simulate_keystrokes("pageup");
        assert_eq!(value(&state, cx), 175.0);
        cx.simulate_keystrokes("pagedown pagedown");
        assert_eq!(value(&state, cx), 75.0);
        cx.simulate_keystrokes("end");
        assert_eq!(value(&state, cx), 200.0);
        cx.simulate_keystrokes("pageup");
        assert_eq!(value(&state, cx), 200.0, "clamped at max");
        cx.simulate_keystrokes("home");
        assert_eq!(value(&state, cx), 0.0);
        assert_eq!(
            *events.borrow(),
            [75.0, 100.0, 125.0, 175.0, 125.0, 75.0, 200.0, 0.0]
                .map(TlSliderEvent::Commit)
                .to_vec()
        );
    }

    #[gpui_kit::test]
    fn set_from_elsewhere_no_event(cx: &mut TestAppContext) {
        let (cx, state, events) = setup(cx, 100.0);
        let before = thumb_center(&state, cx);
        cx.update(|_, cx| set_from_elsewhere(&state, 150.0, cx));
        cx.run_until_parked();

        assert_eq!(value(&state, cx), 150.0);
        assert!(events.borrow().is_empty());
        let after = thumb_center(&state, cx);
        assert!(after.x > before.x, "thumb moved: {before:?} to {after:?}");
    }

    #[gpui_kit::test]
    fn value_text_clears_the_thumb_at_max(cx: &mut TestAppContext) {
        let (cx, state, _) = setup(cx, 200.0);
        let track = track(&state, cx);
        let value = cx.debug_bounds("TlSliderValue").expect("value drawn");
        let thumb_right = track.right() + px(THUMB_ACTIVE / 2.0);
        assert!(
            value.left() - thumb_right >= px(4.0),
            "thumb right {thumb_right:?}, value left {:?}",
            value.left()
        );
    }
}
