//! Keyboard-only focus ring (UI-FND-003, R-123, U-42).
//!
//! GPUI has no `focus-visible`, but the window records whether the last input was a key press
//! (`Window::last_input_was_keyboard`: a key down sets it, a mouse move or mouse down clears it).
//! Views pass `TlFocusRing::show(focused, window)` and draw the ring only when it is true.
//!
//! Three focus styles:
//! - Most controls: `TlFocusRing::ring`, 2 px solid `accent`, 2 px outside the border box.
//! - Tree-table rows: `TlFocusRing::row_focus_border`, a 1 px inset `accent` border, because the
//!   list clips its children and would cut off an outside ring.
//! - Inputs: the `input_focus` background plus a 1 px `accent` border, no ring. The input
//!   component draws that itself.

use gpui_kit::{
    AbsoluteLength, Hsla, ParentElement, Pixels, Styled, Window, div, prelude::FluentBuilder as _,
    px,
};

use crate::theme::TlTheme;

/// `true`: the ring shows only after keyboard input (U-42). `false`: the ring shows whenever the
/// element is focused, the kit default (R-123).
pub const KEYBOARD_ONLY_RING: bool = true;

/// Ring width and gap outside the border box.
const RING: Pixels = px(2.);

pub struct TlFocusRing;

impl TlFocusRing {
    /// Whether a focused element shows its focus ring now.
    pub fn show(focused: bool, window: &Window) -> bool {
        focused && (!KEYBOARD_ONLY_RING || window.last_input_was_keyboard())
    }

    /// Adds the ring to `el` as an absolute child: 2 px solid `accent`, 2 px outside the border
    /// box, corner radius `radius + 2`. `radius` is the element's own corner radius. Border
    /// widths are read from `el` and must be set in px, as every tasklens component does. A
    /// clipping ancestor cuts the ring off, so leave it 2 px of room.
    pub fn ring<E: Styled + ParentElement>(mut el: E, radius: Pixels, theme: &TlTheme) -> E {
        let borders = el.style().border_widths.clone();
        let width = |w: Option<AbsoluteLength>| match w {
            Some(AbsoluteLength::Pixels(p)) => p,
            _ => px(0.),
        };
        el.child(
            div()
                .when(cfg!(test), |d| d.debug_selector(|| "tl-focus-ring".into()))
                .absolute()
                .flex_none()
                .top(-(RING + width(borders.top)))
                .right(-(RING + width(borders.right)))
                .bottom(-(RING + width(borders.bottom)))
                .left(-(RING + width(borders.left)))
                .border_2()
                .border_color(theme.accent)
                .rounded(radius + RING),
        )
    }

    /// The focus border for a tree-table row: 1 px `accent`, drawn inset.
    pub fn row_focus_border(theme: &TlTheme) -> (Pixels, Hsla) {
        (px(1.), theme.accent)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Context, FocusHandle, InteractiveElement, IntoElement, KeyBinding, Modifiers, Render,
        TestAppContext, point,
    };

    use super::*;

    gpui_kit::actions!(tl_focus_test, [FocusNext]);

    struct TwoTargets {
        a: FocusHandle,
        b: FocusHandle,
    }

    impl Render for TwoTargets {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context("TlFocusTest")
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .size_full()
                .child(div().id("a").track_focus(&self.a).size(px(40.)))
                .child(div().id("b").track_focus(&self.b).size(px(40.)))
        }
    }

    #[gpui_kit::test]
    fn ring_only_after_keyboard(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new("tab", FocusNext, Some("TlFocusTest"))]);
        });
        let (view, cx) = cx.add_window_view(|_, cx| TwoTargets {
            a: cx.focus_handle().tab_stop(true),
            b: cx.focus_handle().tab_stop(true),
        });

        cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.a.is_focused(window), "the click focuses a");
            assert!(!TlFocusRing::show(v.a.is_focused(window), window));
        });

        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.b.is_focused(window), "Tab moves focus to b");
            assert!(TlFocusRing::show(v.b.is_focused(window), window));
            assert!(!TlFocusRing::show(v.a.is_focused(window), window));
        });
    }
}
