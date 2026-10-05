//! `TlSwitch`: the on/off control for boolean settings in the Settings window (UI-FND-013,
//! APP-023, D-003, APP-036). Menus use check items instead.
//!
//! Built on the kit's unstyled base `Switch`, `SwitchTrack` and `SwitchThumb`, which own the
//! click, Space and Enter activation and the switch role. The styled kit `Switch` (`.small()`)
//! is not used because its sizes, label gap, label text size and label side are fixed (its
//! `label_side` has no setter), and its focus ring is not keyboard-only.
//!
//! Small size only: track 28 × 16, thumb 12, inset 2, all × ui_scale. Off: track
//! `scroll_thumb`. On: track `accent`. Thumb white. The thumb slides over `m_fast` (120 ms,
//! ease-out cubic) and moves at once when `App::reduce_motion()` is set.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{Switch, SwitchThumb, SwitchTrack};
use gpui_kit::{
    App, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Transition, Window, div, gpui, prelude::FluentBuilder as _, px, rgb,
    transition,
};

use super::focus::TlFocusRing;
use crate::theme::{TlTheme, current, metrics};

/// `m_fast`: the thumb slide (UI-FND-003 motion).
const M_FAST: Duration = Duration::from_millis(120);

/// Track hover: lightness up by 0.08.
const HOVER_STEP: f32 = 0.08;

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The track fill: `accent` when on, `scroll_thumb` when off.
fn track_bg(theme: &TlTheme, checked: bool) -> Hsla {
    if checked {
        theme.accent
    } else {
        theme.scroll_thumb
    }
}

/// A switch with an optional label on its left. The owner stores the value: `on_change`
/// reports the requested value, and the owner renders it back through `checked`.
#[derive(IntoElement)]
pub struct TlSwitch {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
}

impl TlSwitch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            disabled: false,
            label: None,
            on_change: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Text left of the track. It is also the name screen readers announce.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A disabled switch is drawn at opacity 0.5, takes no focus and ignores input.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the requested value on click, Space or Enter. Never called while disabled.
    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TlSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let (track_w, track_h, thumb, inset) = (px(28. * s), px(16. * s), px(12. * s), px(2. * s));

        let focus: FocusHandle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let show_ring = !self.disabled && TlFocusRing::show(focus.is_focused(window), window);

        let thumb_x = transition(
            (self.id.clone(), "thumb"),
            if self.checked {
                track_w - thumb - inset * 2.
            } else {
                px(0.)
            },
            Transition::new(M_FAST),
            window,
            cx,
        );

        let bg = track_bg(&theme, self.checked);
        let hover_bg = Hsla {
            l: (bg.l + HOVER_STEP).min(1.0),
            ..bg
        };
        let radius = track_h / 2.;

        let track = SwitchTrack::new((self.id.clone(), "track"))
            .checked(self.checked)
            .disabled(self.disabled)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .w(track_w)
            .h(track_h)
            .p(inset)
            .rounded(radius)
            .bg(bg)
            .when(!self.disabled, |t| t.hover(|st| st.bg(hover_bg)))
            .child(
                SwitchThumb::new(self.checked)
                    .disabled(self.disabled)
                    .size(thumb)
                    .left(thumb_x)
                    .rounded(thumb / 2.)
                    .bg(rgb(0xFFFFFF)),
            );
        let track = if show_ring {
            TlFocusRing::ring(track, radius, &theme)
        } else {
            track
        };

        let on_change = self.on_change;
        Switch::new(self.id)
            .checked(self.checked)
            .disabled(self.disabled)
            .track_focus(&focus)
            .when_some(self.label.clone(), |sw, label| {
                sw.accessibility_label(label)
            })
            .when_some(on_change, |sw, on_change| {
                sw.on_change(move |next, _, window, cx| on_change(next, window, cx))
            })
            .flex()
            .flex_row()
            .items_center()
            .gap(m.sp[4])
            .when(self.disabled, |sw| sw.opacity(0.5))
            .when_some(self.label, |sw, label| {
                sw.child(
                    div()
                        .text_size(m.text.base)
                        .text_color(theme.text_primary)
                        .child(label),
                )
            })
            .child(track)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::component::theme::Theme;
    use gpui_kit::{
        Context, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render, TestAppContext,
        VisualTestContext, point,
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

    struct Harness {
        checked: bool,
        disabled: bool,
        calls: Rc<RefCell<Vec<bool>>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let calls = self.calls.clone();
            div().id("root").tab_group().size(px(200.)).child(
                TlSwitch::new("switch")
                    .label("Keep history")
                    .checked(self.checked)
                    .disabled(self.disabled)
                    .on_change(move |next, _, _| calls.borrow_mut().push(next)),
            )
        }
    }

    fn harness(
        cx: &mut TestAppContext,
        checked: bool,
        disabled: bool,
    ) -> (&mut VisualTestContext, Rc<RefCell<Vec<bool>>>) {
        cx.update(setup);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let (_, cx) = cx.add_window_view({
            let calls = calls.clone();
            move |_, _| Harness {
                checked,
                disabled,
                calls,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, calls)
    }

    fn focus_switch(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.focus_next(cx));
        cx.update(|window, cx| {
            assert!(window.focused(cx).is_some(), "the switch takes focus");
            window.draw(cx).clear(cx);
        });
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
    }

    #[gpui_kit::test]
    fn space_toggles(cx: &mut TestAppContext) {
        let (cx, calls) = harness(cx, false, false);
        focus_switch(cx);
        press(cx, "space");
        assert_eq!(*calls.borrow(), vec![true]);
    }

    #[gpui_kit::test]
    fn enter_toggles(cx: &mut TestAppContext) {
        let (cx, calls) = harness(cx, true, false);
        focus_switch(cx);
        press(cx, "enter");
        assert_eq!(*calls.borrow(), vec![false]);
    }

    #[gpui_kit::test]
    fn disabled_is_inert(cx: &mut TestAppContext) {
        let (cx, calls) = harness(cx, false, true);
        cx.simulate_click(point(px(10.), px(8.)), Modifiers::none());
        cx.simulate_click(point(px(90.), px(8.)), Modifiers::none());
        press(cx, "space");
        press(cx, "enter");
        assert!(calls.borrow().is_empty());
        cx.update(|window, cx| assert!(window.focused(cx).is_none(), "no focus while disabled"));
    }

    #[gpui_kit::test]
    fn track_colours(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            let theme = current(cx).clone();
            let kit = &Theme::global(cx).colors;
            assert_eq!(track_bg(&theme, true), theme.accent);
            assert_eq!(track_bg(&theme, false), theme.scroll_thumb);
            // The kit mapping agrees, so kit switches and TlSwitch match.
            assert_eq!(track_bg(&theme, true), kit.primary);
            assert_eq!(track_bg(&theme, false), kit.switch);
        });
    }
}
