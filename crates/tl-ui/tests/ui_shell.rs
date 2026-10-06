//! Main window shell tests: the `TlSettings` Global (E20-001, ADR-011, R-017, ADR-013).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::settings::{Settings, UiState};
use tl_model::table::TableMode;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::settings_global::{
    Section, SettingKey, SettingKind, SettingValue, TlSettings, animations_on,
};
use tl_ui::testing::fake_engine::FakeEngine;

/// Installs `EngineGlobal` and `BootSettings` with `settings`, then `TlSettings`. Returns the
/// engine and the persist call count.
fn install(cx: &mut TestAppContext, settings: Settings) -> (Arc<FakeEngine>, Rc<Cell<u32>>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let persisted = Rc::new(Cell::new(0));
    let count = Rc::clone(&persisted);
    cx.update(|cx| {
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: Some(Box::new(move |_, _| count.set(count.get() + 1))),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    (engine, persisted)
}

fn versions(cx: &mut TestAppContext) -> Vec<u64> {
    cx.read(|cx| {
        let tl = TlSettings::get(cx);
        Section::ALL.iter().map(|s| tl.version(*s)).collect()
    })
}

/// The sections whose version is not 1.
fn bumped(cx: &mut TestAppContext) -> Vec<Section> {
    let v = versions(cx);
    Section::ALL
        .into_iter()
        .zip(v)
        .filter(|(_, v)| *v != 1)
        .map(|(s, _)| s)
        .collect()
}

#[gpui_kit::test]
fn settings_init_from_boot(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.appearance.ui_scale_pct = 125;
    let (_, persisted) = install(cx, settings);

    cx.read(|cx| {
        assert_eq!(TlSettings::settings(cx).appearance.ui_scale_pct, 125);
        assert_eq!(TlSettings::get(cx).state_version(), 1);
        assert!(cx.global::<BootSettings>().persist.is_none());
    });
    assert!(versions(cx).iter().all(|v| *v == 1));
    assert_eq!(persisted.get(), 0);
}

#[gpui_kit::test]
fn settings_update_bumps_only_changed_section(cx: &mut TestAppContext) {
    let (engine, persisted) = install(cx, Settings::default());

    cx.update(|cx| TlSettings::update(cx, |d| d.settings.motion.animations = false));

    assert_eq!(bumped(cx), vec![Section::Motion]);
    cx.read(|cx| assert_eq!(TlSettings::get(cx).version(Section::Motion), 2));
    assert_eq!(persisted.get(), 1);
    assert!(engine.settings().is_empty());
    // Kit components read only `reduce_motion`, so turning animations off sets it.
    cx.read(|cx| assert!(cx.reduce_motion()));
    cx.update(|cx| TlSettings::update(cx, |d| d.settings.motion.animations = true));
    cx.read(|cx| assert!(!cx.reduce_motion()));
}

#[gpui_kit::test]
fn settings_engine_fields_sent(cx: &mut TestAppContext) {
    let (engine, persisted) = install(cx, Settings::default());

    cx.update(|cx| TlSettings::update(cx, |d| d.settings.processes.hide_system = true));

    assert_eq!(bumped(cx), vec![Section::Processes]);
    assert_eq!(persisted.get(), 1);
    let sent = engine.settings();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].hide_system);
}

#[gpui_kit::test]
fn settings_noop_update_does_nothing(cx: &mut TestAppContext) {
    let (engine, persisted) = install(cx, Settings::default());
    let observed = Rc::new(Cell::new(0));
    let seen = Rc::clone(&observed);
    let _sub = cx.update(|cx| cx.observe_global::<TlSettings>(move |_| seen.set(seen.get() + 1)));

    let current = cx.read(|cx| TlSettings::settings(cx).motion.animations);
    cx.update(|cx| TlSettings::update(cx, |d| d.settings.motion.animations = current));
    cx.run_until_parked();

    assert!(bumped(cx).is_empty());
    cx.read(|cx| assert_eq!(TlSettings::get(cx).state_version(), 1));
    assert_eq!(persisted.get(), 0);
    assert!(engine.settings().is_empty());
    assert_eq!(observed.get(), 0);
}

#[gpui_kit::test]
fn settings_preview_does_not_persist(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.processes.interval_ms = 1000;
    let (engine, persisted) = install(cx, settings);

    cx.update(|cx| {
        TlSettings::preview(cx, |d| {
            d.settings.appearance.ui_scale_pct = 150;
            d.settings.processes.interval_ms = 2000;
        })
    });
    cx.read(|cx| assert_eq!(TlSettings::get(cx).version(Section::Appearance), 2));
    assert_eq!(persisted.get(), 0);
    assert!(engine.settings().is_empty());

    cx.update(|cx| TlSettings::update(cx, |d| d.settings.appearance.ui_scale_pct = 150));
    assert_eq!(persisted.get(), 1);
    // The previewed engine field reaches the engine with the persisting write.
    let sent = engine.settings();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].interval_ms, 2000);

    // Nothing left to persist.
    cx.update(|cx| TlSettings::update(cx, |d| d.settings.appearance.ui_scale_pct = 150));
    assert_eq!(persisted.get(), 1);
}

#[gpui_kit::test]
fn settings_update_clamps(cx: &mut TestAppContext) {
    let (_, persisted) = install(cx, Settings::default());

    cx.update(|cx| TlSettings::update(cx, |d| d.settings.appearance.ui_scale_pct = 203));

    cx.read(|cx| assert_eq!(TlSettings::settings(cx).appearance.ui_scale_pct, 200));
    assert_eq!(persisted.get(), 1);
}

#[gpui_kit::test]
fn settings_toggle_bool_and_reject_non_bool(cx: &mut TestAppContext) {
    let (_, persisted) = install(cx, Settings::default());
    let before = cx.read(|cx| TlSettings::settings(cx).window.close_with_esc);
    assert!(!before);

    cx.update(|cx| TlSettings::toggle(cx, SettingKey::WindowCloseWithEsc));
    cx.read(|cx| assert!(TlSettings::settings(cx).window.close_with_esc));
    cx.update(|cx| TlSettings::toggle(cx, SettingKey::WindowCloseWithEsc));
    cx.read(|cx| assert!(!TlSettings::settings(cx).window.close_with_esc));
    assert_eq!(persisted.get(), 2);

    let snapshot = cx.read(|cx| TlSettings::settings(cx).clone());
    let v = versions(cx);
    cx.update(|cx| TlSettings::toggle(cx, SettingKey::ProcessesMode));
    cx.read(|cx| assert_eq!(*TlSettings::settings(cx), snapshot));
    assert_eq!(versions(cx), v);
    assert_eq!(persisted.get(), 2);
}

#[gpui_kit::test]
fn settings_set_enum_by_serde_name(cx: &mut TestAppContext) {
    let (_, persisted) = install(cx, Settings::default());

    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::ProcessesMode,
            SettingValue::Str("tree".into()),
        )
    });
    cx.read(|cx| assert_eq!(TlSettings::settings(cx).processes.mode, TableMode::Tree));
    assert_eq!(persisted.get(), 1);

    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::ProcessesMode,
            SettingValue::Str("sideways".into()),
        )
    });
    cx.read(|cx| assert_eq!(TlSettings::settings(cx).processes.mode, TableMode::Tree));
    assert_eq!(persisted.get(), 1);

    // Kind mismatch and out-of-type-range integers change nothing.
    cx.update(|cx| {
        TlSettings::set(cx, SettingKey::ProcessesMode, SettingValue::Bool(true));
        TlSettings::set(
            cx,
            SettingKey::AppearanceUiScalePct,
            SettingValue::Int(70_000),
        );
    });
    assert_eq!(persisted.get(), 1);

    // In range: assigned, then clamped.
    cx.update(|cx| TlSettings::set(cx, SettingKey::AppearanceUiScalePct, SettingValue::Int(500)));
    cx.read(|cx| assert_eq!(TlSettings::settings(cx).appearance.ui_scale_pct, 200));
    assert_eq!(persisted.get(), 2);
}

#[gpui_kit::test]
fn settings_set_rejects_map_key(cx: &mut TestAppContext) {
    let (_, persisted) = install(cx, Settings::default());
    let snapshot = cx.read(|cx| TlSettings::settings(cx).clone());

    cx.update(|cx| {
        TlSettings::set(cx, SettingKey::Fans, SettingValue::Str("x".into()));
        TlSettings::set(
            cx,
            SettingKey::PerformanceDeviceOverrides,
            SettingValue::StrList(Arc::from(Vec::new())),
        );
    });

    cx.read(|cx| assert_eq!(*TlSettings::settings(cx), snapshot));
    assert!(bumped(cx).is_empty());
    assert_eq!(persisted.get(), 0);
}

