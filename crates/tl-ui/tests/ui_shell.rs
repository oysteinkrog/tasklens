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
    assert_eq!(names.len(), 87);
    assert_eq!(names.iter().collect::<HashSet<_>>().len(), 87);
    assert_eq!(names[0], "general.start_tab");
    assert_eq!(names[86], "debug.show_fps");
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
    assert_eq!(bools, 57);
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
        MenuBarFactory, MenuRowHeight, TitleBarProbe, TitleBarView, keep_above_supported,
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

    fn server(ui_scale_pct: u16) -> Opts {
        Opts {
            ui_scale_pct,
            probe: TitleBarProbe {
                client: Some(false),
                ..TitleBarProbe::default()
            },
            ..Opts::default()
        }
    }

    #[gpui_kit::test]
    fn server_mode_menu_row(cx: &mut TestAppContext) {
        let r = rig(cx, server(100));
        let bar = bounds(r.cx, "TitleBar");
        assert_eq!(bar.size.height, px(28.));
        assert_eq!(bar.size.width, px(1360.));
        let parts = ["TitleBarIcon", "TitleBarMenu", "Toolbar", "TitleBarSpacer"];
        let xs: Vec<Pixels> = parts.iter().map(|p| bounds(r.cx, p).origin.x).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "left to right: {xs:?}");
        assert_eq!(bounds(r.cx, "TitleBarIcon").origin.x, px(12.));
        let pin = bounds(r.cx, "TitleBarPin");
        assert_eq!(bounds(r.cx, "TitleBarSpacer").right(), pin.origin.x);
        assert_eq!(pin.right(), bar.right(), "the pin is the last item");
        for id in ["minimize", "maximize", "close"] {
            let selector = match id {
                "minimize" => "TitleBarCaption-minimize",
                "maximize" => "TitleBarCaption-maximize",
                _ => "TitleBarCaption-close",
            };
            assert!(r.cx.debug_bounds(selector).is_none(), "no {id} button");
        }
        assert!(r.cx.debug_bounds("TitleBarDrag").is_none());
        let captions = r.cx.update(|_, cx| r.view.read(cx).drawn_captions().len());
        assert_eq!(captions, 0);
        let height =
            r.cx.update(|_, cx| cx.try_global::<MenuRowHeight>().map(|h| h.0));
        assert_eq!(height, Some(px(24.)), "menu buttons are 24 tall");
    }

    #[gpui_kit::test]
    fn server_mode_no_drag(cx: &mut TestAppContext) {
        let r = rig(cx, server(100));
        let at = bounds(r.cx, "TitleBarSpacer").center();
        r.cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        r.cx.simulate_mouse_move(
            at + point(px(10.), px(2.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        r.cx.simulate_mouse_up(
            at + point(px(10.), px(2.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        down(r.cx, at, MouseButton::Left, 2);
        r.cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        down(r.cx, at, MouseButton::Right, 1);
        assert_eq!(probe(r.cx, |p| (p.moves, p.zooms)), (0, 0));
        assert!(probe(r.cx, |p| p.window_menus.is_empty()));
    }

    #[gpui_kit::test]
    fn server_mode_scales(cx: &mut TestAppContext) {
        let r = rig(cx, server(150));
        assert_eq!(bounds(r.cx, "TitleBar").size.height, px(42.));
        let height =
            r.cx.update(|_, cx| cx.try_global::<MenuRowHeight>().map(|h| h.0));
        assert_eq!(height, Some(px(36.)));
    }

    #[gpui_kit::test]
    fn client_bar_clears_menu_row_height(cx: &mut TestAppContext) {
        let r = rig(cx, server(100));
        r.cx.update(|_, cx| cx.global_mut::<TitleBarProbe>().client = Some(true));
        r.view.update(r.cx, |_, cx| cx.notify());
        draw(r.cx);
        assert_eq!(bounds(r.cx, "TitleBar").size.height, px(34.));
        assert!(r.cx.update(|_, cx| !cx.has_global::<MenuRowHeight>()));
    }
}

mod tabstrip {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::{
        AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement, Modifiers,
        MouseButton, ParentElement, Pixels, Point, Render, Role, Styled, TestAppContext,
        VisualTestContext, Window, div, point, px, size,
    };
    use smallvec::SmallVec;
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::table::TableMode;
    use tl_ui::actions::SelectTab;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::components::menu::context_menu_layer;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::current;
    use tl_ui::window::filter::FilterBox;
    use tl_ui::window::tabstrip::{
        AccentRuleProbe, FILTER_GAP, OVERFLOW_BUTTON, OVERFLOW_SLOT, TabStrip,
    };

    const W: f32 = 1280.;

    /// Holds the strip at the window's top and counts left presses that reach it.
    struct Host {
        strip: Entity<TabStrip>,
        downs: Rc<Cell<u32>>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let downs = self.downs.clone();
            div()
                .size_full()
                .on_mouse_down(MouseButton::Left, move |_, _, _| downs.set(downs.get() + 1))
                .child(self.strip.clone())
                .children(context_menu_layer(window, cx))
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

    /// With the real filter box in the slot the line ends at its outline, right of the
    /// "Include children" checkbox when it shows.
    #[gpui_kit::test]
    fn accent_rule_ends_at_filter_outline(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        let strip = r.strip.clone();
        r.cx.update(|window, cx| {
            let filter = cx.new(|cx| FilterBox::new(Tab::Processes, window, cx));
            strip.update(cx, |s, cx| s.set_filter_slot(Some(filter.into()), cx));
        });
        draw(r.cx);
        let set_mode = |r: &mut Rig, mode: TableMode| {
            r.cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.processes.mode = mode));
            r.cx.run_until_parked();
            draw(r.cx);
        };

        // Checkbox hidden: the outline starts at the slot's left edge, as before.
        assert!(r.cx.debug_bounds("FilterBox-include-children").is_none());
        let slot = bounds(r.cx, "TabStripFilter");
        let outline = bounds(r.cx, "FilterBox-outline");
        assert_eq!(outline.left(), slot.left());
        assert_eq!(rule(&mut r).end_x, slot.left());

        // Checkbox shown (Tree): the line ends at the outline, not at the checkbox.
        set_mode(&mut r, TableMode::Tree);
        let include = bounds(r.cx, "FilterBox-include-children");
        let slot = bounds(r.cx, "TabStripFilter");
        let outline = bounds(r.cx, "FilterBox-outline");
        assert_eq!(include.left(), slot.left());
        assert!(outline.left() > include.right());
        let shown = rule(&mut r);
        assert_eq!(shown.end_x, outline.left());
        assert_eq!(shown.path_bounds.right(), outline.left());
        assert!(has_vertex(
            &shown,
            point(outline.left(), shown.strip.bottom())
        ));

        // Hidden again: back to the slot's left edge.
        set_mode(&mut r, TableMode::Grouped);
        assert!(r.cx.debug_bounds("FilterBox-include-children").is_none());
        let slot = bounds(r.cx, "TabStripFilter");
        assert_eq!(rule(&mut r).end_x, slot.left());
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

    /// Puts the real filter box in the slot, resizes the window to `width` and draws until the
    /// tab fit has settled.
    fn with_filter_box(r: &mut Rig, width: f32) -> Entity<FilterBox> {
        let strip = r.strip.clone();
        let filter = r.cx.update(|window, cx| {
            let filter = cx.new(|cx| FilterBox::new(Tab::Processes, window, cx));
            strip.update(cx, |s, cx| {
                s.set_filter_slot(Some(filter.clone().into()), cx)
            });
            filter
        });
        resize(r, width);
        filter
    }

    fn resize(r: &mut Rig, width: f32) {
        r.cx.simulate_resize(size(px(width), px(200.)));
        settle(r.cx);
    }

    /// Draws twice: the second frame uses the widths the first one measured.
    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.run_until_parked();
            draw(cx);
        }
    }

    fn hidden(r: &mut Rig) -> Vec<Tab> {
        let strip = r.strip.clone();
        r.cx.update(|_, cx| strip.read(cx).hidden_tabs().to_vec())
    }

    fn drawn_tabs(r: &mut Rig) -> Vec<Tab> {
        Tab::ALL
            .into_iter()
            .filter(|t| r.cx.debug_bounds(selector(*t)).is_some())
            .collect()
    }

    /// The labels of the open menu's items, top to bottom.
    fn menu_items(r: &mut Rig) -> Vec<(String, Bounds<Pixels>)> {
        let mut items: Vec<_> = r.cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .filter(|s| s.role() == Some(Role::MenuItem))
                .map(|s| (s.label().unwrap_or_default().to_string(), s.bounds()))
                .collect()
        });
        items.sort_by(|a, b| f32::from(a.1.origin.y).total_cmp(&f32::from(b.1.origin.y)));
        items
    }

    fn open_overflow_menu(r: &mut Rig) -> Vec<(String, Bounds<Pixels>)> {
        let at = bounds(r.cx, "TlButton-TabStripOverflow-button").center();
        r.cx.simulate_click(at, Modifiers::none());
        r.cx.run_until_parked();
        draw(r.cx);
        menu_items(r)
    }

    #[gpui_kit::test]
    fn overflow_none_when_wide(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        with_filter_box(&mut r, 1360.);
        assert_eq!(drawn_tabs(&mut r), Tab::ALL);
        assert!(hidden(&mut r).is_empty());
        assert!(r.cx.debug_bounds("TabStripOverflow").is_none());
        // The tabs end left of the filter box with room to spare.
        let last = bounds(r.cx, "TabStrip-information");
        let filter = bounds(r.cx, "TabStripFilter");
        assert!(last.right() + px(FILTER_GAP + OVERFLOW_SLOT) <= filter.left());
    }

    #[gpui_kit::test]
    fn overflow_keeps_active_tab(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Information, false);
        with_filter_box(&mut r, 800.);

        let shown = drawn_tabs(&mut r);
        let hidden = hidden(&mut r);
        assert!(
            shown.contains(&Tab::Information),
            "active tab shown: {shown:?}"
        );
        assert!(!hidden.is_empty());
        assert!(!hidden.contains(&Tab::Information));
        let mut every: Vec<Tab> = shown.iter().chain(&hidden).copied().collect();
        every.sort_by_key(|t| *t as usize);
        assert_eq!(every, Tab::ALL, "each tab once");
        assert!(
            hidden.windows(2).all(|w| (w[0] as usize) < (w[1] as usize)),
            "R-023 order: {hidden:?}"
        );

        // Information takes the last shown place; the button sits right after it.
        let info = bounds(r.cx, "TabStrip-information");
        let shown_right = shown
            .iter()
            .map(|t| bounds(r.cx, selector(*t)).right())
            .fold(px(0.), Pixels::max);
        assert_eq!(info.right(), shown_right);
        let button = bounds(r.cx, "TlButton-TabStripOverflow-button");
        assert_eq!(button.left(), info.right());
        assert_eq!(button.size, size(px(OVERFLOW_BUTTON), px(OVERFLOW_BUTTON)));
        let filter = bounds(r.cx, "TabStripFilter");
        assert!(button.right() < filter.left());

        // The menu lists the hidden tabs in R-023 order, below the button.
        let items = open_overflow_menu(&mut r);
        let labels: Vec<&str> = items.iter().map(|(l, _)| l.as_str()).collect();
        let expected: Vec<String> = hidden
            .iter()
            .map(|t| {
                [
                    "Processes",
                    "Performance",
                    "Services",
                    "Startup Apps",
                    "Connections",
                    "Information",
                ][*t as usize]
                    .to_string()
            })
            .collect();
        assert_eq!(labels, expected);
        assert!(items[0].1.top() >= button.bottom());
        assert_eq!(r.downs.get(), 0, "the press stops at the button");
    }

    #[gpui_kit::test]
    fn overflow_menu_selects_tab(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Information, false);
        with_filter_box(&mut r, 640.);
        assert!(hidden(&mut r).contains(&Tab::Services));
        assert!(r.cx.debug_bounds("TabStrip-services").is_none());

        let items = open_overflow_menu(&mut r);
        let services = items
            .iter()
            .find(|(l, _)| l == "Services")
            .expect("Services listed")
            .1;
        r.cx.simulate_click(services.center(), Modifiers::none());
        r.cx.run_until_parked();
        assert_eq!(*r.selects.borrow(), [2]);

        // The main window answers SelectTab(2) by making Services active.
        r.strip
            .update(r.cx, |s, cx| s.set_active(Tab::Services, cx));
        settle(r.cx);
        assert!(r.cx.debug_bounds("TabStrip-services").is_some());
        assert!(!hidden(&mut r).contains(&Tab::Services));
        assert!(menu_items(&mut r).is_empty(), "the menu closed");
    }

    #[gpui_kit::test]
    fn overflow_ignores_filter_animation(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        r.cx.update(|_, cx| cx.set_reduce_motion(false));
        // A width where the wide box hides one more tab than the narrow one: the narrow fit
        // leaves less than the 120 px the box grows by.
        let filter = with_filter_box(&mut r, 1000.);
        let narrow = r.cx.update(|_, cx| filter.read(cx).width());
        let strip = r.strip.clone();
        let fit = |r: &mut Rig, filter_w: Pixels| {
            r.cx.update(|_, cx| {
                strip
                    .read(cx)
                    .visible_tabs(px(1000.) - filter_w - px(FILTER_GAP + OVERFLOW_SLOT))
                    .0
                    .to_vec()
            })
        };
        let before = fit(&mut r, narrow);
        let after = fit(&mut r, narrow + px(120.));
        assert_eq!(drawn_tabs(&mut r), before);
        assert!(after.len() < before.len(), "{before:?} then {after:?}");

        r.cx.update(|window, cx| filter.update(cx, |f, cx| f.focus_and_select_all(window, cx)));
        let mut sets = Vec::new();
        let mut widths = Vec::new();
        for _ in 0..16 {
            r.cx.executor()
                .advance_clock(std::time::Duration::from_millis(16));
            r.cx.run_until_parked();
            draw(r.cx);
            sets.push(drawn_tabs(&mut r));
            widths.push(r.cx.update(|_, cx| filter.read(cx).width()));
        }
        assert!(
            widths
                .iter()
                .any(|w| *w > narrow + px(1.) && *w < narrow + px(119.)),
            "the box animated: {widths:?}"
        );
        assert_eq!(widths.last(), Some(&(narrow + px(120.))));
        // The set changes at most once, at the start, straight to the final fit.
        assert!(sets[1..].iter().all(|s| *s == after), "{sets:?}");
        assert!(sets[0] == before || sets[0] == after, "{sets:?}");
        let target = r.cx.update(|_, cx| filter.read(cx).target_width());
        assert_eq!(target, narrow + px(120.));
    }

    /// `FilterBox::target_width` stays at the final width on every animation frame, both when
    /// the box grows and when it shrinks back, while `width` moves.
    #[gpui_kit::test]
    fn filter_target_width_during_animation(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, false);
        r.cx.update(|_, cx| cx.set_reduce_motion(false));
        let filter = with_filter_box(&mut r, 1360.);
        let narrow = r.cx.update(|_, cx| filter.read(cx).width());
        assert_eq!(r.cx.update(|_, cx| filter.read(cx).target_width()), narrow);

        let run = |r: &mut Rig, target: Pixels| {
            let mut moved = false;
            for _ in 0..16 {
                r.cx.executor()
                    .advance_clock(std::time::Duration::from_millis(16));
                r.cx.run_until_parked();
                draw(r.cx);
                let (width, at) =
                    r.cx.update(|_, cx| (filter.read(cx).width(), filter.read(cx).target_width()));
                assert_eq!(at, target, "width {width:?}");
                moved |= width != target && width != narrow && width != narrow + px(120.);
            }
            assert!(moved, "the box animated");
            assert_eq!(r.cx.update(|_, cx| filter.read(cx).width()), target);
        };

        r.cx.update(|window, cx| filter.update(cx, |f, cx| f.focus_and_select_all(window, cx)));
        run(&mut r, narrow + px(120.));

        r.cx.update(|window, cx| window.blur(cx));
        run(&mut r, narrow);
    }

    #[gpui_kit::test]
    fn overflow_reshapes_on_scale(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100, all(), Tab::Processes, true);
        settle(r.cx);
        let strip = r.strip.clone();
        let shapes = |r: &mut Rig| r.cx.update(|_, cx| strip.read(cx).label_shapes());
        let widths = |r: &mut Rig| r.cx.update(|_, cx| strip.read(cx).label_widths().to_vec());
        let first = shapes(&mut r);
        let at_100 = widths(&mut r);
        assert_eq!(first, 1, "shaped once when first drawn");
        r.strip.update(r.cx, |_, cx| cx.notify());
        settle(r.cx);
        assert_eq!(shapes(&mut r), first, "a plain re-render does not shape");

        r.cx.update(|_, cx| {
            tl_ui::theme::store::apply_metrics(
                &AppearanceSettings {
                    ui_scale_pct: 150,
                    ..AppearanceSettings::default()
                },
                cx,
            )
        });
        settle(r.cx);
        assert_eq!(shapes(&mut r), first + 1, "scale change shapes once");
        let at_150 = widths(&mut r);
        assert!(
            at_100.iter().zip(&at_150).all(|(a, b)| b.1 > a.1),
            "{at_100:?} {at_150:?}"
        );

        r.cx.update(|_, cx| tl_ui::i18n::set_test_catalogue(cx, "shell-tab-services = Dienste\n"));
        settle(r.cx);
        assert_eq!(shapes(&mut r), first + 2, "language change shapes once");
        assert_eq!(labels(&mut r)[2], "Dienste");
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

    mod health {
        use std::sync::Arc;
        use std::time::Duration;

        use gpui_kit::{Hsla, Modifiers, TestAppContext};
        use tl_model::command::Tab;
        use tl_model::health::{
            EngineCaps, HealthSnapshot, HelperState, SourceHealth, SourceId, SourceState,
        };
        use tl_ui::theme::current;

        use super::{Rig, bounds, draw, drawn, read, rig};

        const NOW: u64 = 100_000_000_000;

        fn snapshot(version: u64, states: &[(SourceId, SourceState)]) -> HealthSnapshot {
            let mut sources: [SourceHealth; SourceId::COUNT] =
                std::array::from_fn(|_| SourceHealth::default());
            for &(id, state) in states {
                sources[id.index()] = SourceHealth {
                    state,
                    last_ok_ns: Some(NOW - 12_000_000_000),
                    last_error: (state == SourceState::Failed).then(|| Arc::from("read failed")),
                    restarts: 0,
                };
            }
            HealthSnapshot {
                version,
                sources,
                helper: HelperState::Missing,
                helper_start: None,
                caps: EngineCaps::empty(),
                setup: Arc::default(),
            }
        }

        fn set_health(r: &mut Rig, health: &HealthSnapshot, tab: Tab) {
            let chip = r.chip.clone();
            r.cx.update(|_, cx| chip.update(cx, |c, cx| c.set_health(health, tab, NOW, cx)));
            draw(r.cx);
        }

        fn warning(r: &mut Rig) -> Hsla {
            r.cx.update(|_, cx| current(cx).status.warning)
        }

        #[gpui_kit::test]
        fn health_item_stale_text_and_tooltip(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, false);
            let health = snapshot(1, &[(SourceId::GpuDrm, SourceState::Stale)]);
            set_health(&mut r, &health, Tab::Performance);
            let w = warning(&mut r);
            assert_eq!(
                drawn(&mut r),
                vec![(" \u{b7} ".to_owned(), w), ("GPU data stale".to_owned(), w)]
            );
            assert!(
                read(&mut r, |c| c.is_visible()),
                "visible with no page text"
            );
            let items = read(&mut r, |c| c.health_items().to_vec());
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].source, SourceId::GpuDrm);
            assert_eq!(items[0].tooltip.as_ref(), "No new data for 12 s");
            assert!(read(&mut r, |c| c.has_stale()));
        }

        #[gpui_kit::test]
        fn health_items_limit(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, false);
            let health = snapshot(
                1,
                &[
                    (SourceId::Cpu, SourceState::Failed),
                    (SourceId::Memory, SourceState::Failed),
                    (SourceId::Disk, SourceState::Failed),
                ],
            );
            set_health(&mut r, &health, Tab::Performance);
            assert_eq!(
                read(&mut r, |c| c.drawn_text()),
                " \u{b7} CPU unavailable \u{b7} Memory unavailable \u{b7} +1 more"
            );
            let items = read(&mut r, |c| c.health_items().to_vec());
            assert_eq!(items[0].tooltip.as_ref(), "read failed");
            assert!(!read(&mut r, |c| c.has_stale()));
        }

        #[gpui_kit::test]
        fn health_items_dedupe_gpu(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, false);
            let health = snapshot(
                1,
                &[
                    (SourceId::GpuNvidia, SourceState::Stale),
                    (SourceId::GpuDrm, SourceState::Stale),
                ],
            );
            set_health(&mut r, &health, Tab::Performance);
            assert_eq!(read(&mut r, |c| c.drawn_text()), " \u{b7} GPU data stale");
            assert_eq!(read(&mut r, |c| c.health_items().len()), 1);
        }

        #[gpui_kit::test]
        fn health_items_follow_tab(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, false);
            let health = snapshot(1, &[(SourceId::SystemdSystem, SourceState::Failed)]);
            set_health(&mut r, &health, Tab::Processes);
            assert!(read(&mut r, |c| c.health_items().is_empty()));
            assert!(!read(&mut r, |c| c.is_visible()));
            set_health(&mut r, &health, Tab::Services);
            assert_eq!(
                read(&mut r, |c| c.drawn_text()),
                " \u{b7} Services unavailable"
            );
        }

        #[gpui_kit::test]
        fn health_item_opens_collector_health(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, false);
            let opened = std::rc::Rc::new(std::cell::Cell::new(0));
            let o = opened.clone();
            r.cx.update(|_, cx| {
                cx.on_action(move |_: &tl_ui::actions::OpenCollectorHealth, _| o.set(o.get() + 1));
            });
            let health = snapshot(1, &[(SourceId::Cpu, SourceState::Failed)]);
            set_health(&mut r, &health, Tab::Performance);
            let item = bounds(r.cx, "StatusChip-health-0");
            r.cx.simulate_click(item.center(), Modifiers::none());
            assert_eq!(opened.get(), 1, "one dispatch per click");

            let page = r.page.clone();
            r.cx.update(|window, cx| {
                page.focus(window, cx);
                window.focus_next(cx);
            });
            super::press(r.cx, "enter");
            assert_eq!(
                opened.get(),
                2,
                "Tab from the page reaches the item; Enter acts"
            );
        }

        #[gpui_kit::test]
        fn restricted_item_ten_seconds(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, false);
            let health = snapshot(1, &[]);
            set_health(&mut r, &health, Tab::Services);
            let chip = r.chip.clone();
            r.cx.update(|_, cx| chip.update(cx, |c, cx| c.note_restricted_refusal(42, cx)));
            draw(r.cx);
            assert_eq!(
                read(&mut r, |c| c.drawn_text()),
                " \u{b7} Restricted: 42 processes"
            );
            set_health(&mut r, &health, Tab::Processes);
            assert!(!read(&mut r, |c| c.is_visible()), "never on Processes");
            set_health(&mut r, &health, Tab::Services);
            assert!(read(&mut r, |c| c.is_visible()));

            r.cx.executor().advance_clock(Duration::from_millis(9_900));
            r.cx.run_until_parked();
            draw(r.cx);
            assert!(read(&mut r, |c| c.is_visible()), "still shown before 10 s");
            r.cx.executor().advance_clock(Duration::from_millis(100));
            r.cx.run_until_parked();
            draw(r.cx);
            assert!(!read(&mut r, |c| c.is_visible()), "gone after 10 s");
            assert!(drawn(&mut r).is_empty());
        }

        #[gpui_kit::test]
        fn health_items_no_notify_when_same(cx: &mut TestAppContext) {
            let mut r = rig(cx, 100, None, true);
            set_health(
                &mut r,
                &snapshot(1, &[(SourceId::Cpu, SourceState::Failed)]),
                Tab::Performance,
            );
            let before = read(&mut r, |c| c.renders());
            // A new version with a change the chip does not show (Sensors Late).
            set_health(
                &mut r,
                &snapshot(
                    2,
                    &[
                        (SourceId::Cpu, SourceState::Failed),
                        (SourceId::Sensors, SourceState::Late),
                    ],
                ),
                Tab::Performance,
            );
            assert_eq!(
                read(&mut r, |c| c.renders()),
                before,
                "same items: no render"
            );
            set_health(
                &mut r,
                &snapshot(3, &[(SourceId::Memory, SourceState::Failed)]),
                Tab::Performance,
            );
            assert_eq!(
                read(&mut r, |c| c.renders()),
                before + 1,
                "new items render"
            );
        }
    }
}

