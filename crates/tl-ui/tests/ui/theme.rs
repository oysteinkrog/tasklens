//! Theme store integration tests (E17-A1, E17-008, UI-FND-040, UI-FND-042).

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::theme::Theme;
use gpui_kit::{App, Hsla, TestAppContext, rgb};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_ui::theme::store::{init, init_with_file};
use tl_ui::theme::user_store::{PersistTheme, ThemePersist};
use tl_ui::theme::{ThemeError, ThemeEvent, ThemeMode, ThemeStore, UiMetrics, current};

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

fn init_with(active: &str, cx: &mut App) {
    gpui_kit::init(cx);
    let theme = ThemeSettings {
        active: active.to_owned(),
        ..ThemeSettings::default()
    };
    init(cx, &theme, &AppearanceSettings::default(), Vec::new());
}

#[gpui_kit::test]
fn theme_init_installs_globals(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_with("Default Dark", cx);
        assert!(cx.has_global::<ThemeStore>());
        assert!(cx.has_global::<UiMetrics>());
        assert!(cx.has_global::<Theme>());
        assert_eq!(&*current(cx).name, "Default Dark");
        assert!(current(cx).builtin);
        assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));
        assert_eq!(Theme::global(cx).colors.primary, hex(0x0277A6));
    });
}

#[gpui_kit::test]
fn unknown_active_falls_back_to_dark(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_with("No Such Theme", cx);
        assert_eq!(&*current(cx).name, "Default Dark");
        assert_eq!(Theme::global(cx).colors.background, hex(0x191B1D));
    });
}

#[gpui_kit::test]
fn light_active_at_start(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_with("Default Light", cx);
        assert_eq!(&*current(cx).name, "Default Light");
        assert_eq!(Theme::global(cx).colors.background, hex(0xF3F4F6));
    });
}

/// Installs a recording persist callback and the store with the user themes in `json`.
fn init_user(json: &str, cx: &mut App) -> Rc<RefCell<Vec<PersistTheme>>> {
    gpui_kit::init(cx);
    let rec: Rc<RefCell<Vec<PersistTheme>>> = Rc::default();
    let r = rec.clone();
    cx.set_global(ThemePersist(Box::new(move |p| r.borrow_mut().push(p))));
    let toast = init_with_file(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Some(json),
        1,
    );
    assert_eq!(toast, None);
    rec
}

/// A themes file with one light user theme "Ocean": Default Light's colours with a new name.
fn ocean_light_file() -> String {
    let builtin = tl_ui::theme::builtin::load();
    let light = &builtin[1];
    assert_eq!(&*light.name, "Default Light");
    let mut def = light.clone();
    def.name = "Ocean".into();
    tl_ui::theme::user_store::serialize_user_themes(&[def])
}

#[gpui_kit::test]
fn delete_active_falls_back_by_mode(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let rec = init_user(&ocean_light_file(), cx);
        assert_eq!(
            ThemeStore::copy_theme("Ocean", cx),
            Ok(ThemeEvent::Copied {
                name: "Ocean copy".into()
            })
        );
        assert_eq!(&*current(cx).name, "Ocean copy");
        assert_eq!(current(cx).mode, ThemeMode::Light);
        rec.borrow_mut().clear();

        assert_eq!(
            ThemeStore::delete_theme("Ocean copy", cx),
            Ok(ThemeEvent::Deleted {
                name: "Ocean copy".into()
            })
        );
        assert_eq!(&*current(cx).name, "Default Light");
        assert!(current(cx).builtin);
        assert_eq!(Theme::global(cx).colors.background, hex(0xF3F4F6));
        let names: Vec<String> = cx
            .global::<ThemeStore>()
            .user
            .iter()
            .map(|d| d.name.to_string())
            .collect();
        assert_eq!(names, ["Ocean"]);
        {
            let rec = rec.borrow();
            assert_eq!(rec.len(), 1);
            let PersistTheme::Save(saved) = &rec[0] else {
                panic!("expected Save, got {:?}", rec[0]);
            };
            assert_eq!(saved.len(), 1);
            assert_eq!(&*saved[0].name, "Ocean");
        }

        // Deleting an inactive theme keeps the active one.
        rec.borrow_mut().clear();
        ThemeStore::delete_theme("Ocean", cx).unwrap();
        assert_eq!(&*current(cx).name, "Default Light");
        assert!(matches!(&rec.borrow()[0], PersistTheme::Save(s) if s.is_empty()));

        rec.borrow_mut().clear();
        assert_eq!(
            ThemeStore::delete_theme("Default Dark", cx),
            Err(ThemeError::BuiltinCannotBeDeleted)
        );
        assert_eq!(
            ThemeStore::delete_theme("Ocean", cx),
            Err(ThemeError::UnknownTheme("Ocean".into()))
        );
        assert!(rec.borrow().is_empty());
    });
}

/// E17-007: `TasklensUi::init` follows the portal color-scheme from the snapshot.
#[cfg(feature = "test-hooks")]
#[gpui_kit::test]
fn scheme_change_from_snapshot_switches_theme(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use tl_model::client::EngineClient;
    use tl_model::settings::{Settings, UiState};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::metrics;
    use tl_ui::{AppEnv, TasklensUi, UiInit};

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let mut settings = Settings::default();
    settings.theme.follow_system = true;
    let (_tx, rx) = async_channel::bounded(4);
    let writes = Rc::new(RefCell::new(0u32));
    let w = writes.clone();
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(move |_, _| *w.borrow_mut() += 1),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| TasklensUi::init(cx, client, init));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(&*current(cx).name, "Default Dark"));

    let publish_scheme = |scheme: u8| {
        engine.publish_with(|s| {
            let system = Arc::make_mut(&mut s.system);
            system.header.version += 1;
            system.color_scheme = scheme;
        });
    };

    publish_scheme(1);
    cx.run_until_parked();
    let n = cx.update(|cx| {
        assert_eq!(&*current(cx).name, "Default Dark");
        assert_eq!(cx.global::<ThemeStore>().system_scheme, 1);
        metrics(cx).style_version
    });

    publish_scheme(2);
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(&*current(cx).name, "Default Light");
        assert_eq!(Theme::global(cx).colors.background, hex(0xF3F4F6));
        assert_eq!(metrics(cx).style_version, n + 1);
    });

    // A SYSTEM change with the same scheme does not activate again.
    publish_scheme(2);
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(metrics(cx).style_version, n + 1));

    // FollowSystem writes no settings.
    assert_eq!(*writes.borrow(), 0);
}
