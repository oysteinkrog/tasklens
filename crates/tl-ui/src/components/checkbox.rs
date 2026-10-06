//! `TlCheckbox`: a 16 px checkbox with an indeterminate state (UI-FND-011).
//!
//! The kit `Checkbox` is 9.75 px at rem 13, too small, so tasklens draws its own. The whole row
//! (box and label) is one tab stop and one click target. Space toggles through the
//! `ToggleCheckbox` action; the keyboard click GPUI sends for Space and Enter on key up is
//! ignored, so one Space press fires `on_change` once. The owner stores the new value and
//! notifies.

use std::rc::Rc;

use gpui_kit::component::Colorize;
use gpui_kit::{
    Animation, AnimationExt, App, ClickEvent, ElementId, FocusHandle, Hsla, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px, relative,
};

use crate::components::focus::TlFocusRing;
use crate::components::icons::{TlIcon, icon};
use crate::components::motion::{M_FAST, ease_out_cubic};
use crate::theme::{TlTheme, current, metrics};

gpui_kit::actions!(tasklens, [ToggleCheckbox]);

/// Box side at 100% scale.
const BOX: f32 = 16.;
/// Smallest box corner radius at 100% scale, also when `rounded_corners` is off.
const MIN_RADIUS: f32 = 3.;
/// Row height in menus and settings, and inline (filter strip), at 100% scale.
const ROW: f32 = 24.;
const ROW_INLINE: f32 = 20.;
/// The check mark grows from this fraction of `i_xs` to full size.
const MARK_START_SCALE: f32 = 0.6;
/// Hover lightens and press darkens a checked fill by this fraction.
const SHADE: f32 = 0.08;
/// The check and minus colour, `#FFFFFF` on every theme.
const MARK_COLOR: Hsla = gpui_kit::white();
/// The group name the row gives its box for hover and pressed styles.
const GROUP: &str = "tl-checkbox";

