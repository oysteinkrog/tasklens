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
            persist_themes: None,
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