#[test]
fn setting_key_all_matches_table_a() {
    let names: Vec<&str> = SettingKey::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(names.len(), 85);
    assert_eq!(names.iter().collect::<HashSet<_>>().len(), 85);
    assert_eq!(names[0], "general.start_tab");
    assert_eq!(names[84], "debug.show_fps");
    for key in SettingKey::ALL {
        let prefix = key.as_str().split('.').next().unwrap_or_default();
        assert_eq!(key.section().as_str(), prefix, "{key:?}");
    }
    // Sections appear in table (a) order.
    let mut order: Vec<Section> = SettingKey::ALL.iter().map(|k| k.section()).collect();
    order.dedup();
    assert_eq!(order, Section::ALL);
    assert_eq!(SettingKey::Fans.kind(), SettingKind::Map);
    let bools = SettingKey::ALL
        .iter()
        .filter(|k| k.kind() == SettingKind::Bool)
        .count();
    assert_eq!(bools, 55);
}

#[gpui_kit::test]
fn settings_state_update_persists(cx: &mut TestAppContext) {
    let (_, persisted) = install(cx, Settings::default());
    let last = cx.read(|cx| TlSettings::state(cx).tabs.last_tab);
    let other = if last == Tab::Services {
        Tab::Startup
    } else {
        Tab::Services
    };

    cx.update(|cx| TlSettings::update(cx, |d| d.state.tabs.last_tab = other));

    cx.read(|cx| {
        assert_eq!(TlSettings::state(cx).tabs.last_tab, other);
        assert_eq!(TlSettings::get(cx).state_version(), 2);
    });
    assert!(bumped(cx).is_empty());
    assert_eq!(persisted.get(), 1);
}

#[gpui_kit::test]
fn animations_on_respects_reduce_motion(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.motion.animations = true;
    install(cx, settings);
    cx.read(|cx| assert!(animations_on(cx)));

    cx.update(|cx| cx.set_reduce_motion(true));
    cx.read(|cx| assert!(!animations_on(cx)));

    // A reduce-motion set outside TlSettings stays after animations go off and on again.
    cx.update(|cx| TlSettings::toggle(cx, SettingKey::MotionAnimations));
    cx.update(|cx| TlSettings::toggle(cx, SettingKey::MotionAnimations));
    cx.read(|cx| assert!(cx.reduce_motion()));
}

