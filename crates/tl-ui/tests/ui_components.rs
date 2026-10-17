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

/// A button, a checkbox with `tab_stop(false)` and a switch. Counts the checkbox changes.
struct NoTabStop {
    root: FocusHandle,
    changes: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Render for NoTabStop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let changes = self.changes.clone();
        div()
            .id("root")
            .key_context(ROOT_CONTEXT)
            .track_focus(&self.root)
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .size(px(600.))
            .p(px(16.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(probe(
                "probe-button",
                TlButton::new("button", TlButtonKind::Secondary).label("Run"),
            ))
            .child(probe(
                "probe-checkbox",
                TlCheckbox::new("checkbox")
                    .label("Show kernel times")
                    .tab_stop(false)
                    .on_change(move |_, _, _| changes.set(changes.get() + 1)),
            ))
            .child(probe("probe-switch", TlSwitch::new("switch").label("On")))
    }
}

#[gpui_kit::test]
fn checkbox_tab_stop_false_skipped_and_click_not_focused(cx: &mut TestAppContext) {
    cx.update(setup);
    let changes = std::rc::Rc::new(std::cell::Cell::new(0));
    let c = changes.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let root = cx.focus_handle();
        window.focus(&root, cx);
        NoTabStop { root, changes: c }
    });
    draw(cx);

    cx.simulate_keystrokes("tab");
    assert_eq!(focused_probe(cx), Some("probe-button"));
    cx.simulate_keystrokes("tab");
    assert_eq!(
        focused_probe(cx),
        Some("probe-switch"),
        "Tab skips the checkbox"
    );

    let checkbox = cx
        .debug_bounds("probe-checkbox")
        .expect("the checkbox is drawn");
    cx.simulate_click(checkbox.center(), Modifiers::none());
    assert_eq!(changes.get(), 1, "the click still toggles");
    let root = cx.update(|window, cx| view.read(cx).root.is_focused(window));
    assert!(root, "the click focuses the container, not the checkbox");
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

    type Build = Box<dyn Fn(&mut Window, &mut Context<Host>) -> Vec<AnyElement>>;
    /// The stepper and select states, created on the first render.
    type States = Rc<Cell<Option<(Entity<InputState>, Entity<TlSelectState>)>>>;

    /// A window whose root takes Tab, Shift+Tab and the page Copy, around `build()`.
    struct Host {
        root: FocusHandle,
        build: Build,
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
        let window_state: States = Rc::default();
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
                    indent: None,
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

/// The key/value grid state words and the CPU Details cache and boot time words come from the
/// catalogue, translated once per strings version (R-132).
#[cfg(feature = "test-hooks")]
mod words {
    use gpui_kit::{MouseButton, SharedString, point};
    use tl_model::snapshot::{CacheInfo, CacheLevelInfo, CpuStaticInfo, SystemSnapshot};
    use tl_model::validity::Validity;
    use tl_ui::components::kv_grid::{
        KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord,
        kv_words, shown_text_in,
    };
    use tl_ui::components::tooltip::DELAY_CONTROL;
    use tl_ui::i18n::{init_strings, set_test_catalogue, tr_calls};
    use tl_ui::pages::performance::cpu_details::{
        CpuLiveInputs, ROW_L1, ROW_L3, ROW_UP_TIME, cpu_details_model, update_live, update_static,
    };
    use tl_ui::pages::performance::details::KeyValueModel;

    use super::*;

    const MARKERS: &str = "
fnd-kv-collecting = <collecting>
fnd-kv-restricted = <restricted>
fnd-kv-unavailable = <unavailable>
fnd-kv-not-supported = <not-supported>
fnd-kv-not-reported = <not-reported>
fnd-kv-needs-setup = <needs-setup>
fnd-kv-no-match = <no-match>
fnd-kv-copy-value = <copy-value>
fnd-kv-copy-row = <copy-row>
fnd-kv-copy-section = <copy-section-in-a-test-locale>
perf-cpu-tip-boot = <boot {$time}>
perf-cpu-tip-l1-data = <data {$size}>
perf-cpu-tip-l1-instruction = <instruction {$size}>
perf-cpu-tip-l4 = <l4 {$size}>
perf-cpu-time-unknown = <unknown>
";

    /// One value per non-valid state, in this order.
    fn state_values() -> Vec<TlValue> {
        let state = |validity, word| TlValue {
            validity,
            word,
            ..TlValue::new("0")
        };
        vec![
            state(Validity::WarmingUp, None),
            state(Validity::Restricted, None),
            state(Validity::Failed, None),
            state(Validity::Unsupported, None),
            state(Validity::Unsupported, Some(UnsupportedWord::NotReported)),
            state(Validity::Unsupported, Some(UnsupportedWord::NeedsSetup)),
        ]
    }

    const SHOWN: [&str; 6] = [
        "<collecting>",
        "<restricted>",
        "<unavailable>",
        "<not-supported>",
        "<not-reported>",
        "<needs-setup>",
    ];

    struct Grid;

    impl Render for Grid {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = state_values()
                .into_iter()
                .enumerate()
                .map(|(i, value)| TlKvRow {
                    label: format!("Row {i}:").into(),
                    value,
                })
                .collect();
            let section = TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    indent: None,
                    heading: None,
                    rows,
                }],
            };
            div().size(px(800.)).child(
                TlKeyValueGrid::new("grid", KvLayout::Inline)
                    .copy_on_click(true)
                    .sections(&[section]),
            )
        }
    }

    #[gpui_kit::test]
    fn kv_grid_words_follow_test_locale(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            init_strings(cx, "en-US");
            assert_eq!(kv_words(cx).restricted, "Restricted");
            set_test_catalogue(cx, MARKERS);
            let before = tr_calls();
            let words = kv_words(cx);
            assert!(tr_calls() > before, "a new strings version translates");
            let shown: Vec<SharedString> = state_values()
                .iter()
                .filter_map(|v| shown_text_in(v, &words))
                .collect();
            assert_eq!(shown, SHOWN);
        });

        let (_, cx) = cx.add_window_view(|_, _| Grid);
        draw(cx);
        let restricted = cx.debug_bounds("TlKv-value-1").expect("value drawn");
        cx.simulate_click(restricted.center(), Modifiers::none());
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("<restricted>"),
            "the grid draws the translated word"
        );

        let before = tr_calls();
        for _ in 0..60 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        assert_eq!(tr_calls(), before, "60 frames make no tr call");
    }

    /// A counted text has a form per plural category, and writing it makes no tr call
    /// (o55.37).
    #[gpui_kit::test]
    fn counted_template_plural_forms(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::details::Template;

        const KEY: &str = "perf-gpu-enc-sessions";
        fn written(t: &Template, n: u64) -> String {
            let mut out = String::new();
            t.write_count(&mut out, n, &[&n])
                .expect("a String takes the text");
            out
        }
        cx.update(|cx| {
            setup(cx);
            init_strings(cx, "en-US");
            let t = Template::counted(cx, KEY, &["n"], 0);
            assert_eq!(t, Template::counted_en(KEY, &["n"], 0));
            let before = tr_calls();
            assert_eq!(written(&t, 0), "0 active sessions");
            assert_eq!(written(&t, 1), "1 active session");
            assert_eq!(written(&t, 2), "2 active sessions");
            assert_eq!(written(&t, 21), "21 active sessions");
            assert_eq!(tr_calls(), before, "writes make no tr call");

            // The sample 0 shows twice in the other form, so the next sample marks the count.
            set_test_catalogue(
                cx,
                "perf-gpu-enc-sessions = { $n ->\n    [one] <one {$n}>\n   *[other] <{$n} of 0>\n}\n",
            );
            let t = Template::counted(cx, KEY, &["n"], 0);
            assert_eq!(written(&t, 1), "<one 1>");
            assert_eq!(written(&t, 5), "<5 of 0>");
        });
    }

    /// An empty filtered grid over a grid with a titled section.
    struct OwnText;

    impl Render for OwnText {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let section = TlKvSection {
                title: Some("Memory".into()),
                blocks: vec![TlKvBlock {
                    indent: None,
                    heading: None,
                    rows: vec![TlKvRow {
                        label: "Total:".into(),
                        value: TlValue::new("32 GiB"),
                    }],
                }],
            };
            div()
                .w(px(800.))
                .child(
                    div()
                        .h(px(100.))
                        .child(TlKeyValueGrid::new("empty", KvLayout::Inline).filtered_empty(true)),
                )
                .child(TlKeyValueGrid::new("band", KvLayout::Inline).sections(&[section]))
        }
    }

    fn width(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
        f32::from(cx.debug_bounds(selector).expect(selector).size.width)
    }

    /// The test text system gives every char the same advance, so a text `width` wide is `text`
    /// when it is `text`'s length in chars of the en-US text, up to a pixel of rounding.
    fn assert_drawn(width: f32, en_width: f32, en: &str, text: &str) {
        let expected = en_width / en.chars().count() as f32 * text.chars().count() as f32;
        assert!(
            (width - expected).abs() <= 1.,
            "{text} drawn: {width} px, expected {expected} px"
        );
    }

    fn tooltip_width(cx: &mut VisualTestContext) -> f32 {
        let icon = cx
            .debug_bounds("TlButton-band-copy-section0-icon")
            .expect("copy section button");
        cx.simulate_mouse_move(point(px(1.), px(1.)), None, Modifiers::none());
        draw(cx);
        cx.simulate_mouse_move(icon.center(), None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        cx.run_until_parked();
        draw(cx);
        draw(cx);
        width(cx, "TlTooltip-text")
    }

    #[gpui_kit::test]
    fn kv_grid_own_text_follows_test_locale(cx: &mut TestAppContext) {
        const NO_MATCH: &str = "No properties match the current filter.";
        cx.update(|cx| {
            setup(cx);
            init_strings(cx, "en-US");
            let words = kv_words(cx);
            assert_eq!(words.no_match, NO_MATCH);
            assert_eq!(
                [&words.copy_value, &words.copy_row, &words.copy_section],
                ["Copy value", "Copy row", "Copy section"]
            );
        });
        let (_, cx) = cx.add_window_view(|_, _| OwnText);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        draw(cx);
        let empty_en = width(cx, "TlKv-empty");
        let tip_en = tooltip_width(cx);

        cx.update(|window, cx| {
            window.refresh();
            set_test_catalogue(cx, MARKERS);
            let words = kv_words(cx);
            assert_eq!(
                [
                    &words.no_match,
                    &words.copy_value,
                    &words.copy_row,
                    &words.copy_section
                ],
                [
                    "<no-match>",
                    "<copy-value>",
                    "<copy-row>",
                    "<copy-section-in-a-test-locale>"
                ]
            );
        });
        draw(cx);
        let empty = width(cx, "TlKv-empty");
        assert_drawn(empty, empty_en, NO_MATCH, "<no-match>");
        let tip = tooltip_width(cx);
        assert_drawn(
            tip,
            tip_en,
            "Copy section",
            "<copy-section-in-a-test-locale>",
        );

        // Frames with the context menu open read the cached words.
        let value = cx.debug_bounds("TlKv-value-0").expect("value drawn");
        cx.simulate_mouse_down(value.center(), MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(value.center(), MouseButton::Right, Modifiers::none());
        draw(cx);
        let before = tr_calls();
        for _ in 0..60 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        assert_eq!(tr_calls(), before, "60 frames make no tr call");
        // Items: copy value, copy row, copy section.
        cx.simulate_keystrokes("down down down enter");
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("Memory\n    Total: 32 GiB"),
            "the menu's copy section item"
        );
    }

    fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
        Some(CacheLevelInfo {
            instances,
            instance_kib,
            total_kib: u64::from(instances) * instance_kib,
        })
    }

    fn tip(model: &KeyValueModel, label: &str) -> Option<String> {
        let row = model.rows.iter().find(|r| r.label == label)?;
        row.tooltip.as_ref().map(|t| t.to_string())
    }

    #[gpui_kit::test]
    fn cpu_cache_and_boot_words_follow_test_locale(cx: &mut TestAppContext) {
        let mut model = cpu_details_model();
        cx.update(|cx| {
            init_strings(cx, "en-US");
            set_test_catalogue(cx, MARKERS);
            assert!(model.refresh_strings(cx));
        });
        let info = CpuStaticInfo {
            cores: 8,
            logical: 16,
            caches: CacheInfo {
                l1d: level(16, 32),
                l1i: level(16, 32),
                l3: level(1, 98_304),
                l4: level(1, 131_072),
                ..CacheInfo::default()
            },
            ..CpuStaticInfo::default()
        };
        let before = tr_calls();
        assert!(update_static(&mut model, &info));
        assert_eq!(
            tip(&model, ROW_L1).as_deref(),
            Some("<data 16 × 32 KiB> + <instruction 16 × 32 KiB>")
        );
        assert_eq!(
            tip(&model, ROW_L3).as_deref(),
            Some("98\u{2009}304 KiB · 1 × 96 MiB · <l4 128 MiB>")
        );

        // A boot time past what a date holds.
        let sys = SystemSnapshot {
            boot_time_s: i64::MAX,
            ..SystemSnapshot::default()
        };
        update_live(&mut model, &sys, &CpuLiveInputs::default());
        assert_eq!(
            tip(&model, ROW_UP_TIME).as_deref(),
            Some("<boot <unknown>>")
        );

        for _ in 0..60 {
            update_static(&mut model, &info);
            update_live(&mut model, &sys, &CpuLiveInputs::default());
        }
        assert_eq!(tr_calls(), before, "updates make no tr call");
    }
}

