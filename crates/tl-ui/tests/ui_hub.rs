//! SnapshotHub and `TasklensUi::init` integration tests with `FakeEngine` and `TestAppContext`
//! (E16-003, E16-A1, ADR-005, ADR-010, ADR-013). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext, AssetSource, Context, Entity, Font, FontWeight, IntoElement, ParentElement,
    QuitMode, Render, Styled, TestAppContext, TestDispatcher, Window, div, font,
};
use tl_model::client::EngineClient;
use tl_model::command::AppRequest;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::SeriesView;
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{SeriesSet, Snapshot};
use tl_ui::app::{
    AppRequestHandler, BootSettings, EngineGlobal, UiExitSlot, open_first_for_test, start_for_test,
};
use tl_ui::components::icons::TlAssets;
use tl_ui::demand::GlobalDemand;
use tl_ui::hub::GlobalHub;
use tl_ui::hub::{ComponentMask, HiddenChanged, SnapshotChanged, SnapshotHub};
use tl_ui::i18n::Strings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::text::fonts::BundledFontStatus;
use tl_ui::theme::{ThemeStore, UiMetrics};
use tl_ui::{
    AppEnv, OpenHooks, OpenMode, StartOptions, TasklensUi, UiExit, UiInit, UiLaunch, request_exit,
};

fn hub_with_engine(cx: &mut TestAppContext) -> (Arc<FakeEngine>, Entity<SnapshotHub>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(client, cx));
    (engine, hub)
}

/// Records every `SnapshotChanged` the hub emits.
fn record_changes(
    hub: &Entity<SnapshotHub>,
    cx: &mut TestAppContext,
) -> Rc<RefCell<Vec<SnapshotChanged>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    cx.update(|cx| {
        cx.subscribe(hub, move |_, ev: &SnapshotChanged, _| {
            sink.borrow_mut().push(ev.clone());
        })
        .detach();
    });
    seen
}

/// Records every `HiddenChanged` the hub emits.
fn record_hidden(hub: &Entity<SnapshotHub>, cx: &mut TestAppContext) -> Rc<RefCell<Vec<bool>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    cx.update(|cx| {
        cx.subscribe(hub, move |_, ev: &HiddenChanged, _| {
            sink.borrow_mut().push(ev.hidden);
        })
        .detach();
    });
    seen
}

fn sid(device: u16, metric: MetricId) -> SeriesId {
    SeriesId {
        device: DeviceIdx(device),
        metric,
    }
}

fn view(id: SeriesId, version: u64) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id,
        version,
        period_ns: 1_000_000_000,
        samples: Arc::from([]),
    })
}

/// Replaces the series set with `views` (sorted by id) and the given set header version.
fn set_series(s: &mut Snapshot, header_version: u64, mut views: Vec<Arc<SeriesView>>) {
    views.sort_by_key(|v| v.id);
    let mut set = SeriesSet::default();
    set.header.version = header_version;
    set.series = views.into();
    s.series = Arc::new(set);
}

#[gpui_kit::test]
fn ignores_old_sequence(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let seen = record_changes(&hub, cx);

    let mut snap = Snapshot::clone(&engine.latest());
    snap.sequence = 5;
    Arc::make_mut(&mut snap.processes).header.version = 1;
    engine.publish(snap);
    cx.run_until_parked();
    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), 5);
    assert_eq!(seen.borrow().len(), 1);

    for sequence in [5, 4] {
        let mut old = Snapshot::clone(&engine.latest());
        old.sequence = sequence;
        Arc::make_mut(&mut old.processes).header.version += 10;
        let old = Arc::new(old);
        hub.update(cx, |h, cx| h.install(Arc::clone(&old), cx));
        cx.run_until_parked();
        hub.read_with(cx, |h, _| {
            assert_eq!(h.current().sequence, 5);
            assert_eq!(h.current().processes.header.version, 1);
        });
    }
    assert_eq!(seen.borrow().len(), 1);
}

/// Bumps one component version.
type Bump = fn(&mut Snapshot);

