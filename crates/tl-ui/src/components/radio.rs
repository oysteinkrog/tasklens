//! Radio buttons and a radio group with a roving tab index (UI-FND-012, APP-023, APP-037).
//!
//! The kit `RadioGroup` is not used because it has no arrow keys. `TlRadioGroup` is one tab
//! stop: only the selected radio is in the tab order. Up/Left and Down/Right move the selection
//! and the focus (wrapping), Home/End go to the first and last item. The owner stores the index
//! from `on_change` and notifies.

use std::rc::Rc;

use gpui_kit::{
    Animation, AnimationExt, App, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder,
};

use super::focus::TlFocusRing;
use super::motion::{M_FAST, ease_out_cubic};
use crate::theme::{current, metrics};

gpui_kit::actions!(tasklens, [RadioPrev, RadioNext, RadioFirst, RadioLast]);

/// Key context of `TlRadioGroup`.
pub const RADIO_GROUP_CONTEXT: &str = "TlRadioGroup";

/// Hover and press group name of one radio row.
const ROW_GROUP: &str = "tl-radio-row";

/// Binds the arrow keys, Home and End inside `TlRadioGroup`.
pub fn bind_keys_radio(cx: &mut App) {
    let ctx = Some(RADIO_GROUP_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", RadioPrev, ctx),
        KeyBinding::new("left", RadioPrev, ctx),
        KeyBinding::new("down", RadioNext, ctx),
        KeyBinding::new("right", RadioNext, ctx),
        KeyBinding::new("home", RadioFirst, ctx),
        KeyBinding::new("end", RadioLast, ctx),
    ]);
}

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type ChangeHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Per-radio state that lives across frames.
struct RadioState {
    /// Used when the owner passes no focus handle.
    focus: FocusHandle,
    was_checked: bool,
    /// Counts unchecked-to-checked changes. 0 means the dot has never grown, so the first frame
    /// draws it at full size with no animation.
    grow: usize,
}

/// One radio: a 16 px circle with the label to the right.
#[derive(IntoElement)]
pub struct TlRadio {
    id: ElementId,
    label: SharedString,
    checked: bool,
    disabled: bool,
    focus_handle: Option<FocusHandle>,
    on_click: Option<ClickHandler>,
}

impl TlRadio {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            checked: false,
            disabled: false,
            focus_handle: None,
            on_click: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The handle to track. Without one the radio keeps its own handle and is a tab stop.
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for TlRadio {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let checked = self.checked;
        let disabled = self.disabled;

        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| RadioState {
            focus: cx.focus_handle().tab_stop(true),
            was_checked: checked,
            grow: 0,
        });
        let (own_focus, grow) = state.update(cx, |s, _| {
            if checked && !s.was_checked {
                s.grow += 1;
            }
            s.was_checked = checked;
            (s.focus.clone(), s.grow)
        });
        let focus = match self.focus_handle {
            Some(handle) => handle,
            None => own_focus.tab_stop(!disabled),
        };

        // Sizes: circle 16 (`sp6`), dot 8 (`sp4`), row 24 (`sp7`), all × ui_scale.
        let circle_size = m.sp[6];
        let dot_size = m.sp[4];
        let border = if checked {
            theme.accent
        } else {
            theme.text_secondary
        };
        let mut circle = div()
            .id("circle")
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(circle_size)
            .rounded_full()
            .border_1()
            .border_color(border)
            .bg(theme.input)
            .when(!disabled && !checked, |el| {
                el.group_hover(ROW_GROUP, |s| s.border_color(theme.text_primary))
            })
            .when(!disabled, |el| {
                el.group_active(ROW_GROUP, |s| s.bg(theme.header))
            });

        if checked {
            let selector = format!("{}-dot", self.id);
            let dot = div()
                .debug_selector(move || selector)
                .flex_none()
                .rounded_full()
                .bg(theme.accent);
            circle = if grow == 0 || cx.reduce_motion() {
                circle.child(dot.size(dot_size))
            } else {
                circle.child(dot.with_animation(
                    ("tl-radio-dot", grow),
                    Animation::new(M_FAST).with_easing(ease_out_cubic),
                    move |el, t| el.size(dot_size * t),
                ))
            };
        }

        if TlFocusRing::show(focus.is_focused(window), window) {
            circle = TlFocusRing::ring(circle, circle_size / 2.0, &theme);
        }

        div()
            .id(self.id)
            .group(ROW_GROUP)
            .track_focus(&focus)
            .flex()
            .flex_row()
            .items_center()
            .gap(m.sp[3])
            .h(m.sp[7])
            .when(disabled, |el| el.opacity(0.5))
            .child(circle)
            .child(
                div()
                    .text_size(m.text.base)
                    .text_color(theme.text_primary)
                    .child(self.label),
            )
            .when_some(self.on_click.filter(|_| !disabled), |el, on_click| {
                el.on_click(move |_, window, cx| {
                    focus.focus(window, cx);
                    on_click(window, cx);
                })
            })
    }
}