/// `TlSecondaryWindow` controls in the Tab order (UI-FND-021).
mod frame {
    use gpui_kit::base::Root;
    use gpui_kit::{
        AppContext as _, Context, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        WindowHandle, div, px, size,
    };
    use tl_ui::components::button::{TlButton, TlButtonKind};
    use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};

    use super::{draw, probe, rings, setup};

    /// One button in a probe.
    struct OneButton;

    impl Render for OneButton {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p(px(16.)).child(probe(
                "probe-button",
                TlButton::new("button", TlButtonKind::Secondary).label("Run"),
            ))
        }
    }

    fn open(cx: &mut TestAppContext) -> (WindowHandle<Root>, VisualTestContext) {
        cx.update(setup);
        let spec = TlWindowSpec::new(
            TlWindowKind::Tool { resizable: true },
            "tasklens Settings",
            size(px(600.), px(400.)),
        );
        let handle = cx
            .update(|cx| TlSecondaryWindow::open(cx, spec, |_, cx| cx.new(|_| OneButton)))
            .expect("window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        draw(&mut vcx);
        (handle, vcx)
    }

    fn on_button(vcx: &mut VisualTestContext) -> bool {
        draw(vcx);
        vcx.update(|window, _| {
            window
                .context_stack()
                .iter()
                .any(|c| c.contains("probe-button"))
        })
    }

    fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
        let id = handle.window_id();
        cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
    }

    /// Without keep-above the pin is disabled and not a tab stop; close still is, after the
    /// content.
    #[gpui_kit::test]
    fn close_after_content_in_tab_order(cx: &mut TestAppContext) {
        let (handle, mut vcx) = open(cx);
        vcx.simulate_keystrokes("tab");
        assert!(on_button(&mut vcx), "Tab starts in the content");
        assert!(rings(&mut vcx) > 0, "the button's ring");

        vcx.simulate_keystrokes("tab");
        assert!(!on_button(&mut vcx), "Tab moves to close");
        assert!(
            vcx.update(|window, cx| window.focused(cx).is_some()),
            "close holds focus"
        );
        assert!(rings(&mut vcx) > 0, "the close button's ring");

        vcx.simulate_keystrokes("tab");
        assert!(on_button(&mut vcx), "Tab wraps to the content");

        vcx.simulate_keystrokes("shift-tab");
        assert!(!on_button(&mut vcx), "Shift+Tab moves back to close");
        // A focused button activates on the key up.
        let keystroke = Keystroke::parse("space").expect("keystroke");
        vcx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        vcx.simulate_event(KeyUpEvent { keystroke });
        vcx.run_until_parked();
        assert!(!is_open(cx, handle), "Space on close closes the window");
    }
}

