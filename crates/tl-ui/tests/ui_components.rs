//! Shared component tests in a real window (E15-A1). Later component beads append their own
//! sections below.
//!
//! Base-control keyboard suite: one window with a `TlButton`, `TlCheckbox`, `TlSwitch`,
//! `TlTextInput` and `TlTabs`. Each control sits in a probe `div` with its own key context, so a
//! test can tell which control holds focus from `Window::context_stack`.

use gpui_kit::component::input::InputState;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, Modifiers, ParentElement, Pixels, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, px,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_ui::components::button::{TlButton, TlButtonKind, TlButtonSize};
use tl_ui::components::checkbox::{TlCheckbox, bind_keys_components};
use tl_ui::components::input::{TlInputSize, TlTextInput};
use tl_ui::components::switch::TlSwitch;
use tl_ui::components::tabs::{TlTabSpec, TlTabs};
use tl_ui::theme::current;

gpui_kit::actions!(tl_components_test, [FocusNext, FocusPrev]);

const ROOT_CONTEXT: &str = "TlComponentsTest";

/// The probe contexts in tab order.
const ORDER: [&str; 5] = [
    "probe-button",
    "probe-checkbox",
    "probe-switch",
    "probe-input",
    "probe-tabs",
];

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    tl_ui::theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
    bind_keys_components(cx);
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, Some(ROOT_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrev, Some(ROOT_CONTEXT)),
    ]);
}

fn probe(context: &'static str, child: impl IntoElement) -> AnyElement {
    div()
        .key_context(context)
        .debug_selector(move || context.into())
        .flex_none()
        .child(child)
        .into_any_element()
}

fn tab_specs() -> Vec<TlTabSpec> {
    vec![TlTabSpec::new("General"), TlTabSpec::new("Threads")]
}

/// The suite window. `sizes` renders every size of each control instead of one of each.
struct Suite {
    root: FocusHandle,
    input_sm: Entity<InputState>,
    input_md: Entity<InputState>,
    sizes: bool,
}

impl Suite {
    fn controls(&self) -> Vec<AnyElement> {
        vec![
            probe(
                "probe-button",
                TlButton::new("button", TlButtonKind::Secondary).label("Run"),
            ),
            probe(
                "probe-checkbox",
                TlCheckbox::new("checkbox")
                    .checked(Some(false))
                    .label("Show kernel times"),
            ),
            probe("probe-switch", TlSwitch::new("switch").label("Animations")),
            probe(
                "probe-input",
                TlTextInput::new("input", &self.input_md).width(px(200.)),
            ),
            div()
                .w(px(400.))
                .child(probe(
                    "probe-tabs",
                    TlTabs::new("tabs")
                        .tabs(tab_specs())
                        .selected(0)
                        .overflow_tooltip("More tabs"),
                ))
                .into_any_element(),
        ]
    }

    fn sizes(&self) -> Vec<AnyElement> {
        vec![
            probe(
                "button-sm",
                TlButton::new("button-sm", TlButtonKind::Secondary)
                    .label("Small")
                    .size(TlButtonSize::Sm),
            ),
            probe(
                "button-md",
                TlButton::new("button-md", TlButtonKind::Secondary)
                    .label("Medium")
                    .size(TlButtonSize::Md),
            ),
            probe(
                "input-sm",
                TlTextInput::new("input-sm", &self.input_sm)
                    .size(TlInputSize::Sm)
                    .width(px(200.)),
            ),
            probe(
                "input-md",
                TlTextInput::new("input-md", &self.input_md)
                    .size(TlInputSize::Md)
                    .width(px(200.)),
            ),
            probe(
                "checkbox",
                TlCheckbox::new("checkbox")
                    .checked(Some(true))
                    .label("Show kernel times"),
            ),
            div()
                .w(px(400.))
                .child(probe(
                    "tabs",
                    TlTabs::new("tabs")
                        .tabs(tab_specs())
                        .selected(0)
                        .overflow_tooltip("More tabs"),
                ))
                .into_any_element(),
        ]
    }
}

impl Render for Suite {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(13.));
        let children = if self.sizes {
            self.sizes()
        } else {
            self.controls()
        };
        div()
            .id("root")
            .key_context(ROOT_CONTEXT)
            .track_focus(&self.root)
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
            .size(px(600.))
            .p(px(16.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .children(children)
    }
}

fn open(cx: &mut TestAppContext, sizes: bool) -> &mut VisualTestContext {
    cx.update(setup);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let root = cx.focus_handle();
        window.focus(&root, cx);
        Suite {
            root,
            input_sm: cx.new(|cx| InputState::new(window, cx)),
            input_md: cx.new(|cx| InputState::new(window, cx)),
            sizes,
        }
    });
    draw(cx);
    cx
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The probe that holds focus, if any.
fn focused_probe(cx: &mut VisualTestContext) -> Option<&'static str> {
    draw(cx);
    cx.update(|window, _| {
        let stack = window.context_stack();
        ORDER
            .into_iter()
            .find(|probe| stack.iter().any(|c| c.contains(probe)))
    })
}

/// Painted focus rings: 2 px `accent` borders.
fn rings(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let accent = current(cx).accent;
        let two = 2. * window.scale_factor();
        window
            .painted_quads()
            .iter()
            .filter(|q| q.border_color == accent && q.border_widths.top.0 == two)
            .count()
    })
}

fn height(cx: &mut VisualTestContext, selector: &'static str) -> Pixels {
    draw(cx);
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} drawn"))
        .size
        .height
}

#[gpui_kit::test]
fn tab_order_base_controls(cx: &mut TestAppContext) {
    let cx = open(cx, false);
    assert_eq!(focused_probe(cx), None, "the root holds focus at the start");

    for expected in ORDER {
        cx.simulate_keystrokes("tab");
        assert_eq!(focused_probe(cx), Some(expected), "Tab moves to {expected}");
    }

    for expected in ORDER.into_iter().rev().skip(1) {
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(
            focused_probe(cx),
            Some(expected),
            "Shift+Tab moves back to {expected}"
        );
    }
}

#[gpui_kit::test]
fn ring_after_keyboard_not_after_click(cx: &mut TestAppContext) {
    let cx = open(cx, false);
    let checkbox = cx
        .debug_bounds("probe-checkbox")
        .expect("the checkbox is drawn");
    cx.simulate_click(checkbox.center(), Modifiers::none());
    assert_eq!(
        focused_probe(cx),
        Some("probe-checkbox"),
        "the click focuses"
    );
    assert_eq!(rings(cx), 0, "no ring after a click");

    cx.simulate_keystrokes("tab");
    assert_eq!(focused_probe(cx), Some("probe-switch"));
    assert!(rings(cx) > 0, "a ring after Tab");
}

#[gpui_kit::test]
fn heights_at_rem_13(cx: &mut TestAppContext) {
    let cx = open(cx, true);
    for (selector, expected) in [
        ("button-sm", 24.),
        ("button-md", 28.),
        ("input-sm", 24.),
        ("input-md", 28.),
        ("checkbox", 24.),
        ("tabs", 32.),
    ] {
        assert_eq!(height(cx, selector), px(expected), "{selector}");
    }
}