mod main_window {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
        Global, InteractiveElement, IntoElement, Modifiers, Pixels, Render, Styled, TestAppContext,
        VisualTestContext, Window, WindowBounds, WindowOptions, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{
        AppearanceSettings, Decorations, Settings, StartTab, ThemeSettings, UiState, WindowState,
    };
    use tl_ui::actions::{
        CloseWindow, MinimizeWindow, NextTab, PrevTab, Quit, RefreshCurrentTab, SelectTab,
        ShowMainWindow, ToggleMainWindow, ZoomWindow,
    };
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions, UiExit, UiExitSlot};
    use tl_ui::components::toast::{self, ToastKind};
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{
        self as shell, CloseOutcome, GlobalShellEvents, MainWindow, PageFactory, PageSlot,
        PauseChanged, ShellPage, ShellState, first_frame_ms_from, main_window_options,
        main_window_view, open_main_window, push_main_toast, restored_bounds, set_test_page,
    };
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    use tl_ui::window::status::StatusChanged;
    use tl_ui::window::titlebar::TitleBarProbe;
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
        /// `appearance.decorations`.
        decorations: Decorations,
        /// The decorations the window reports (`TitleBarProbe.client`). The GPUI test window
        /// reports server decorations, so the default installs the probe with client ones.
        client: Option<bool>,
        /// Counts persist calls when set.
        persisted: Option<Rc<std::cell::Cell<u32>>>,
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
                decorations: AppearanceSettings::default().decorations,
                client: Some(true),
                persisted: None,
            }
        }
    }

    /// Every Global the main window needs, with fake pages for `opts.pages`. Opens no window.
    fn setup(cx: &mut TestAppContext, opts: &Opts) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings {
                ui_scale_pct: opts.ui_scale_pct,
                decorations: opts.decorations,
                ..AppearanceSettings::default()
            };
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::i18n::init_strings(cx, "en-US");
            let mut settings = Settings {
                appearance,
                ..Settings::default()
            };
            settings.general.start_tab = opts.start_tab;
            // Tab switches are instant here; the tab_switch tests turn the animation on.
            settings.motion.tab_switch_ms = 0;
            let mut state = UiState::default();
            state.tabs.last_tab = opts.last_tab;
            let persist = opts.persisted.clone().map(|count| {
                Box::new(move |_: &Settings, _: &UiState| count.set(count.get() + 1))
                    as tl_ui::app::PersistFn
            });
            cx.set_global(BootSettings {
                settings,
                state,
                persist,
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
            if opts.client.is_some() {
                cx.set_global(TitleBarProbe {
                    client: opts.client,
                    ..TitleBarProbe::default()
                });
            }
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

    fn toast_top(vcx: &mut VisualTestContext) -> Pixels {
        let main = main(vcx);
        let toasts = vcx.read_entity(&main, |m, _| m.toasts().clone());
        vcx.read_entity(&toasts, |q, _| q.top())
    }

    #[gpui_kit::test]
    fn server_mode_layout_and_toast_top(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                pages: &[Tab::Processes],
                decorations: Decorations::Server,
                client: Some(false),
                ..Opts::default()
            },
        );
        assert_eq!(
            vcx.update(|window, _| window.viewport_size()),
            size(px(1280.), px(800.))
        );
        assert_eq!(bounds(&mut vcx, "TitleBar").size.height, px(28.));
        assert_eq!(bounds(&mut vcx, "TabStrip").origin.y, px(28.));
        assert_eq!(toast_top(&mut vcx), px(66.));
    }

    #[gpui_kit::test]
    fn layout_follows_reported_decorations(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                pages: &[Tab::Processes],
                decorations: Decorations::Server,
                client: Some(true),
                ..Opts::default()
            },
        );
        assert_eq!(bounds(&mut vcx, "TitleBar").size.height, px(34.));
        assert!(vcx.debug_bounds("TitleBarCaption-close").is_some());
        assert!(vcx.debug_bounds("TitleBarDrag").is_some());
        assert_eq!(bounds(&mut vcx, "TabStrip").origin.y, px(34.));
        assert_eq!(toast_top(&mut vcx), px(72.));

        // A configure event that switches the kind re-renders the cached bar and moves the toasts.
        vcx.update(|_, cx| cx.global_mut::<TitleBarProbe>().client = Some(false));
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        draw(&mut vcx);
        vcx.run_until_parked();
        assert_eq!(bounds(&mut vcx, "TitleBar").size.height, px(28.));
        assert!(vcx.debug_bounds("TitleBarCaption-close").is_none());
        assert_eq!(bounds(&mut vcx, "TabStrip").origin.y, px(28.));
        assert_eq!(toast_top(&mut vcx), px(66.));
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
                pages: &[Tab::Processes, Tab::Services],
                // Startup has no page arm; Performance has one since E24-A1.
                start_tab: StartTab::Startup,
                ..Opts::default()
            },
        );
        assert_eq!(active(&mut vcx), Tab::Processes);
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

        // Information has a real page arm (tasklens-9o0.27), so it needs the engine globals.
        let mut vcx = rig(
            cx_b,
            Opts {
                pages: &[Tab::Processes, Tab::Performance],
                start_tab: StartTab::Performance,
                engine: true,
                ..Opts::default()
            },
        );
        act(&mut vcx, NextTab);
        assert_eq!(
            active(&mut vcx),
            Tab::Information,
            "Services, Startup and Connections are skipped"
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

    /// Reads through the app, so it works after the main window is removed.
    fn exit(cx: &TestAppContext) -> Option<UiExit> {
        cx.read(|cx| cx.try_global::<UiExitSlot>().and_then(|s| s.0.clone()))
    }

    fn saved(vcx: &mut VisualTestContext) -> WindowState {
        vcx.update(|_, cx| TlSettings::state(cx).window.clone())
    }

    fn state_version(vcx: &mut VisualTestContext) -> u64 {
        vcx.update(|_, cx| TlSettings::get(cx).state_version())
    }

    fn wait(vcx: &mut VisualTestContext, ms: u64) {
        vcx.executor().advance_clock(Duration::from_millis(ms));
        vcx.run_until_parked();
    }

    fn resize(vcx: &mut VisualTestContext, w: f32, h: f32) {
        vcx.simulate_resize(size(px(w), px(h)));
        vcx.run_until_parked();
    }

    fn probe<R>(vcx: &mut VisualTestContext, f: impl FnOnce(&mut TitleBarProbe) -> R) -> R {
        vcx.update(|_, cx| f(cx.default_global::<TitleBarProbe>()))
    }

    /// Removes the main window through `apply_close(RemoveWindow)`.
    fn remove_main(vcx: &mut VisualTestContext) {
        let main = main(vcx);
        vcx.update(|window, cx| {
            main.update(cx, |m, cx| {
                m.apply_close_for_test(CloseOutcome::RemoveWindow, window, cx)
            })
        });
        // The removed window stops being the platform's active window, as on a desktop.
        vcx.deactivate_window();
        vcx.run_until_parked();
    }

    fn window_state(w: u32, h: u32, maximized: bool) -> WindowState {
        WindowState {
            width: w,
            height: h,
            maximized,
        }
    }

    #[gpui_kit::test]
    fn close_quits_by_default(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        assert_eq!(exit(&vcx), None);
        act(&mut vcx, CloseWindow);
        assert_eq!(exit(&vcx), Some(UiExit::Quit));
    }

    #[gpui_kit::test]
    fn close_to_tray_without_tray_quits(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        vcx.update(|_, cx| TlSettings::update(cx, |d| d.settings.tray.close_to_tray = true));
        act(&mut vcx, CloseWindow);
        assert_eq!(exit(&vcx), Some(UiExit::Quit));
    }

    #[gpui_kit::test]
    fn compositor_close_follows_decision(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        resize(&mut vcx, 900., 600.);
        assert!(
            !vcx.simulate_close(),
            "tasklens decides; the platform never closes the window"
        );
        vcx.run_until_parked();
        assert_eq!(exit(&vcx), Some(UiExit::Quit));
        assert_eq!(saved(&mut vcx), window_state(900, 600, false));
    }

    #[gpui_kit::test]
    fn quit_always_quits(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.tray.close_to_tray = true;
                d.settings.tray.minimize_to_tray = true;
            })
        });
        resize(&mut vcx, 1000., 700.);
        act(&mut vcx, Quit);
        assert_eq!(exit(&vcx), Some(UiExit::Quit));
        assert_eq!(saved(&mut vcx), window_state(1000, 700, false));
    }

    #[gpui_kit::test]
    fn close_saves_size_first(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let before = state_version(&mut vcx);
        resize(&mut vcx, 1000., 700.);
        // No clock advance: the close writes at once, not after the debounce.
        act(&mut vcx, CloseWindow);
        assert_eq!(saved(&mut vcx), window_state(1000, 700, false));
        assert_eq!(state_version(&mut vcx), before + 1);
        assert_eq!(exit(&vcx), Some(UiExit::Quit));
        wait(&mut vcx, 1000);
        assert_eq!(
            state_version(&mut vcx),
            before + 1,
            "the pending write was dropped"
        );
    }

    #[gpui_kit::test]
    fn geometry_save_debounced(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let before = state_version(&mut vcx);
        resize(&mut vcx, 1000., 700.);
        wait(&mut vcx, 100);
        resize(&mut vcx, 1100., 720.4);
        wait(&mut vcx, 100);
        resize(&mut vcx, 1200., 740.6);
        wait(&mut vcx, 499);
        assert_eq!(state_version(&mut vcx), before, "nothing before 500 ms");
        wait(&mut vcx, 1);
        assert_eq!(state_version(&mut vcx), before + 1, "written once");
        assert_eq!(saved(&mut vcx), window_state(1200, 741, false));
        wait(&mut vcx, 1000);
        assert_eq!(state_version(&mut vcx), before + 1);
    }

    #[gpui_kit::test]
    fn maximized_state_saved(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        probe(&mut vcx, |_| ());
        act(&mut vcx, ZoomWindow);
        assert_eq!(probe(&mut vcx, |p| p.zooms), 1);
        // The compositor maximizes the window and sends the new size.
        probe(&mut vcx, |p| p.maximized = Some(true));
        resize(&mut vcx, 1600., 900.);
        wait(&mut vcx, 500);
        assert_eq!(saved(&mut vcx), window_state(1600, 900, true));
    }

    #[gpui_kit::test]
    fn minimize_button_minimizes(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        probe(&mut vcx, |_| ());
        act(&mut vcx, MinimizeWindow);
        assert_eq!(probe(&mut vcx, |p| p.minimizes), 1);
        assert_eq!(exit(&vcx), None);
    }

    #[gpui_kit::test]
    fn show_main_window_reopens(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                start_tab: StartTab::Processes,
                ..Opts::default()
            },
        );
        act(&mut vcx, SelectTab(2));
        remove_main(&mut vcx);
        cx.read(|cx| {
            let state = cx.global::<ShellState>();
            assert!(state.main_window.is_none() && state.main_view.is_none());
            assert!(cx.windows().is_empty());
        });
        assert_eq!(exit(&vcx), None, "removing the window does not quit");

        cx.update(|cx| cx.dispatch_action(&ShowMainWindow));
        cx.run_until_parked();
        let handle = cx
            .read(|cx| cx.global::<ShellState>().main_window)
            .expect("main window reopened");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        draw(&mut vcx);
        assert_eq!(
            active(&mut vcx),
            Tab::Services,
            "reopens on last_tab, not general.start_tab"
        );
        assert!(vcx.update(|window, _| window.is_window_active()));
    }

    #[gpui_kit::test]
    fn toggle_main_window_activates_inactive(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        vcx.deactivate_window();
        assert!(!vcx.update(|window, _| window.is_window_active()));
        vcx.update(|_, cx| cx.dispatch_action(&ToggleMainWindow));
        vcx.run_until_parked();
        assert!(vcx.update(|window, _| window.is_window_active()));
    }

    #[gpui_kit::test]
    fn toggle_main_window_minimizes_active(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        probe(&mut vcx, |_| ());
        assert!(vcx.update(|window, _| window.is_window_active()));
        act(&mut vcx, ToggleMainWindow);
        assert_eq!(probe(&mut vcx, |p| p.minimizes), 1);
        assert!(main_window_view_exists(&mut vcx));
        assert_eq!(exit(&vcx), None);
    }

    fn main_window_view_exists(vcx: &mut VisualTestContext) -> bool {
        vcx.update(|_, cx| main_window_view(cx).is_some())
    }

    #[gpui_kit::test]
    fn no_frames_when_idle(cx: &mut TestAppContext) {
        let mut vcx = rig(
            cx,
            Opts {
                engine: true,
                ..Opts::default()
            },
        );
        frame(&mut vcx);
        frame(&mut vcx);
        wait(&mut vcx, 1000);
        let driver = vcx.update(|window, cx| AnimationDriver::global(window, cx));
        let requested = vcx.read_entity(&driver, |d, _| d.stats.requested);
        for _ in 0..10 {
            wait(&mut vcx, 100);
        }
        assert_eq!(
            vcx.read_entity(&driver, |d, _| d.stats.requested),
            requested,
            "no frame requested while idle"
        );
        assert!(!vcx.read_entity(&driver, |d, _| d.is_armed()));
    }

    fn set_show_fps(vcx: &mut VisualTestContext, on: bool) {
        vcx.update(|_, cx| TlSettings::update(cx, |d| d.settings.debug.show_fps = on));
        vcx.run_until_parked();
        draw(vcx);
    }

    /// `(overlay shown, overlay drawn, driver sampling)`.
    fn fps_state(vcx: &mut VisualTestContext) -> (bool, bool, bool) {
        let main = main(vcx);
        let driver = vcx.update(|window, cx| AnimationDriver::global(window, cx));
        let shown = vcx.update(|_, cx| main.read(cx).fps_overlay().read(cx).is_shown());
        let drawn = vcx.debug_bounds("FpsOverlay").is_some();
        let sampling = vcx.read_entity(&driver, |d, _| d.stats.sampling);
        (shown, drawn, sampling)
    }

    #[gpui_kit::test]
    fn fps_overlay_hidden_by_default(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        assert_eq!(fps_state(&mut vcx), (false, false, false));
    }

    #[test]
    fn fps_text_format() {
        let mut cpu: Vec<u64> = [500_000; 36].into_iter().chain([700_000; 36]).collect();
        // Out of order: the percentiles sort the slice.
        cpu.reverse();
        assert_eq!(
            shell::fps_text(72, Duration::from_millis(500), &mut cpu, 0),
            "144 fps · frame p50 0.50 ms · p99 0.70 ms · stalls 0"
        );
    }

    #[test]
    fn fps_text_idle() {
        assert_eq!(
            shell::fps_text(0, Duration::from_millis(500), &mut [], 2),
            "idle · stalls 2"
        );
    }

    #[gpui_kit::test]
    fn fps_overlay_requests_no_frames(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        set_show_fps(&mut vcx, true);
        assert_eq!(fps_state(&mut vcx), (true, true, true));
        let driver = vcx.update(|window, cx| AnimationDriver::global(window, cx));
        let requested = vcx.read_entity(&driver, |d, _| d.stats.requested);
        for _ in 0..4 {
            wait(&mut vcx, 500);
            draw(&mut vcx);
        }
        assert_eq!(
            vcx.read_entity(&driver, |d, _| d.stats.requested),
            requested,
            "the overlay requested a frame"
        );
        let main = main(&mut vcx);
        let text = vcx.update(|_, cx| main.read(cx).fps_overlay().read(cx).text().to_string());
        assert_eq!(text, "idle · stalls 0");
    }

    #[gpui_kit::test]
    fn fps_overlay_stops_when_off(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        set_show_fps(&mut vcx, true);
        assert_eq!(fps_state(&mut vcx), (true, true, true));
        set_show_fps(&mut vcx, false);
        assert_eq!(fps_state(&mut vcx), (false, false, false));
        // No timer is left: time passing changes nothing.
        wait(&mut vcx, 2000);
        assert_eq!(fps_state(&mut vcx), (false, false, false));
    }

    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn open_child(cx: &mut TestAppContext, key: WindowKey) -> AnyWindowHandle {
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.update(cx, |registry, cx| {
                registry.open_or_focus(key.clone(), cx, |cx| {
                    cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Blank))
                        .map(Into::into)
                })
            });
            registry.read(cx).get(&key).expect("child registered")
        })
    }

    fn is_open(cx: &mut TestAppContext, handle: AnyWindowHandle) -> bool {
        cx.read(|cx| {
            cx.windows()
                .iter()
                .any(|w| w.window_id() == handle.window_id())
        })
    }

    #[gpui_kit::test]
    fn remove_window_closes_children(cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
        for (cx, keep_detached) in [(cx_a, false), (cx_b, true)] {
            setup(cx, &Opts::default());
            cx.update(|cx| {
                GlobalRegistry::init(cx);
                TlSettings::update(cx, |d| {
                    d.settings.graphs.keep_detached_on_close = keep_detached
                });
            });
            let mut vcx = open(cx);
            let about = open_child(cx, WindowKey::About);
            let graph = open_child(cx, WindowKey::DetachedGraph(0));
            remove_main(&mut vcx);
            cx.run_until_parked();
            assert!(!is_open(cx, about), "a child closes with the main window");
            assert_eq!(
                is_open(cx, graph),
                keep_detached,
                "a detached graph stays only with keep_detached_on_close"
            );
            assert_eq!(exit(cx), None);
        }
    }

    /// Records every `PauseChanged` on `GlobalShellEvents`.
    fn pause_events(vcx: &mut VisualTestContext) -> Rc<RefCell<Vec<bool>>> {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        vcx.update(|_, cx| {
            let events = cx.global::<GlobalShellEvents>().0.clone();
            cx.subscribe(&events, move |_, ev: &PauseChanged, _| {
                sink.borrow_mut().push(ev.0)
            })
            .detach();
        });
        seen
    }

    fn modifiers(vcx: &mut VisualTestContext, control: bool, shift: bool) {
        vcx.simulate_modifiers_change(Modifiers {
            control,
            shift,
            ..Modifiers::default()
        });
        vcx.run_until_parked();
        draw(vcx);
    }

    fn paused(vcx: &mut VisualTestContext) -> bool {
        vcx.update(|_, cx| cx.global::<ShellState>().paused)
    }

    #[gpui_kit::test]
    fn ctrl_pause_starts_and_ends(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let seen = pause_events(&mut vcx);
        modifiers(&mut vcx, true, false);
        assert!(paused(&mut vcx));
        assert_eq!(*seen.borrow(), [true]);
        assert_eq!(chip_text(&mut vcx), "[PAUSED - Ctrl]");
        modifiers(&mut vcx, false, false);
        assert!(!paused(&mut vcx));
        assert_eq!(*seen.borrow(), [true, false]);
        assert!(!chip_text(&mut vcx).contains("PAUSED"));
    }

    #[gpui_kit::test]
    fn ctrl_pause_releases_on_deactivate(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let seen = pause_events(&mut vcx);
        modifiers(&mut vcx, true, false);
        vcx.deactivate_window();
        vcx.run_until_parked();
        assert!(!paused(&mut vcx));
        assert_eq!(*seen.borrow(), [true, false]);
        draw(&mut vcx);
        assert!(!chip_text(&mut vcx).contains("PAUSED"));
    }

    #[gpui_kit::test]
    fn ctrl_pause_needs_active_window(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let seen = pause_events(&mut vcx);
        vcx.deactivate_window();
        modifiers(&mut vcx, true, false);
        assert!(!paused(&mut vcx));
        assert!(seen.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn ctrl_pause_emits_once(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let seen = pause_events(&mut vcx);
        modifiers(&mut vcx, true, false);
        modifiers(&mut vcx, true, true);
        assert!(paused(&mut vcx));
        assert_eq!(*seen.borrow(), [true]);
    }

    /// UI-PRC-017: Ctrl with another modifier already down does not start a pause.
    #[gpui_kit::test]
    fn ctrl_with_other_modifier_does_not_pause(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let seen = pause_events(&mut vcx);
        modifiers(&mut vcx, false, true);
        modifiers(&mut vcx, true, true);
        assert!(!paused(&mut vcx), "Shift, then Ctrl");
        modifiers(&mut vcx, false, false);
        assert!(seen.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn remove_window_ends_ctrl_pause(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let seen = pause_events(&mut vcx);
        modifiers(&mut vcx, true, false);
        assert!(paused(&mut vcx));
        // No deactivation follows: the removed window's own observer is gone.
        let main = main(&mut vcx);
        vcx.update(|window, cx| {
            main.update(cx, |m, cx| {
                m.apply_close_for_test(CloseOutcome::RemoveWindow, window, cx)
            })
        });
        vcx.run_until_parked();
        // The window is gone, so read through the app.
        assert!(!vcx.cx.read(|cx| cx.global::<ShellState>().paused));
        assert_eq!(*seen.borrow(), [true, false]);
    }

    /// UI scale keys and the metrics observer (E20-013).
    mod scale {
        use std::cell::Cell;
        use std::rc::Rc;

        use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext, px};
        use tl_model::command::Tab;
        use tl_ui::actions::{ScaleDown, ScaleReset, ScaleUp};
        use tl_ui::settings_global::TlSettings;
        use tl_ui::theme::metrics;

        use super::{Opts, act, bounds, draw, main, rig, toast_top};

        fn scale_rig(cx: &mut TestAppContext, ui_scale_pct: u16) -> VisualTestContext {
            rig(
                cx,
                Opts {
                    ui_scale_pct,
                    pages: &[Tab::Processes],
                    ..Opts::default()
                },
            )
        }

        fn pct(vcx: &mut VisualTestContext) -> u16 {
            vcx.update(|_, cx| TlSettings::settings(cx).appearance.ui_scale_pct)
        }

        fn toasts(vcx: &mut VisualTestContext) -> Vec<String> {
            let main = main(vcx);
            let queue = vcx.read_entity(&main, |m, _| m.toasts().clone());
            vcx.read_entity(&queue, |q, _| q.visible_texts())
                .into_iter()
                .map(|t| t.to_string())
                .collect()
        }

        fn style_version(vcx: &mut VisualTestContext) -> u64 {
            vcx.update(|_, cx| metrics(cx).style_version)
        }

        #[gpui_kit::test]
        fn scale_keys_step_and_clamp(cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
            let mut vcx = scale_rig(cx_a, 195);
            act(&mut vcx, ScaleUp);
            assert_eq!(pct(&mut vcx), 200);
            act(&mut vcx, ScaleUp);
            assert_eq!(pct(&mut vcx), 200);
            assert_eq!(toasts(&mut vcx), ["UI scale changed to 200%"]);

            let mut vcx = scale_rig(cx_b, 75);
            act(&mut vcx, ScaleDown);
            assert_eq!(pct(&mut vcx), 75);
            assert!(toasts(&mut vcx).is_empty());
        }

        #[gpui_kit::test]
        fn scale_reset_toast(cx: &mut TestAppContext) {
            let mut vcx = scale_rig(cx, 130);
            act(&mut vcx, ScaleReset);
            assert_eq!(pct(&mut vcx), 100);
            assert_eq!(toasts(&mut vcx), ["UI scale changed to 100%"]);
        }

        #[gpui_kit::test]
        fn scale_toast_replaced(cx: &mut TestAppContext) {
            let mut vcx = scale_rig(cx, 100);
            for _ in 0..3 {
                act(&mut vcx, ScaleUp);
            }
            assert_eq!(pct(&mut vcx), 115);
            assert_eq!(toasts(&mut vcx), ["UI scale changed to 115%"]);
        }

        #[gpui_kit::test]
        fn row_height_metrics(cx: &mut TestAppContext) {
            let mut vcx = scale_rig(cx, 100);
            vcx.update(|_, cx| {
                TlSettings::update(cx, |d| {
                    d.settings.appearance.row_height_pct = 120;
                    d.settings.appearance.ui_scale_pct = 150;
                })
            });
            vcx.run_until_parked();
            let row = vcx.update(|_, cx| f32::from(metrics(cx).row_height));
            assert!((row - 50.4).abs() < 1e-4, "row height {row}");
        }

        #[gpui_kit::test]
        fn toast_top_follows_tab_strip(cx: &mut TestAppContext) {
            let mut vcx = scale_rig(cx, 100);
            assert_eq!(toast_top(&mut vcx), px(72.));
            vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.appearance.ui_scale_pct = 150)
            });
            vcx.run_until_parked();
            draw(&mut vcx);
            assert_eq!(toast_top(&mut vcx), px(108.));
            assert_eq!(bounds(&mut vcx, "TitleBar").size.height, px(51.));
        }

        #[gpui_kit::test]
        fn scale_preview_not_persisted(cx: &mut TestAppContext) {
            let persisted = Rc::new(Cell::new(0));
            let mut vcx = rig(
                cx,
                Opts {
                    pages: &[Tab::Processes],
                    persisted: Some(persisted.clone()),
                    ..Opts::default()
                },
            );
            let before = persisted.get();
            vcx.update(|_, cx| {
                TlSettings::preview(cx, |d| d.settings.appearance.ui_scale_pct = 125)
            });
            vcx.run_until_parked();
            assert_eq!(vcx.update(|_, cx| metrics(cx).ui_scale), 1.25);
            assert_eq!(persisted.get(), before);
        }

        #[gpui_kit::test]
        fn scale_factor_bumps_style_version(cx: &mut TestAppContext) {
            let mut vcx = scale_rig(cx, 100);
            let main = main(&mut vcx);
            let report = |vcx: &mut VisualTestContext, factor: f32| {
                vcx.update(|_, cx| {
                    main.update(cx, |m, cx| m.scale_factor_changed_for_test(factor, cx))
                });
                vcx.run_until_parked();
            };
            report(&mut vcx, 1.1);
            let before = style_version(&mut vcx);
            report(&mut vcx, 2.0);
            assert_eq!(style_version(&mut vcx), before + 1);
            report(&mut vcx, 2.0);
            assert_eq!(style_version(&mut vcx), before + 1, "same factor, no bump");
            assert_eq!(pct(&mut vcx), 100);
        }
    }

    /// The shell filter box in the main window (E20-009).
    mod filter {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::time::Duration;

        use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext, px};
        use tl_model::command::Tab;
        use tl_model::table::TableMode;
        use tl_ui::actions::{ClearFilter, FocusFilter, SelectTab};
        use tl_ui::app::UiExit;
        use tl_ui::components::input::{FILTER_WIDTH, FILTER_WIDTH_WIDE};
        use tl_ui::settings_global::TlSettings;
        use tl_ui::theme::current;
        use tl_ui::window::filter::{
            FILTER_DEBOUNCE, FilterBox, include_children_visible, shown_on,
        };
        use tl_ui::window::main::{FilterChanged, GlobalShellEvents};

        use super::{Opts, act, draw, exit, main, page, rig};

        type Events = Rc<RefCell<Vec<(Tab, String)>>>;

        struct Rig {
            vcx: VisualTestContext,
            events: Events,
        }

        /// The main window on Processes with every fake page, recording `FilterChanged`.
        fn filter_rig(cx: &mut TestAppContext) -> Rig {
            let mut vcx = rig(cx, Opts::default());
            let events: Events = Rc::default();
            let seen = events.clone();
            vcx.update(|_, cx| {
                let bus = cx.global::<GlobalShellEvents>().0.clone();
                cx.subscribe(&bus, move |_, e: &FilterChanged, _| {
                    seen.borrow_mut().push((e.tab, e.text.to_string()));
                })
                .detach();
            });
            Rig { vcx, events }
        }

        impl Rig {
            fn filter(&mut self) -> Entity<FilterBox> {
                let main = main(&mut self.vcx);
                self.vcx.read_entity(&main, |m, _| m.filter_box().clone())
            }

            fn input_text(&mut self) -> String {
                let f = self.filter();
                self.vcx
                    .update(|_, cx| f.read(cx).input().read(cx).text().to_string())
            }

            fn focus_filter(&mut self) {
                let f = self.filter();
                self.vcx.update(|window, cx| {
                    let input = f.read(cx).input().clone();
                    input.update(cx, |s, cx| s.focus(window, cx));
                });
                draw(&mut self.vcx);
            }

            fn page_focus(&mut self) -> gpui_kit::FocusHandle {
                let tab = super::active(&mut self.vcx);
                let page = page(&mut self.vcx, tab).expect("page built");
                self.vcx.read_entity(&page, |p, _| p.focus.clone())
            }

            fn focus_page(&mut self) {
                let focus = self.page_focus();
                self.vcx.update(|window, cx| window.focus(&focus, cx));
                draw(&mut self.vcx);
            }

            fn filter_focused(&mut self) -> bool {
                let f = self.filter();
                self.vcx
                    .update(|window, cx| f.read(cx).is_focused(window, cx))
            }

            fn page_focused(&mut self) -> bool {
                let focus = self.page_focus();
                self.vcx.update(|window, _| focus.is_focused(window))
            }

            fn wait(&mut self, ms: u64) {
                self.vcx.executor().advance_clock(Duration::from_millis(ms));
                self.vcx.run_until_parked();
            }

            fn events(&self) -> Vec<(Tab, String)> {
                self.events.borrow().clone()
            }

            /// Types `text` into the filter, lets the debounce fire and forgets the events.
            fn type_settled(&mut self, text: &str) {
                self.focus_filter();
                self.vcx.simulate_input(text);
                self.wait(200);
                draw(&mut self.vcx);
                self.events.borrow_mut().clear();
            }

            fn select(&mut self, tab: Tab) {
                act(&mut self.vcx, SelectTab(tab as usize));
            }

            fn set_mode(&mut self, mode: TableMode) {
                self.vcx
                    .update(|_, cx| TlSettings::update(cx, |d| d.settings.processes.mode = mode));
                self.vcx.run_until_parked();
                draw(&mut self.vcx);
            }

            fn include_shown(&mut self) -> bool {
                self.vcx
                    .debug_bounds("FilterBox-include-children")
                    .is_some()
            }

            fn include_children(&mut self) -> bool {
                self.vcx
                    .update(|_, cx| TlSettings::settings(cx).processes.filter_include_children)
            }
        }

        /// The main window on Processes in Tree mode, with the `ToggleSetting` handler.
        fn tree_rig(cx: &mut TestAppContext) -> Rig {
            let mut r = filter_rig(cx);
            r.vcx
                .update(|_, cx| tl_ui::actions::register_global_handlers(cx));
            r.set_mode(TableMode::Tree);
            r
        }

        #[gpui_kit::test]
        fn filter_debounce_100ms(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.focus_filter();
            for (i, ch) in ["f", "i", "r"].into_iter().enumerate() {
                if i > 0 {
                    r.wait(30);
                }
                r.vcx.simulate_input(ch);
            }
            assert!(r.events().is_empty(), "nothing before the quiet time");
            r.wait(FILTER_DEBOUNCE.as_millis() as u64 - 1);
            assert!(r.events().is_empty(), "nothing at 99 ms");
            r.wait(1);
            assert_eq!(r.events(), [(Tab::Processes, "fir".to_owned())]);
            r.wait(500);
            assert_eq!(r.events().len(), 1, "exactly one");
        }

        #[gpui_kit::test]
        fn filter_esc_rule(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.type_settled("fire");
            assert!(r.filter_focused());

            // Esc with text runs FilterEscape, not the kit's clear (which would debounce).
            r.vcx.simulate_keystrokes("escape");
            assert_eq!(r.input_text(), "");
            assert_eq!(r.events(), [(Tab::Processes, String::new())], "at once");
            assert!(r.page_focused());
            r.wait(200);
            assert_eq!(r.events().len(), 1, "no second empty query");

            // Empty filter with focus: Esc only moves focus.
            r.events.borrow_mut().clear();
            r.focus_filter();
            r.vcx.simulate_keystrokes("escape");
            assert!(r.page_focused());
            assert!(r.events().is_empty());
            // A further Esc reaches the window's Esc chain: with "Close with Esc" on, it quits.
            r.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.window.close_with_esc = true)
            });
            r.vcx.simulate_keystrokes("escape");
            r.vcx.run_until_parked();
            assert_eq!(
                exit(&r.vcx),
                Some(UiExit::Quit),
                "a further Esc reaches EscapeChain"
            );
        }

        #[gpui_kit::test]
        fn filter_enter_moves_focus(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.type_settled("ssh");
            r.vcx.simulate_keystrokes("enter");
            assert!(r.page_focused());
            assert_eq!(r.input_text(), "ssh");

            r.focus_filter();
            r.vcx.simulate_keystrokes("down");
            assert!(r.page_focused(), "Down does the same");
            assert_eq!(r.input_text(), "ssh");
            r.wait(200);
            assert!(r.events().is_empty());
        }

        #[gpui_kit::test]
        fn filter_per_tab_text(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.type_settled("ssh");
            r.select(Tab::Services);
            assert_eq!(r.input_text(), "");
            r.select(Tab::Processes);
            assert_eq!(r.input_text(), "ssh");
            let f = r.filter();
            r.vcx.read_entity(&f, |f, _| {
                assert_eq!(f.text(Tab::Processes).as_ref(), "ssh");
                assert_eq!(f.text(Tab::Services).as_ref(), "");
            });
            r.wait(200);
            assert!(r.events().is_empty(), "the swap emits nothing");
        }

        #[gpui_kit::test]
        fn filter_hidden_on_performance(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            assert!(r.vcx.debug_bounds("FilterBox").is_some());
            assert_eq!(Tab::ALL.iter().filter(|t| !shown_on(**t)).count(), 1);
            r.select(Tab::Performance);
            assert!(r.vcx.debug_bounds("FilterBox").is_none());
            assert!(r.vcx.debug_bounds("TabStripFilter").is_none());
            r.focus_page();
            act(&mut r.vcx, FocusFilter);
            assert!(!r.filter_focused(), "FocusFilter does nothing");
            assert!(r.page_focused());
        }

        #[gpui_kit::test]
        fn filter_ctrl_f_selects_all(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.type_settled("abc");
            r.focus_page();
            r.vcx.simulate_keystrokes("ctrl-f");
            draw(&mut r.vcx);
            assert!(r.filter_focused());
            let f = r.filter();
            let range = r
                .vcx
                .update(|_, cx| f.read(cx).input().read(cx).selected_range());
            assert_eq!(range, 0..3);
        }

        #[gpui_kit::test]
        fn clear_filter_action(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.type_settled("abc");
            r.focus_page();
            act(&mut r.vcx, ClearFilter);
            assert_eq!(r.input_text(), "");
            assert_eq!(r.events(), [(Tab::Processes, String::new())]);
            r.wait(200);
            assert_eq!(r.events().len(), 1, "emitted once");
            assert!(r.page_focused());
        }

        #[gpui_kit::test]
        fn filter_width_on_focus(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            r.vcx
                .update(|_, cx| TlSettings::update(cx, |d| d.settings.motion.animations = false));
            r.focus_page();
            let width = |vcx: &mut VisualTestContext| {
                draw(vcx);
                vcx.debug_bounds("FilterBox-input")
                    .expect("input drawn")
                    .size
                    .width
            };
            assert_eq!(width(&mut r.vcx), px(FILTER_WIDTH));
            r.focus_filter();
            assert_eq!(
                width(&mut r.vcx),
                px(FILTER_WIDTH_WIDE),
                "instant with animations off"
            );
            let f = r.filter();
            let outlined = r.vcx.read_entity(&f, |f, _| f.width());
            let drawn = r.vcx.debug_bounds("FilterBox").expect("box drawn");
            assert_eq!(outlined, drawn.size.width);
            assert_eq!(drawn.right(), px(1280.), "flush with the right edge");
        }

        #[gpui_kit::test]
        fn filter_inactive_outline(cx: &mut TestAppContext) {
            let mut r = filter_rig(cx);
            let f = r.filter();
            let (accent, divider) = r
                .vcx
                .update(|_, cx| (current(cx).accent, current(cx).divider));
            assert_eq!(r.vcx.read_entity(&f, |f, _| f.outline_color()), accent);
            r.vcx.deactivate_window();
            r.vcx.run_until_parked();
            draw(&mut r.vcx);
            assert_eq!(r.vcx.read_entity(&f, |f, _| f.outline_color()), divider);
        }

        #[gpui_kit::test]
        fn include_children_visibility(cx: &mut TestAppContext) {
            use TableMode::{Flat, Grouped, Tree};
            assert!(!include_children_visible(Tab::Processes, "", Grouped));
            assert!(include_children_visible(Tab::Processes, "", Tree));
            assert!(include_children_visible(Tab::Processes, "ssh", Grouped));
            assert!(!include_children_visible(Tab::Services, "ssh", Grouped));
            assert!(!include_children_visible(Tab::Services, "", Tree));
            assert!(!include_children_visible(Tab::Processes, "", Flat));

            let mut r = filter_rig(cx);
            assert!(!r.include_shown(), "grouped, empty filter");
            r.set_mode(TableMode::Tree);
            assert!(r.include_shown(), "tree, empty filter");

            // Drawn left of the outline, 16 px away, and counted in the box width.
            let include = r.vcx.debug_bounds("FilterBox-include-children").unwrap();
            let outline = r.vcx.debug_bounds("FilterBox-outline").unwrap();
            let drawn = r.vcx.debug_bounds("FilterBox").unwrap();
            assert_eq!(include.right() + px(16.), outline.left());
            assert_eq!(drawn.left(), include.left());
            let f = r.filter();
            assert_eq!(r.vcx.read_entity(&f, |f, _| f.width()), drawn.size.width);

            r.set_mode(TableMode::Grouped);
            assert!(!r.include_shown());
            r.type_settled("ssh");
            assert!(r.include_shown(), "grouped, filter ssh");

            r.select(Tab::Services);
            r.type_settled("ssh");
            assert!(!r.include_shown(), "Services, filter ssh");
        }

        #[gpui_kit::test]
        fn include_children_toggles_setting(cx: &mut TestAppContext) {
            let mut r = tree_rig(cx);
            assert!(!r.include_children());
            assert!(r.vcx.debug_bounds("TlCheckbox-check").is_none());
            let label = r
                .vcx
                .debug_bounds("FilterBox-include-children-label")
                .expect("label drawn");
            r.vcx.simulate_click(label.center(), Modifiers::none());
            r.vcx.run_until_parked();
            draw(&mut r.vcx);
            assert!(r.include_children());
            assert!(
                r.vcx.debug_bounds("TlCheckbox-check").is_some(),
                "the box shows checked"
            );
        }

        #[gpui_kit::test]
        fn include_children_tab_stop(cx: &mut TestAppContext) {
            let mut r = tree_rig(cx);
            r.focus_filter();
            let next = |r: &mut Rig| {
                r.vcx.simulate_keystrokes("tab");
                r.vcx.run_until_parked();
                draw(&mut r.vcx);
            };
            next(&mut r);
            assert!(!r.filter_focused());
            assert!(!r.page_focused());
            r.vcx.simulate_keystrokes("space");
            r.vcx.run_until_parked();
            assert!(r.include_children(), "Space toggles the focused checkbox");
            next(&mut r);
            assert!(r.page_focused(), "the page follows the checkbox");
        }
    }

    /// Type-to-filter from a focused table (E20-A2, UI-SHL-124).
    mod type_to_filter {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::time::Duration;

        use gpui_kit::{
            AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
            IntoElement, Render, Styled, TestAppContext, VisualTestContext, Window, div, point, px,
        };
        use tl_model::command::Tab;
        use tl_ui::actions::{RefreshCurrentTab, SelectTab};
        use tl_ui::components::menu::{TlMenu, context_menu_open, open_context_menu_at};
        use tl_ui::window::filter::{FILTER_DEBOUNCE, FilterBox};
        use tl_ui::window::main::{
            FilterChanged, GlobalShellEvents, MainWindow, PageSlot, ShellPage, set_test_page,
        };
        use tl_ui::window::status::StatusChanged;

        use super::{Opts, act, draw, main, open, setup};

        /// A page whose focus element sits in the "Table" key context, as `TreeTable` does.
        struct TablePage {
            focus: FocusHandle,
        }

        impl Render for TablePage {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .id("table-page")
                    .key_context("Table")
                    .track_focus(&self.focus)
                    .size_full()
            }
        }

        impl EventEmitter<StatusChanged> for TablePage {}

        impl ShellPage for TablePage {
            fn primary_focus(&self) -> FocusHandle {
                self.focus.clone()
            }
        }

        fn table_page(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
            let page = cx.new(|cx| TablePage {
                focus: cx.focus_handle().tab_stop(true),
            });
            PageSlot::new(page, cx)
        }

        type Events = Rc<RefCell<Vec<(Tab, String)>>>;

        struct Rig {
            vcx: VisualTestContext,
            events: Events,
        }

        /// The main window on `start` with table pages on Processes and Performance, recording
        /// `FilterChanged`, with the table focused.
        fn table_rig(cx: &mut TestAppContext, start: Tab) -> Rig {
            setup(cx, &Opts::default());
            cx.update(|cx| {
                set_test_page(cx, Tab::Processes, table_page);
                set_test_page(cx, Tab::Performance, table_page);
            });
            let mut vcx = open(cx);
            let events: Events = Rc::default();
            let seen = events.clone();
            vcx.update(|_, cx| {
                let bus = cx.global::<GlobalShellEvents>().0.clone();
                cx.subscribe(&bus, move |_, e: &FilterChanged, _| {
                    seen.borrow_mut().push((e.tab, e.text.to_string()));
                })
                .detach();
            });
            if start != Tab::Processes {
                act(&mut vcx, SelectTab(start as usize));
            }
            let mut r = Rig { vcx, events };
            r.focus_table();
            r
        }

        impl Rig {
            fn filter(&mut self) -> Entity<FilterBox> {
                let main = main(&mut self.vcx);
                self.vcx.read_entity(&main, |m, _| m.filter_box().clone())
            }

            fn input_text(&mut self) -> String {
                let f = self.filter();
                self.vcx
                    .update(|_, cx| f.read(cx).input().read(cx).text().to_string())
            }

            fn cursor(&mut self) -> std::ops::Range<usize> {
                let f = self.filter();
                self.vcx
                    .update(|_, cx| f.read(cx).input().read(cx).selected_range())
            }

            fn table_focus(&mut self) -> FocusHandle {
                let main = main(&mut self.vcx);
                let tab = self.vcx.read_entity(&main, |m, _| m.active_tab());
                let page = self
                    .vcx
                    .read_entity(&main, |m, _| m.page_entity::<TablePage>(tab))
                    .expect("table page built");
                self.vcx.read_entity(&page, |p, _| p.focus.clone())
            }

            fn focus_table(&mut self) {
                let focus = self.table_focus();
                self.vcx.update(|window, cx| window.focus(&focus, cx));
                draw(&mut self.vcx);
            }

            fn table_focused(&mut self) -> bool {
                let focus = self.table_focus();
                self.vcx.update(|window, _| focus.is_focused(window))
            }

            fn filter_focused(&mut self) -> bool {
                let f = self.filter();
                self.vcx
                    .update(|window, cx| f.read(cx).is_focused(window, cx))
            }

            fn key(&mut self, keys: &str) {
                self.vcx.simulate_keystrokes(keys);
                draw(&mut self.vcx);
            }

            fn wait(&mut self, ms: u64) {
                self.vcx.executor().advance_clock(Duration::from_millis(ms));
                self.vcx.run_until_parked();
            }

            fn events(&self) -> Vec<(Tab, String)> {
                self.events.borrow().clone()
            }

            /// The key context names of the focused element, outermost first.
            fn context_names(&mut self) -> Vec<String> {
                self.vcx.update(|window, _| {
                    window
                        .context_stack()
                        .iter()
                        .flat_map(|c| {
                            ["Root", "Global", "ShellFilter", "Input", "Table"]
                                .into_iter()
                                .filter(|n| c.contains(n))
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .collect()
                })
            }
        }

        #[gpui_kit::test]
        fn printable_key_routes_to_filter(cx: &mut TestAppContext) {
            let mut r = table_rig(cx, Tab::Processes);
            assert!(r.table_focused());
            r.key("f");
            assert_eq!(r.input_text(), "f");
            assert!(r.filter_focused());
            assert!(r.events().is_empty(), "nothing before the quiet time");
            r.wait(FILTER_DEBOUNCE.as_millis() as u64);
            assert_eq!(r.events(), [(Tab::Processes, "f".to_owned())]);
            r.wait(500);
            assert_eq!(r.events().len(), 1, "exactly one");

            // U-45: the filter's container sets "ShellFilter" outside the kit "Input".
            let names = r.context_names();
            let at = |n: &str| names.iter().position(|x| x == n);
            assert!(at("Global") < at("ShellFilter"), "{names:?}");
            assert!(at("ShellFilter") < at("Input"), "{names:?}");

            // The text is "ab" with the caret at its start; the key goes at the end.
            r.key("backspace");
            r.vcx.simulate_input("ab");
            r.key("home");
            assert_eq!(r.cursor(), 0..0);
            r.focus_table();
            r.key("c");
            assert_eq!(r.input_text(), "abc");
            assert_eq!(r.cursor(), 3..3, "caret at the end");
            assert!(r.filter_focused());
        }

        #[gpui_kit::test]
        fn shift_letter_routes(cx: &mut TestAppContext) {
            let mut r = table_rig(cx, Tab::Processes);
            r.key("shift-f");
            assert_eq!(r.input_text(), "F");
            assert!(r.filter_focused());
        }

        #[gpui_kit::test]
        fn space_not_routed(cx: &mut TestAppContext) {
            let mut r = table_rig(cx, Tab::Processes);
            r.key("space");
            assert_eq!(r.input_text(), "");
            assert!(r.table_focused());
            r.wait(200);
            assert!(r.events().is_empty());
        }

        #[gpui_kit::test]
        fn ctrl_letter_not_routed(cx: &mut TestAppContext) {
            let mut r = table_rig(cx, Tab::Processes);
            r.key("a b");
            r.wait(200);
            r.focus_table();
            r.key("ctrl-f");
            assert!(r.filter_focused(), "FocusFilter ran");
            assert_eq!(r.input_text(), "ab", "no 'f' typed");
            assert_eq!(r.cursor(), 0..2, "FocusFilter selects all");
        }

        #[gpui_kit::test]
        fn no_routing_on_performance(cx: &mut TestAppContext) {
            let mut r = table_rig(cx, Tab::Performance);
            assert!(r.table_focused());
            r.key("a");
            assert!(r.table_focused());
            assert_eq!(r.input_text(), "");
            r.wait(200);
            assert!(r.events().is_empty());
        }

        #[gpui_kit::test]
        fn no_routing_while_menu_open(cx: &mut TestAppContext) {
            let mut r = table_rig(cx, Tab::Processes);
            let focus = r.table_focus();
            r.vcx.update(|window, cx| {
                let menu = TlMenu::build(window, cx, focus, |m, _, _| {
                    m.item("Refresh", Box::new(RefreshCurrentTab))
                });
                open_context_menu_at(point(px(20.), px(80.)), window, cx, menu);
            });
            draw(&mut r.vcx);
            r.key("a");
            assert!(r.vcx.update(|window, cx| context_menu_open(window, cx)));
            assert!(!r.filter_focused());
            assert_eq!(r.input_text(), "");
            r.wait(200);
            assert!(r.events().is_empty());
        }
    }

    /// Hide system processes and the view-mode handlers (E20-017).
    mod process_view {
        use std::sync::Arc;

        use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};
        use tl_model::client::EngineClient;
        use tl_model::command::Tab;
        use tl_model::table::TableMode;
        use tl_ui::actions::{CycleViewMode, SetViewMode, ToggleHideSystem};
        use tl_ui::app::EngineGlobal;
        use tl_ui::settings_global::TlSettings;
        use tl_ui::testing::fake_engine::FakeEngine;
        use tl_ui::window::main::view_mode_tooltip;

        use super::{Opts, act, main, open, setup};

        /// The main window with `mode`, `history_mode` and a fake engine that records every
        /// `set_settings` made after the setup.
        fn view_rig(
            cx: &mut TestAppContext,
            mode: TableMode,
            history_mode: bool,
        ) -> (VisualTestContext, Arc<FakeEngine>) {
            setup(
                cx,
                &Opts {
                    pages: &[Tab::Processes],
                    ..Opts::default()
                },
            );
            let engine = FakeEngine::new();
            cx.update(|cx| {
                TlSettings::update(cx, |d| {
                    d.settings.processes.mode = mode;
                    d.state.processes.history_mode = history_mode;
                });
                let client: Arc<dyn EngineClient> = engine.clone();
                cx.set_global(EngineGlobal(client));
            });
            (open(cx), engine)
        }

        fn toasts(vcx: &mut VisualTestContext) -> Vec<String> {
            let main = main(vcx);
            let queue = vcx.read_entity(&main, |m, _| m.toasts().clone());
            vcx.read_entity(&queue, |q, _| q.visible_texts())
                .into_iter()
                .map(|t| t.to_string())
                .collect()
        }

        fn mode(vcx: &mut VisualTestContext) -> TableMode {
            vcx.update(|_, cx| TlSettings::settings(cx).processes.mode)
        }

        fn hide_system(vcx: &mut VisualTestContext) -> bool {
            vcx.update(|_, cx| TlSettings::settings(cx).processes.hide_system)
        }

        #[gpui_kit::test]
        fn hide_system_toggle_toast(cx: &mut TestAppContext) {
            let (mut vcx, engine) = view_rig(cx, TableMode::Grouped, false);
            assert!(!hide_system(&mut vcx));

            act(&mut vcx, ToggleHideSystem);
            assert!(hide_system(&mut vcx));
            let sent = engine.settings();
            assert_eq!(sent.len(), 1);
            assert!(sent[0].hide_system);
            assert_eq!(toasts(&mut vcx), ["Hiding system processes"]);

            act(&mut vcx, ToggleHideSystem);
            assert!(!hide_system(&mut vcx));
            let sent = engine.settings();
            assert_eq!(sent.len(), 2);
            assert!(!sent[1].hide_system);
            assert_eq!(
                toasts(&mut vcx),
                ["Hiding system processes", "Showing system processes"]
            );
        }

        #[gpui_kit::test]
        fn hide_system_works_in_history(cx: &mut TestAppContext) {
            let (mut vcx, engine) = view_rig(cx, TableMode::Grouped, true);
            act(&mut vcx, ToggleHideSystem);
            assert!(hide_system(&mut vcx));
            assert_eq!(engine.settings().len(), 1);
            assert_eq!(toasts(&mut vcx), ["Hiding system processes"]);
        }

        #[gpui_kit::test]
        fn view_mode_cycle_order(cx: &mut TestAppContext) {
            let (mut vcx, _) = view_rig(cx, TableMode::Grouped, false);
            let mut modes = Vec::new();
            for _ in 0..3 {
                act(&mut vcx, CycleViewMode);
                modes.push(mode(&mut vcx));
            }
            assert_eq!(
                modes,
                [TableMode::Flat, TableMode::Tree, TableMode::Grouped]
            );
            assert_eq!(
                toasts(&mut vcx),
                [
                    "View mode: List View",
                    "View mode: Tree View",
                    "View mode: Task Manager Grouping",
                ]
            );
        }

        #[gpui_kit::test]
        fn view_mode_ignored_in_history(cx: &mut TestAppContext) {
            let (mut vcx, engine) = view_rig(cx, TableMode::Grouped, true);
            act(&mut vcx, CycleViewMode);
            act(&mut vcx, SetViewMode(TableMode::Tree));
            assert_eq!(mode(&mut vcx), TableMode::Grouped);
            assert!(engine.settings().is_empty());
            assert!(toasts(&mut vcx).is_empty());
        }

        #[gpui_kit::test]
        fn set_view_mode_same_is_noop(cx: &mut TestAppContext) {
            let (mut vcx, _) = view_rig(cx, TableMode::Tree, false);
            let version = vcx.update(|_, cx| {
                TlSettings::get(cx).version(tl_ui::settings_global::Section::Processes)
            });
            act(&mut vcx, SetViewMode(TableMode::Tree));
            assert_eq!(mode(&mut vcx), TableMode::Tree);
            let after = vcx.update(|_, cx| {
                TlSettings::get(cx).version(tl_ui::settings_global::Section::Processes)
            });
            assert_eq!(after, version, "no settings write");
            assert!(toasts(&mut vcx).is_empty());

            act(&mut vcx, SetViewMode(TableMode::Flat));
            assert_eq!(mode(&mut vcx), TableMode::Flat);
            assert_eq!(toasts(&mut vcx), ["View mode: List View"]);
        }

        #[gpui_kit::test]
        fn view_mode_tooltip_text(cx: &mut TestAppContext) {
            let (mut vcx, _) = view_rig(cx, TableMode::Flat, false);
            let mode = mode(&mut vcx);
            let tip = vcx.update(|_, cx| view_mode_tooltip(mode, cx));
            assert_eq!(tip.as_ref(), "Toggle view mode (current: List View)");
        }
    }

    // ---- Tab switch animation (E20-021) ----

    /// The outline's x as last drawn and the page opacity.
    fn anim_state(vcx: &mut VisualTestContext) -> (Pixels, f32) {
        let main = main(vcx);
        vcx.update(|_, cx| {
            let m = main.read(cx);
            let rule = m
                .tabstrip_view()
                .read(cx)
                .accent_rule()
                .expect("rule drawn");
            (rule.active.origin.x, m.page_opacity())
        })
    }

    fn tab_x(vcx: &mut VisualTestContext, selector: &'static str) -> Pixels {
        bounds(vcx, selector).origin.x
    }

    fn animating(vcx: &mut VisualTestContext) -> bool {
        let main = main(vcx);
        vcx.update(|_, cx| main.read(cx).tabstrip_view().read(cx).is_animating())
    }

    /// Animation frames asked for by the outline slide and the page fade together.
    fn anim_frames(vcx: &mut VisualTestContext) -> u32 {
        let main = main(vcx);
        vcx.update(|_, cx| {
            let m = main.read(cx);
            m.tabstrip_view().read(cx).anim_frames() + m.fade_frames()
        })
    }

    /// The rig with `motion.tab_switch_ms` at its 250 ms default; the rig itself turns it off.
    fn anim_rig(cx: &mut TestAppContext) -> VisualTestContext {
        let mut vcx = rig(cx, Opts::default());
        vcx.update(|_, cx| TlSettings::update(cx, |d| d.settings.motion.tab_switch_ms = 250));
        vcx
    }

    /// Moves the clock, runs the animation frame callbacks (they mark the strip and the window
    /// for redraw), then draws.
    fn advance(vcx: &mut VisualTestContext, ms: u64) {
        vcx.executor().advance_clock(Duration::from_millis(ms));
        frame(vcx);
        draw(vcx);
    }

    #[gpui_kit::test]
    fn tab_switch_animates(cx: &mut TestAppContext) {
        let mut vcx = anim_rig(cx);
        let old = tab_x(&mut vcx, "TabStrip-processes");
        let new = tab_x(&mut vcx, "TabStrip-performance");
        assert_eq!(anim_state(&mut vcx).0, old);
        act(&mut vcx, SelectTab(1));
        assert_eq!(active(&mut vcx), Tab::Performance);
        advance(&mut vcx, 125);
        let (x, opacity) = anim_state(&mut vcx);
        assert!(old < x && x < new, "{old:?} < {x:?} < {new:?}");
        assert!(0. < opacity && opacity < 1., "{opacity}");
        advance(&mut vcx, 125);
        assert_eq!(anim_state(&mut vcx), (new, 1.0));
        assert!(!animating(&mut vcx));
    }

    #[gpui_kit::test]
    fn tab_switch_instant_when_reduced_motion(cx: &mut TestAppContext) {
        let mut vcx = anim_rig(cx);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        let new = tab_x(&mut vcx, "TabStrip-performance");
        act(&mut vcx, SelectTab(1));
        frame(&mut vcx);
        draw(&mut vcx);
        assert_eq!(anim_state(&mut vcx), (new, 1.0));
        assert!(!animating(&mut vcx));
        assert_eq!(anim_frames(&mut vcx), 0, "no animation runs");
    }

    #[gpui_kit::test]
    fn tab_switch_zero_ms_instant(cx: &mut TestAppContext) {
        let mut vcx = anim_rig(cx);
        vcx.update(|_, cx| TlSettings::update(cx, |d| d.settings.motion.tab_switch_ms = 0));
        let new = tab_x(&mut vcx, "TabStrip-performance");
        act(&mut vcx, SelectTab(1));
        assert_eq!(anim_state(&mut vcx), (new, 1.0));
        assert_eq!(anim_frames(&mut vcx), 0);
    }

    #[gpui_kit::test]
    fn tab_switch_restarts_from_current(cx: &mut TestAppContext) {
        let mut vcx = anim_rig(cx);
        act(&mut vcx, SelectTab(1));
        advance(&mut vcx, 125);
        let (mid, _) = anim_state(&mut vcx);
        act(&mut vcx, SelectTab(2));
        assert_eq!(active(&mut vcx), Tab::Services);
        assert_eq!(
            anim_state(&mut vcx).0,
            mid,
            "the new slide starts where the old one is"
        );
        let target = tab_x(&mut vcx, "TabStrip-services");
        advance(&mut vcx, 125);
        let (x, _) = anim_state(&mut vcx);
        assert!(mid < x && x < target, "{mid:?} < {x:?} < {target:?}");
        advance(&mut vcx, 125);
        assert_eq!(anim_state(&mut vcx), (target, 1.0));
    }

    #[gpui_kit::test]
    fn tab_switch_no_frames_after_end(cx: &mut TestAppContext) {
        let mut vcx = anim_rig(cx);
        act(&mut vcx, SelectTab(1));
        advance(&mut vcx, 250);
        let frames = anim_frames(&mut vcx);
        assert!(frames > 0, "the switch animated");
        for _ in 0..4 {
            advance(&mut vcx, 250);
        }
        assert_eq!(anim_frames(&mut vcx), frames, "no frame after the end");
        assert!(!animating(&mut vcx));
    }

    // ---- History toggle (E20-025) ----

    use tl_ui::actions::ToggleHistoryMode;

    fn history_mode(vcx: &mut VisualTestContext) -> bool {
        vcx.update(|_, cx| TlSettings::state(cx).processes.history_mode)
    }

    fn first_label(vcx: &mut VisualTestContext) -> String {
        let main = main(vcx);
        vcx.update(|_, cx| {
            let strip = main.read(cx).tabstrip_view().read(cx);
            strip.drawn_labels()[0].to_string()
        })
    }

    fn drawn_history(vcx: &mut VisualTestContext) -> tl_ui::window::tabstrip::DrawnHistory {
        let main = main(vcx);
        vcx.update(|_, cx| {
            let strip = main.read(cx).tabstrip_view().read(cx);
            strip.drawn_history().cloned().expect("history icon drawn")
        })
    }

    #[gpui_kit::test]
    fn history_toggle_selects_processes(cx: &mut TestAppContext) {
        let persisted = Rc::new(std::cell::Cell::new(0));
        let mut vcx = rig(
            cx,
            Opts {
                start_tab: StartTab::Performance,
                persisted: Some(persisted.clone()),
                ..Opts::default()
            },
        );
        assert_eq!(active(&mut vcx), Tab::Performance);
        assert!(!history_mode(&mut vcx));
        let before = persisted.get();
        act(&mut vcx, ToggleHistoryMode);
        assert_eq!(active(&mut vcx), Tab::Processes);
        assert!(history_mode(&mut vcx));
        assert!(persisted.get() > before, "the state is saved");
        draw(&mut vcx);
        assert_eq!(first_label(&mut vcx), "History");
    }

    #[gpui_kit::test]
    fn history_icon_click_toggles_off(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        vcx.update(|_, cx| TlSettings::update(cx, |d| d.state.processes.history_mode = true));
        vcx.run_until_parked();
        draw(&mut vcx);
        assert_eq!(first_label(&mut vcx), "History");
        let created = log(&mut vcx, |l| l.created.clone());
        let at = bounds(&mut vcx, "TabStrip-history").center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
        draw(&mut vcx);
        assert!(!history_mode(&mut vcx));
        assert_eq!(first_label(&mut vcx), "Processes");
        assert_eq!(active(&mut vcx), Tab::Processes);
        assert_eq!(
            log(&mut vcx, |l| l.created.clone()),
            created,
            "no page built"
        );
    }

    #[gpui_kit::test]
    fn history_icon_colours_and_tooltips(cx: &mut TestAppContext) {
        let mut vcx = rig(cx, Opts::default());
        let (accent, secondary, hover) = vcx.update(|_, cx| {
            let t = tl_ui::theme::current(cx);
            (t.accent, t.text_secondary, t.tab_hover)
        });
        let off = drawn_history(&mut vcx);
        assert_eq!(off.color, secondary);
        assert_eq!(off.tooltip.as_ref(), "Show history");

        // Hover: a tab_hover square under the icon.
        let icon = bounds(&mut vcx, "TabStrip-history");
        vcx.simulate_mouse_move(icon.center(), None, Modifiers::none());
        let square = vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            window.painted_quads().iter().any(|q| {
                q.background == gpui_kit::Background::from(hover) && q.bounds == icon.scale(scale)
            })
        });
        assert!(square, "hover draws the tab_hover square");

        vcx.update(|_, cx| TlSettings::update(cx, |d| d.state.processes.history_mode = true));
        vcx.run_until_parked();
        draw(&mut vcx);
        let on = drawn_history(&mut vcx);
        assert_eq!(on.color, accent);
        assert_eq!(on.tooltip.as_ref(), "Show live processes");
    }
}