/// Enter in a `TlDialog` window runs one result (R-104, tasklens-ug9.35). The keys go through the
/// window's real dispatch as a key down and a key up, because a focused button clicks on the key
/// up and `simulate_keystrokes` sends only the key down.
mod dialog_enter {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::base::Root;
    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::component::input::InputState;
    use gpui_kit::{
        AnyWindowHandle, Context, Entity, Focusable as _, IntoElement, KeyDownEvent, KeyUpEvent,
        Keystroke, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualContext as _, VisualTestContext, Window, WindowHandle, div,
    };
    use tl_ui::components::dialog::{DialogButton, DialogResult, TlDialog, TlDialogView};
    use tl_ui::components::input::TlTextInput;
    use tl_ui::components::window_frame::TlSecondaryWindow;

    use super::{draw, setup};

    struct Parent;

    impl Render for Parent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("parent")
        }
    }

    type Results = Rc<RefCell<Vec<DialogResult>>>;

    struct Opened {
        handle: WindowHandle<Root>,
        view: Entity<TlDialogView>,
        vcx: VisualTestContext,
        results: Results,
    }

    /// Opens `dialog` modal to a new parent window and returns its window.
    fn open(cx: &mut TestAppContext, dialog: TlDialog) -> Opened {
        cx.update(setup);
        cx.update(tl_ui::components::window_frame::bind_keys);
        let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
        let results: Results = Rc::default();
        let sink = results.clone();
        cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    dialog.open(window, cx, move |result, _, _| {
                        sink.borrow_mut().push(result)
                    })
                })
                .expect("parent")
                .expect("dialog opens")
        });
        cx.run_until_parked();
        let handle = cx
            .update(|cx| {
                cx.windows()
                    .into_iter()
                    .find(|w| w.window_id() != parent.window_id())
            })
            .and_then(|w| w.downcast::<Root>())
            .expect("dialog window");
        let view = cx
            .update(|cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))
            .map(|frame| cx.read(|cx| frame.read(cx).content().clone()))
            .expect("dialog view");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        draw(&mut vcx);
        Opened {
            handle,
            view,
            vcx,
            results,
        }
    }

    /// One Enter press as the platform sends it: key down, then key up.
    fn press_enter(vcx: &mut VisualTestContext) {
        let keystroke = Keystroke::parse("enter").expect("keystroke");
        vcx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        // A key down that closed the window leaves no window for the key up.
        let id = vcx.window_handle().window_id();
        if vcx.read(|cx| cx.windows().iter().any(|w| w.window_id() == id)) {
            vcx.simulate_event(KeyUpEvent { keystroke });
        }
        vcx.run_until_parked();
    }

    fn focused(o: &mut Opened) -> Option<DialogButton> {
        let view = o.view.clone();
        o.vcx
            .update(|window, cx| view.read(cx).focused_button(window))
    }

    fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
        let id = handle.window_id();
        cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
    }

    #[gpui_kit::test]
    fn enter_on_focused_cancel_cancels_once(cx: &mut TestAppContext) {
        let mut o = open(cx, TlDialog::confirm("Delete this custom theme?", "Delete"));
        assert_eq!(
            focused(&mut o),
            Some(DialogButton::Action),
            "focus starts on Delete"
        );
        o.vcx.simulate_keystrokes("shift-tab");
        draw(&mut o.vcx);
        assert_eq!(focused(&mut o), Some(DialogButton::Cancel));

        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::Cancelled],
            "Cancel once"
        );
        assert!(!is_open(cx, o.handle));
    }

    #[gpui_kit::test]
    fn enter_on_focused_action_confirms_once(cx: &mut TestAppContext) {
        let mut o = open(cx, TlDialog::confirm("Delete this custom theme?", "Delete"));
        assert_eq!(focused(&mut o), Some(DialogButton::Action));
        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::Confirmed { checkbox: false }]
        );
    }

    #[gpui_kit::test]
    fn enter_in_text_field_runs_default_once(cx: &mut TestAppContext) {
        let field: Rc<RefCell<Option<Entity<InputState>>>> = Rc::default();
        let slot = field.clone();
        let dialog = TlDialog::confirm("Rename this theme?", "Rename").extra(move |window, cx| {
            let state = window.use_keyed_state("dialog-field", cx, InputState::new);
            *slot.borrow_mut() = Some(state.clone());
            div()
                .w_full()
                .child(TlTextInput::new("dialog-field", &state))
                .into_any_element()
        });
        let mut o = open(cx, dialog);
        let state = field.borrow().clone().expect("field drawn");
        o.vcx
            .update(|window, cx| state.update(cx, |s, cx| s.focus(window, cx)));
        draw(&mut o.vcx);
        assert_eq!(focused(&mut o), None, "focus in the field");

        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::Confirmed { checkbox: false }],
            "the default action once"
        );
    }

    #[gpui_kit::test]
    fn enter_in_text_field_of_destructive_cancels(cx: &mut TestAppContext) {
        let field: Rc<RefCell<Option<Entity<InputState>>>> = Rc::default();
        let slot = field.clone();
        let dialog = TlDialog::confirm("Reset all settings?", "Reset")
            .destructive()
            .extra(move |window, cx| {
                let state = window.use_keyed_state("dialog-field", cx, InputState::new);
                *slot.borrow_mut() = Some(state.clone());
                TlTextInput::new("dialog-field", &state).into_any_element()
            });
        let mut o = open(cx, dialog);
        let state = field.borrow().clone().expect("field drawn");
        o.vcx
            .update(|window, cx| state.update(cx, |s, cx| s.focus(window, cx)));
        draw(&mut o.vcx);
        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::Cancelled],
            "a stray Enter cancels"
        );
    }

    #[gpui_kit::test]
    fn enter_on_frame_close_cancels_once(cx: &mut TestAppContext) {
        let mut o = open(cx, TlDialog::confirm("Delete this custom theme?", "Delete"));
        // Delete is the last footer button; close comes after the content in the Tab order.
        o.vcx.simulate_keystrokes("tab");
        draw(&mut o.vcx);
        assert_eq!(focused(&mut o), None, "focus on the frame's close button");
        assert!(
            o.vcx.update(|window, cx| window.focused(cx).is_some()),
            "close holds focus"
        );

        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::Cancelled],
            "Cancel once"
        );
        assert!(!is_open(cx, o.handle));
    }

    // Text-input mode (tasklens-o55.41).

    fn rename_fan() -> TlDialog {
        TlDialog::confirm("Name for this fan:", "Rename")
            .title("Rename fan")
            .text_input("Fan 1", "Fan name")
    }

    fn field(o: &Opened) -> Entity<InputState> {
        o.vcx
            .read(|cx| o.view.read(cx).dialog().input_state().cloned())
            .expect("the dialog has a field")
    }

    /// The accessible name drawn inside the dialog's field.
    fn field_name(o: &mut Opened) -> Option<String> {
        let field = o
            .vcx
            .debug_bounds("TlTextInput-TlDialogInput")
            .expect("field drawn");
        o.vcx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .filter(|s| field.contains(&s.bounds().center()))
                .find_map(|s| s.label().map(str::to_owned))
        })
    }

    #[gpui_kit::test]
    fn text_input_starts_focused_with_text_selected(cx: &mut TestAppContext) {
        let mut o = open(cx, rename_fan());
        let state = field(&o);
        assert_eq!(focused(&mut o), None, "no footer button has focus");
        let (in_field, selected) = o.vcx.update(|window, cx| {
            let s = state.read(cx);
            (
                s.focus_handle(cx).is_focused(window),
                s.selected_text().to_string(),
            )
        });
        assert!(in_field, "focus starts in the field");
        assert_eq!(selected, "Fan 1", "the initial text is selected");
        assert_eq!(field_name(&mut o).as_deref(), Some("Rename fan"));

        let mut o = open(cx, rename_fan().input_label("Fan name"));
        assert_eq!(field_name(&mut o).as_deref(), Some("Fan name"));
    }

    #[gpui_kit::test]
    fn text_input_enter_returns_typed_text(cx: &mut TestAppContext) {
        let mut o = open(cx, rename_fan());
        // Typing replaces the selected initial text.
        o.vcx.simulate_input("Front intake");
        draw(&mut o.vcx);
        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::ConfirmedText("Front intake".into())],
            "Enter in the field confirms once with its text"
        );
        assert!(!is_open(cx, o.handle));
    }

    #[gpui_kit::test]
    fn text_input_enter_confirms_in_destructive_dialog(cx: &mut TestAppContext) {
        let mut o = open(cx, rename_fan().destructive());
        press_enter(&mut o.vcx);
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::ConfirmedText("Fan 1".into())]
        );
    }

    #[gpui_kit::test]
    fn text_input_escape_cancels(cx: &mut TestAppContext) {
        let mut o = open(cx, rename_fan());
        o.vcx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(*o.results.borrow(), [DialogResult::Cancelled]);
        assert!(!is_open(cx, o.handle), "Esc closes the window");
    }

    #[gpui_kit::test]
    fn text_input_action_button_returns_text(cx: &mut TestAppContext) {
        let mut o = open(cx, rename_fan());
        // Backspace deletes the selected initial text.
        o.vcx.simulate_keystrokes("backspace");
        draw(&mut o.vcx);
        let action = o.vcx.debug_bounds("TlDialogAction").expect("Rename");
        o.vcx.simulate_click(action.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            *o.results.borrow(),
            [DialogResult::ConfirmedText(String::new())],
            "an emptied field confirms with empty text"
        );
    }
}

