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

/// Display-component suite (E15-A2): a radio group, slider, stepper, select and link in one
/// window, validity glyph cells in a row, and Ctrl+C routing out of a stacked key/value grid.
mod display {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::base::TextSelectionLayer;
    use gpui_kit::base::slider::SliderState;
    use gpui_kit::{Bounds, SharedString};
    use tl_model::validity::Validity;
    use tl_ui::components::kv_grid::{
        self, KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue,
    };
    use tl_ui::components::link::TlLink;
    use tl_ui::components::radio::TlRadioGroup;
    use tl_ui::components::select::{self, TlSelect, TlSelectItem, TlSelectState};
    use tl_ui::components::slider::TlSlider;
    use tl_ui::components::stepper::TlStepper;
    use tl_ui::components::validity::glyph;
    use tl_ui::theme::metrics;

    use super::*;

    gpui_kit::actions!(tl_components_display_test, [OpenLink, PageCopy]);

    /// The probe contexts in tab order.
    const ORDER: [&str; 5] = [
        "probe-radio",
        "probe-slider",
        "probe-stepper",
        "probe-select",
        "probe-link",
    ];

    fn setup(cx: &mut App) {
        super::setup(cx);
        select::bind_keys(cx);
        kv_grid::init(cx);
        cx.bind_keys([KeyBinding::new("ctrl-c", PageCopy, Some(ROOT_CONTEXT))]);
    }