/// Toolbar slot tests (E20-003, UI-SHL-010, R-108, UI-SHL-004).
mod toolbar {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        AnyElement, App, Background, Context, FocusHandle, InteractiveElement, IntoElement,
        Modifiers, MouseButton, ParentElement, Pixels, Point, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, point, px,
    };
    use smallvec::SmallVec;
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::table::TableMode;
    use tl_ui::components::icons::TlIcon;
    use tl_ui::components::tooltip::DELAY_CONTROL;
    use tl_ui::theme::current;
    use tl_ui::window::toolbar::{
        DefaultToolbar, Toolbar, ToolbarContent, ToolbarModel, toolbar_icon, toolbar_toggle,
    };

    gpui_kit::actions!(tl_toolbar_test, [Go]);

    fn model() -> ToolbarModel {
        ToolbarModel {
            tab: Tab::Processes,
            history_mode: false,
            hide_system: false,
            mode: TableMode::Grouped,
            helper: HelperState::Missing,
            helper_installed: false,
            picker_available: false,
            picking: false,
            window_active: true,
        }
    }

    /// Two Toggle buttons, "a" (on) and "b" (off).
    struct TwoButtons;

    impl ToolbarContent for TwoButtons {
        fn buttons(
            &self,
            model: &ToolbarModel,
            _: &mut Window,
            _: &mut App,
        ) -> SmallVec<[AnyElement; 2]> {
            SmallVec::from_iter([
                toolbar_toggle("a", TlIcon::Pin, true, "Pin".into(), Box::new(Go), model)
                    .into_any_element(),
                toolbar_toggle("b", TlIcon::Eye, false, "Eye".into(), Box::new(Go), model)
                    .into_any_element(),
            ])
        }
    }

    type Build = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

    /// A stand-in title bar: it counts `Go` actions, and a left mouse-down followed by a move
    /// counts as a window move, the way the title bar calls `start_window_move`.
    struct Host {
        /// The focused root, so a button's action reaches `on_action` as in the real window.
        focus: FocusHandle,
        build: Build,
        hits: Rc<Cell<usize>>,
        moves: Rc<Cell<usize>>,
        armed: Rc<Cell<bool>>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let hits = self.hits.clone();
            let (down, up) = (self.armed.clone(), self.armed.clone());
            let (armed, moves) = (self.armed.clone(), self.moves.clone());
            div()
                .id("titlebar")
                .track_focus(&self.focus)
                .size(px(600.))
                .flex()
                .flex_row()
                .items_start()
                .on_action(move |_: &Go, _, _| hits.set(hits.get() + 1))
                .on_mouse_down(MouseButton::Left, move |_, _, _| down.set(true))
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(false))
                .on_mouse_move(move |e, _, _| {
                    if armed.get() && e.pressed_button == Some(MouseButton::Left) {
                        moves.set(moves.get() + 1);
                    }
                })
                .child((self.build)(window, cx))
        }
    }

    struct Rig<'a> {
        cx: &'a mut VisualTestContext,
        hits: Rc<Cell<usize>>,
        moves: Rc<Cell<usize>>,
    }

    fn rig(
        cx: &mut TestAppContext,
        ui_scale_pct: u16,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Rig<'_> {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings {
                    ui_scale_pct,
                    ..AppearanceSettings::default()
                },
                Vec::new(),
            );
        });
        let hits = Rc::new(Cell::new(0));
        let moves = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let (hits, moves) = (hits.clone(), moves.clone());
            move |_, cx| Host {
                focus: cx.focus_handle(),
                build: Box::new(build),
                hits,
                moves,
                armed: Rc::new(Cell::new(false)),
            }
        });
        cx.update(|window, cx| {
            window.activate_window();
            view.read(cx).focus.clone().focus(window, cx);
        });
        // Activation lands on the next park; until then the window ignores mouse moves.
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        Rig { cx, hits, moves }
    }

    fn page(content: impl ToolbarContent, model: ToolbarModel) -> Build {
        Box::new(move |window, cx| {
            let page = content.buttons(&model, window, cx);
            Toolbar::new(model.clone(), page).into_any_element()
        })
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> gpui_kit::Bounds<Pixels> {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn away(cx: &mut VisualTestContext) {
        cx.simulate_mouse_move(point(px(300.), px(200.)), None, Modifiers::none());
    }

    /// Asserts two square buttons of side `side` whose centres are `pitch` apart on one row.
    fn assert_pair(cx: &mut VisualTestContext, side: f32, pitch: f32) {
        let a = bounds(cx, "TlButton-a");
        let b = bounds(cx, "TlButton-b");
        for x in [a, b] {
            assert_eq!((x.size.width, x.size.height), (px(side), px(side)));
        }
        let (ca, cb): (Point<Pixels>, Point<Pixels>) = (a.center(), b.center());
        assert_eq!(ca.y, cb.y, "one row");
        assert_eq!(f32::from(cb.x - ca.x), pitch, "centre pitch");
        assert_eq!(bounds(cx, "Toolbar").size.height, px(side));
    }

    #[gpui_kit::test]
    fn toolbar_two_page_buttons_pitch(cx: &mut TestAppContext) {
        let r = rig(cx, 100, page(TwoButtons, model()));
        assert_pair(r.cx, 24., 31.);
        assert_eq!(
            f32::from(bounds(r.cx, "Toolbar").size.width),
            24. + 7. + 24.
        );
    }

    #[gpui_kit::test]
    fn toolbar_scales(cx: &mut TestAppContext) {
        let r = rig(cx, 150, page(TwoButtons, model()));
        assert_pair(r.cx, 36., 46.5);
    }

    #[gpui_kit::test]
    fn toolbar_default_is_empty(cx: &mut TestAppContext) {
        let r = rig(cx, 100, page(DefaultToolbar, model()));
        let empty =
            r.cx.update(|window, cx| DefaultToolbar.buttons(&model(), window, cx).is_empty());
        assert!(empty, "no page buttons");
        assert_eq!(bounds(r.cx, "Toolbar").size.width, px(0.));
        assert!(r.cx.debug_bounds("TlButton-a").is_none());
    }

    #[gpui_kit::test]
    fn toolbar_toggle_not_tab_stop(cx: &mut TestAppContext) {
        let r = rig(cx, 100, page(TwoButtons, model()));
        away(r.cx);
        let fills = r.cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let on = Background::from(current(cx).accent.alpha(0.25));
            window
                .painted_quads()
                .iter()
                .filter(|q| q.background == on)
                .count()
        });
        assert_eq!(fills, 1, "button a has the Toggle on fill, b has none");

        let (before, after) = r.cx.update(|window, cx| {
            let before = window.focused(cx);
            window.focus_next(cx);
            (before, window.focused(cx))
        });
        assert!(before.is_some(), "the host root holds focus");
        assert_eq!(after, before, "Tab finds no stop in the toolbar");
    }

    #[gpui_kit::test]
    fn toolbar_disabled_keeps_tooltip(cx: &mut TestAppContext) {
        let r = rig(cx, 100, |window, cx| {
            let m = model();
            let page = SmallVec::from_iter([toolbar_icon(
                "a",
                TlIcon::Search,
                "Find".into(),
                Box::new(Go),
                true,
                &m,
            )
            .into_any_element()]);
            let _ = (window, cx);
            Toolbar::new(m, page).into_any_element()
        });
        let a = bounds(r.cx, "TlButton-a");
        r.cx.simulate_mouse_move(a.center(), None, Modifiers::none());
        r.cx.executor().advance_clock(DELAY_CONTROL);
        r.cx.run_until_parked();
        assert!(
            bounds(r.cx, "TlTooltip").size.width > px(0.),
            "the tooltip shows on a disabled button"
        );

        r.cx.simulate_click(a.center(), Modifiers::none());
        r.cx.run_until_parked();
        assert_eq!(r.hits.get(), 0, "a click dispatches nothing");
    }

    #[gpui_kit::test]
    fn toolbar_inactive_icon_colour(cx: &mut TestAppContext) {
        let inactive = ToolbarModel {
            window_active: false,
            ..model()
        };
        let r = rig(cx, 100, page(TwoButtons, inactive.clone()));
        bounds(r.cx, "TlButton-a");
        r.cx.update(|_, cx| {
            let theme = current(cx).clone();
            for m in [&inactive, &model()] {
                let toggle = toolbar_toggle("t", TlIcon::Pin, true, "Pin".into(), Box::new(Go), m);
                let plain = toolbar_icon("i", TlIcon::Eye, "Eye".into(), Box::new(Go), false, m);
                if m.window_active {
                    assert_eq!(toggle.icon_color(&theme), theme.accent, "on toggle: accent");
                    assert_eq!(plain.icon_color(&theme), theme.text_primary);
                } else {
                    assert_eq!(toggle.icon_color(&theme), theme.text_secondary);
                    assert_eq!(plain.icon_color(&theme), theme.text_secondary);
                }
            }
        });
    }

    #[gpui_kit::test]
    fn toolbar_button_no_drag(cx: &mut TestAppContext) {
        let r = rig(cx, 100, page(TwoButtons, model()));
        let a = bounds(r.cx, "TlButton-a").center();
        r.cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
        r.cx.simulate_mouse_move(
            a + point(px(40.), px(5.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        r.cx.simulate_mouse_up(
            a + point(px(40.), px(5.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(
            r.moves.get(),
            0,
            "a press on a button never moves the window"
        );

        // The same gesture on the empty title bar does.
        let empty = point(px(400.), px(10.));
        r.cx.simulate_mouse_down(empty, MouseButton::Left, Modifiers::none());
        r.cx.simulate_mouse_move(
            empty + point(px(40.), px(5.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(r.moves.get(), 1);

        r.cx.simulate_mouse_up(
            empty + point(px(40.), px(5.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        r.cx.simulate_click(a, Modifiers::none());
        assert_eq!(
            r.hits.get(),
            1,
            "a click still dispatches the button's action"
        );
    }
}

/// Title bar tests (E20-004, UI-SHL-003, UI-SHL-004, UI-SHL-005, U-44).
mod titlebar {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        AnyElement, AnyView, App, AppContext as _, Background, Bounds, Context, Entity,
        IntoElement, Modifiers, MouseButton, MouseDownEvent, Pixels, Render, Styled,
        TestAppContext, VisualTestContext, Window, WindowControls, div, point, px, size,
    };
    use smallvec::SmallVec;
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::table::TableMode;
    use tl_model::validity::Validity;
    use tl_ui::actions::CloseWindow;
    use tl_ui::components::icons::TlIcon;
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::current;
    use tl_ui::window::titlebar::{
        MenuBarFactory, TitleBarProbe, TitleBarView, keep_above_supported,
    };
    use tl_ui::window::toolbar::{ToolbarContent, ToolbarModel, toolbar_toggle};

    gpui_kit::actions!(tl_titlebar_test, [Go]);

    struct Menu;

    impl Render for Menu {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(120.)).h(px(24.))
        }
    }

    fn menu_bar(_: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| Menu).into()
    }

    struct TwoButtons;

    impl ToolbarContent for TwoButtons {
        fn buttons(
            &self,
            model: &ToolbarModel,
            _: &mut Window,
            _: &mut App,
        ) -> SmallVec<[AnyElement; 2]> {
            SmallVec::from_iter([
                toolbar_toggle("a", TlIcon::Pin, true, "Pin".into(), Box::new(Go), model)
                    .into_any_element(),
                toolbar_toggle("b", TlIcon::Eye, false, "Eye".into(), Box::new(Go), model)
                    .into_any_element(),
            ])
        }
    }

    fn model() -> ToolbarModel {
        ToolbarModel {
            tab: Tab::Processes,
            history_mode: false,
            hide_system: false,
            mode: TableMode::Grouped,
            helper: HelperState::Missing,
            helper_installed: false,
            picker_available: false,
            picking: false,
            window_active: true,
        }
    }

    struct Opts {
        ui_scale_pct: u16,
        menu: bool,
        probe: TitleBarProbe,
    }

    impl Default for Opts {
        fn default() -> Self {
            Self {
                ui_scale_pct: 100,
                menu: true,
                probe: TitleBarProbe {
                    client: Some(true),
                    ..TitleBarProbe::default()
                },
            }
        }
    }

    struct Rig<'a> {
        cx: &'a mut VisualTestContext,
        view: Entity<TitleBarView>,
        closes: Rc<Cell<u32>>,
    }

    fn rig(cx: &mut TestAppContext, opts: Opts) -> Rig<'_> {
        let closes = Rc::new(Cell::new(0));
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings {
                    ui_scale_pct: opts.ui_scale_pct,
                    ..AppearanceSettings::default()
                },
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_global(opts.probe);
            if opts.menu {
                cx.set_global(MenuBarFactory(menu_bar));
            }
            let count = Rc::clone(&closes);
            cx.on_action(move |_: &CloseWindow, _| count.set(count.get() + 1));
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut bar = TitleBarView::new(window, cx);
            bar.set_toolbar(model(), Box::new(TwoButtons), cx);
            bar
        });
        cx.simulate_resize(size(px(1360.), px(200.)));
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        draw(cx);
        Rig { cx, view, closes }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn probe<R>(cx: &mut VisualTestContext, f: impl FnOnce(&TitleBarProbe) -> R) -> R {
        cx.update(|_, cx| f(cx.global::<TitleBarProbe>()))
    }

    fn drag_point(cx: &mut VisualTestContext) -> gpui_kit::Point<Pixels> {
        bounds(cx, "TitleBarDrag").center()
    }

    fn down(
        cx: &mut VisualTestContext,
        at: gpui_kit::Point<Pixels>,
        button: MouseButton,
        n: usize,
    ) {
        cx.simulate_event(MouseDownEvent {
            position: at,
            button,
            modifiers: Modifiers::none(),
            click_count: n,
            first_mouse: false,
        });
    }

    #[gpui_kit::test]
    fn titlebar_parts_order(cx: &mut TestAppContext) {
        let r = rig(cx, Opts::default());
        let parts = [
            "TitleBarIcon",
            "TitleBarMenu",
            "Toolbar",
            "TitleBarDrag",
            "TitleBarCaption-minimize",
            "TitleBarCaption-maximize",
            "TitleBarCaption-close",
        ];
        let xs: Vec<Pixels> = parts.iter().map(|p| bounds(r.cx, p).origin.x).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "left to right: {xs:?}");
        let bar = bounds(r.cx, "TitleBar");
        assert_eq!(bar.size.height, px(34.));
        assert_eq!(bar.size.width, px(1360.));
        let close = bounds(r.cx, "TitleBarCaption-close");
        assert_eq!(close.right(), bar.right(), "flush right");
        assert_eq!(bounds(r.cx, "TitleBarIcon").origin.x, px(12.));
    }

    #[gpui_kit::test]
    fn titlebar_parts_order_without_menu(cx: &mut TestAppContext) {
        let r = rig(
            cx,
            Opts {
                menu: false,
                ..Opts::default()
            },
        );
        assert!(r.cx.debug_bounds("TitleBarMenu").is_none());
        assert!(bounds(r.cx, "Toolbar").origin.x > bounds(r.cx, "TitleBarIcon").origin.x);
    }

    #[gpui_kit::test]
    fn titlebar_drag_starts_move(cx: &mut TestAppContext) {
        let r = rig(cx, Opts::default());
        let at = drag_point(r.cx);
        r.cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        r.cx.simulate_mouse_move(
            at + point(px(10.), px(2.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        r.cx.simulate_mouse_move(
            at + point(px(20.), px(4.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(probe(r.cx, |p| p.moves), 1);

        // A move after the button is up starts nothing.
        r.cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        r.cx.simulate_mouse_move(at, None, Modifiers::none());
        assert_eq!(probe(r.cx, |p| p.moves), 1);

        // The app icon is a drag part too.
        let icon = bounds(r.cx, "TitleBarIcon").center();
        r.cx.simulate_mouse_down(icon, MouseButton::Left, Modifiers::none());
        r.cx.simulate_mouse_move(
            icon + point(px(5.), px(0.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(probe(r.cx, |p| p.moves), 2);
    }

    #[gpui_kit::test]
    fn titlebar_double_click_zooms(cx: &mut TestAppContext) {
        let r = rig(cx, Opts::default());
        let at = drag_point(r.cx);
        down(r.cx, at, MouseButton::Left, 1);
        r.cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        down(r.cx, at, MouseButton::Left, 2);
        r.cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        assert_eq!(probe(r.cx, |p| p.zooms), 1);
        assert_eq!(probe(r.cx, |p| p.moves), 0);
    }

    #[gpui_kit::test]
    fn titlebar_right_click_window_menu(cx: &mut TestAppContext) {
        let r = rig(cx, Opts::default());
        let at = drag_point(r.cx);
        down(r.cx, at, MouseButton::Right, 1);
        assert_eq!(probe(r.cx, |p| p.window_menus.clone()), vec![at]);

        let close = bounds(r.cx, "TitleBarCaption-close").center();
        down(r.cx, close, MouseButton::Right, 1);
        down(r.cx, close, MouseButton::Middle, 1);
        assert_eq!(probe(r.cx, |p| p.window_menus.len()), 1);
        assert_eq!(probe(r.cx, |p| (p.moves, p.zooms)), (0, 0));
        assert_eq!(r.closes.get(), 0);
    }

    #[gpui_kit::test]
    fn titlebar_button_no_drag(cx: &mut TestAppContext) {
        let r = rig(cx, Opts::default());
        for part in ["TitleBarCaption-close", "TlButton-a", "TitleBarMenu"] {
            let at = bounds(r.cx, part).center();
            r.cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
            r.cx.simulate_mouse_move(
                at + point(px(3.), px(2.)),
                MouseButton::Left,
                Modifiers::none(),
            );
            r.cx.simulate_mouse_up(
                at + point(px(3.), px(2.)),
                MouseButton::Left,
                Modifiers::none(),
            );
            assert_eq!(probe(r.cx, |p| p.moves), 0, "{part} never drags");
        }
    }

    #[gpui_kit::test]
    fn titlebar_close_dispatches(cx: &mut TestAppContext) {
        let r = rig(cx, Opts::default());
        let close = bounds(r.cx, "TitleBarCaption-close").center();
        r.cx.simulate_click(close, Modifiers::none());
        r.cx.run_until_parked();
        assert_eq!(r.closes.get(), 1);
    }

    #[gpui_kit::test]
    fn caption_hides_unsupported(cx: &mut TestAppContext) {
        let r = rig(
            cx,
            Opts {
                probe: TitleBarProbe {
                    client: Some(true),
                    controls: Some(WindowControls {
                        maximize: false,
                        ..WindowControls::default()
                    }),
                    ..TitleBarProbe::default()
                },
                ..Opts::default()
            },
        );
        assert!(r.cx.debug_bounds("TitleBarCaption-maximize").is_none());
        let ids: Vec<&str> = r.cx.update(|_, cx| {
            r.view
                .read(cx)
                .drawn_captions()
                .iter()
                .map(|c| c.id)
                .collect()
        });
        assert_eq!(ids, ["minimize", "close"]);
        let min = bounds(r.cx, "TitleBarCaption-minimize");
        let close = bounds(r.cx, "TitleBarCaption-close");
        assert_eq!(min.right(), close.origin.x, "no gap");
        assert_eq!(
            close.right(),
            bounds(r.cx, "TitleBar").right(),
            "flush right"
        );
    }

    #[gpui_kit::test]
    fn caption_restore_when_maximized(cx: &mut TestAppContext) {
        let r = rig(
            cx,
            Opts {
                probe: TitleBarProbe {
                    client: Some(true),
                    maximized: Some(true),
                    ..TitleBarProbe::default()
                },
                ..Opts::default()
            },
        );
        let max =
            r.cx.update(|_, cx| r.view.read(cx).drawn_captions()[1].clone());
        assert_eq!(max.id, "maximize");
        assert_eq!(max.icon, TlIcon::Copy);
        assert_eq!(max.tooltip.as_ref(), "Restore");

        r.cx.update(|_, cx| cx.global_mut::<TitleBarProbe>().maximized = Some(false));
        r.view.update(r.cx, |_, cx| cx.notify());
        draw(r.cx);
        let max =
            r.cx.update(|_, cx| r.view.read(cx).drawn_captions()[1].clone());
        assert_eq!(max.icon, TlIcon::Square);
        assert_eq!(max.tooltip.as_ref(), "Maximize");
    }

    #[gpui_kit::test]
    fn titlebar_inactive_look(cx: &mut TestAppContext) {
        let mut r = rig(cx, Opts::default());
        let (primary, secondary, titlebar) = r.cx.update(|_, cx| {
            let t = current(cx);
            (t.text_primary, t.text_secondary, t.titlebar)
        });
        assert_eq!(colors_of(&mut r), vec![primary; 3]);

        r.cx.deactivate_window();
        r.cx.run_until_parked();
        draw(r.cx);
        assert_eq!(colors_of(&mut r), vec![secondary; 3]);
        let bar = bounds(r.cx, "TitleBar");
        let fill = r.cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            window
                .painted_quads()
                .iter()
                .any(|q| q.background == Background::from(titlebar) && q.bounds == bar.scale(scale))
        });
        assert!(fill, "the bar keeps the titlebar background");
    }

    /// The caption icon colours as last drawn.
    fn colors_of(r: &mut Rig) -> Vec<gpui_kit::Hsla> {
        let view = r.view.clone();
        r.cx.update(|_, cx| {
            view.read(cx)
                .drawn_captions()
                .iter()
                .map(|c| c.color)
                .collect()
        })
    }

    #[gpui_kit::test]
    fn keep_above_supported_by_validity(cx: &mut TestAppContext) {
        assert!(!cx.read(keep_above_supported), "no hub");
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        cx.update(|cx| cx.set_global(GlobalHub(hub)));
        for (validity, supported) in [
            (Validity::Unsupported, false),
            (Validity::Valid, true),
            (Validity::WarmingUp, false),
            (Validity::Stale, true),
        ] {
            engine.publish_with(|s| {
                let w = Arc::make_mut(&mut s.windows);
                w.header.validity = validity;
                w.header.version += 1;
            });
            cx.run_until_parked();
            assert_eq!(cx.read(keep_above_supported), supported, "{validity:?}");
        }
    }

    #[gpui_kit::test]
    fn titlebar_scales(cx: &mut TestAppContext) {
        let r = rig(
            cx,
            Opts {
                ui_scale_pct: 150,
                ..Opts::default()
            },
        );
        assert_eq!(bounds(r.cx, "TitleBar").size.height, px(51.));
        for id in ["minimize", "maximize", "close"] {
            let b = match id {
                "minimize" => bounds(r.cx, "TitleBarCaption-minimize"),
                "maximize" => bounds(r.cx, "TitleBarCaption-maximize"),
                _ => bounds(r.cx, "TitleBarCaption-close"),
            };
            assert_eq!((b.size.width, b.size.height), (px(54.), px(51.)), "{id}");
        }
    }
}

mod tabstrip {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement, Modifiers,
        MouseButton, ParentElement, Pixels, Point, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, point, px, size,
    };
    use smallvec::SmallVec;
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui::actions::SelectTab;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::current;
    use tl_ui::window::tabstrip::{AccentRuleProbe, TabStrip};

    const W: f32 = 1280.;

    /// Holds the strip at the window's top and counts left presses that reach it.
    struct Host {
        strip: Entity<TabStrip>,
        downs: Rc<Cell<u32>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let downs = self.downs.clone();
            div()
                .size_full()
                .on_mouse_down(MouseButton::Left, move |_, _, _| downs.set(downs.get() + 1))
                .child(self.strip.clone())
        }
    }

    /// A stand-in for the filter box: 220 wide.
    struct Filter;

    impl Render for Filter {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(220.)).h_full()
        }
    }

    struct Rig<'a> {
        cx: &'a mut VisualTestContext,
        strip: Entity<TabStrip>,
        downs: Rc<Cell<u32>>,
        selects: Rc<RefCell<Vec<usize>>>,
    }

    fn all() -> SmallVec<[Tab; 6]> {
        SmallVec::from_slice(&Tab::ALL)
    }

    fn rig(
        cx: &mut TestAppContext,
        ui_scale_pct: u16,
        tabs: SmallVec<[Tab; 6]>,
        active: Tab,
        filter: bool,
    ) -> Rig<'_> {
        let selects = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings {
                    ui_scale_pct,
                    ..AppearanceSettings::default()
                },
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
            let engine = FakeEngine::new();
            let client: Arc<dyn EngineClient> = engine;
            cx.set_global(EngineGlobal(client));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            let seen = Rc::clone(&selects);
            cx.on_action(move |a: &SelectTab, _| seen.borrow_mut().push(a.0));
        });
        let downs = Rc::new(Cell::new(0));
        let mut strip = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let s = cx.new(|cx| {
                let mut s = TabStrip::new(tabs, active, window, cx);
                if filter {
                    s.set_filter_slot(Some(cx.new(|_| Filter).into()), cx);
                }
                s
            });
            strip = Some(s.clone());
            Host {
                strip: s,
                downs: downs.clone(),
            }
        });
        cx.simulate_resize(size(px(W), px(200.)));
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        draw(cx);
        Rig {
            cx,
            strip: strip.expect("strip built"),
            downs,
            selects,
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn selector(tab: Tab) -> &'static str {
        match tab {
            Tab::Processes => "TabStrip-processes",
            Tab::Performance => "TabStrip-performance",
            Tab::Services => "TabStrip-services",
            Tab::Startup => "TabStrip-startup",
            Tab::Connections => "TabStrip-connections",
            Tab::Information => "TabStrip-information",
        }
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn labels(r: &mut Rig) -> Vec<String> {
        let strip = r.strip.clone();
        r.cx.update(|_, cx| {
            strip
                .read(cx)
                .drawn_labels()
                .iter()
                .map(|l| l.to_string())
                .collect()
        })
    }

    fn rule(r: &mut Rig) -> AccentRuleProbe {
        let strip = r.strip.clone();
        r.cx.update(|_, cx| strip.read(cx).accent_rule())
            .expect("rule built")
    }

    fn builds(r: &mut Rig) -> u32 {
        let strip = r.strip.clone();
        r.cx.update(|_, cx| strip.read(cx).rule_builds())
    }

    fn has_vertex(rule: &AccentRuleProbe, at: Point<Pixels>) -> bool {
        rule.vertices
            .iter()
            .any(|v| (v.x - at.x).abs() < px(0.01) && (v.y - at.y).abs() < px(0.01))
    }

    #[gpui_kit::test]
    fn tabstrip_order_and_labels(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        assert_eq!(
            labels(&mut r),
            [
                "Processes",
                "Performance",
                "Services",
                "Startup Apps",
                "Connections",
                "Information"
            ]
        );
        let xs: Vec<Pixels> = Tab::ALL
            .iter()
            .map(|t| bounds(r.cx, selector(*t)).origin.x)
            .collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "left to right: {xs:?}");
        assert_eq!(xs[0], px(0.));
        let strip = bounds(r.cx, "TabStrip");
        assert_eq!(strip.size.height, px(30.));
        let tab = bounds(r.cx, "TabStrip-services");
        assert_eq!(tab.size.height, px(30.));
    }

    #[gpui_kit::test]
    fn tabstrip_only_built_tabs(cx: &mut TestAppContext) {
        let tabs = SmallVec::from_slice(&[Tab::Processes, Tab::Performance]);
        let mut r = rig(cx, 100, tabs, Tab::Processes, false);
        assert_eq!(labels(&mut r), ["Processes", "Performance"]);
        for t in &Tab::ALL[2..] {
            assert!(r.cx.debug_bounds(selector(*t)).is_none(), "{t:?} absent");
        }
    }

    #[gpui_kit::test]
    fn tabstrip_history_label(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        assert_eq!(labels(&mut r)[0], "Processes");
        r.cx.update(|_, cx| TlSettings::update(cx, |d| d.state.processes.history_mode = true));
        r.cx.run_until_parked();
        draw(r.cx);
        assert_eq!(labels(&mut r)[0], "History");
        assert_eq!(labels(&mut r)[1], "Performance");
    }

    #[gpui_kit::test]
    fn accent_rule_bounds(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, true);
        let filter = bounds(r.cx, "TabStripFilter");
        assert_eq!((filter.left(), filter.right()), (px(1060.), px(W)));
        let active = bounds(r.cx, "TabStrip-processes");
        let rule = rule(&mut r);
        assert_eq!(rule.active, active);
        assert_eq!(rule.end_x, px(1060.));
        // Processes sits at x = 0, so the left part of the line is empty.
        assert_eq!(rule.path_bounds.left(), active.left());
        assert_eq!(rule.path_bounds.right(), px(1060.), "nothing right of 1060");
        assert!(rule.vertices.iter().all(|v| v.x <= px(1060.)));
        let bottom = rule.strip.bottom();
        assert!(has_vertex(
            &rule,
            point(active.right() + px(6.), bottom - px(2.))
        ));
        assert!(has_vertex(&rule, point(px(1060.), bottom)));

        // With Performance active the line runs from x = 0 to its left edge.
        r.strip
            .update(r.cx, |s, cx| s.set_active(Tab::Performance, cx));
        draw(r.cx);
        let active = bounds(r.cx, "TabStrip-performance");
        let rule = self::rule(&mut r);
        assert_eq!(rule.active, active);
        assert_eq!(rule.path_bounds.left(), px(0.));
        assert!(has_vertex(&rule, point(px(0.), bottom - px(2.))));
        assert!(has_vertex(
            &rule,
            point(active.left() - px(6.), bottom - px(2.))
        ));
        assert!(has_vertex(
            &rule,
            point(active.right() + px(6.), bottom - px(2.))
        ));
        assert_eq!(rule.path_bounds.right(), px(1060.));
    }

    #[gpui_kit::test]
    fn accent_rule_to_edge_without_filter(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Services, false);
        assert!(r.cx.debug_bounds("TabStripFilter").is_none());
        let rule = rule(&mut r);
        assert_eq!(rule.end_x, px(W));
        assert_eq!(rule.path_bounds.right(), px(W));
        assert!(has_vertex(&rule, point(px(W), rule.strip.bottom())));
    }

    #[gpui_kit::test]
    fn inactive_window_uses_divider_outline(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        let (accent, divider) = r.cx.update(|_, cx| {
            let t = current(cx);
            (t.accent, t.divider)
        });
        assert_ne!(accent, divider);
        assert_eq!(rule(&mut r).color, accent);

        r.cx.deactivate_window();
        r.cx.run_until_parked();
        draw(r.cx);
        assert_eq!(rule(&mut r).color, divider);
    }

    #[gpui_kit::test]
    fn tab_click_selects(cx: &mut TestAppContext) {
        let r = rig(cx, 100, all(), Tab::Processes, false);
        let at = bounds(r.cx, "TabStrip-services").center();
        r.cx.simulate_click(at, Modifiers::none());
        r.cx.run_until_parked();
        assert_eq!(*r.selects.borrow(), [2]);
        assert_eq!(r.downs.get(), 0, "the press stops at the tab");

        // Right and middle presses select nothing.
        for button in [MouseButton::Right, MouseButton::Middle] {
            r.cx.simulate_mouse_down(at, button, Modifiers::none());
            r.cx.simulate_mouse_up(at, button, Modifiers::none());
        }
        r.cx.run_until_parked();
        assert_eq!(*r.selects.borrow(), [2]);
    }

    #[gpui_kit::test]
    fn accent_rule_cached(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, true);
        // Opening, resizing and activating the window each changed the key.
        let first = builds(&mut r);
        assert!(first >= 1);
        r.strip.update(r.cx, |_, cx| cx.notify());
        draw(r.cx);
        r.strip
            .update(r.cx, |s, cx| s.set_active(Tab::Processes, cx));
        draw(r.cx);
        assert_eq!(builds(&mut r), first, "same key, no rebuild");

        r.strip
            .update(r.cx, |s, cx| s.set_active(Tab::Services, cx));
        draw(r.cx);
        assert_eq!(builds(&mut r), first + 1);
    }

    #[gpui_kit::test]
    fn tabstrip_scales(cx: &mut TestAppContext) {
        let mut r = rig(cx, 150, all(), Tab::Processes, false);
        assert_eq!(bounds(r.cx, "TabStrip").size.height, px(45.));
        assert_eq!(bounds(r.cx, "TabStrip-processes").size.height, px(45.));
        let rule = rule(&mut r);
        assert_eq!(rule.width, px(3.));
        // The outline's inner edge is 3 px in from the tab's left edge.
        let bottom = rule.strip.bottom();
        assert!(has_vertex(
            &rule,
            point(rule.active.left() + px(3.), bottom)
        ));
        assert!(has_vertex(
            &rule,
            point(rule.active.right() - px(3.), bottom)
        ));
        assert!(has_vertex(&rule, point(px(W), bottom - px(3.))));
    }
}