/// TlMenu value slot (tasklens-o55.41).
mod menu_value {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
        Render, Role, Styled as _, TestAppContext, Window, div, point, px,
    };
    use tl_ui::components::menu::{TlMenu, context_menu_layer, open_context_menu_at};

    use super::{draw, setup};

    gpui_kit::actions!(tl_menu_value_test, [PickSensor, PickNone]);

    struct Host {
        focus: FocusHandle,
        ran: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let ran = self.ran.clone();
            let ran_none = self.ran.clone();
            div()
                .size(px(800.))
                .track_focus(&self.focus)
                .on_action(move |_: &PickSensor, _, _| ran.borrow_mut().push("sensor"))
                .on_action(move |_: &PickNone, _, _| ran_none.borrow_mut().push("none"))
                .children(context_menu_layer(window, cx))
        }
    }

    #[gpui_kit::test]
    fn value_is_drawn_right_and_named(cx: &mut TestAppContext) {
        cx.update(setup);
        let (view, cx) = cx.add_window_view(|_, cx| Host {
            focus: cx.focus_handle(),
            ran: Rc::default(),
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        let focus = cx.update(|_, cx| view.read(cx).focus.clone());
        cx.update(|window, cx| {
            focus.focus(window, cx);
            let menu = TlMenu::build(window, cx, focus, |menu, _, _| {
                menu.radio_item("nct6797 CPUTIN", true, Box::new(PickSensor))
                    .value("48 °C")
                    .radio_item("None", false, Box::new(PickNone))
            });
            open_context_menu_at(point(px(20.), px(20.)), window, cx, menu);
        });
        cx.run_until_parked();
        draw(cx);

        let items: Vec<_> = cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .filter(|s| s.role() == Some(Role::MenuItem))
                .collect()
        });
        let names: Vec<_> = items.iter().filter_map(|s| s.label()).collect();
        assert_eq!(
            names,
            ["nct6797 CPUTIN 48 °C", "None"],
            "the value follows the label in the accessible name"
        );
        assert!(
            cx.debug_bounds("TlMenuValue-1").is_none(),
            "no value, no slot"
        );

        let row = items[0].bounds();
        let value = cx.debug_bounds("TlMenuValue-0").expect("value drawn");
        assert!(row.contains(&value.center()), "the value sits in its row");
        assert!(
            value.left() > row.center().x,
            "the value is right-aligned: {value:?} in {row:?}"
        );
        assert!(
            row.right() - value.right() <= px(12.),
            "only the row padding is right of the value"
        );

        // The value does not change the action.
        cx.simulate_click(row.center(), Modifiers::none());
        cx.run_until_parked();
        let ran = cx.update(|_, cx| view.read(cx).ran.borrow().clone());
        assert_eq!(ran, ["sensor"]);
    }
}