    /// A window whose root takes Tab, Shift+Tab and the page Copy, around `build()`.
    struct Host {
        root: FocusHandle,
        build: Box<dyn Fn(&mut Window, &mut Context<Self>) -> Vec<AnyElement>>,
        page_copies: Rc<Cell<usize>>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(13.));
            let copies = self.page_copies.clone();
            let children = (self.build)(window, cx);
            div()
                .id("root")
                .key_context(ROOT_CONTEXT)
                .track_focus(&self.root)
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
                .on_action(move |_: &PageCopy, _, _| copies.set(copies.get() + 1))
                .size(px(800.))
                .p(px(16.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(12.))
                .child(TextSelectionLayer)
                .children(children)
        }
    }

    fn open_host(
        cx: &mut TestAppContext,
        build: impl Fn(&mut Window, &mut Context<Host>) -> Vec<AnyElement> + 'static,
    ) -> (&mut VisualTestContext, Rc<Cell<usize>>) {
        cx.update(setup);
        let copies = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let copies = copies.clone();
            move |window, cx| {
                let root = cx.focus_handle();
                window.focus(&root, cx);
                Host {
                    root,
                    build: Box::new(build),
                    page_copies: copies,
                }
            }
        });
        draw(cx);
        (cx, copies)
    }

    fn controls_host(cx: &mut TestAppContext) -> &mut VisualTestContext {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(75.)
                .max(200.)
                .step(5.)
                .default_value(100.)
        });
        let window_state: Rc<Cell<Option<(Entity<InputState>, Entity<TlSelectState>)>>> =
            Rc::default();
        open_host(cx, move |window, cx| {
            let (stepper, select) = match window_state.take() {
                Some(states) => states,
                None => (
                    cx.new(|cx| InputState::new(window, cx)),
                    cx.new(|cx| {
                        TlSelectState::new(
                            vec![
                                TlSelectItem::new("bits", "Bits"),
                                TlSelectItem::new("bytes", "Bytes"),
                            ],
                            Some("bits"),
                            cx,
                        )
                    }),
                ),
            };
            window_state.set(Some((stepper.clone(), select.clone())));
            vec![
                probe(
                    "probe-radio",
                    TlRadioGroup::new("radio")
                        .items(vec!["Off".into(), "On".into(), "Auto".into()])
                        .selected(1),
                ),
                probe("probe-slider", TlSlider::new(&slider).label("UI Scale")),
                probe(
                    "probe-stepper",
                    TlStepper::new("stepper", &stepper)
                        .range(100, 10_000, 100)
                        .value(1000),
                ),
                probe("probe-select", TlSelect::new("select", &select)),
                probe(
                    "probe-link",
                    TlLink::new("link", "Network settings", Box::new(OpenLink)),
                ),
            ]
        })
        .0
    }

    /// The probe that holds focus, if any.
    fn focused(cx: &mut VisualTestContext) -> Option<&'static str> {
        draw(cx);
        cx.update(|window, _| {
            let stack = window.context_stack();
            ORDER
                .into_iter()
                .find(|probe| stack.iter().any(|c| c.contains(probe)))
        })
    }

    #[gpui_kit::test]
    fn tab_order_display_controls(cx: &mut TestAppContext) {
        let cx = controls_host(cx);
        assert_eq!(focused(cx), None, "the root holds focus at the start");

        for expected in ORDER {
            cx.simulate_keystrokes("tab");
            assert_eq!(focused(cx), Some(expected), "Tab moves to {expected}");
        }

        for expected in ORDER.into_iter().rev().skip(1) {
            cx.simulate_keystrokes("shift-tab");
            assert_eq!(
                focused(cx),
                Some(expected),
                "Shift+Tab moves back to {expected}"
            );
        }
    }

    const ALL: [Validity; 7] = [
        Validity::Valid,
        Validity::WarmingUp,
        Validity::Stale,
        Validity::Restricted,
        Validity::Unsupported,
        Validity::Missing,
        Validity::Failed,
    ];
    const CELLS: [&str; 7] = [
        "cell-0", "cell-1", "cell-2", "cell-3", "cell-4", "cell-5", "cell-6",
    ];
    const CELL_WIDTH: f32 = 72.;

    #[gpui_kit::test]
    fn glyph_cells_align(cx: &mut TestAppContext) {
        let shift = Rc::new(Cell::new(0usize));
        let (cx, _) = open_host(cx, {
            let shift = shift.clone();
            move |_, cx| {
                let m = *metrics(cx);
                let theme = tl_ui::theme::current(cx).clone();
                let value: SharedString = "12.5%".into();
                let cells = (0..7).map(|i| {
                    let validity = ALL[(i + shift.get()) % 7];
                    div()
                        .debug_selector(move || CELLS[i].into())
                        .flex_none()
                        .w(px(CELL_WIDTH))
                        .child(glyph(validity, &value, false, &m, &theme))
                });
                vec![div().flex().flex_row().children(cells).into_any_element()]
            }
        });

        // Bounds of a selector drawn more than once are the last one's, so each state in turn
        // goes into the last cell and is measured there.
        for (k, &validity) in ALL.iter().enumerate() {
            shift.set(k + 1);
            cx.update(|window, _| window.refresh());
            draw(cx);

            let cells: Vec<Bounds<Pixels>> = CELLS
                .iter()
                .map(|c| cx.debug_bounds(c).expect("cell drawn"))
                .collect();
            for (i, cell) in cells.iter().enumerate() {
                assert_eq!(
                    cell.size.width,
                    px(CELL_WIDTH),
                    "{validity:?}: cell {i} width"
                );
                assert_eq!(
                    cell.left(),
                    cells[0].left() + px(CELL_WIDTH * i as f32),
                    "{validity:?}: cell {i} position"
                );
            }

            let last = cells[6];
            let glyph = cx.debug_bounds("TlValidityGlyph").expect("glyph drawn");
            assert_eq!(
                (glyph.left(), glyph.right()),
                (last.left(), last.right()),
                "{validity:?}: the glyph fills its cell"
            );
            let inner = if validity == Validity::Restricted {
                "TlValidityGlyph-lock"
            } else {
                "TlValidityGlyph-text"
            };
            let inner = cx.debug_bounds(inner).expect("glyph content drawn");
            assert_eq!(
                inner.right(),
                last.right(),
                "{validity:?}: the content sits at the right edge"
            );
        }
    }

    #[gpui_kit::test]
    fn stacked_grid_ctrl_c_propagates_without_selection(cx: &mut TestAppContext) {
        let (cx, page_copies) = open_host(cx, |_, _| {
            let row = |label: &str, value: &str| TlKvRow {
                label: label.to_owned().into(),
                value: TlValue::new(value.to_owned()),
            };
            let section = TlKvSection {
                title: Some("Memory".into()),
                blocks: vec![TlKvBlock {
                    heading: None,
                    rows: vec![row("In use", "4.2 GiB"), row("Available", "11.6 GiB")],
                }],
            };
            vec![probe(
                "probe-grid",
                div()
                    .w(px(600.))
                    .child(TlKeyValueGrid::new("grid", KvLayout::Stacked).sections(&[section])),
            )]
        });

        let grid = cx.debug_bounds("probe-grid").expect("grid drawn");
        cx.simulate_click(grid.center(), Modifiers::none());
        draw(cx);
        let in_grid = cx.update(|window, _| {
            window
                .context_stack()
                .iter()
                .any(|c| c.contains(kv_grid::KEY_CONTEXT))
        });
        assert!(in_grid, "the click focuses the grid");

        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(page_copies.get(), 1, "Ctrl+C goes on to the page's Copy");
        assert!(
            cx.read_from_clipboard().is_none(),
            "the grid copied nothing"
        );
    }
}
