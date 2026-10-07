//! Settings window frame, page list and `PageBuilder` (E21-018, UI-SHL-060, APP-023). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowHandle, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{NetworkUnit, Settings, StartTab, UiState};
use tl_model::table::TableMode;
use tl_ui::actions::OpenSettings;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::slider::TlSliderEvent;
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind};
use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::ShellState;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::settings::{
    PageBuilder, PageId, SIDEBAR_W, SettingsWindow, TestPages, all_pages,
};

struct Parent;

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("parent")
    }
}

type Frame = TlSecondaryWindow<SettingsWindow>;

/// A test page (under the Advanced id) with the Animations switch and a smooth scroll amount
/// slider.
fn test_pages(_: &mut gpui_kit::App) -> Vec<PageBuilder> {
    let mut page = PageBuilder::new(PageId::Advanced);
    page.switch("Motion", SettingKey::MotionAnimations, "Animations", None)
        .slider(
            "Motion",
            SettingKey::MotionSmoothScrollAmount,
            "Smooth scroll amount",
            1..=10,
            1,
            |v| format!("{v}").into(),
        );
    vec![page]
}

/// `TasklensUi::init` with a persist counter and the test page. Returns the engine and the
/// number of persist calls.
fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, Rc<Cell<u32>>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let persists = Rc::new(Cell::new(0));
    let count = persists.clone();
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(move |_, _| count.set(count.get() + 1)),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        cx.set_global(TestPages(test_pages));
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    (engine, persists)
}

fn settings_window(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::Settings)
    })
    .and_then(|h| h.downcast::<Root>())
}

/// Dispatches `OpenSettings` from a parent window `times` times and returns the window.
fn open(cx: &mut TestAppContext, times: usize) -> (Entity<Frame>, VisualTestContext) {
    let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    for _ in 0..times {
        pcx.update(|window, _| window.activate_window());
        pcx.dispatch_action(OpenSettings);
        pcx.run_until_parked();
    }
    let handle = settings_window(cx).expect("settings window registered");
    let frame = cx
        .update(|cx| Frame::find(handle, cx))
        .expect("settings frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    (frame, vcx)
}

fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

#[gpui_kit::test]
fn settings_window_opens_and_refocuses(cx: &mut TestAppContext) {
    setup(cx);
    let before = cx.update(|cx| cx.windows().len());
    let (frame, vcx) = open(cx, 2);
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        before + 2,
        "the parent and one Settings window"
    );
    let id = settings_window(cx).expect("open").window_id();
    assert_eq!(
        cx.update(|cx| cx.active_window().map(|w| w.window_id())),
        Some(id),
        "the Settings window has focus"
    );
    let (title, kind) = vcx.read(|cx| (frame.read(cx).title().to_string(), frame.read(cx).kind()));
    assert_eq!(title, "tasklens Settings");
    assert_eq!(kind, TlWindowKind::Tool { resizable: true });
}

#[gpui_kit::test]
fn settings_window_size_and_sidebar(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let viewport = vcx.update(|window, _| window.viewport_size());
    assert_eq!(viewport, size(px(900.), px(640.)));
    assert_eq!(SIDEBAR_W, 220.);
    assert!(vcx.debug_bounds("SettingsWindowBody").is_some());
}

#[gpui_kit::test]
fn settings_esc_closes(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open(cx, 1);
    drop(frame);
    let handle = settings_window(cx).expect("open");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!is_open(cx, handle), "Esc closes the window");
}

#[gpui_kit::test]
fn page_builder_records_keys(cx: &mut TestAppContext) {
    setup(cx);
    let (pages, before) =
        cx.update(|cx| (all_pages(cx), TlSettings::settings(cx).motion.animations));
    // The test page comes after the real ones.
    let test_page = pages.last().expect("pages");
    assert_eq!(test_page.id(), PageId::Advanced);
    assert_eq!(
        test_page.keys(),
        [
            SettingKey::MotionAnimations,
            SettingKey::MotionSmoothScrollAmount
        ]
    );
    // Toggling the switch writes through TlSettings.
    cx.update(|cx| test_page.set_bool(SettingKey::MotionAnimations, !before, cx));
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.animations),
        !before
    );
}

#[gpui_kit::test]
fn page_builder_slider_persists_on_release(cx: &mut TestAppContext) {
    let (_, persists) = setup(cx);
    let (frame, vcx) = open(cx, 1);
    let state = vcx
        .read(|cx| {
            frame
                .read(cx)
                .content()
                .read(cx)
                .slider_state(SettingKey::MotionSmoothScrollAmount)
                .cloned()
        })
        .expect("slider state created in new");
    let before = persists.get();
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Preview(7.))));
    cx.run_until_parked();
    assert_eq!(persists.get(), before, "Change writes in memory only");
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.smooth_scroll_amount),
        7
    );
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Commit(8.))));
    cx.run_until_parked();
    assert_eq!(persists.get(), before + 1, "Release persists once");
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.smooth_scroll_amount),
        8
    );
}