/// TlButton accessible name and icon hover, TlKeyValueGrid label tooltips (tasklens-9o0.34).
mod names_and_label_tips {
    use std::rc::Rc;
    use std::time::Duration;

    use gpui_kit::{SharedString, point};
    use tl_ui::components::icons::TlIcon;
    use tl_ui::components::kv_grid::{
        KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue,
    };
    use tl_ui::components::tooltip::DELAY_CONTROL;

    use super::*;

    struct Grid {
        tips: Rc<[(usize, SharedString)]>,
    }

    impl Render for Grid {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let row = |label: &str, value: &str| TlKvRow {
                label: label.to_owned().into(),
                value: TlValue::new(value.to_owned()),
            };
            let section = |rows| TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    indent: None,
                    heading: None,
                    rows,
                }],
            };
            let sections = [
                section(vec![row("Threads:", "5 596")]),
                section(vec![row("Handles:", "253 248")]),
            ];
            div().size(px(600.)).child(
                TlKeyValueGrid::new("grid", KvLayout::Inline)
                    .sections(&sections)
                    .label_tooltips(self.tips.clone()),
            )
        }
    }

    fn hover_for(cx: &mut VisualTestContext, at: gpui_kit::Point<Pixels>, wait: Duration) -> bool {
        cx.simulate_mouse_move(point(px(590.), px(590.)), None, Modifiers::none());
        draw(cx);
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.executor().advance_clock(wait);
        cx.run_until_parked();
        draw(cx);
        draw(cx);
        cx.debug_bounds("TlTooltip-text").is_some()
    }

    #[gpui_kit::test]
    fn kv_grid_label_tooltip_by_flat_row(cx: &mut TestAppContext) {
        cx.update(setup);
        let tips: Rc<[(usize, SharedString)]> = Rc::from(vec![(
            1,
            SharedString::from("Open file descriptors, system-wide"),
        )]);
        let (_, cx) = cx.add_window_view(move |_, _| Grid { tips: tips.clone() });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        assert!(
            cx.debug_bounds("TlKv-label-0").is_none(),
            "row 0 has no label tooltip"
        );
        let label = cx
            .debug_bounds("TlKv-label-1")
            .expect("row 1 of the second section is flat row 1");
        assert!(
            !hover_for(
                cx,
                label.center(),
                DELAY_CONTROL - Duration::from_millis(50)
            ),
            "not before the control delay"
        );
        assert!(
            hover_for(cx, label.center(), DELAY_CONTROL),
            "shown after the control delay"
        );
        let value = cx.debug_bounds("TlKv-value-1").expect("value drawn");
        assert!(
            !hover_for(cx, value.center(), DELAY_CONTROL),
            "the value has no tooltip of its own"
        );
    }

    struct CopyHost {
        name: Option<&'static str>,
    }

    impl Render for CopyHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut button = TlButton::new("copy", TlButtonKind::Icon)
                .icon(TlIcon::Copy)
                .icon_muted(true)
                .icon_hover(true)
                .size(TlButtonSize::Sm)
                .tooltip("Copy section");
            if let Some(name) = self.name {
                button = button.accessible_name(name);
            }
            div().size(px(200.)).child(button)
        }
    }

    #[gpui_kit::test]
    fn button_accessible_name_separate_from_tooltip(cx: &mut TestAppContext) {
        cx.update(setup);
        let theme = cx.update(|cx| current(cx).clone());
        let button = TlButton::new("copy", TlButtonKind::Icon)
            .icon(TlIcon::Copy)
            .icon_muted(true)
            .icon_hover(true)
            .tooltip("Copy section");
        assert_eq!(
            button.accessible_text().map(|s| s.as_ref()),
            Some("Copy section")
        );
        assert_eq!(button.icon_color(&theme), theme.text_secondary);
        assert_eq!(button.icon_hover_color(&theme), Some(theme.text_primary));
        let button = button.accessible_name("Copy Processor");
        assert_eq!(
            button.accessible_text().map(|s| s.as_ref()),
            Some("Copy Processor")
        );

        let (_, cx) = cx.add_window_view(|_, _| CopyHost {
            name: Some("Copy Processor"),
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        if let Some(tree) = cx.update(|window, _| window.debug_a11y_tree_json()) {
            assert!(
                tree.contains("Copy Processor"),
                "the a11y tree uses the name"
            );
        }
        // Hovering redraws the muted icon in its hover colour without losing the button.
        let b = cx.debug_bounds("TlButton-copy").expect("button drawn");
        cx.simulate_mouse_move(b.center(), None, Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("TlButton-copy-icon").is_some());
    }
}

/// TlTabs tooltips on enabled cells (tasklens-gma.40).
mod tab_tooltips {
    use gpui_kit::point;
    use tl_ui::components::tooltip::DELAY_CONTROL;

    use super::*;

    struct Strip;

    impl Render for Strip {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size(px(600.))
                .child(TlTabs::new("tips").selected(0).tabs(vec![
                    TlTabSpec::new("General"),
                    TlTabSpec::new("Threads").tooltip("Threads (Alt+2)"),
                ]))
        }
    }

    fn hover_shows_tooltip(cx: &mut VisualTestContext, cell: &'static str) -> bool {
        cx.simulate_mouse_move(point(px(590.), px(590.)), None, Modifiers::none());
        draw(cx);
        let at = cx.debug_bounds(cell).expect("cell drawn").center();
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        cx.run_until_parked();
        draw(cx);
        draw(cx);
        cx.debug_bounds("TlTooltip-text").is_some()
    }

    #[gpui_kit::test]
    fn enabled_tab_shows_tooltip_after_delay(cx: &mut TestAppContext) {
        cx.update(setup);
        let (_, cx) = cx.add_window_view(|_, _| Strip);
        cx.update(|window, _| window.activate_window());
        draw(cx);
        assert!(
            !hover_shows_tooltip(cx, "TlTabs-cell-0"),
            "no tooltip without text"
        );
        assert!(hover_shows_tooltip(cx, "TlTabs-cell-1"));
    }
}
