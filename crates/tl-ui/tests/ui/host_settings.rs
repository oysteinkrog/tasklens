//! Host settings in the window (tasklens-q4x76.23, ADR-018 Settings): the window reads
//! `processes.interval_ms` and `[history]` only from the engine's snapshot, keeps their controls
//! disabled until it has seen them, writes them with the revision it read, and never writes them
//! to `settings.toml`. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::client::EngineClient;
use tl_model::remote_proto::HostSettings;
use tl_model::settings::{Settings, TimelineClick, UiState};
use tl_model::snapshot::Snapshot;
use tl_model::time::BootId;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::host_settings::{self, HOST_KEYS};
use tl_ui::pages::performance::overview::timeline::click_mode;
use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::menus_settings;
use tl_ui::window::settings::{PageBuilder, PageId, all_pages};

/// Every `settings.toml` write, in order.
type Persisted = Rc<RefCell<Vec<Settings>>>;

/// `TasklensUi::init` over `engine` with `settings` as the loaded `settings.toml`.
fn setup(cx: &mut TestAppContext, engine: &Arc<FakeEngine>, settings: Settings) -> Persisted {
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let persisted: Persisted = Rc::default();
    let sink = Rc::clone(&persisted);
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(move |s, _| sink.borrow_mut().push(s.clone())),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    persisted
}

fn processes_page(cx: &mut TestAppContext) -> PageBuilder {
    cx.update(all_pages)
        .into_iter()
        .find(|p| p.id() == PageId::Processes)
        .expect("Processes page listed")
}

/// Before the engine gave its host settings, no control sends anything and every host row is
/// disabled; once they arrive, a change goes out with the revision read.
#[gpui_kit::test]
fn a_fresh_client_never_sends_values_it_has_not_read(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    // An engine that has not given them yet: revision 0.
    engine.publish(Snapshot::empty(BootId(0), 0));
    setup(cx, &engine, Settings::default());
    assert!(!cx.update(|cx| host_settings::known(cx)));

    cx.update(|cx| {
        TlSettings::set(cx, SettingKey::ProcessesIntervalMs, SettingValue::Int(2000));
        TlSettings::toggle(cx, SettingKey::HistoryEnabled);
        TlSettings::set(cx, SettingKey::HistoryRetentionH, SettingValue::Int(12));
        host_settings::change(cx, |v| v.history.persist = false);
    });
    cx.run_until_parked();
    assert!(
        engine.host_writes().is_empty(),
        "nothing read, nothing sent"
    );
    assert!(engine.settings().is_empty());

    let page = processes_page(cx);
    for key in HOST_KEYS {
        assert!(page.is_disabled(key), "{key:?} waits for the engine");
    }
    assert_eq!(
        page.tooltip_of(SettingKey::HistoryEnabled).as_deref(),
        Some("Waiting for the engine's settings")
    );
    assert!(
        !page.is_disabled(SettingKey::ProcessesMode),
        "a window setting"
    );

    // The engine gives them: the rows open, and a change carries revision 1.
    engine.publish_host_settings(|_| {});
    cx.run_until_parked();
    let page = processes_page(cx);
    assert!(!page.is_disabled(SettingKey::ProcessesIntervalMs));
    assert!(!page.is_disabled(SettingKey::HistoryEnabled));
    assert_eq!(page.tooltip_of(SettingKey::HistoryEnabled), None);
    cx.update(|cx| page.set_bool(SettingKey::HistoryEnabled, true, cx));
    cx.run_until_parked();
    let writes = engine.host_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, 1);
    assert!(writes[0].1.history.as_ref().is_some_and(|h| h.enabled));
    assert_eq!(writes[0].1.interval_ms, None, "only what changed");
    assert_eq!(writes[0].1.remote_admin, None, "never from the window");
    assert!(cx.update(|cx| host_settings::values(cx).history.enabled));
}