#[gpui_kit::test]
fn settings_window_ignores_unrelated_snapshots(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    let (frame, mut vcx) = open(cx, 1);
    let content = vcx.read(|cx| frame.read(cx).content().clone());
    let notifies = Rc::new(Cell::new(0));
    let count = notifies.clone();
    let _watch = vcx.update(|_, cx| cx.observe(&content, move |_, _| count.set(count.get() + 1)));
    engine.publish_with(|s| {
        Arc::make_mut(&mut s.processes).header.version += 1;
    });
    cx.run_until_parked();
    assert_eq!(notifies.get(), 0, "a PROCESSES snapshot does not re-render");
    let start = vcx.read(|cx| content.read(cx).render_count());
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        vcx.read(|cx| content.read(cx).render_count()) > start,
        "the counter counts"
    );

    // The device catalog does re-render it.
    engine.publish_with(|s| Arc::make_mut(&mut s.devices).version += 1);
    cx.run_until_parked();
    assert!(notifies.get() > 0, "DEVICES re-renders");
}

/// The real page with `id`.
fn page(cx: &mut TestAppContext, id: PageId) -> PageBuilder {
    cx.update(all_pages)
        .into_iter()
        .find(|p| p.id() == id)
        .expect("page listed")
}

#[gpui_kit::test]
fn general_default_tab_options(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| cx.default_global::<ShellState>().built_tabs = Tab::ALL.into_iter().collect());
    let general = page(cx, PageId::General);
    let labels: Vec<String> = general
        .dropdown_labels(SettingKey::GeneralStartTab)
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(
        labels,
        [
            "Processes",
            "Performance",
            "Services",
            "Startup Apps",
            "Connections",
            "Information",
            "Last used"
        ]
    );
    cx.update(|cx| general.pick(SettingKey::GeneralStartTab, "Last used", cx));
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).general.start_tab),
        StartTab::LastUsed
    );
    let stored = cx.update(|cx| serde_json::to_value(&TlSettings::settings(cx).general));
    assert_eq!(stored.expect("serialize")["start_tab"], "last_used");
}

#[gpui_kit::test]
fn general_language_options_and_desc(cx: &mut TestAppContext) {
    setup(cx);
    let general = page(cx, PageId::General);
    let labels = general.dropdown_labels(SettingKey::GeneralLanguage);
    assert_eq!(labels, ["System default", "English (United States)"]);
    assert_eq!(
        general
            .description_of(SettingKey::GeneralLanguage)
            .as_deref(),
        Some("Numbers and dates keep the same format in every language.")
    );
    cx.update(|cx| general.pick(SettingKey::GeneralLanguage, "English (United States)", cx));
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).general.language.clone()),
        "en-US"
    );
}

#[gpui_kit::test]
fn units_network_dropdown_sends_engine_settings(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    let units = page(cx, PageId::Units);
    assert_eq!(
        units.dropdown_labels(SettingKey::UnitsNetwork),
        ["Bits per second", "Bytes per second"]
    );
    cx.update(|cx| units.pick(SettingKey::UnitsNetwork, "Bytes per second", cx));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).units.network),
        NetworkUnit::Bytes
    );
    assert_eq!(
        engine.settings().last().map(|s| s.network),
        Some(NetworkUnit::Bytes),
        "the engine gets the bytes unit"
    );
}

#[gpui_kit::test]
fn general_units_keys(cx: &mut TestAppContext) {
    setup(cx);
    let keys: Vec<SettingKey> = [PageId::General, PageId::Units]
        .into_iter()
        .flat_map(|id| page(cx, id).keys().to_vec())
        .collect();
    assert_eq!(
        keys,
        [
            SettingKey::GeneralStartTab,
            SettingKey::GeneralLanguage,
            SettingKey::UnitsNetwork,
            SettingKey::UnitsNetworkAutoScale
        ]
    );
}

#[gpui_kit::test]
fn motion_reduced_motion_notes(cx: &mut TestAppContext) {
    // `setup` turns the system reduce-motion on.
    setup(cx);
    let motion = page(cx, PageId::Motion);
    assert_eq!(
        motion
            .page_description_text()
            .map(|t| t.to_string())
            .as_deref(),
        Some("Your system's reduced motion setting turns all of these off.")
    );
    assert_eq!(
        motion.banners(),
        ["Reduced motion is on in your system settings"]
    );
    assert_eq!(
        motion
            .description_of(SettingKey::MotionSmoothGraphs)
            .as_deref(),
        Some("Turned off by the system reduce-motion setting")
    );
    assert!(
        cx.update(|cx| TlSettings::settings(cx).motion.smooth_graphs),
        "Smooth Graphs keeps its stored value"
    );

    cx.update(|cx| cx.set_reduce_motion(false));
    let motion = page(cx, PageId::Motion);
    assert!(motion.banners().is_empty(), "no banner");
    assert_eq!(motion.description_of(SettingKey::MotionSmoothGraphs), None);
}