/// The status chip (E20-006, UI-SHL-050).
mod status {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        AnyView, AppContext as _, Bounds, Context, Entity, FocusHandle, Hsla, InteractiveElement,
        IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ParentElement, Pixels, Render,
        StyleRefinement, Styled, TestAppContext, VisualTestContext, Window, div,
        prelude::FluentBuilder as _, px, size,
    };
    use smallvec::{SmallVec, smallvec};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::components::badge::{TlBadgeKind, badge_colors};
    use tl_ui::theme::current;
    use tl_ui::window::status::{StatusAction, StatusActionKind, StatusChip};

    gpui_kit::actions!(tl_status_test, [ShowRestricted, ShowAdmin]);

    /// A page with one Tab stop above the chip. `max_w` caps the chip's slot, as `MainWindow`
    /// does; `cached` embeds the chip with `.cached(..)`, as `MainWindow` does.
    struct Host {
        page: FocusHandle,
        chip: Entity<StatusChip>,
        max_w: Option<f32>,
        cached: bool,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let view = AnyView::from(self.chip.clone());
            let chip = if self.cached {
                view.cached(StyleRefinement::default().w(px(400.)).h(px(40.)))
                    .into_any_element()
            } else {
                view.into_any_element()
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().id("page").track_focus(&self.page).size(px(50.)))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .when_some(self.max_w, |slot, w| slot.max_w(px(w)))
                        .child(chip),
                )
        }
    }

    struct Rig<'a> {
        cx: &'a mut VisualTestContext,
        chip: Entity<StatusChip>,
        page: FocusHandle,
        restricted: Rc<Cell<u32>>,
        admin: Rc<Cell<u32>>,
    }

    fn rig(
        cx: &mut TestAppContext,
        ui_scale_pct: u16,
        max_w: Option<f32>,
        cached: bool,
    ) -> Rig<'_> {
        let restricted = Rc::new(Cell::new(0));
        let admin = Rc::new(Cell::new(0));
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings {
                    ui_scale_pct,
                    ..AppearanceSettings::default()
                },
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
            let (r, a) = (restricted.clone(), admin.clone());
            cx.on_action(move |_: &ShowRestricted, _| r.set(r.get() + 1));
            cx.on_action(move |_: &ShowAdmin, _| a.set(a.get() + 1));
        });
        let mut parts = None;
        let (_, cx) = cx.add_window_view(|_, cx| {
            let chip = cx.new(StatusChip::new);
            let page = cx.focus_handle().tab_stop(true);
            parts = Some((chip.clone(), page.clone()));
            Host {
                page,
                chip,
                max_w,
                cached,
            }
        });
        let (chip, page) = parts.expect("host built");
        cx.simulate_resize(size(px(800.), px(200.)));
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        draw(cx);
        Rig {
            cx,
            chip,
            page,
            restricted,
            admin,
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn set_text(r: &mut Rig, text: &str) {
        let chip = r.chip.clone();
        r.cx.update(|_, cx| chip.update(cx, |c, cx| c.set_page_text(text, cx)));
        draw(r.cx);
    }

    fn set_paused(r: &mut Rig, paused: bool) {
        let chip = r.chip.clone();
        r.cx.update(|_, cx| chip.update(cx, |c, cx| c.set_paused(paused, cx)));
        draw(r.cx);
    }

    fn set_actions(r: &mut Rig, actions: SmallVec<[StatusAction; 2]>) {
        let chip = r.chip.clone();
        r.cx.update(|_, cx| chip.update(cx, |c, cx| c.set_page_actions(actions, cx)));
        draw(r.cx);
    }

    fn read<R>(r: &mut Rig, f: impl FnOnce(&StatusChip) -> R) -> R {
        let chip = r.chip.clone();
        r.cx.update(|_, cx| f(chip.read(cx)))
    }

    fn drawn(r: &mut Rig) -> Vec<(String, Hsla)> {
        read(r, |c| {
            c.drawn()
                .iter()
                .map(|(s, color)| (s.to_string(), *color))
                .collect()
        })
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn colors(r: &mut Rig) -> (Hsla, Hsla, Hsla) {
        r.cx.update(|_, cx| {
            let t = current(cx);
            (t.text_primary, t.text_secondary, t.accent)
        })
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).expect("valid keystroke");
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
    }

    fn action(
        kind: StatusActionKind,
        label: &'static str,
        action: Box<dyn gpui_kit::Action>,
    ) -> StatusAction {
        StatusAction {
            kind,
            label: label.into(),
            tooltip: "Details".into(),
            action,
        }
    }

    #[gpui_kit::test]
    fn status_chip_segments(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, false);
        set_text(&mut r, "41 / 296 Processes | 3 processes selected");
        let (primary, secondary, _) = colors(&mut r);
        assert_eq!(
            drawn(&mut r),
            vec![
                ("41 / 296 Processes".to_owned(), primary),
                (" | ".to_owned(), secondary),
                ("3 processes selected".to_owned(), primary),
            ]
        );
        assert_eq!(
            read(&mut r, |c| c.laid_out_text()).as_deref(),
            Some("41 / 296 Processes | 3 processes selected")
        );
    }

    #[gpui_kit::test]
    fn status_chip_hidden_when_empty(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, false);
        assert!(!read(&mut r, |c| c.is_visible()));
        let b = bounds(r.cx, "StatusChip");
        assert_eq!((b.size.width, b.size.height), (px(0.), px(0.)));
        assert!(drawn(&mut r).is_empty());
    }

    #[gpui_kit::test]
    fn status_chip_paused_only(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, false);
        set_paused(&mut r, true);
        assert!(read(&mut r, |c| c.is_visible()));
        let (_, _, accent) = colors(&mut r);
        assert_eq!(drawn(&mut r), vec![("[PAUSED - Ctrl]".to_owned(), accent)]);
        assert!(bounds(r.cx, "StatusChip").size.width > px(0.));
    }

    #[gpui_kit::test]
    fn status_chip_paused(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, false);
        set_text(&mut r, "296 Processes");
        set_paused(&mut r, true);
        assert_eq!(
            read(&mut r, |c| c.drawn_text()),
            "296 Processes [PAUSED - Ctrl]"
        );
        assert_eq!(
            read(&mut r, |c| c.laid_out_text()).as_deref(),
            Some("296 Processes [PAUSED - Ctrl]")
        );
        let (_, _, accent) = colors(&mut r);
        assert_eq!(
            drawn(&mut r).last(),
            Some(&("[PAUSED - Ctrl]".to_owned(), accent))
        );
    }

    #[gpui_kit::test]
    fn status_chip_no_notify_on_same_text(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, true);
        set_text(&mut r, "296 Processes");
        let before = read(&mut r, |c| c.renders());
        set_text(&mut r, "296 Processes");
        assert_eq!(
            read(&mut r, |c| c.renders()),
            before,
            "same text: no render"
        );
        set_text(&mut r, "297 Processes");
        assert_eq!(
            read(&mut r, |c| c.renders()),
            before + 1,
            "new text renders"
        );
    }

    #[gpui_kit::test]
    fn status_chip_truncates(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, Some(300.), false);
        let long = "296 Processes | 3 processes selected | ".repeat(3);
        set_text(&mut r, long.trim_end_matches(" | "));
        let laid = read(&mut r, |c| c.laid_out_text()).expect("text laid out");
        assert!(laid.ends_with('…'), "cut at the end: {laid:?}");
        assert_eq!(bounds(r.cx, "StatusChip").size.width, px(300.));
    }

    #[gpui_kit::test]
    fn status_chip_scales(cx: &mut TestAppContext) {
        let mut r = rig(cx, 150, None, false);
        set_text(&mut r, "296 Processes");
        assert_eq!(bounds(r.cx, "StatusChip").size.height, px(39.));
    }

    #[gpui_kit::test]
    fn status_chip_link_action(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, false);
        set_text(&mut r, "296 Processes");
        set_actions(
            &mut r,
            smallvec![action(
                StatusActionKind::Link,
                "42 restricted",
                Box::new(ShowRestricted)
            )],
        );
        assert_eq!(
            read(&mut r, |c| c.drawn_text()),
            "296 Processes | 42 restricted"
        );
        let (primary, secondary, _) = colors(&mut r);
        let parts = drawn(&mut r);
        assert_eq!(parts[1], (" | ".to_owned(), secondary));
        assert_eq!(parts[2], ("42 restricted".to_owned(), primary));

        let link = bounds(r.cx, "StatusChip-action-0");
        r.cx.simulate_click(link.center(), Modifiers::none());
        assert_eq!(r.restricted.get(), 1, "one dispatch per click");

        let page = r.page.clone();
        r.cx.update(|window, cx| {
            page.focus(window, cx);
            window.focus_next(cx);
        });
        assert!(
            !r.cx.update(|window, _| page.is_focused(window)),
            "Tab leaves the page"
        );
        press(r.cx, "enter");
        assert_eq!(r.restricted.get(), 2, "Tab from the page reaches the link");
        press(r.cx, "space");
        assert_eq!(r.restricted.get(), 3, "Space acts too");
    }

    #[gpui_kit::test]
    fn status_chip_badge_action(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, None, false);
        set_text(&mut r, "296 Processes");
        set_actions(
            &mut r,
            smallvec![action(
                StatusActionKind::Badge,
                "Administrator mode",
                Box::new(ShowAdmin)
            )],
        );
        let info =
            r.cx.update(|_, cx| badge_colors(TlBadgeKind::Info, current(cx)).text);
        let parts = drawn(&mut r);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].0, " ", "one space before the badge");
        assert_eq!(parts[2], ("Administrator mode".to_owned(), info));

        let text = bounds(r.cx, "StatusChip-text");
        let badge = bounds(r.cx, "TlBadge");
        let part = bounds(r.cx, "StatusChip-action-0");
        assert!(badge.left() > text.right(), "the badge follows the text");
        assert!(part.contains(&badge.center()));

        r.cx.simulate_click(badge.center(), Modifiers::none());
        assert_eq!(r.admin.get(), 1);
    }
}

