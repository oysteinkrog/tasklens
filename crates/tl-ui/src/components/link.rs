//! `TlLink`: inline link text (UI-FND-032), for the PPID, Executable and Unit links, documentation
//! links, "Network settings", "Needs setup" and "Additional setup…".
//!
//! Colour `status.info`, underlined on hover only, pointer cursor, a Tab stop. Click, Enter or
//! Space dispatches the owner's action from the link, so it bubbles to the owner's `on_action`.
//! GPUI turns Enter and Space on a focused element into a keyboard click on key up, so `on_click`
//! covers all three and no key binding is needed. Right-click is left to the owner.
//!
//! [`TlLink::on_click_bounds`] replaces the dispatch with the owner's handler, called with the
//! link's bounds, so the owner can open a popup under it (a keyboard click passes the bounds
//! GPUI gives it; a mouse click passes the bounds of the last paint).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Colorize;
use gpui_kit::{
    Action, App, Bounds, ClickEvent, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, div,
};

use crate::components::focus::TlFocusRing;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::style::label_style;
use crate::theme::{TlTheme, current, metrics};

/// Press darkens the link colour by this fraction.
const PRESS_SHADE: f32 = 0.08;

/// The owner's click handler, with the link's bounds.
pub type LinkClick = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)>;

/// A link. The owner handles `action`; the link only dispatches it.
#[derive(IntoElement)]
pub struct TlLink {
    id: ElementId,
    text: SharedString,
    action: Box<dyn Action>,
    text_size: Option<Pixels>,
    disabled_reason: Option<SharedString>,
    on_click: Option<LinkClick>,
    tab_stop: bool,
    /// The owner's focus handle, used instead of the link's own.
    focus: Option<FocusHandle>,
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
            on_click: None,
            tab_stop: true,
            focus: None,
        }
    }

    /// Click, Enter or Space calls `handler` with the link's bounds instead of dispatching the
    /// action.
    pub fn on_click_bounds(mut self, handler: LinkClick) -> Self {
        self.on_click = Some(handler);
        self
    }

    /// Whether Tab stops on the link. Default `true`. An owner whose region is one focus stop
    /// with its own keys (the Properties field grid) turns it off; a click still focuses it.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    /// Takes `focus` instead of the link's own handle, so the owner can name the link in a focus
    /// order.
    pub fn focus_handle(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
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

        let focus: FocusHandle = match self.focus.clone() {
            Some(focus) => focus,
            None => window
                .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone(),
        }
        .tab_stop(self.tab_stop);

        let size = self.text_size.unwrap_or(m.text.base);
        // The label style first: `truncate` adds to the same text style.
        let mut text = div();
        *text.text_style() = label_style(&m, size, FontWeight::NORMAL);
        let text = text.min_w_0().truncate().child(self.text.clone());

        let tooltip_id = ElementId::NamedChild(Arc::new(self.id.clone()), "tooltip".into());
        // The bounds of the last paint, for a mouse click with an owner handler.
        let painted = self.on_click.as_ref().map(|_| {
            let id = ElementId::NamedChild(Arc::new(self.id.clone()), "bounds".into());
            window
                .use_keyed_state(id, cx, |_, _| Rc::new(Cell::new(Bounds::default())))
                .read(cx)
                .clone()
        });

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
        if let Some(cell) = &painted {
            let cell = cell.clone();
            link = link.child(
                canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        }

        // A disabled link tracks no focus, so it is neither a Tab stop nor focusable.
        if !disabled && TlFocusRing::show(focus.is_focused(window), window) {
            link = TlFocusRing::ring(link, m.radius_small, &theme);
        }

        if let Some(reason) = self.disabled_reason {
            let state = HoverTooltip::state(tooltip_id, window, cx);
            let build = TlTooltip::text(reason).build();
            HoverTooltip::attach(&state, link, DELAY_CONTROL, build, window, cx)
        } else {
            let (action, on_click) = (self.action, self.on_click);
            link.track_focus(&focus)
                .cursor_pointer()
                .hover(|s| s.underline())
                .active(move |s| s.text_color(look.pressed))
                .on_click(move |ev, window, cx| {
                    focus.focus(window, cx);
                    match (&on_click, &painted) {
                        (Some(handler), Some(painted)) => {
                            let bounds = match ev {
                                ClickEvent::Keyboard(k) => k.bounds,
                                _ => painted.get(),
                            };
                            handler(bounds, window, cx);
                        }
                        _ => window.dispatch_action(action.boxed_clone(), cx),
                    }
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
        /// Set: the link calls the owner's handler, which records the bounds.
        clicked: Option<Rc<Cell<Option<Bounds<Pixels>>>>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let dispatched = self.dispatched.clone();
            let mut link = TlLink::new("link", "systemd", Box::new(OpenTarget));
            if let Some(clicked) = self.clicked.clone() {
                link = link.on_click_bounds(Rc::new(move |b, _, _| clicked.set(Some(b))));
            }
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
                clicked: None,
            }
        });
        cx.run_until_parked();
        (cx, dispatched)
    }

    /// A link with an owner click handler; the handler records the bounds it is given.
    fn harness_bounds(
        cx: &mut TestAppContext,
    ) -> (
        &mut VisualTestContext,
        Rc<Cell<usize>>,
        Rc<Cell<Option<Bounds<Pixels>>>>,
    ) {
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
        let clicked = Rc::new(Cell::new(None));
        let (_, cx) = cx.add_window_view({
            let (dispatched, clicked) = (dispatched.clone(), clicked.clone());
            move |_, _| Harness {
                disabled: false,
                dispatched,
                clicked: Some(clicked),
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, dispatched, clicked)
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
    fn on_click_bounds_replaces_dispatch(cx: &mut TestAppContext) {
        let (cx, dispatched, clicked) = harness_bounds(cx);
        cx.simulate_click(point(px(4.), px(6.)), Modifiers::none());
        let mouse = clicked.take().expect("the handler ran");
        assert_eq!(dispatched.get(), 0, "no dispatch");
        assert_eq!(mouse.origin, point(px(0.), px(0.)));
        assert!(mouse.size.width > px(0.) && mouse.size.height > px(0.));

        focus_link(cx);
        press(cx, "enter");
        let keyboard = clicked.take().expect("Enter runs the handler");
        assert_eq!(keyboard, mouse, "the keyboard click passes the same bounds");
        assert_eq!(dispatched.get(), 0);
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