/// Startup step 11 after the first frame (tasklens-jqk.20, UI-SHL-110, UI-SHL-093, APP-048).
mod startup_items {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, SystemTime};

    use gpui_kit::{AppContext as _, Context, FocusHandle, IntoElement, Render, Window, div};
    use gpui_kit::{SharedString, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient;
    use tl_model::command::{AppRequest, Command, KWinCommand, Tab};
    use tl_model::ids::ProcessKey;
    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::{
        ComponentHeader, FrameGeometry, ProcessSnapshot, WindowEntry, WindowsSnapshot,
    };
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::app::{
        AppEnv, CrashReport, StartOptions, TasklensUi, ToastKindTag, UiExit, UiExitSlot, UiInit,
    };
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{
        CloseOutcome, MainWindow, PageSlot, SELECT_PID_WAIT, ShellPage, main_window_view,
        open_main_window, set_test_page,
    };
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    use tl_ui::window::status::StatusChanged;
    use tl_ui::window::titlebar::TitleBarProbe;

    const PID: u32 = 4242;

    struct Rig {
        engine: Arc<FakeEngine>,
        /// `mark_crash_seen` calls.
        seen: Arc<AtomicU32>,
        version: u64,
        /// The sender tl-app keeps for `UiInit.app_requests`.
        requests: async_channel::Sender<AppRequest>,
    }

    /// Default settings with instant tab switches (E20-021), so frame counts do not see the
    /// switch animation.
    fn no_tab_animation() -> Settings {
        let mut settings = Settings::default();
        settings.motion.tab_switch_ms = 0;
        settings
    }

    /// `TasklensUi::init` with `toasts` queued, a pending crash report when `crash`, and
    /// `start` as the command line. Opens no window.
    fn init(
        cx: &mut TestAppContext,
        toasts: Vec<(ToastKindTag, SharedString)>,
        crash: bool,
        start: StartOptions,
    ) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (requests, rx) = async_channel::bounded(4);
        let seen = Arc::new(AtomicU32::new(0));
        let mut env = AppEnv::test();
        if crash {
            env.pending_crash = Some(CrashReport {
                path: "/tmp/crash-1.txt".into(),
                text: "panic at x".into(),
                at: SystemTime::UNIX_EPOCH,
            });
            let count = seen.clone();
            env.mark_crash_seen = Arc::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
            });
        }
        let init = UiInit {
            settings: no_tab_animation(),
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: toasts,
            env,
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            cx.set_global(start);
            cx.set_reduce_motion(true);
        });
        cx.run_until_parked();
        Rig {
            engine,
            seen,
            version: 0,
            requests,
        }
    }

    /// Opens the main window and draws its first frame, so `after_first_frame` runs.
    fn open(cx: &mut TestAppContext) -> VisualTestContext {
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        vcx.run_until_parked();
        vcx
    }

    /// Closes the main window and opens it again, as a reopen from the tray does.
    fn reopen(vcx: VisualTestContext, cx: &mut TestAppContext) -> VisualTestContext {
        let mut vcx = vcx;
        vcx.update(|window, _| window.remove_window());
        vcx.run_until_parked();
        open(cx)
    }

    fn main_window(cx: &mut TestAppContext) -> gpui_kit::Entity<MainWindow> {
        cx.update(|cx| main_window_view(cx).expect("main window open"))
    }

    fn toasts(cx: &mut TestAppContext) -> Vec<String> {
        let main = main_window(cx);
        cx.update(|cx| {
            let texts = main.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        })
    }

    fn record(pid: u32) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: ProcessKey {
                start_ticks: 100,
                boot_id: BootId(0),
                pid,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a("demo"),
                name: a("demo"),
                exe: None,
                cmdline: None,
                user: a("u"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("demo"),
            }),
            group: GroupKind::System.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    impl Rig {
        /// Publishes a process snapshot holding `pids`.
        fn publish(&mut self, cx: &mut TestAppContext, pids: &[u32]) {
            self.version += 1;
            let version = self.version;
            let records: Vec<ProcessRecord> = pids.iter().map(|&p| record(p)).collect();
            self.engine.publish_with(|s| {
                s.processes = Arc::new(ProcessSnapshot {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    records: records.into(),
                    ..ProcessSnapshot::default()
                });
            });
            cx.run_until_parked();
        }
    }

    fn jumps(cx: &mut TestAppContext) -> u32 {
        let main = main_window(cx);
        cx.update(|cx| main.read(cx).select_pid_jumps())
    }

    fn select_pid(pid: u32) -> StartOptions {
        StartOptions {
            tab: Some(tl_model::settings::StartTab::Performance),
            select_pid: Some(pid),
            ..StartOptions::default()
        }
    }

    #[gpui_kit::test]
    fn startup_toasts_after_first_frame(cx: &mut TestAppContext) {
        let queued = vec![
            (ToastKindTag::Warning, SharedString::from("test warning")),
            (ToastKindTag::Info, SharedString::from("test info")),
        ];
        init(cx, queued, false, StartOptions::default());
        let vcx = open(cx);
        assert_eq!(toasts(cx), ["test warning", "test info"]);

        // A reopened window does not show them again.
        let _vcx = reopen(vcx, cx);
        assert!(toasts(cx).is_empty(), "{:?}", toasts(cx));
    }

    #[gpui_kit::test]
    fn crash_report_shown_after_first_frame(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), true, StartOptions::default());
        let vcx = open(cx);
        let crash = cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::CrashReport)
        });
        assert!(crash.is_some(), "the Crash Report window opened");
        let _vcx = reopen(vcx, cx);
        assert_eq!(
            r.seen.load(Ordering::Relaxed),
            1,
            "shown and marked seen once"
        );
    }

    #[gpui_kit::test]
    fn select_pid_waits_for_process(cx: &mut TestAppContext) {
        let mut r = init(cx, Vec::new(), false, select_pid(PID));
        let _vcx = open(cx);
        let main = main_window(cx);
        assert_eq!(cx.update(|cx| main.read(cx).active_tab()), Tab::Processes);
        assert_eq!(jumps(cx), 0, "no record yet");

        cx.executor().advance_clock(Duration::from_secs(1));
        r.publish(cx, &[1, PID]);
        assert_eq!(jumps(cx), 1);
        assert_eq!(cx.update(|cx| main.read(cx).active_tab()), Tab::Processes);
        r.publish(cx, &[1, PID, 7]);
        assert_eq!(jumps(cx), 1, "selected once");
        cx.executor().advance_clock(SELECT_PID_WAIT);
        cx.run_until_parked();
        assert!(toasts(cx).iter().all(|t| !t.contains("not found")));
    }

    #[gpui_kit::test]
    fn select_pid_not_found_after_3s(cx: &mut TestAppContext) {
        let mut r = init(cx, Vec::new(), false, select_pid(PID));
        let _vcx = open(cx);
        cx.executor().advance_clock(SELECT_PID_WAIT);
        cx.run_until_parked();
        assert!(
            toasts(cx).iter().any(|t| t == "Process 4242 not found"),
            "{:?}",
            toasts(cx)
        );
        r.publish(cx, &[PID]);
        assert_eq!(jumps(cx), 0, "a late process is not selected");
    }

    #[gpui_kit::test]
    fn start_minimized_without_tray(cx: &mut TestAppContext) {
        let start = StartOptions {
            start_minimized: true,
            ..StartOptions::default()
        };
        init(cx, Vec::new(), false, start);
        cx.update(|cx| cx.set_global(TitleBarProbe::default()));
        let vcx = open(cx);
        let first = main_window(cx);
        assert_eq!(cx.update(|cx| first.read(cx).start_minimizes()), 1);
        let _vcx = reopen(vcx, cx);
        let second = main_window(cx);
        assert_eq!(cx.update(|cx| second.read(cx).start_minimizes()), 0);
    }

    #[gpui_kit::test]
    fn first_frame_items_noop(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        let _vcx = open(cx);
        let main = main_window(cx);
        cx.update(|cx| {
            let m = main.read(cx);
            assert_eq!(m.first_frames(), 1);
            assert_eq!(m.select_pid_jumps(), 0);
            assert_eq!(m.start_minimizes(), 0);
        });
        assert!(toasts(cx).is_empty());
        assert_eq!(r.seen.load(Ordering::Relaxed), 0);
        let crash = cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::CrashReport)
        });
        assert!(crash.is_none());
    }

    // ---- AppRequest handler (E20-019) ----

    impl Rig {
        /// Sends `req` as a second launch does and runs the app until idle.
        fn request(&self, cx: &mut TestAppContext, req: AppRequest) {
            self.requests.try_send(req).expect("the request is queued");
            cx.run_until_parked();
        }

        /// Publishes a windows snapshot with `validity` and one window of this process.
        fn publish_windows(&self, cx: &mut TestAppContext, validity: Validity, caption: &str) {
            let entry = WindowEntry {
                internal_id: Arc::from("abc"),
                pid: std::process::id(),
                caption: Arc::from(caption),
                resource_class: Arc::from("tasklens"),
                desktop_file_name: Arc::from("io.github.oysteinkrog.Tasklens"),
                minimized: false,
                maximized: false,
                keep_above: false,
                frame_geometry: FrameGeometry::default(),
                window_type: 0,
            };
            self.engine.publish_with(|s| {
                let version = s.windows.header.version + 1;
                s.windows = Arc::new(WindowsSnapshot {
                    header: ComponentHeader {
                        version,
                        validity,
                        ..ComponentHeader::default()
                    },
                    windows: vec![entry].into(),
                    pick: None,
                });
            });
            cx.run_until_parked();
        }

        fn kwin_activates(&self) -> Vec<Arc<str>> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::KWin(KWinCommand::Activate { internal_id }) => Some(internal_id),
                    _ => None,
                })
                .collect()
        }
    }

    const ACTIVATE: AppRequest = AppRequest::Activate { token: None };

    /// A blank page for tabs that have no page arm yet.
    struct Blank(FocusHandle);

    impl gpui_kit::EventEmitter<StatusChanged> for Blank {}

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    impl ShellPage for Blank {
        fn primary_focus(&self) -> FocusHandle {
            self.0.clone()
        }
    }

    fn blank(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| Blank(cx.focus_handle()));
        PageSlot::new(page, cx)
    }

    fn active_tab(cx: &mut TestAppContext) -> Tab {
        let main = main_window(cx);
        cx.update(|cx| main.read(cx).active_tab())
    }

    #[gpui_kit::test]
    fn activate_reopens_hidden_window(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        let mut vcx = open(cx);
        let first = main_window(cx);
        vcx.update(|window, cx| {
            first.update(cx, |this, cx| {
                this.apply_close_for_test(CloseOutcome::RemoveWindow, window, cx)
            })
        });
        vcx.run_until_parked();
        cx.update(|cx| {
            assert!(main_window_view(cx).is_none(), "the window was removed");
            TlSettings::update(cx, |d| d.state.tabs.last_tab = Tab::Information);
        });

        r.request(cx, ACTIVATE);
        let second = main_window(cx);
        assert_ne!(first.entity_id(), second.entity_id(), "a new main window");
        assert_eq!(active_tab(cx), Tab::Information, "opened on last_tab");
        assert!(r.kwin_activates().is_empty());
    }

    #[gpui_kit::test]
    fn activate_brings_window_forward(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        let mut vcx = open(cx);
        let main = main_window(cx);
        vcx.deactivate_window();
        vcx.run_until_parked();
        assert!(!vcx.update(|window, _| window.is_window_active()));

        r.request(cx, ACTIVATE);
        assert!(vcx.update(|window, _| window.is_window_active()));
        assert_eq!(main.entity_id(), main_window(cx).entity_id(), "same window");
        assert!(
            r.kwin_activates().is_empty(),
            "the KWin bridge is not up, so no KWin command"
        );
    }

    #[gpui_kit::test]
    fn activate_sends_kwin_activate(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        let _vcx = open(cx);
        r.publish_windows(cx, Validity::Valid, "other caption");
        r.request(cx, ACTIVATE);
        assert!(r.kwin_activates().is_empty(), "no window titled tasklens");

        r.publish_windows(cx, Validity::Valid, "tasklens");
        r.request(cx, ACTIVATE);
        assert_eq!(r.kwin_activates(), [Arc::<str>::from("abc")]);
    }

    #[gpui_kit::test]
    fn app_request_select_tab(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        cx.update(|cx| set_test_page(cx, Tab::Services, blank));
        let _vcx = open(cx);
        assert_ne!(active_tab(cx), Tab::Services);

        r.request(cx, ACTIVATE);
        r.request(cx, AppRequest::SelectTab(Tab::Services));
        assert_eq!(active_tab(cx), Tab::Services);
    }

    #[gpui_kit::test]
    fn app_request_select_pid(cx: &mut TestAppContext) {
        let start = StartOptions {
            tab: Some(tl_model::settings::StartTab::Performance),
            ..StartOptions::default()
        };
        let mut r = init(cx, Vec::new(), false, start);
        let _vcx = open(cx);
        assert_eq!(active_tab(cx), Tab::Performance);

        r.request(cx, AppRequest::SelectPid(77));
        assert_eq!(active_tab(cx), Tab::Processes);
        assert_eq!(jumps(cx), 0, "no record yet");
        cx.executor().advance_clock(Duration::from_millis(500));
        r.publish(cx, &[1, 77]);
        assert_eq!(jumps(cx), 1);
        r.publish(cx, &[1, 77, 9]);
        assert_eq!(jumps(cx), 1, "the jump runs once");
        cx.executor().advance_clock(SELECT_PID_WAIT);
        cx.run_until_parked();
        assert!(toasts(cx).iter().all(|t| !t.contains("not found")));
    }

    #[gpui_kit::test]
    fn app_request_quit(cx: &mut TestAppContext) {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        let _vcx = open(cx);
        r.request(cx, AppRequest::Quit);
        let exit = cx.read(|cx| cx.try_global::<UiExitSlot>().and_then(|s| s.0.clone()));
        assert_eq!(exit, Some(UiExit::Quit));
    }

    // ---- Always-on-top pin (E20-022) ----

    use gpui_kit::{Modifiers, MouseButton, Pixels, point, px};
    use tl_ui::components::icons::TlIcon;
    use tl_ui::window::titlebar::DrawnPin;

    impl Rig {
        fn keep_aboves(&self) -> Vec<bool> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::KWin(KWinCommand::KeepAbove { pid, caption, on }) => {
                        assert_eq!(pid, std::process::id());
                        assert_eq!(caption, None, "None means the main window");
                        Some(on)
                    }
                    _ => None,
                })
                .collect()
        }
    }

    /// `init` with a probe that reports `client` decorations.
    fn init_bar(cx: &mut TestAppContext, client: bool) -> Rig {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        cx.update(|cx| {
            cx.set_global(TitleBarProbe {
                client: Some(client),
                ..TitleBarProbe::default()
            })
        });
        r
    }

    fn drawn_pin(cx: &mut TestAppContext) -> DrawnPin {
        let main = main_window(cx);
        cx.update(|cx| {
            let bar = main.read(cx).titlebar_view().clone();
            bar.read(cx).drawn_pin().cloned().expect("pin drawn")
        })
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn pin_bounds(vcx: &mut VisualTestContext) -> gpui_kit::Bounds<Pixels> {
        vcx.debug_bounds("TitleBarPin").expect("pin drawn")
    }

    fn on_top(cx: &mut TestAppContext) -> bool {
        cx.read(|cx| TlSettings::settings(cx).window.always_on_top)
    }

    #[gpui_kit::test]
    fn pin_toggles_setting(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let mut vcx = open(cx);
        r.publish_windows(cx, Validity::Valid, "tasklens");
        redraw(&mut vcx);
        let pin = drawn_pin(cx);
        assert!(pin.enabled && !pin.on);
        assert_eq!(pin.icon, TlIcon::PinOff);
        assert_eq!(pin.tooltip.as_ref(), "Always on top");
        let b = pin_bounds(&mut vcx);
        assert_eq!((b.size.width, b.size.height), (px(36.), px(34.)));

        vcx.simulate_click(b.center(), Modifiers::none());
        redraw(&mut vcx);
        assert!(on_top(cx));
        assert_eq!(r.keep_aboves(), [true]);
        let pin = drawn_pin(cx);
        assert!(pin.on);
        assert_eq!(pin.icon, TlIcon::Pin);
        assert_eq!(pin.tooltip.as_ref(), "Always on top (on)");

        vcx.simulate_click(b.center(), Modifiers::none());
        redraw(&mut vcx);
        assert!(!on_top(cx));
        assert_eq!(r.keep_aboves(), [true, false]);
    }

    #[gpui_kit::test]
    fn pin_disabled_without_bridge(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let mut vcx = open(cx);
        r.publish_windows(cx, Validity::Unsupported, "tasklens");
        redraw(&mut vcx);
        let pin = drawn_pin(cx);
        assert!(!pin.enabled);
        assert_eq!(pin.tooltip.as_ref(), "Not supported by this desktop");
        let b = pin_bounds(&mut vcx);
        vcx.simulate_click(b.center(), Modifiers::none());
        redraw(&mut vcx);
        assert!(!on_top(cx));
        assert!(r.keep_aboves().is_empty(), "a click sends nothing");
    }

    #[gpui_kit::test]
    fn pin_applied_at_start_once(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        cx.update(|cx| TlSettings::update(cx, |d| d.settings.window.always_on_top = true));
        let mut vcx = open(cx);
        r.publish_windows(cx, Validity::WarmingUp, "tasklens");
        assert!(r.keep_aboves().is_empty(), "the bridge is not up yet");
        r.publish_windows(cx, Validity::Valid, "tasklens");
        assert_eq!(r.keep_aboves(), [true]);
        r.publish_windows(cx, Validity::Valid, "tasklens");
        redraw(&mut vcx);
        assert_eq!(r.keep_aboves(), [true], "sent exactly once");
        assert!(drawn_pin(cx).on);
    }

    #[gpui_kit::test]
    fn server_mode_has_pin(cx: &mut TestAppContext) {
        init_bar(cx, false);
        let mut vcx = open(cx);
        redraw(&mut vcx);
        let bar = vcx.debug_bounds("TitleBar").expect("menu row drawn");
        let pin = pin_bounds(&mut vcx);
        assert_eq!((pin.size.width, pin.size.height), (px(36.), px(28.)));
        assert_eq!(pin.right(), bar.right(), "last item");
        let spacer = vcx.debug_bounds("TitleBarSpacer").expect("spacer");
        assert_eq!(spacer.right(), pin.origin.x);
    }

    #[gpui_kit::test]
    fn pin_no_drag(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let mut vcx = open(cx);
        r.publish_windows(cx, Validity::Valid, "tasklens");
        redraw(&mut vcx);
        let at = pin_bounds(&mut vcx).center();
        vcx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(
            at + point(px(10.), px(2.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        vcx.simulate_mouse_up(
            at + point(px(10.), px(2.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        let moves = cx.read(|cx| cx.global::<TitleBarProbe>().moves);
        assert_eq!(moves, 0, "no window drag from the pin");
    }

    // ---- Secondary-window pins (E20-029) ----

    use tl_ui::components::window_frame::ToggleAlwaysOnTop;
    use tl_ui::window::settings::open_settings;

    impl Rig {
        /// `KeepAbove` commands as (caption, on).
        fn keep_above_captions(&self) -> Vec<(Option<String>, bool)> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::KWin(KWinCommand::KeepAbove { pid, caption, on }) => {
                        assert_eq!(pid, std::process::id());
                        Some((caption.map(|c| c.to_string()), on))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    fn pin(cx: &mut TestAppContext, window: WindowKey, caption: &str) {
        let action = ToggleAlwaysOnTop {
            window,
            caption: SharedString::from(caption.to_owned()),
        };
        cx.update(|cx| cx.dispatch_action(&action));
        cx.run_until_parked();
    }

    fn settings_open(cx: &mut TestAppContext) -> bool {
        cx.read(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::Settings)
                .is_some()
        })
    }

    #[gpui_kit::test]
    fn secondary_pin_sends_keep_above_with_caption(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let _vcx = open(cx);
        r.publish_windows(cx, Validity::Valid, "tasklens");
        let before =
            cx.read(|cx| TlSettings::get(cx).version(tl_ui::settings_global::Section::Window));
        pin(cx, WindowKey::Settings, "Settings");
        assert_eq!(
            r.keep_above_captions(),
            [(Some("Settings".to_owned()), true)]
        );
        pin(cx, WindowKey::Settings, "Settings");
        assert_eq!(
            r.keep_above_captions(),
            [
                (Some("Settings".to_owned()), true),
                (Some("Settings".to_owned()), false)
            ]
        );
        let after =
            cx.read(|cx| TlSettings::get(cx).version(tl_ui::settings_global::Section::Window));
        assert_eq!(before, after, "no setting changed");
        assert!(!on_top(cx));
    }

    #[gpui_kit::test]
    fn secondary_pin_unsupported_sends_nothing(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let _vcx = open(cx);
        r.publish_windows(cx, Validity::Unsupported, "tasklens");
        pin(cx, WindowKey::Settings, "Settings");
        assert!(r.keep_above_captions().is_empty());
    }

    #[gpui_kit::test]
    fn secondary_pin_forgotten_on_close(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let _vcx = open(cx);
        r.publish_windows(cx, Validity::Valid, "tasklens");
        cx.update(open_settings);
        cx.run_until_parked();
        assert!(settings_open(cx));
        pin(cx, WindowKey::Settings, "Settings");
        let handle = cx.read(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::Settings)
                .expect("settings open")
        });
        cx.update(|cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
        cx.run_until_parked();
        assert!(!settings_open(cx));
        cx.update(open_settings);
        cx.run_until_parked();
        pin(cx, WindowKey::Settings, "Settings");
        let sent: Vec<bool> = r
            .keep_above_captions()
            .into_iter()
            .map(|(_, on)| on)
            .collect();
        assert_eq!(sent, [true, true], "the reopened window starts unpinned");
    }

    #[gpui_kit::test]
    fn main_pin_unchanged(cx: &mut TestAppContext) {
        let r = init_bar(cx, true);
        let _vcx = open(cx);
        r.publish_windows(cx, Validity::Valid, "tasklens");
        pin(cx, WindowKey::Main, "tasklens");
        assert!(on_top(cx));
        assert_eq!(r.keep_above_captions(), [(None, true)]);
    }

    // ---- Startup public IP fetch (E20-024) ----

    use tl_model::command::DeviceCommand;

    impl Rig {
        fn ip_fetches(&self) -> usize {
            self.engine
                .commands()
                .into_iter()
                .filter(|c| matches!(c, Command::Device(DeviceCommand::FetchPublicIp)))
                .count()
        }
    }

    /// `init` with the two `[network]` keys set.
    fn init_ip(cx: &mut TestAppContext, auto: bool, accepted: bool) -> Rig {
        let r = init(cx, Vec::new(), false, StartOptions::default());
        cx.update(|cx| {
            TlSettings::update(cx, |d| {
                d.settings.network.auto_fetch_public_ip = auto;
                d.settings.network.public_ip_notice_accepted = accepted;
            })
        });
        r
    }

    fn windows(cx: &mut TestAppContext) -> usize {
        cx.read(|cx| cx.windows().len())
    }

    #[gpui_kit::test]
    fn public_ip_auto_fetch_when_accepted(cx: &mut TestAppContext) {
        let r = init_ip(cx, true, true);
        let _vcx = open(cx);
        assert_eq!(r.ip_fetches(), 1);
    }

    #[gpui_kit::test]
    fn public_ip_no_fetch_without_consent(cx: &mut TestAppContext) {
        let r = init_ip(cx, true, false);
        let _vcx = open(cx);
        assert_eq!(r.ip_fetches(), 0);
        assert_eq!(windows(cx), 1, "no notice window opens");
    }

    #[gpui_kit::test]
    fn public_ip_no_fetch_when_off(cx: &mut TestAppContext) {
        let r = init_ip(cx, false, true);
        let _vcx = open(cx);
        assert_eq!(r.ip_fetches(), 0);
    }

    #[gpui_kit::test]
    fn public_ip_auto_fetch_once_per_process(cx: &mut TestAppContext) {
        let r = init_ip(cx, true, true);
        let vcx = open(cx);
        assert_eq!(r.ip_fetches(), 1);
        let _vcx = reopen(vcx, cx);
        assert_eq!(r.ip_fetches(), 1, "a reopened window does not fetch again");
    }

    #[gpui_kit::test]
    fn public_ip_auto_fetch_retries_busy(cx: &mut TestAppContext) {
        let r = init_ip(cx, true, true);
        r.engine.set_busy(true);
        let _vcx = open(cx);
        assert_eq!(r.ip_fetches(), 0, "refused as busy");
        r.engine.set_busy(false);
        cx.executor().advance_clock(Duration::from_millis(999));
        cx.run_until_parked();
        assert_eq!(r.ip_fetches(), 0, "not before 1 s");
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        assert_eq!(r.ip_fetches(), 1, "sent again 1 s later");
    }

    // ---- Shield slot and unlock at launch (E20-026) ----

    use std::cell::Cell;
    use std::rc::Rc;

    use tl_model::health::HelperState;
    use tl_ui::components::shield::StartHelperSession;
    use tl_ui::window::main::HelperInstalled;

    /// `init` with the helper `installed`, `privilege.unlock_at_launch`, Shift held at launch,
    /// blank pages for the unbuilt tabs, and a counter of `StartHelperSession`.
    fn helper_rig(
        cx: &mut TestAppContext,
        installed: bool,
        unlock: bool,
        shift: bool,
    ) -> (Rig, Rc<Cell<u32>>) {
        let start = StartOptions {
            shift_held: shift,
            ..StartOptions::default()
        };
        let r = init(cx, Vec::new(), false, start);
        let starts = Rc::new(Cell::new(0));
        let count = starts.clone();
        cx.update(|cx| {
            cx.set_global(HelperInstalled(installed));
            TlSettings::update(cx, |d| d.settings.privilege.unlock_at_launch = unlock);
            for tab in [Tab::Services, Tab::Startup, Tab::Connections] {
                set_test_page(cx, tab, blank);
            }
            cx.on_action(move |_: &StartHelperSession, _| count.set(count.get() + 1));
        });
        (r, starts)
    }

    impl Rig {
        fn publish_helper(&self, cx: &mut TestAppContext, helper: HelperState) {
            self.engine.publish_with(|s| {
                let health = Arc::make_mut(&mut s.health);
                health.helper = helper;
                health.version += 1;
            });
            cx.run_until_parked();
        }
    }

    fn select(vcx: &mut VisualTestContext, cx: &mut TestAppContext, tab: Tab) {
        let main = main_window(cx);
        vcx.update(|window, cx| main.update(cx, |m, cx| m.select_tab(tab, window, cx)));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn shield_slot_every_tab(cx: &mut TestAppContext) {
        let (r, _) = helper_rig(cx, true, false, false);
        let mut vcx = open(cx);
        r.publish_helper(cx, HelperState::Locked);
        for tab in Tab::ALL {
            select(&mut vcx, cx, tab);
            assert_eq!(main_window(cx).read_with(cx, |m, _| m.active_tab()), tab);
            assert!(vcx.debug_bounds("TlShield").is_some(), "shield on {tab:?}");
        }

        // Not installed: no shield and no slot.
        cx.update(|cx| cx.set_global(HelperInstalled(false)));
        select(&mut vcx, cx, Tab::Processes);
        assert!(vcx.debug_bounds("TlShield").is_none());
        let toolbar_installed_off = vcx.debug_bounds("Toolbar").map(|b| b.size.width);
        cx.update(|cx| cx.set_global(HelperInstalled(true)));
        select(&mut vcx, cx, Tab::Performance);
        select(&mut vcx, cx, Tab::Processes);
        let toolbar_installed_on = vcx.debug_bounds("Toolbar").map(|b| b.size.width);
        assert!(
            toolbar_installed_off < toolbar_installed_on,
            "no space without the helper: {toolbar_installed_off:?} < {toolbar_installed_on:?}"
        );
    }

    #[gpui_kit::test]
    fn unlock_at_launch_once(cx: &mut TestAppContext) {
        let (_r, starts) = helper_rig(cx, true, true, false);
        let vcx = open(cx);
        assert_eq!(starts.get(), 1);
        let _vcx = reopen(vcx, cx);
        assert_eq!(starts.get(), 1, "not again on a reopened window");
    }

    #[gpui_kit::test]
    fn shift_at_launch_unlocks(cx: &mut TestAppContext) {
        let (_r, starts) = helper_rig(cx, true, false, true);
        let _vcx = open(cx);
        assert_eq!(starts.get(), 1);
    }

    #[gpui_kit::test]
    fn unlock_needs_helper(cx: &mut TestAppContext) {
        let (_r, starts) = helper_rig(cx, false, true, false);
        let _vcx = open(cx);
        assert_eq!(starts.get(), 0);
    }

    #[gpui_kit::test]
    fn request_launch_unlock_once(cx: &mut TestAppContext) {
        let (_r, starts) = helper_rig(cx, true, true, false);
        let mut vcx = open(cx);
        assert_eq!(starts.get(), 1);
        let main = main_window(cx);
        vcx.update(|window, cx| main.update(cx, |m, cx| m.request_launch_unlock(window, cx)));
        vcx.run_until_parked();
        assert_eq!(starts.get(), 1, "E29's later call does nothing");
    }

    #[gpui_kit::test]
    fn shield_follows_helper_state(cx: &mut TestAppContext) {
        let (r, _) = helper_rig(cx, true, false, false);
        let mut vcx = open(cx);
        r.publish_helper(cx, HelperState::Locked);
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(vcx.debug_bounds("TlShield-icon-shield").is_some());
        r.publish_helper(cx, HelperState::Unlocked);
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(vcx.debug_bounds("TlShield-icon-unlocked").is_some());
        assert!(vcx.debug_bounds("TlShield-icon-shield").is_none());
    }
}