/// Another window changed the interval after this one read it: the write conflicts, nothing of
/// it applies, and the window shows the current values.
#[gpui_kit::test]
fn a_conflicting_write_shows_the_current_values(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    setup(cx, &engine, Settings::default());
    engine.change_host_settings_elsewhere(|v| v.interval_ms = 5000);

    cx.update(|cx| TlSettings::set(cx, SettingKey::ProcessesIntervalMs, SettingValue::Int(2000)));
    cx.run_until_parked();

    let writes = engine.host_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!((writes[0].0, writes[0].1.interval_ms), (1, Some(2000)));
    let shown = cx.update(|cx| host_settings::current(cx));
    assert_eq!((shown.rev, shown.values.interval_ms), (2, 5000));
    // The Refresh menu and the Settings page mark 5.0s, the current value, not 2.0s.
    assert_eq!(
        cx.update(|cx| menus_settings::refresh_marks(cx)),
        [false, false, false, false, true, false]
    );
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).processes.interval_ms),
        1000,
        "settings.toml is not where it lives"
    );
}

/// Host keys changed through the window go to the engine; every `settings.toml` write keeps the
/// values that were loaded, also after a direct `TlSettings::update` of those keys.
#[gpui_kit::test]
fn the_window_never_writes_host_keys_to_settings_toml(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let mut loaded = Settings::default();
    loaded.processes.interval_ms = 500;
    loaded.history.enabled = true;
    loaded.history.retention_h = 3;
    let persisted = setup(cx, &engine, loaded.clone());

    // Each change is made from the newest snapshot, so the second waits for the first's.
    cx.update(|cx| TlSettings::set(cx, SettingKey::ProcessesIntervalMs, SettingValue::Int(2000)));
    cx.run_until_parked();
    cx.update(|cx| TlSettings::set(cx, SettingKey::HistoryRetentionH, SettingValue::Int(12)));
    cx.run_until_parked();
    cx.update(|cx| {
        TlSettings::update(cx, |d| {
            d.settings.history.persist = false;
            d.settings.processes.interval_ms = 10_000;
            // A window key, so this write reaches settings.toml.
            d.settings.motion.animations = false;
        });
    });
    cx.run_until_parked();

    let writes = persisted.borrow().clone();
    assert!(!writes.is_empty(), "the window key was saved");
    for saved in &writes {
        assert_eq!(saved.processes.interval_ms, loaded.processes.interval_ms);
        assert_eq!(saved.history, loaded.history);
    }
    assert!(!writes.last().expect("a write").motion.animations);
    // The two host changes went to the engine, one after the other.
    let sent = engine.host_writes();
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].0, sent[0].1.interval_ms), (1, Some(2000)));
    assert_eq!(sent[1].0, 2, "from the revision the first change made");
    let host = engine.latest().host_settings.clone();
    assert_eq!(*host, {
        let mut want = HostSettings {
            rev: 3,
            ..HostSettings::default()
        };
        want.values.interval_ms = 2000;
        want.values.history.retention_h = 12;
        want
    });
}

/// UI-PRF-057 reads a `history` timeline click as `pause` while recording is off. Recording is a
/// host setting, so the rule reads the engine's value. The saved choice is kept: a window started
/// from a `settings.toml` with recording off still opens History mode once the host records.
#[gpui_kit::test]
fn the_history_timeline_click_follows_the_host_recording_setting(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    // settings.toml from before the engine service: recording off, the history click saved.
    let mut saved = Settings::default();
    saved.performance.timeline_click = TimelineClick::History;
    let persisted = setup(cx, &engine, saved);
    let state = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            (
                TlSettings::settings(cx).performance.timeline_click,
                click_mode(cx),
            )
        })
    };
    assert_eq!(
        state(cx),
        (TimelineClick::History, TimelineClick::Pause),
        "kept, read as pause before the engine gave recording"
    );

    engine.publish_host_settings(|v| v.history.enabled = true);
    cx.run_until_parked();
    assert_eq!(state(cx), (TimelineClick::History, TimelineClick::History));

    // Another write keeps the choice while the host stops recording.
    engine.publish_host_settings(|v| v.history.enabled = false);
    cx.run_until_parked();
    cx.update(|cx| TlSettings::update(cx, |d| d.settings.motion.animations = false));
    assert_eq!(state(cx), (TimelineClick::History, TimelineClick::Pause));
    let last = persisted.borrow().last().cloned().expect("persisted");
    assert_eq!(last.performance.timeline_click, TimelineClick::History);
    assert!(!last.history.enabled, "the host key is not written");
}
