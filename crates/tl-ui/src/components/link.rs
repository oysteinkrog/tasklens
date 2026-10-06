//! `TlLink`: inline link text (UI-FND-032), for the PPID, Executable and Unit links, documentation
//! links, "Network settings", "Needs setup" and "Additional setup…".
//!
//! Colour `status.info`, underlined on hover only, pointer cursor, a Tab stop. Click, Enter or
//! Space dispatches the owner's action from the link, so it bubbles to the owner's `on_action`.
//! GPUI turns Enter and Space on a focused element into a keyboard click on key up, so `on_click`
//! covers all three and no key binding is needed. Right-click is left to the owner.

use gpui_kit::component::Colorize;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    Action, App, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled,
    Window, div,
};

use crate::components::focus::TlFocusRing;
use crate::text::style::label_style;
use crate::theme::{TlTheme, current, metrics};

/// Press darkens the link colour by this fraction.
const PRESS_SHADE: f32 = 0.08;

/// A link. The owner handles `action`; the link only dispatches it.
#[derive(IntoElement)]
pub struct TlLink {
    id: ElementId,
    text: SharedString,
    action: Box<dyn Action>,
    text_size: Option<Pixels>,
    disabled_reason: Option<SharedString>,
}

impl TlLink {
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        action: Box<dyn Action>,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            action,
            text_size: None,
            disabled_reason: None,
        }
    }

    /// Text size of the surrounding text. Default `t_base`.
    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = Some(size);
        self
    }

    /// Drawn in `text_secondary` with no underline and no pointer, takes no focus and ignores
    /// input. Hovering shows `reason` as a tooltip.
    pub fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.disabled_reason = Some(reason.into());
        self
    }
}

/// Colours and pointer of a link.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LinkLook {
    color: Hsla,
    /// Colour while the mouse button is down. Equal to `color` when disabled.
    pressed: Hsla,
    /// Underline and pointer cursor on hover.
    interactive: bool,
}

fn link_look(disabled: bool, theme: &TlTheme) -> LinkLook {
    if disabled {
        LinkLook {
            color: theme.text_secondary,
            pressed: theme.text_secondary,
            interactive: false,
        }
    } else {
        LinkLook {
            color: theme.status.info,
            pressed: theme.status.info.darken(PRESS_SHADE),
            interactive: true,
        }
    }
}

impl RenderOnce for TlLink {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let disabled = self.disabled_reason.is_some();
        let look = link_look(disabled, &theme);

        let focus: FocusHandle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_stop(true);

        let size = self.text_size.unwrap_or(m.text.base);
        // The label style first: `truncate` adds to the same text style.
        let mut text = div();
        *text.text_style() = label_style(&m, size, FontWeight::NORMAL);
        let text = text.min_w_0().truncate().child(self.text.clone());

        // The outer box carries the ring and does not clip; the inner text truncates.
        let mut link = div()
            .id(self.id)
            .relative()
            .flex()
            .min_w_0()
            .max_w_full()
            .role(Role::Link)
            .aria_label(self.text)
            .key_context("TlLink")
            .text_color(look.color)
            .child(text);

        // A disabled link tracks no focus, so it is neither a Tab stop nor focusable.
        if !disabled && TlFocusRing::show(focus.is_focused(window), window) {
            link = TlFocusRing::ring(link, m.radius_small, &theme);
        }

        if let Some(reason) = self.disabled_reason {
            link.tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx))
        } else {
            let action = self.action;
            link.track_focus(&focus)
                .cursor_pointer()
                .hover(|s| s.underline())
                .active(move |s| s.text_color(look.pressed))
                .on_click(move |_, window, cx| {
                    focus.focus(window, cx);
                    window.dispatch_action(action.boxed_clone(), cx);
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        Context, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render, TestAppContext,
        VisualTestContext, point, px,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_link_test, [OpenTarget]);

    struct Harness {
        disabled: bool,
        dispatched: Rc<Cell<usize>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let dispatched = self.dispatched.clone();
            let link = TlLink::new("link", "systemd", Box::new(OpenTarget));
            let link = if self.disabled {
                link.disabled("Not reported")
            } else {
                link
            };
            div()
                .id("root")
                .size(px(200.))
                .on_action(move |_: &OpenTarget, _, _| dispatched.set(dispatched.get() + 1))
                .child(link)
        }
    }

    fn harness(
        cx: &mut TestAppContext,
        disabled: bool,
    ) -> (&mut VisualTestContext, Rc<Cell<usize>>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
        });
        let dispatched = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let dispatched = dispatched.clone();
            move |_, _| Harness {
                disabled,
                dispatched,
            }
        });
        cx.run_until_parked();
        (cx, dispatched)
    }

    fn focus_link(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.focus_next(cx));
        cx.update(|window, cx| {
            assert!(window.focused(cx).is_some(), "the link is a tab stop");
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

    fn underlines(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.painted_underlines().len()
        })
    }

    #[gpui_kit::test]
    fn hover_underlines(cx: &mut TestAppContext) {
        let (cx, _) = harness(cx, false);
        assert_eq!(underlines(cx), 0, "no underline at rest");

        cx.simulate_mouse_move(point(px(4.), px(6.)), None, Modifiers::none());
        cx.run_until_parked();
        assert!(underlines(cx) > 0, "underline on hover");

        let theme = cx.update(|_, cx| current(cx).clone());
        let look = link_look(false, &theme);
        assert!(look.interactive, "pointer cursor on hover");
        assert_eq!(look.color, theme.status.info);
        assert_eq!(look.pressed, theme.status.info.darken(PRESS_SHADE));
    }

    #[gpui_kit::test]
    fn enter_dispatches_action(cx: &mut TestAppContext) {
        let (cx, dispatched) = harness(cx, false);
        focus_link(cx);
        press(cx, "enter");
        assert_eq!(dispatched.get(), 1);
    }

    #[gpui_kit::test]
    fn space_dispatches_action(cx: &mut TestAppContext) {
        let (cx, dispatched) = harness(cx, false);
        focus_link(cx);
        press(cx, "space");
        assert_eq!(dispatched.get(), 1);
    }

    #[gpui_kit::test]
    fn click_dispatches_action(cx: &mut TestAppContext) {
        let (cx, dispatched) = harness(cx, false);
        cx.simulate_click(point(px(4.), px(6.)), Modifiers::none());
        assert_eq!(dispatched.get(), 1);
    }

    #[gpui_kit::test]
    fn disabled_inert(cx: &mut TestAppContext) {
        let (cx, dispatched) = harness(cx, true);
        cx.simulate_click(point(px(4.), px(6.)), Modifiers::none());
        cx.update(|window, cx| window.focus_next(cx));
        press(cx, "enter");
        press(cx, "space");
        assert_eq!(dispatched.get(), 0);
        cx.update(|window, cx| assert!(window.focused(cx).is_none(), "not a tab stop"));

        cx.simulate_mouse_move(point(px(4.), px(6.)), None, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(underlines(cx), 0, "no underline while disabled");

        let theme = cx.update(|_, cx| current(cx).clone());
        let look = link_look(true, &theme);
        assert_eq!(look.color, theme.text_secondary);
        assert!(!look.interactive);
    }
}