#[gpui_kit::test]
fn motion_scroll_amount_disabled(cx: &mut TestAppContext) {
    setup(cx);
    assert!(!page(cx, PageId::Motion).is_disabled(SettingKey::MotionSmoothScrollAmount));
    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::MotionSmoothScroll,
            SettingValue::Bool(false),
        )
    });
    assert!(page(cx, PageId::Motion).is_disabled(SettingKey::MotionSmoothScrollAmount));
}

#[gpui_kit::test]
fn motion_tab_switch_slider(cx: &mut TestAppContext) {
    let (_, persists) = setup(cx);
    let (frame, vcx) = open(cx, 1);
    let state = vcx
        .read(|cx| {
            frame
                .read(cx)
                .content()
                .read(cx)
                .slider_state(SettingKey::MotionTabSwitchMs)
                .cloned()
        })
        .expect("Tab Switch slider state");
    let before = persists.get();
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Commit(300.))));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.tab_switch_ms),
        300
    );
    assert_eq!(persists.get(), before + 1, "Release persists");
}

#[gpui_kit::test]
fn motion_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::Motion).keys(),
        [
            SettingKey::MotionAnimations,
            SettingKey::MotionSmoothGraphs,
            SettingKey::MotionSmoothScroll,
            SettingKey::MotionSmoothScrollAmount,
            SettingKey::MotionTabSwitchMs
        ]
    );
}

#[gpui_kit::test]
fn processes_refresh_dropdown(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    let processes = page(cx, PageId::Processes);
    assert_eq!(
        processes.dropdown_labels(SettingKey::ProcessesIntervalMs),
        ["0.25s", "0.5s", "1.0s", "2.0s", "5.0s", "10.0s"]
    );
    cx.update(|cx| processes.pick(SettingKey::ProcessesIntervalMs, "2.0s", cx));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).processes.interval_ms),
        2000
    );
    assert_eq!(
        engine.settings().last().map(|s| s.interval_ms),
        Some(2000),
        "the engine gets the interval"
    );
}

#[gpui_kit::test]
fn processes_hide_system_engine_settings(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::ProcessesHideSystem,
            SettingValue::Bool(false),
        )
    });
    let processes = page(cx, PageId::Processes);
    cx.update(|cx| processes.set_bool(SettingKey::ProcessesHideSystem, true, cx));
    cx.run_until_parked();
    assert!(cx.update(|cx| TlSettings::settings(cx).processes.hide_system));
    assert_eq!(
        engine.settings().last().map(|s| s.hide_system),
        Some(true),
        "the engine gets hide_system"
    );
}

#[gpui_kit::test]
fn processes_mode_no_toast(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let processes = page(cx, PageId::Processes);
    assert_eq!(
        processes.dropdown_labels(SettingKey::ProcessesMode),
        ["Tree View", "Task Manager Grouping", "List View"]
    );
    vcx.update(|_, cx| processes.pick(SettingKey::ProcessesMode, "List View", cx));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).processes.mode),
        TableMode::Flat
    );
    let toasts = vcx.update(|window, cx| {
        let queue =
            ToastQueue::of_window(window, cx).expect("the Settings window has a toast layer");
        let q = queue.read(cx);
        (q.visible_texts(), q.waiting_texts())
    });
    assert!(toasts.0.is_empty() && toasts.1.is_empty(), "{toasts:?}");
}

#[gpui_kit::test]
fn processes_page_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::Processes).keys(),
        [
            SettingKey::ProcessesIntervalMs,
            SettingKey::ProcessesMode,
            SettingKey::ProcessesTreeGuideLines,
            SettingKey::ProcessesHighlightNewExited,
            SettingKey::ProcessesFilterIncludeChildren,
            SettingKey::ProcessesHideSystem,
            SettingKey::ProcessesSubtreeTotals,
            SettingKey::TablesColumnResize,
            SettingKey::TablesDimZeroValues,
            SettingKey::PropertiesHideUnnamedHandles,
            SettingKey::PropertiesSharedObjectsOnly,
            SettingKey::PropertiesEnvMask
        ]
    );
}