/// Binds the keys of every shared component. Called once from `TasklensUi::init` (E16-A1).
/// Later component beads add their bindings here.
pub fn bind_keys_components(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("space", ToggleCheckbox, Some("TlCheckbox"))]);
    super::menu::bind_keys(cx);
    super::radio::bind_keys_radio(cx);
    super::select::bind_keys(cx);
    super::slider::bind_keys(cx);
    super::tabs::bind_keys(cx);
}

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A checkbox with a label. `checked(None)` is the indeterminate state.
#[derive(IntoElement)]
pub struct TlCheckbox {
    id: ElementId,
    checked: Option<bool>,
    label: Option<SharedString>,
    inline: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl TlCheckbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: Some(false),
            label: None,
            inline: false,
            disabled: false,
            on_change: None,
        }
    }

    /// `Some(true)` checked, `Some(false)` unchecked, `None` indeterminate.
    pub fn checked(mut self, checked: Option<bool>) -> Self {
        self.checked = checked;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Row height 20 (filter strip) instead of 24.
    pub fn inline(mut self, inline: bool) -> Self {
        self.inline = inline;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new value on a click or Space.
    pub fn on_change(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

/// The value a toggle sets: unchecked and indeterminate become checked.
fn toggled(checked: Option<bool>) -> bool {
    checked != Some(true)
}

/// Colours and glyph of the box at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BoxLook {
    fill: Hsla,
    border: Hsla,
    hover_fill: Hsla,
    hover_border: Hsla,
    pressed_fill: Hsla,
    glyph: Option<TlIcon>,
}

fn box_look(checked: Option<bool>, theme: &TlTheme) -> BoxLook {
    let glyph = match checked {
        Some(true) => Some(TlIcon::Check),
        None => Some(TlIcon::Minus),
        Some(false) => None,
    };
    if glyph.is_some() {
        let hover = theme.accent.lighten(SHADE);
        BoxLook {
            fill: theme.accent,
            border: theme.accent,
            hover_fill: hover,
            hover_border: hover,
            pressed_fill: theme.accent.darken(SHADE),
            glyph,
        }
    } else {
        BoxLook {
            fill: theme.input,
            border: theme.text_secondary,
            hover_fill: theme.input,
            hover_border: theme.text_primary,
            pressed_fill: theme.header,
            glyph,
        }
    }
}

/// Per-element state kept across frames while the checkbox is rendered.
struct CheckboxState {
    focus: FocusHandle,
    /// The glyph drawn last frame.
    glyph: Option<TlIcon>,
    /// Counts glyph appearances after the first frame. Keys the mark animation, so each new
    /// glyph grows once; 0 means the glyph was there from the first frame and does not animate.
    epoch: u32,
}

impl RenderOnce for TlCheckbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let look = box_look(self.checked, &theme);

        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| CheckboxState {
            focus: cx.focus_handle(),
            glyph: look.glyph,
            epoch: 0,
        });
        let (focus, epoch) = state.update(cx, |s, _| {
            if look.glyph != s.glyph {
                if look.glyph.is_some() {
                    s.epoch += 1;
                }
                s.glyph = look.glyph;
            }
            (s.focus.clone().tab_stop(!self.disabled), s.epoch)
        });

        let scale = |v: f32| px(v * m.ui_scale);
        let radius = m.radius_small.max(scale(MIN_RADIUS));
        let mark_size = m.icon.xs;

        let mark = look.glyph.map(|glyph| {
            let mark = icon(glyph, mark_size, MARK_COLOR);
            let mark = if epoch == 0 {
                mark.into_any_element()
            } else {
                mark.with_animation(
                    ("tl-checkbox-mark", epoch as usize),
                    Animation::new(M_FAST).with_easing(ease_out_cubic),
                    move |mark, t| {
                        mark.size(mark_size * (MARK_START_SCALE + (1. - MARK_START_SCALE) * t))
                    },
                )
                .into_any_element()
            };
            div()
                .debug_selector(move || glyph_selector(glyph).into())
                .flex_none()
                .child(mark)
        });

        let mut check_box = div()
            .id("box")
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(scale(BOX))
            .rounded(radius)
            .border(px(1.))
            .border_color(look.border)
            .bg(look.fill)
            .children(mark);
        if !self.disabled {
            check_box = check_box
                .group_hover(GROUP, |s| {
                    s.bg(look.hover_fill).border_color(look.hover_border)
                })
                .group_active(GROUP, |s| s.bg(look.pressed_fill));
        }
        if TlFocusRing::show(focus.is_focused(window), window) {
            check_box = TlFocusRing::ring(check_box, radius, &theme);
        }

        let row_height = scale(if self.inline { ROW_INLINE } else { ROW });
        let mut row = div()
            .id(self.id)
            .group(GROUP)
            .flex()
            .items_center()
            .gap(m.sp[3])
            .h(row_height)
            .track_focus(&focus)
            .key_context("TlCheckbox")
            .child(check_box)
            .children(self.label.map(|label| {
                div()
                    .text_size(m.text.base)
                    .line_height(relative(m.line_height))
                    .text_color(theme.text_primary)
                    .child(label)
            }));

        if self.disabled {
            row = row.opacity(0.5);
        } else {
            let checked = self.checked;
            let on_action = self.on_change.clone();
            let on_click = self.on_change;
            let click_focus = focus.clone();
            row = row
                .on_action(move |_: &ToggleCheckbox, window, cx| {
                    if let Some(f) = &on_action {
                        f(toggled(checked), window, cx);
                    }
                })
                .on_click(move |event, window, cx| {
                    // Space comes through `ToggleCheckbox`; GPUI's keyboard click on key up
                    // would fire a second time, and Enter does not toggle a checkbox.
                    if matches!(event, ClickEvent::Keyboard(_)) {
                        return;
                    }
                    click_focus.focus(window, cx);
                    if let Some(f) = &on_click {
                        f(toggled(checked), window, cx);
                    }
                });
        }
        row
    }
}