mod main_window {
    use std::sync::{Arc, Mutex};

    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Global,
        InteractiveElement, IntoElement, Pixels, Render, Styled, TestAppContext, VisualTestContext,
        Window, WindowBounds, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{
        AppearanceSettings, Settings, StartTab, ThemeSettings, UiState, WindowState,
    };
    use tl_ui::actions::{NextTab, PrevTab, RefreshCurrentTab, SelectTab};
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::components::toast::{self, ToastKind};
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageFactory, PageSlot, ShellPage, first_frame_ms_from,
        main_window_options, main_window_view, open_main_window, push_main_toast, restored_bounds,
        set_test_page,
    };
    use tl_ui::window::status::StatusChanged;
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    /// Pages built and pages refreshed, in order.
    #[derive(Default)]
    struct Log {
        created: Vec<Tab>,
        refreshed: Vec<Tab>,
    }

    impl Global for Log {}

    struct FakePage {
        tab: Tab,
        focus: FocusHandle,
        text: String,
    }

    impl EventEmitter<StatusChanged> for FakePage {}

    impl Render for FakePage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id(self.tab.as_str())
                .track_focus(&self.focus)
                .size_full()
        }
    }

    impl ShellPage for FakePage {
        fn status_text(&self, out: &mut String) {
            out.push_str(&self.text);
        }

        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }

        fn refresh(&mut self, cx: &mut Context<Self>) {
            cx.default_global::<Log>().refreshed.push(self.tab);
        }
    }

    fn fake<const I: usize>(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let tab = Tab::ALL[I];
        cx.default_global::<Log>().created.push(tab);
        let page = cx.new(|cx| FakePage {
            tab,
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
        });
        PageSlot::new(page, cx)
    }

    const FAKES: [PageFactory; 6] = [
        fake::<0>, fake::<1>, fake::<2>, fake::<3>, fake::<4>, fake::<5>,
    ];

    struct Opts {
        ui_scale_pct: u16,
        pages: &'static [Tab],
        start_tab: StartTab,
        arg: Option<StartTab>,
        last_tab: Tab,
        engine: bool,
    }

    impl Default for Opts {
        fn default() -> Self {
            Self {
                ui_scale_pct: 100,
                pages: &Tab::ALL,
                start_tab: StartTab::Processes,
                arg: None,
                last_tab: Tab::Processes,
                engine: false,
            }
        }
    }

    /// Every Global the main window needs, with fake pages for `opts.pages`. Opens no window.
    fn setup(cx: &mut TestAppContext, opts: &Opts) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings {
                ui_scale_pct: opts.ui_scale_pct,
                ..AppearanceSettings::default()
            };
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::i18n::init_strings(cx, "en-US");
            let mut settings = Settings {
                appearance,
                ..Settings::default()
            };
            settings.general.start_tab = opts.start_tab;
            let mut state = UiState::default();
            state.tabs.last_tab = opts.last_tab;
            cx.set_global(BootSettings {
                settings,
                state,
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            if opts.engine {
                let engine = FakeEngine::new();
                let client: Arc<dyn EngineClient> = engine;
                cx.set_global(EngineGlobal(client.clone()));
                let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
                let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
                cx.set_global(GlobalHub(hub));
                cx.set_global(GlobalDemand(demand));
            }
            toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            cx.set_global(StartOptions {
                tab: opts.arg,
                ..StartOptions::default()
            });
            for tab in opts.pages {
                set_test_page(cx, *tab, FAKES[*tab as usize]);
            }
            cx.set_global(Log::default());
            shell::init(cx);
        });
    }

    fn open(cx: &mut TestAppContext) -> VisualTestContext {
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        draw(&mut vcx);
        vcx
    }

    fn rig(cx: &mut TestAppContext, opts: Opts) -> VisualTestContext {
        setup(cx, &opts);
        open(cx)
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn frame(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        vcx.run_until_parked();
    }

    fn main(vcx: &mut VisualTestContext) -> Entity<MainWindow> {
        vcx.update(|_, cx| main_window_view(cx).expect("main window open"))
    }

    fn active(vcx: &mut VisualTestContext) -> Tab {
        let main = main(vcx);
        vcx.read_entity(&main, |m, _| m.active_tab())
    }

    fn page(vcx: &mut VisualTestContext, tab: Tab) -> Option<Entity<FakePage>> {
        let main = main(vcx);
        vcx.read_entity(&main, |m, _| m.page_entity::<FakePage>(tab))
    }

    fn act(vcx: &mut VisualTestContext, action: impl gpui_kit::Action) {
        vcx.dispatch_action(action);
        vcx.run_until_parked();
        draw(vcx);
    }

    fn log<R>(vcx: &mut VisualTestContext, f: impl FnOnce(&Log) -> R) -> R {
        vcx.update(|_, cx| f(cx.global::<Log>()))
    }

    fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn chip_text(vcx: &mut VisualTestContext) -> String {
        let main = main(vcx);
        let chip = vcx.read_entity(&main, |m, _| m.status_chip().clone());
        vcx.read_entity(&chip, |c, _| c.drawn_text())
    }

    fn emit_status(vcx: &mut VisualTestContext, tab: Tab, text: &str) {
        let page = page(vcx, tab).expect("page built");
        let text = text.to_owned();
        vcx.update(|_, cx| {
            page.update(cx, |p, cx| {
                p.text = text;
                cx.emit(StatusChanged);
            })
        });
        vcx.run_until_parked();
        draw(vcx);
    }

    #[gpui_kit::test]
    fn main_window_layout_regions(cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
        for (cx, scale, expect) in [(cx_a, 100, [34., 30., 736.]), (cx_b, 150, [51., 45., 704.])] {
            let mut vcx = rig(
                cx,
                Opts {
                    ui_scale_pct: scale,
                    pages: &[Tab::Processes],
                    ..Opts::default()
                },
            );
            assert_eq!(
                vcx.update(|window, _| window.viewport_size()),
                size(px(1280.), px(800.))
            );
            let heights = [
                bounds(&mut vcx, "TitleBar").size.height,
                bounds(&mut vcx, "TabStrip").size.height,
                bounds(&mut vcx, "MainWindowPage").size.height,
            ];
            assert_eq!(heights, expect.map(px), "at {scale} %");
            assert_eq!(bounds(&mut vcx, "MainWindowPage").size.width, px(1280.));
        }
    }

    #[gpui_kit::test]
    fn main_window_min_size(cx: &mut TestAppContext) {
        setup(cx, &Opts::default());
        let options = cx.update(|cx| main_window_options(cx));
        assert_eq!(options.window_min_size, Some(size(px(800.), px(500.))));
        assert_eq!(
            options.app_id.as_deref(),
            Some("io.github.oysteinkrog.Tasklens")
        );
        assert_eq!(
            options.titlebar.and_then(|t| t.title).as_deref(),
            Some("tasklens")
        );
        assert!(options.show && options.focus);
    }

    #[test]
    fn geometry_restore_clamps() {
        let small = WindowState {
            width: 640,
            height: 400,
            maximized: false,
        };
        match restored_bounds(&small) {
            WindowBounds::Windowed(b) => {
                assert_eq!(b.size, size(px(800.), px(500.)));
                assert_eq!(b.origin, gpui_kit::point(px(0.), px(0.)));
            }
            other => panic!("windowed bounds expected, got {other:?}"),
        }
        let maximized = WindowState {
            maximized: true,
            ..small
        };
        assert!(matches!(
            restored_bounds(&maximized),
            WindowBounds::Maximized(b) if b.size == size(px(800.), px(500.))
        ));
    }

    #[gpui_kit::test]
    fn startup_tab_arg_wins(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                pages: &[Tab::Performance, Tab::Services],
                start_tab: StartTab::Performance,
                arg: Some(StartTab::Services),
                ..Opts::default()
            },
        );
        assert_eq!(active(&mut vcx), Tab::Services);
        assert!(page(&mut vcx, Tab::Services).is_some());
        assert!(page(&mut vcx, Tab::Performance).is_none());
        assert_eq!(log(&mut vcx, |l| l.created.clone()), vec![Tab::Services]);
    }

    #[gpui_kit::test]
    fn startup_tab_last_used(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                pages: &[Tab::Processes, Tab::Connections],
                start_tab: StartTab::LastUsed,
                last_tab: Tab::Connections,
                ..Opts::default()
            },
        );
        assert_eq!(active(&mut vcx), Tab::Connections);
    }

    #[gpui_kit::test]
    fn startup_tab_unbuilt_falls_back(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                pages: &[Tab::Performance, Tab::Services],
                start_tab: StartTab::Processes,
                ..Opts::default()
            },
        );
        assert_eq!(active(&mut vcx), Tab::Performance);
    }

    #[gpui_kit::test]
    fn tabstrip_keys(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        vcx.simulate_keystrokes("ctrl-3");
        vcx.run_until_parked();
        draw(&mut vcx);
        assert_eq!(active(&mut vcx), Tab::Services);
        let services = page(&mut vcx, Tab::Services).expect("Services built");
        assert_eq!(
            vcx.update(|_, cx| TlSettings::state(cx).tabs.last_tab),
            Tab::Services
        );
        let focus = vcx.read_entity(&services, |p, _| p.focus.clone());
        assert!(vcx.update(|window, _| focus.is_focused(window)));
    }

    #[gpui_kit::test]
    fn tab_wraps_and_skips_unbuilt(cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
        let mut vcx = rig(
            cx_a,
            Opts {
                start_tab: StartTab::Information,
                ..Opts::default()
            },
        );
        assert_eq!(active(&mut vcx), Tab::Information);
        act(&mut vcx, NextTab);
        assert_eq!(active(&mut vcx), Tab::Processes);
        act(&mut vcx, PrevTab);
        assert_eq!(active(&mut vcx), Tab::Information);

        let mut vcx = rig(
            cx_b,
            Opts {
                pages: &[Tab::Processes, Tab::Performance],
                start_tab: StartTab::Performance,
                ..Opts::default()
            },
        );
        act(&mut vcx, NextTab);
        assert_eq!(active(&mut vcx), Tab::Processes);
        act(&mut vcx, SelectTab(2));
        assert_eq!(
            active(&mut vcx),
            Tab::Processes,
            "an unbuilt tab is not selected"
        );
        act(&mut vcx, SelectTab(9));
        assert_eq!(
            active(&mut vcx),
            Tab::Processes,
            "out of range does nothing"
        );
    }

    #[gpui_kit::test]
    fn status_text_from_active_page(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        act(&mut vcx, SelectTab(2));
        act(&mut vcx, SelectTab(0));
        emit_status(&mut vcx, Tab::Processes, "12 Items");
        assert_eq!(chip_text(&mut vcx), "12 Items");
        // The cached chip keeps its content width and sits at the page's bottom-left.
        let chip = bounds(&mut vcx, "StatusChip");
        let page = bounds(&mut vcx, "MainWindowPage");
        assert!(
            chip.size.width > px(0.) && chip.size.width < px(200.),
            "{chip:?}"
        );
        assert_eq!(chip.size.height, px(26.));
        assert_eq!(chip.bottom_left(), page.bottom_left());
        emit_status(&mut vcx, Tab::Services, "99 Items");
        assert_eq!(
            chip_text(&mut vcx),
            "12 Items",
            "an inactive page changes nothing"
        );
        act(&mut vcx, SelectTab(2));
        assert_eq!(
            chip_text(&mut vcx),
            "99 Items",
            "a switch reads the new page"
        );
    }

    #[gpui_kit::test]
    fn page_created_once_and_kept(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        act(&mut vcx, SelectTab(2));
        let first = page(&mut vcx, Tab::Services).expect("Services built");
        act(&mut vcx, SelectTab(0));
        act(&mut vcx, SelectTab(2));
        let again = page(&mut vcx, Tab::Services).expect("Services kept");
        assert_eq!(first.entity_id(), again.entity_id());
        assert_eq!(
            log(&mut vcx, |l| l.created.clone()),
            vec![Tab::Processes, Tab::Services]
        );
    }

    #[gpui_kit::test]
    fn refresh_routes_to_active_page(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        act(&mut vcx, SelectTab(2));
        act(&mut vcx, SelectTab(0));
        act(&mut vcx, RefreshCurrentTab);
        assert_eq!(log(&mut vcx, |l| l.refreshed.clone()), vec![Tab::Processes]);
    }

    #[gpui_kit::test]
    fn first_frame_no_wait(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                engine: true,
                ..Opts::default()
            },
        );
        let main = main(&mut vcx);
        assert_eq!(vcx.read_entity(&main, |m, _| m.first_frames()), 0);
        frame(&mut vcx);
        assert_eq!(vcx.read_entity(&main, |m, _| m.first_frames()), 1);
        frame(&mut vcx);
        assert_eq!(
            vcx.read_entity(&main, |m, _| m.first_frames()),
            1,
            "once per window"
        );
    }

    #[test]
    fn first_frame_ms_parse() {
        let stat = "1234 (tl (x) y) S 1 1 1 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 8 0 4200 1 2";
        assert_eq!(first_frame_ms_from(stat, "45.31 100.00"), Some(3310));
    }

    #[test]
    fn first_frame_ms_parse_garbage() {
        assert_eq!(first_frame_ms_from("garbage", "45.31 100.00"), None);
        assert_eq!(first_frame_ms_from("1 (a) S 1 2", "45.31 100.00"), None);
        let stat = "1234 (tl) S 1 1 1 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 8 0 4200 1 2";
        assert_eq!(first_frame_ms_from(stat, "x.y 1"), None);
        assert_eq!(first_frame_ms_from(stat, ""), None);
    }

    /// Keeps the `first_frame_ms` values of `tasklens::perf` events.
    struct Capture(Arc<Mutex<Vec<u64>>>);

    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            if event.metadata().target() != "tasklens::perf" {
                return;
            }
            struct Ms(Option<u64>);
            impl tracing::field::Visit for Ms {
                fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
                    if f.name() == "first_frame_ms" {
                        self.0 = Some(v);
                    }
                }
                fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            }
            let mut ms = Ms(None);
            event.record(&mut ms);
            let ms = ms.0.expect("the perf event carries first_frame_ms");
            self.0.lock().expect("capture lock").push(ms);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    #[gpui_kit::test]
    fn first_frame_logged_once(cx: &mut TestAppContext) {
        // Other tests hit the same callsite on other threads with no subscriber. With only one
        // scoped dispatcher alive, tracing-core computes a callsite's interest from the
        // registering thread's default alone and can cache it as never enabled. A second live
        // dispatcher makes it ask every registered dispatcher, this test's included.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let logged = Arc::new(Mutex::new(Vec::new()));
        tracing::subscriber::with_default(Capture(logged.clone()), || {
            tracing::callsite::rebuild_interest_cache();
            let mut vcx = rig(cx, Opts::default());
            frame(&mut vcx);
            assert_eq!(logged.lock().expect("capture lock").len(), 1);

            vcx.update(|window, _| window.remove_window());
            vcx.run_until_parked();
            let mut vcx = open(cx);
            frame(&mut vcx);
            let main = main(&mut vcx);
            assert_eq!(vcx.read_entity(&main, |m, _| m.first_frames()), 1);
        });
        assert_eq!(logged.lock().expect("capture lock").len(), 1, "logged once");
    }

    #[gpui_kit::test]
    fn push_main_toast_targets_main_window(cx: &mut TestAppContext) {
        setup(cx, &Opts::default());
        cx.update(|cx| push_main_toast(cx, ToastKind::Info, "x", None, None));
        let mut vcx = open(cx);
        let main = main(&mut vcx);
        let toasts = vcx.read_entity(&main, |m, _| m.toasts().clone());
        assert!(
            vcx.read_entity(&toasts, |q, _| q.visible_texts())
                .is_empty()
        );
        vcx.update(|_, cx| push_main_toast(cx, ToastKind::Info, "x", None, None));
        vcx.run_until_parked();
        assert_eq!(
            vcx.read_entity(&toasts, |q, _| q.visible_texts()),
            vec!["x".to_owned()]
        );
    }
}