/// Puts one present GPU, "Test GPU", in the device catalog.
fn publish_gpu(engine: &FakeEngine, cx: &mut TestAppContext) {
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(vec![DeviceEntry {
                idx: DeviceIdx(0),
                identity: DeviceIdentity::Gpu {
                    uuid: [1; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                generation: 0,
                display_name: Arc::from("Test GPU"),
                present: true,
                parent: None,
                persistent_key: Arc::from(""),
                net_kind: None,
            }]),
        });
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn performance_range_dropdown(cx: &mut TestAppContext) {
    setup(cx);
    let perf = page(cx, PageId::Performance);
    assert_eq!(
        perf.dropdown_labels(SettingKey::PerformanceGraphRangeS),
        ["30 seconds", "1 minute", "5 minutes", "10 minutes"]
    );
    cx.update(|cx| perf.pick(SettingKey::PerformanceGraphRangeS, "5 minutes", cx));
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).performance.graph_range_s),
        300
    );
}

#[gpui_kit::test]
fn performance_gpu_optional_checkboxes(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::PerformanceGpuOptionalGraphs,
            SettingValue::StrList(Arc::from([])),
        )
    });
    let perf = page(cx, PageId::Performance);
    assert_eq!(
        perf.check_values(SettingKey::PerformanceGpuOptionalGraphs),
        ["power", "clocks"]
    );
    cx.update(|cx| perf.set_check(SettingKey::PerformanceGpuOptionalGraphs, "clocks", true, cx));
    let graphs = cx.update(|cx| {
        serde_json::to_value(&TlSettings::settings(cx).performance.gpu_optional_graphs)
            .expect("serialize")
    });
    assert_eq!(graphs, serde_json::json!(["clocks"]));
}

#[gpui_kit::test]
fn performance_hidden_engines_checklist(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_gpu(&engine, cx);
    let perf = page(cx, PageId::Performance);
    let engines = perf.check_values(SettingKey::PerformanceHiddenGpuEngines);
    assert_eq!(engines, ["gpu", "enc", "dec"], "the GPU's engines in order");
    let hidden = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            TlSettings::settings(cx)
                .performance
                .hidden_gpu_engines
                .clone()
        })
    };
    assert!(hidden(cx).is_empty());
    // Checked = shown: unchecking hides the engine.
    cx.update(|cx| perf.set_check(SettingKey::PerformanceHiddenGpuEngines, "dec", false, cx));
    assert_eq!(hidden(cx), ["dec"]);
    cx.update(|cx| perf.set_check(SettingKey::PerformanceHiddenGpuEngines, "dec", true, cx));
    assert!(hidden(cx).is_empty());
}

#[gpui_kit::test]
fn performance_no_gpu_text(cx: &mut TestAppContext) {
    setup(cx);
    let perf = page(cx, PageId::Performance);
    assert!(
        perf.check_values(SettingKey::PerformanceHiddenGpuEngines)
            .is_empty()
    );
    assert_eq!(
        perf.empty_text(SettingKey::PerformanceHiddenGpuEngines)
            .as_deref(),
        Some("No GPU found")
    );
}

#[gpui_kit::test]
fn performance_network_switch_engine_settings(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    assert!(!cx.update(|cx| TlSettings::settings(cx).performance.show_network_virtual));
    let perf = page(cx, PageId::Performance);
    cx.update(|cx| perf.set_bool(SettingKey::PerformanceShowNetworkVirtual, true, cx));
    cx.run_until_parked();
    assert!(cx.update(|cx| TlSettings::settings(cx).performance.show_network_virtual));
    assert_eq!(
        engine.settings().last().map(|s| s.show_network_virtual),
        Some(true),
        "the engine gets show_network_virtual"
    );
}

#[gpui_kit::test]
fn performance_page_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::Performance).keys(),
        [
            SettingKey::PerformanceGraphRangeS,
            SettingKey::PerformanceCurvedLines,
            SettingKey::PerformanceNetworkDynamicScale,
            SettingKey::PerformanceCpuGraphMode,
            SettingKey::PerformanceCpuKernel,
            SettingKey::PerformanceCpuSecondaryGraph,
            SettingKey::PerformanceMemoryComposition,
            SettingKey::PerformanceMemorySwapGraph,
            SettingKey::PerformanceGpuOptionalGraphs,
            SettingKey::PerformanceHiddenGpuEngines,
            SettingKey::PerformanceShowDisks,
            SettingKey::PerformanceShowGpus,
            SettingKey::PerformanceShowFans,
            SettingKey::PerformanceShowBatteries,
            SettingKey::PerformanceShowNetworkWired,
            SettingKey::PerformanceShowNetworkWireless,
            SettingKey::PerformanceShowNetworkVpn,
            SettingKey::PerformanceShowNetworkVirtual,
            SettingKey::PerformanceShowNetworkOther
        ]
    );
}