fn glyph_selector(glyph: TlIcon) -> &'static str {
    match glyph {
        TlIcon::Minus => "TlCheckbox-minus",
        _ => "TlCheckbox-check",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::{
        Context, KeyUpEvent, Keystroke, Modifiers, Render, TestAppContext, VisualTestContext, point,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    struct Harness {
        checked: Option<bool>,
        changes: Rc<RefCell<Vec<bool>>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let changes = self.changes.clone();
            let view = cx.entity();
            div().size_full().child(
                TlCheckbox::new("cb")
                    .checked(self.checked)
                    .label("Include children")
                    .on_change(move |new, _, cx| {
                        changes.borrow_mut().push(new);
                        view.update(cx, |h, cx| {
                            h.checked = Some(new);
                            cx.notify();
                        });
                    }),
            )
        }
    }

    fn open(
        cx: &mut TestAppContext,
        checked: Option<bool>,
    ) -> (
        gpui_kit::Entity<Harness>,
        &mut VisualTestContext,
        Rc<RefCell<Vec<bool>>>,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            bind_keys_components(cx);
        });
        let changes = Rc::new(RefCell::new(Vec::new()));
        let c = changes.clone();
        let (view, cx) = cx.add_window_view(move |_, _| Harness {
            checked,
            changes: c,
        });
        cx.run_until_parked();
        (view, cx, changes)
    }

    fn focus_checkbox(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.focus_next(cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn space_toggles(cx: &mut TestAppContext) {
        let (_, cx, changes) = open(cx, Some(false));
        focus_checkbox(cx);
        cx.simulate_keystrokes("space");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("space").unwrap(),
        });
        assert_eq!(*changes.borrow(), vec![true], "one Space press, one change");
    }

    #[gpui_kit::test]
    fn label_click_toggles(cx: &mut TestAppContext) {
        let (view, cx, changes) = open(cx, Some(false));
        // The box is 16 px wide at x 0; the label starts after the 6 px gap.
        cx.simulate_click(point(px(40.), px(12.)), Modifiers::none());
        assert_eq!(*changes.borrow(), vec![true]);
        cx.simulate_click(point(px(40.), px(12.)), Modifiers::none());
        assert_eq!(*changes.borrow(), vec![true, false]);
        cx.update(|_, cx| assert_eq!(view.read(cx).checked, Some(false)));
    }

    #[gpui_kit::test]
    fn indeterminate_glyph(cx: &mut TestAppContext) {
        let (_, cx, changes) = open(cx, None);
        let theme = cx.update(|_, cx| current(cx).clone());
        let look = box_look(None, &theme);
        assert_eq!(look.glyph, Some(TlIcon::Minus));
        assert_eq!(look.fill, theme.accent);
        assert_eq!(look.border, theme.accent);
        assert!(cx.debug_bounds("TlCheckbox-minus").is_some());
        assert!(cx.debug_bounds("TlCheckbox-check").is_none());

        focus_checkbox(cx);
        cx.simulate_keystrokes("space");
        assert_eq!(
            *changes.borrow(),
            vec![true],
            "indeterminate toggles to checked"
        );
    }

    #[gpui_kit::test]
    fn reduced_motion_instant(cx: &mut TestAppContext) {
        let (view, cx, _) = open(cx, Some(false));
        let full = cx.update(|_, cx| metrics(cx).icon.xs);

        cx.update(|_, cx| cx.set_reduce_motion(true));
        view.update(cx, |h, cx| {
            h.checked = Some(true);
            cx.notify();
        });
        cx.run_until_parked();
        let mark = cx.debug_bounds("TlCheckbox-check").expect("mark drawn");
        assert_eq!(mark.size.width, full, "full scale in the same frame");

        // With motion on, the next appearance starts small.
        cx.update(|_, cx| cx.set_reduce_motion(false));
        view.update(cx, |h, cx| {
            h.checked = Some(false);
            cx.notify();
        });
        cx.run_until_parked();
        view.update(cx, |h, cx| {
            h.checked = Some(true);
            cx.notify();
        });
        cx.run_until_parked();
        let mark = cx.debug_bounds("TlCheckbox-check").expect("mark drawn");
        assert!(
            mark.size.width < full,
            "{:?} grows from 0.6",
            mark.size.width
        );
    }
}