/// Layout of a `TlRadioGroup`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RadioDirection {
    /// 24 px rows with `sp2` between them.
    #[default]
    Vertical,
    /// One row with `sp5` gaps.
    Horizontal,
}

/// Focus handles of the group's radios, one per item.
struct GroupState {
    handles: Vec<FocusHandle>,
}

/// A group of radios that is one tab stop.
#[derive(IntoElement)]
pub struct TlRadioGroup {
    id: ElementId,
    items: Vec<SharedString>,
    selected: usize,
    direction: RadioDirection,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl TlRadioGroup {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            direction: RadioDirection::Vertical,
            disabled: false,
            on_change: None,
        }
    }

    pub fn items(mut self, items: Vec<SharedString>) -> Self {
        self.items = items;
        self
    }

    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    pub fn direction(mut self, direction: RadioDirection) -> Self {
        self.direction = direction;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new index when a click or a key changes the selection.
    pub fn on_change(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

/// Moves `ix` one step back or forward in `0..len`, wrapping.
fn step(ix: usize, len: usize, forward: bool) -> usize {
    if forward {
        (ix + 1) % len
    } else {
        (ix + len - 1) % len
    }
}

/// Focuses item `to` and reports it when it is not the selected item.
fn move_to(
    to: usize,
    selected: usize,
    handles: &[FocusHandle],
    on_change: &Option<ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    handles[to].focus(window, cx);
    if to != selected
        && let Some(f) = on_change
    {
        f(to, window, cx);
    }
}

impl RenderOnce for TlRadioGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let len = self.items.len();
        let selected = self.selected.min(len.saturating_sub(1));
        let disabled = self.disabled;

        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| GroupState {
            handles: Vec::new(),
        });
        let handles: Rc<[FocusHandle]> = state.update(cx, |s, cx| {
            s.handles.truncate(len);
            while s.handles.len() < len {
                s.handles.push(cx.focus_handle());
            }
            s.handles.clone().into()
        });

        // Arrow keys start from the focused radio, else from the selected one.
        let from = {
            let handles = handles.clone();
            move |window: &Window| {
                handles
                    .iter()
                    .position(|h| h.is_focused(window))
                    .unwrap_or(selected)
            }
        };

        let on_change = self.on_change;
        let radios = self.items.into_iter().enumerate().map(|(ix, label)| {
            let handle = handles[ix].clone().tab_stop(!disabled && ix == selected);
            let on_change = on_change.clone();
            TlRadio::new(("tl-radio", ix), label)
                .checked(ix == selected)
                .disabled(disabled)
                .focus_handle(handle)
                .on_click(move |window, cx| {
                    if ix != selected
                        && let Some(f) = &on_change
                    {
                        f(ix, window, cx);
                    }
                })
        });

        let gap = match self.direction {
            RadioDirection::Vertical => m.sp[2],
            RadioDirection::Horizontal => m.sp[5],
        };

        div()
            .id(self.id)
            .key_context(RADIO_GROUP_CONTEXT)
            .flex()
            .map(|el| match self.direction {
                RadioDirection::Vertical => el.flex_col(),
                RadioDirection::Horizontal => el.flex_row().items_center(),
            })
            .gap(gap)
            .when(!disabled && len > 0, |el| {
                let (h1, h2, h3, h4) = (
                    handles.clone(),
                    handles.clone(),
                    handles.clone(),
                    handles.clone(),
                );
                let (c1, c2, c3, c4) = (
                    on_change.clone(),
                    on_change.clone(),
                    on_change.clone(),
                    on_change.clone(),
                );
                let (f1, f2) = (from.clone(), from);
                el.on_action(move |_: &RadioPrev, window, cx| {
                    let to = step(f1(window), len, false);
                    move_to(to, selected, &h1, &c1, window, cx);
                })
                .on_action(move |_: &RadioNext, window, cx| {
                    let to = step(f2(window), len, true);
                    move_to(to, selected, &h2, &c2, window, cx);
                })
                .on_action(move |_: &RadioFirst, window, cx| {
                    move_to(0, selected, &h3, &c3, window, cx);
                })
                .on_action(move |_: &RadioLast, window, cx| {
                    move_to(len - 1, selected, &h4, &c4, window, cx);
                })
            })
            .children(radios)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::{Context, Modifiers, Render, TestAppContext, VisualTestContext, px};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_radio_test, [FocusNext]);

    struct Harness {
        selected: usize,
        changes: Rc<RefCell<Vec<usize>>>,
        before: FocusHandle,
        after: FocusHandle,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let view = cx.entity().downgrade();
            div()
                .key_context("TlRadioTest")
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .size_full()
                .child(div().id("before").track_focus(&self.before).size(px(40.)))
                .child(
                    TlRadioGroup::new("group")
                        .items(vec!["One".into(), "Two".into(), "Three".into()])
                        .selected(self.selected)
                        .on_change(move |ix, _, cx| {
                            view.update(cx, |this, cx| {
                                this.selected = ix;
                                this.changes.borrow_mut().push(ix);
                                cx.notify();
                            })
                            .ok();
                        }),
                )
                .child(div().id("after").track_focus(&self.after).size(px(40.)))
        }
    }

    fn open(
        selected: usize,
        cx: &mut TestAppContext,
    ) -> (
        gpui_kit::Entity<Harness>,
        Rc<RefCell<Vec<usize>>>,
        &mut VisualTestContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            bind_keys_radio(cx);
            cx.bind_keys([KeyBinding::new("tab", FocusNext, Some("TlRadioTest"))]);
        });
        let changes = Rc::new(RefCell::new(Vec::new()));
        let log = changes.clone();
        let (view, cx) = cx.add_window_view(|_, cx| Harness {
            selected,
            changes: log,
            before: cx.focus_handle().tab_stop(true),
            after: cx.focus_handle().tab_stop(true),
        });
        (view, changes, cx)
    }

    /// Focuses the first button and presses Tab, which lands on the selected radio.
    fn tab_into_group(view: &gpui_kit::Entity<Harness>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let before = view.read(cx).before.clone();
            window.focus(&before, cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
    }

    #[gpui_kit::test]
    fn arrow_keys_cycle(cx: &mut TestAppContext) {
        let (view, changes, cx) = open(2, cx);
        tab_into_group(&view, cx);

        cx.simulate_keystrokes("down");
        assert_eq!(*changes.borrow(), [0], "Down wraps from the last item");
        cx.simulate_keystrokes("up");
        assert_eq!(*changes.borrow(), [0, 2], "Up wraps from the first item");
        cx.simulate_keystrokes("left");
        cx.simulate_keystrokes("right");
        assert_eq!(*changes.borrow(), [0, 2, 1, 2]);
        cx.update(|_, cx| assert_eq!(view.read(cx).selected, 2));
    }

    #[gpui_kit::test]
    fn home_end(cx: &mut TestAppContext) {
        let (view, changes, cx) = open(0, cx);
        tab_into_group(&view, cx);

        cx.simulate_keystrokes("end");
        assert_eq!(*changes.borrow(), [2]);
        cx.simulate_keystrokes("home");
        assert_eq!(*changes.borrow(), [2, 0]);
        cx.simulate_keystrokes("home");
        assert_eq!(
            *changes.borrow(),
            [2, 0],
            "Home on the first item changes nothing"
        );
    }

    #[gpui_kit::test]
    fn group_is_one_tab_stop(cx: &mut TestAppContext) {
        let (view, changes, cx) = open(1, cx);
        tab_into_group(&view, cx);
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(!v.before.is_focused(window));
            assert!(!v.after.is_focused(window));
            assert!(window.focused(cx).is_some(), "focus is inside the group");
        });

        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            assert!(
                view.read(cx).after.is_focused(window),
                "the next Tab leaves the group"
            );
        });
        assert!(
            changes.borrow().is_empty(),
            "Tab does not change the selection"
        );

        // Back in the group, Down moves from item 1, which proves Tab focused the selected radio.
        tab_into_group(&view, cx);
        cx.simulate_keystrokes("down");
        assert_eq!(*changes.borrow(), [2]);
    }

    #[gpui_kit::test]
    fn reduced_motion_dot_full_size(cx: &mut TestAppContext) {
        let (view, changes, cx) = open(0, cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        tab_into_group(&view, cx);

        cx.simulate_keystrokes("down");
        assert_eq!(*changes.borrow(), [1]);
        cx.run_until_parked();
        let dot = cx
            .debug_bounds("tl-radio-1-dot")
            .expect("item 1 draws its dot");
        cx.update(|_, cx| {
            let d = metrics(cx).sp[4];
            assert_eq!(dot.size.width, d);
            assert_eq!(dot.size.height, d);
        });
        assert!(cx.debug_bounds("tl-radio-0-dot").is_none());
    }

    #[gpui_kit::test]
    fn click_selects(cx: &mut TestAppContext) {
        let (_view, changes, cx) = open(0, cx);
        cx.run_until_parked();
        // Rows are 24 px from y = 40 with 4 px gaps: item 2 spans y 96..120.
        cx.simulate_click(gpui_kit::point(px(30.), px(108.)), Modifiers::none());
        assert_eq!(*changes.borrow(), [2]);
    }
}