#[gpui_kit::test]
fn mask_per_component(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let seen = record_changes(&hub, cx);

    let bumps: [(ComponentMask, Bump); 13] = [
        (ComponentMask::SYSTEM, |s| {
            Arc::make_mut(&mut s.system).header.version += 1
        }),
        (ComponentMask::SERIES, |s| {
            Arc::make_mut(&mut s.series).header.version += 1
        }),
        (ComponentMask::PROCESSES, |s| {
            Arc::make_mut(&mut s.processes).header.version += 1
        }),
        (ComponentMask::TABLE, |s| {
            Arc::make_mut(&mut s.table).header.version += 1
        }),
        (ComponentMask::CONNECTIONS, |s| {
            Arc::make_mut(&mut s.connections).header.version += 1
        }),
        (ComponentMask::SERVICES, |s| {
            Arc::make_mut(&mut s.services).header.version += 1
        }),
        (ComponentMask::STARTUP, |s| {
            Arc::make_mut(&mut s.startup).header.version += 1
        }),
        (ComponentMask::INVENTORY, |s| {
            Arc::make_mut(&mut s.inventory).header.version += 1
        }),
        (ComponentMask::DEVICES, |s| {
            Arc::make_mut(&mut s.devices).version += 1
        }),
        (ComponentMask::HEALTH, |s| {
            Arc::make_mut(&mut s.health).version += 1
        }),
        (ComponentMask::WINDOWS, |s| {
            Arc::make_mut(&mut s.windows).header.version += 1
        }),
        (ComponentMask::DETAILS, |s| {
            Arc::make_mut(&mut s.details).header.version += 1
        }),
        (ComponentMask::HISTORY, |s| {
            Arc::make_mut(&mut s.history).header.version += 1
        }),
    ];

    for (i, (bit, bump)) in bumps.into_iter().enumerate() {
        engine.publish_with(bump);
        cx.run_until_parked();
        let seen = seen.borrow();
        assert_eq!(seen.len(), i + 1, "one event for {bit:?}");
        let ev = &seen[i];
        assert_eq!(ev.mask, bit);
        assert!(ev.changed_series.is_empty());
    }
    // A process change sets PROCESSES and not TABLE.
    assert!(!seen.borrow()[2].mask.contains(ComponentMask::TABLE));
}

#[gpui_kit::test]
fn changed_series_lists_only_changed(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let seen = record_changes(&hub, cx);

    let ids = [
        sid(0, MetricId::CPU_TOTAL),
        sid(0, MetricId::CPU_KERNEL),
        sid(1, MetricId::CPU_TOTAL),
        sid(1, MetricId::CPU_KERNEL),
        sid(0, MetricId::MEM_USED),
    ];
    engine.publish_with(|s| set_series(s, 1, ids.iter().map(|&id| view(id, 1)).collect()));
    cx.run_until_parked();
    assert_eq!(seen.borrow().len(), 1);

    // Bump two of the five CPU-side series and add one new; the set header stays the same.
    let added = sid(2, MetricId::CPU_TOTAL);
    engine.publish_with(|s| {
        let mut views: Vec<_> = ids
            .iter()
            .map(|&id| {
                let version = if id == ids[1] || id == ids[2] { 2 } else { 1 };
                view(id, version)
            })
            .collect();
        views.push(view(added, 1));
        set_series(s, 1, views);
    });
    cx.run_until_parked();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    let ev = &seen[1];
    assert_eq!(ev.mask, ComponentMask::SERIES);
    let mut expected = vec![ids[1], ids[2], added];
    expected.sort();
    assert_eq!(ev.changed_series.as_slice(), expected.as_slice());
    assert!(!ev.changed_series.spilled());
}

#[gpui_kit::test]
fn ten_thousand_publishes_monotonic(cx: &mut TestAppContext) {
    const LAST: u64 = 10_000;
    let (engine, hub) = hub_with_engine(cx);

    let observed = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&observed);
    cx.update(|cx| {
        cx.subscribe(&hub, move |hub, _: &SnapshotChanged, cx| {
            sink.borrow_mut().push(hub.read(cx).current().sequence);
        })
        .detach();
    });

    // The publisher wakes the hub task from another thread, which the test scheduler rejects as
    // non-deterministic unless parking is allowed.
    cx.executor().allow_parking();
    let publisher = Arc::clone(&engine);
    let thread = std::thread::spawn(move || {
        for _ in 0..LAST {
            publisher.publish_with(|s| Arc::make_mut(&mut s.system).header.version += 1);
        }
    });
    while !thread.is_finished() {
        cx.run_until_parked();
        std::thread::yield_now();
    }
    thread.join().expect("publisher thread");
    cx.run_until_parked();

    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), LAST);
    let observed = observed.borrow();
    assert_eq!(observed.last(), Some(&LAST));
    assert!(
        observed.windows(2).all(|w| w[0] < w[1]),
        "sequence went backwards"
    );
}

/// Draws nothing and notifies itself only when `PROCESSES` changed.
struct ProcessView {
    renders: Rc<Cell<u32>>,
}

impl ProcessView {
    fn new(hub: &Entity<SnapshotHub>, renders: Rc<Cell<u32>>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(hub, |_, _, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::PROCESSES) {
                cx.notify();
            }
        })
        .detach();
        Self { renders }
    }
}

impl Render for ProcessView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child("processes")
    }
}

#[gpui_kit::test]
fn cpu_only_change_does_not_notify_process_view(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let renders = Rc::new(Cell::new(0));
    let counter = Rc::clone(&renders);
    let hub_for_view = hub.clone();
    let (_view, cx) = cx.add_window_view(move |_, cx| ProcessView::new(&hub_for_view, counter, cx));
    cx.run_until_parked();
    let base = renders.get();
    assert!(base >= 1, "the window drew once");

    let cpu = sid(0, MetricId::CPU_TOTAL);
    engine.publish_with(|s| set_series(s, 1, vec![view(cpu, 1)]));
    cx.run_until_parked();
    assert_eq!(renders.get(), base, "CPU-only change must not render");

    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    assert_eq!(renders.get(), base + 1, "process change renders once");
}

#[gpui_kit::test]
fn hidden_after_1000ms_pending_without_render(cx: &mut TestAppContext) {
    let (engine, hub) = hub_with_engine(cx);
    let hidden = record_hidden(&hub, cx);

    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    assert!(!hub.read_with(cx, |h, _| h.is_hidden()));

    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
    assert!(hub.read_with(cx, |h, _| h.is_hidden()));
    assert_eq!(*hidden.borrow(), vec![true]);

    // More time passes: still one event. Snapshots keep installing while hidden.
    cx.executor().advance_clock(Duration::from_millis(2000));
    cx.run_until_parked();
    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), 2);
    assert_eq!(*hidden.borrow(), vec![true]);

    hub.update(cx, |h, cx| h.note_render(cx));
    assert!(!hub.read_with(cx, |h, _| h.is_hidden()));
    assert_eq!(*hidden.borrow(), vec![true, false]);
}

#[gpui_kit::test]
fn no_hidden_without_pending_change(cx: &mut TestAppContext) {
    let (_engine, hub) = hub_with_engine(cx);
    let hidden = record_hidden(&hub, cx);

    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
    }
    assert!(!hub.read_with(cx, |h, _| h.is_hidden()));
    assert!(hidden.borrow().is_empty());
}

// TasklensUi::init (E16-A1).

/// A `UiInit` with default settings and state, no-op callbacks and `AppEnv::test()`. Returns
/// the sender end of the `AppRequest` channel.
fn ui_init(settings: Settings) -> (UiInit, async_channel::Sender<AppRequest>) {
    let (tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    (init, tx)
}

/// Runs init with `settings` against a new `FakeEngine`.
fn run_init(
    cx: &mut TestAppContext,
    settings: Settings,
) -> (Arc<FakeEngine>, async_channel::Sender<AppRequest>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (init, tx) = ui_init(settings);
    cx.update(|cx| TasklensUi::init(cx, client, init));
    cx.run_until_parked();
    (engine, tx)
}

#[gpui_kit::test]
fn init_installs_globals(cx: &mut TestAppContext) {
    run_init(cx, Settings::default());
    cx.update(|cx| {
        assert!(cx.has_global::<ThemeStore>());
        assert!(cx.has_global::<UiMetrics>());
        assert!(cx.has_global::<Strings>());
        assert!(cx.has_global::<EngineGlobal>());
        assert!(cx.has_global::<GlobalHub>());
        assert!(cx.has_global::<GlobalDemand>());
        assert!(cx.has_global::<BootSettings>());
        assert!(cx.has_global::<AppEnv>());
        assert!(cx.has_global::<AppRequestHandler>());
        let boot = cx.global::<BootSettings>();
        // TlSettings::init takes the persist callback out of BootSettings (jqk.3).
        assert!(boot.persist.is_none());
        assert!(cx.has_global::<tl_ui::settings_global::TlSettings>());
        assert!(cx.has_global::<tl_ui::theme::user_store::ThemePersist>());
    });
}

#[gpui_kit::test]
fn init_installs_app_env(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (mut init, _tx) = ui_init(Settings::default());
    init.env.debug = true;
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        assert!(cx.global::<AppEnv>().debug);
    });
}

#[test]
fn init_loads_three_weights() {
    // The default `TestAppContext` text system resolves every font, so use the real one.
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (init, _tx) = ui_init(Settings::default());
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        assert!(cx.global::<BundledFontStatus>().0.ui_ok);
        let text_system = cx.text_system().clone();
        let ids: Vec<_> = [FontWeight::NORMAL, FontWeight::MEDIUM, FontWeight::SEMIBOLD]
            .into_iter()
            .map(|weight| {
                let id = text_system.resolve_font(&Font {
                    weight,
                    ..font("Noto Sans")
                });
                let resolved = text_system.get_font_for_id(id).expect("font id maps back");
                assert_eq!(resolved.family.as_ref(), "Noto Sans", "weight {weight:?}");
                id
            })
            .collect();
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
        assert_ne!(ids[0], ids[2]);
    });
}

#[gpui_kit::test]
fn app_request_reaches_handler(cx: &mut TestAppContext) {
    let (_engine, tx) = run_init(cx, Settings::default());
    let main_thread = std::thread::current().id();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    cx.update(|cx| {
        cx.set_global(AppRequestHandler(Box::new(move |req, _| {
            assert_eq!(std::thread::current().id(), main_thread);
            sink.borrow_mut().push(req);
        })));
    });
    tx.try_send(AppRequest::SelectPid(42))
        .expect("channel has room");
    cx.run_until_parked();
    assert_eq!(*seen.borrow(), vec![AppRequest::SelectPid(42)]);
}

#[gpui_kit::test]
fn init_wires_hub_to_engine(cx: &mut TestAppContext) {
    let (engine, _tx) = run_init(cx, Settings::default());
    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    cx.run_until_parked();
    let hub = cx.update(|cx| cx.global::<GlobalHub>().0.clone());
    assert_eq!(hub.read_with(cx, |h, _| h.current().sequence), 1);
}

#[gpui_kit::test]
fn init_activates_theme_without_crossfade(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.theme.active = "Default Light".to_owned();
    run_init(cx, settings);
    cx.update(|cx| {
        assert_eq!(&*tl_ui::theme::current(cx).name, "Default Light");
        // `init_metrics` sets 1; nothing else bumped it.
        assert_eq!(tl_ui::theme::metrics(cx).style_version, 1);
    });
}

// tl_ui::run (E16-012). `run` blocks, so these go through its parts.

fn exit_slot(cx: &mut TestAppContext) -> Option<UiExit> {
    cx.update(|cx| cx.try_global::<UiExitSlot>().and_then(|s| s.0.clone()))
}

#[gpui_kit::test]
fn open_first_calls_main_hook(cx: &mut TestAppContext) {
    run_init(cx, Settings::default());
    let calls = Rc::new(Cell::new(0));
    let sink = Rc::clone(&calls);
    cx.update(|cx| {
        cx.default_global::<OpenHooks>().main = Some(Box::new(move |_| sink.set(sink.get() + 1)));
        open_first_for_test(cx, OpenMode::Main);
    });
    assert_eq!(calls.get(), 1);
    assert_eq!(exit_slot(cx), None);
}

#[gpui_kit::test]
fn open_first_missing_hook_quits(cx: &mut TestAppContext) {
    run_init(cx, Settings::default());
    cx.update(|cx| open_first_for_test(cx, OpenMode::Gallery));
    assert_eq!(exit_slot(cx), Some(UiExit::Quit));
}

#[gpui_kit::test]
fn request_exit_first_writer_wins(cx: &mut TestAppContext) {
    cx.update(|cx| {
        request_exit(cx, UiExit::DeviceLost);
        request_exit(cx, UiExit::Quit);
    });
    assert_eq!(exit_slot(cx), Some(UiExit::DeviceLost));
}

#[gpui_kit::test]
fn start_options_global_installed(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let (init, _tx) = ui_init(Settings::default());
    let launch = UiLaunch {
        client: engine,
        init,
        open: OpenMode::Gallery,
        start: StartOptions {
            select_pid: Some(42),
            ..StartOptions::default()
        },
    };
    let exit = cx.update(|cx| start_for_test(cx, launch));
    cx.update(|cx| assert_eq!(cx.global::<StartOptions>().select_pid, Some(42)));
    // No gallery hook is registered, so the launch quits; the cell `run` reads agrees.
    assert_eq!(exit_slot(cx), Some(UiExit::Quit));
    assert_eq!(exit.take(), Some(UiExit::Quit));
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui_kit::test]
fn explicit_quit_mode_survives_last_window(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_quit_mode(QuitMode::Explicit));
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, _| window.remove_window())
        .expect("window is open");
    cx.run_until_parked();
    // The test platform's `quit` does nothing, so check that the app still runs and that
    // no exit was requested.
    assert!(cx.windows().is_empty());
    assert_eq!(exit_slot(cx), None);
    let again = cx.add_window(|_, _| Empty);
    assert_eq!(cx.windows(), vec![again.into()]);
}

#[test]
fn run_asset_source_resolves_icons() {
    // `run` passes `TlAssets` to `Application::with_assets`; the kit and tasklens icons must
    // resolve through it.
    let kit_icon = "icons/chevron-down.svg";
    assert!(TlAssets.load(kit_icon).unwrap().is_some(), "{kit_icon}");
    let tl = TlAssets.list("icons/tl/").unwrap();
    assert!(!tl.is_empty());
    assert!(TlAssets.load(&tl[0]).unwrap().is_some());
}
