//! Settings window frame, page list and `PageBuilder` (E21-018, UI-SHL-060, APP-023). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle, div, px, size,
};
use tl_model::actions::{ActionKind, DumpMode};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::command::Tab;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{Decorations, NetworkUnit, Settings, StartTab, UiState};
use tl_model::table::TableMode;
use tl_ui::actions::{OpenAdditionalSetup, OpenSettings};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::dialog::{DialogButton, TlDialogView};
use tl_ui::components::slider::TlSliderEvent;
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind};
use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::ThemeStore;
use tl_ui::window::main::ShellState;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::settings::{
    PageBuilder, PageId, SIDEBAR_W, SettingsWindow, TestPages, all_pages,
};

use super::common::is_open;

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

fn no_test_pages(_: &mut gpui_kit::App) -> Vec<PageBuilder> {
    Vec::new()
}

fn p2_pages(cx: &mut TestAppContext) -> Vec<PageBuilder> {
    cx.update(|cx| {
        cx.set_global(TestPages(no_test_pages));
        all_pages(cx)
    })
}

const E21_P2_KEYS: &[SettingKey] = &[
    SettingKey::GeneralStartTab,
    SettingKey::GeneralLanguage,
    SettingKey::ThemeActive,
    SettingKey::ThemeFollowSystem,
    SettingKey::ThemeLight,
    SettingKey::ThemeDark,
    SettingKey::AppearanceUiScalePct,
    SettingKey::AppearanceRowHeightPct,
    SettingKey::AppearanceDecorations,
    SettingKey::AppearanceRoundedCorners,
    SettingKey::AppearanceHeatColoring,
    SettingKey::AppearanceHeatIntensityPct,
    SettingKey::AppearanceShadowIntensityPct,
    SettingKey::MotionAnimations,
    SettingKey::MotionSmoothGraphs,
    SettingKey::MotionSmoothScroll,
    SettingKey::MotionSmoothScrollAmount,
    SettingKey::MotionTabSwitchMs,
    SettingKey::MotionSavePowerOnBattery,
    SettingKey::UnitsNetwork,
    SettingKey::UnitsNetworkAutoScale,
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
    SettingKey::PropertiesEnvMask,
    SettingKey::PerformanceGraphRangeS,
    SettingKey::PerformanceCurvedLines,
    SettingKey::PerformanceNetworkDynamicScale,
    SettingKey::PerformanceCpuGraphMode,
    SettingKey::PerformanceCpuKernel,
    SettingKey::PerformanceCpuSecondaryGraph,
    SettingKey::PerformanceOverviewCoreSmoothing,
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
    SettingKey::PerformanceShowNetworkOther,
    SettingKey::PerformanceDeviceOrder,
    SettingKey::PerformanceDeviceOverrides,
    SettingKey::GraphsMinigraphCpu,
    SettingKey::GraphsMinigraphMemory,
    SettingKey::GraphsMinigraphDisk,
    SettingKey::GraphsMinigraphNetwork,
    SettingKey::GraphsMinigraphGpu,
    SettingKey::GraphsMinigraphVram,
    SettingKey::GraphsMinigraphGpuTemperature,
    SettingKey::WindowCloseWithEsc,
    SettingKey::WindowAlwaysOnTop,
    SettingKey::StartupStartAtLogin,
    SettingKey::StartupStartMinimized,
    SettingKey::NetworkAutoFetchPublicIp,
    SettingKey::NetworkPublicIpNoticeAccepted,
    SettingKey::ServicesUnitTypes,
    SettingKey::ServicesFilterStates,
    SettingKey::DebugShowFps,
];

/// `TasklensUi::init` with a persist counter and the test page. Returns the engine and the
/// number of persist calls.
fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, Rc<Cell<u32>>) {
    // The window is checked with the kit title bar and the client frame. The default is server
    // decorations (D-035).
    let mut settings = Settings::default();
    settings.appearance.decorations = Decorations::Client;
    setup_with(cx, settings)
}

/// `setup` with the given settings.
fn setup_with(cx: &mut TestAppContext, settings: Settings) -> (Arc<FakeEngine>, Rc<Cell<u32>>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let persists = Rc::new(Cell::new(0));
    let count = persists.clone();
    let init = UiInit {
        settings,
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
    let extra = tl_ui::components::window_frame::client_frame_extra(Decorations::Client);
    assert_eq!(viewport, size(px(900.) + extra, px(640.) + extra));
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

/// Default settings draw no kit title bar (D-035): the Settings window keeps its 900 x 640 size
/// with no frame, the pin sits in the content's top-right corner, the "Use system title bar"
/// switch reads on and Esc still closes the window.
#[gpui_kit::test]
fn settings_window_server_decorations_by_default(cx: &mut TestAppContext) {
    setup_with(cx, Settings::default());
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).appearance.decorations),
        Decorations::Server
    );
    // The switch is on when the stored value is "server" (`PageBuilder::str_switch`).
    let stored = cx.update(|cx| {
        serde_json::to_value(TlSettings::settings(cx))
            .expect("settings serialize")
            .pointer("/appearance/decorations")
            .cloned()
    });
    assert_eq!(stored, Some(serde_json::json!("server")));
    assert!(
        page(cx, PageId::Appearance)
            .keys()
            .contains(&SettingKey::AppearanceDecorations)
    );

    let (frame, mut vcx) = open(cx, 1);
    assert!(!vcx.read(|cx| frame.read(cx).has_title_bar()));
    assert!(vcx.debug_bounds("TlWindowTitleBar").is_none());
    let viewport = vcx.update(|window, _| window.viewport_size());
    assert_eq!(viewport, size(px(900.), px(640.)));
    let body = vcx.debug_bounds("SettingsWindowBody").expect("body drawn");
    let content = vcx.debug_bounds("TlWindowContent").expect("content drawn");
    assert_eq!(content.top(), px(0.), "the content starts at the top");
    assert_eq!(body.size.width, px(900.));
    let pin = vcx.debug_bounds("TlWindowPin").expect("pin drawn");
    assert_eq!(pin.size, size(px(24.), px(24.)));
    assert_eq!(pin.top(), content.top() + px(8.));
    assert_eq!(pin.right(), content.right() - px(8.));

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
            SettingKey::MotionSavePowerOnBattery,
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
fn dump_type_dropdown(cx: &mut TestAppContext) {
    setup(cx);
    let processes = page(cx, PageId::Processes);
    assert_eq!(
        processes.dropdown_labels(SettingKey::ProcessesDumpType),
        ["Mini", "Full"]
    );
    cx.update(|cx| processes.pick(SettingKey::ProcessesDumpType, "Full", cx));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).processes.dump_type),
        DumpMode::Full
    );
}

#[gpui_kit::test]
fn show_disconnected_fans_switch(cx: &mut TestAppContext) {
    setup(cx);
    let performance = page(cx, PageId::Performance);
    assert!(!cx.update(|cx| TlSettings::settings(cx).performance.show_disconnected_fans));
    cx.update(|cx| performance.set_bool(SettingKey::PerformanceShowDisconnectedFans, true, cx));
    assert!(cx.update(|cx| TlSettings::settings(cx).performance.show_disconnected_fans));
}

#[gpui_kit::test]
fn keep_detached_switch(cx: &mut TestAppContext) {
    setup(cx);
    let graphs = page(cx, PageId::Graphs);
    assert!(!cx.update(|cx| TlSettings::settings(cx).graphs.keep_detached_on_close));
    cx.update(|cx| graphs.set_bool(SettingKey::GraphsKeepDetachedOnClose, true, cx));
    assert!(cx.update(|cx| TlSettings::settings(cx).graphs.keep_detached_on_close));
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
            SettingKey::TablesColumnSeparators,
            SettingKey::TablesDimZeroValues,
            SettingKey::PropertiesHideUnnamedHandles,
            SettingKey::PropertiesSharedObjectsOnly,
            SettingKey::PropertiesEnvMask,
            SettingKey::ProcessesDumpType,
            SettingKey::HistoryEnabled,
            SettingKey::HistoryRetentionH,
            SettingKey::HistoryPersist,
            SettingKey::HistoryMaxDiskMb
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
                hot_c: None,
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
            SettingKey::PerformanceOverviewCoreSmoothing,
            SettingKey::PerformanceStatusBar,
            SettingKey::PerformanceOverviewRange,
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
            SettingKey::PerformanceShowNetworkOther,
            SettingKey::PerformanceShowDisconnectedFans,
            SettingKey::PerformanceDeviceOrder,
            SettingKey::PerformanceDeviceOverrides,
            SettingKey::Fans
        ]
    );
}

// Graphs and Services pages (E21-025).

fn json(
    cx: &mut TestAppContext,
    f: impl FnOnce(&Settings) -> serde_json::Value,
) -> serde_json::Value {
    cx.update(|cx| f(TlSettings::settings(cx)))
}

fn unit_types(cx: &mut TestAppContext) -> serde_json::Value {
    json(cx, |s| {
        serde_json::to_value(&s.services.unit_types).expect("serialize")
    })
}

#[gpui_kit::test]
fn graphs_page_switches(cx: &mut TestAppContext) {
    setup(cx);
    let graphs = page(cx, PageId::Graphs);
    assert!(cx.update(|cx| TlSettings::settings(cx).graphs.minigraph_vram));
    cx.update(|cx| graphs.set_bool(SettingKey::GraphsMinigraphVram, false, cx));
    let g = cx.update(|cx| TlSettings::settings(cx).graphs.clone());
    assert!(!g.minigraph_vram);
    assert!(g.minigraph_gpu, "the other switches keep their values");
    cx.update(|cx| graphs.set_bool(SettingKey::GraphsMinigraphGpuTemperature, true, cx));
    assert!(cx.update(|cx| TlSettings::settings(cx).graphs.minigraph_gpu_temperature));
}

#[gpui_kit::test]
fn graphs_reset_button(cx: &mut TestAppContext) {
    setup(cx);
    let resets = Rc::new(Cell::new(0));
    let count = resets.clone();
    cx.update(|cx| {
        cx.on_action(
            move |_: &tl_ui::pages::minigraph::actions::ResetMinigraphLayout, _| {
                count.set(count.get() + 1)
            },
        );
    });
    let (_, mut vcx) = open(cx, 1);
    let graphs = page(cx, PageId::Graphs);
    vcx.update(|window, cx| graphs.press("Reset Minigraph Layout", window, cx));
    vcx.run_until_parked();
    assert_eq!(resets.get(), 1, "ResetMinigraphLayout dispatched once");
}

#[gpui_kit::test]
fn services_unit_types_order(cx: &mut TestAppContext) {
    setup(cx);
    let services = page(cx, PageId::Services);
    assert_eq!(
        services.check_values(SettingKey::ServicesUnitTypes),
        [
            "service",
            "socket",
            "timer",
            "mount",
            "automount",
            "path",
            "target",
            "swap",
            "slice",
            "scope"
        ],
        "column 1, then column 2"
    );
    assert_eq!(
        unit_types(cx),
        serde_json::json!(["service", "socket", "timer", "mount"])
    );
    cx.update(|cx| services.set_check(SettingKey::ServicesUnitTypes, "path", true, cx));
    assert_eq!(
        unit_types(cx),
        serde_json::json!(["service", "socket", "timer", "mount", "path"])
    );
    // A box checked again goes back to its place, not to the end.
    cx.update(|cx| services.set_check(SettingKey::ServicesUnitTypes, "socket", false, cx));
    cx.update(|cx| services.set_check(SettingKey::ServicesUnitTypes, "scope", true, cx));
    cx.update(|cx| services.set_check(SettingKey::ServicesUnitTypes, "socket", true, cx));
    assert_eq!(
        unit_types(cx),
        serde_json::json!(["service", "socket", "timer", "mount", "path", "scope"])
    );
}

#[gpui_kit::test]
fn services_last_unit_type_refused(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::ServicesUnitTypes,
            SettingValue::StrList(Arc::from(["service".into()])),
        )
    });
    let (_, mut vcx) = open(cx, 1);
    let services = page(cx, PageId::Services);
    vcx.update(|_, cx| services.set_check(SettingKey::ServicesUnitTypes, "service", false, cx));
    vcx.run_until_parked();
    assert_eq!(
        unit_types(cx),
        serde_json::json!(["service"]),
        "it stays checked"
    );
    let toasts = vcx.update(|window, cx| {
        let queue =
            ToastQueue::of_window(window, cx).expect("the Settings window has a toast layer");
        let q = queue.read(cx);
        let mut all = q.visible_texts().to_vec();
        all.extend(q.waiting_texts().iter().cloned());
        all.iter().map(|t| t.to_string()).collect::<Vec<_>>()
    });
    assert_eq!(toasts, ["At least one unit type must stay selected"]);

    // With a second type checked, the first can go.
    vcx.update(|_, cx| services.set_check(SettingKey::ServicesUnitTypes, "timer", true, cx));
    vcx.update(|_, cx| services.set_check(SettingKey::ServicesUnitTypes, "service", false, cx));
    assert_eq!(unit_types(cx), serde_json::json!(["timer"]));
}

#[gpui_kit::test]
fn services_filter_states(cx: &mut TestAppContext) {
    setup(cx);
    let services = page(cx, PageId::Services);
    let states = |cx: &mut TestAppContext| {
        json(cx, |s| {
            serde_json::to_value(&s.services.filter_states).expect("serialize")
        })
    };
    assert_eq!(
        services.check_values(SettingKey::ServicesFilterStates),
        ["running", "failed", "stopped", "disabled"]
    );
    assert_eq!(states(cx), serde_json::json!([]), "none checked means all");
    cx.update(|cx| services.set_check(SettingKey::ServicesFilterStates, "failed", true, cx));
    assert_eq!(states(cx), serde_json::json!(["failed"]));
    cx.update(|cx| services.set_check(SettingKey::ServicesFilterStates, "running", true, cx));
    assert_eq!(states(cx), serde_json::json!(["running", "failed"]));
    // Unchecking every state is allowed.
    cx.update(|cx| services.set_check(SettingKey::ServicesFilterStates, "running", false, cx));
    cx.update(|cx| services.set_check(SettingKey::ServicesFilterStates, "failed", false, cx));
    assert_eq!(states(cx), serde_json::json!([]));
}

#[gpui_kit::test]
fn graphs_services_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::Graphs).keys(),
        [
            SettingKey::GraphsMinigraphCpu,
            SettingKey::GraphsMinigraphMemory,
            SettingKey::GraphsMinigraphDisk,
            SettingKey::GraphsMinigraphNetwork,
            SettingKey::GraphsMinigraphGpu,
            SettingKey::GraphsMinigraphVram,
            SettingKey::GraphsMinigraphGpuTemperature,
            SettingKey::GraphsKeepDetachedOnClose
        ]
    );
    assert_eq!(
        page(cx, PageId::Services).keys(),
        [
            SettingKey::ServicesUnitTypes,
            SettingKey::ServicesFilterStates
        ]
    );
    // Page order: Graphs and Services follow Performance.
    let ids: Vec<PageId> = cx.update(all_pages).iter().map(|p| p.id()).collect();
    let perf = ids
        .iter()
        .position(|id| *id == PageId::Performance)
        .expect("Performance listed");
    assert_eq!(ids[perf + 1..perf + 3], [PageId::Graphs, PageId::Services]);
}

// Performance page: device order and visibility list (E21-024).

const DEVICE_LIST: &str = "Device order and visibility";

/// A window that draws only the Performance page's device list, re-rendering on settings and
/// device changes as the Settings window does.
struct DeviceProbe {
    page: PageBuilder,
    label: &'static str,
    _subs: Vec<gpui_kit::Subscription>,
}

impl DeviceProbe {
    /// A probe that draws the custom item labelled `label`.
    fn with_label(label: &'static str, cx: &mut Context<Self>) -> Self {
        let page = all_pages(cx)
            .into_iter()
            .find(|p| p.id() == PageId::Performance)
            .expect("Performance page");
        let hub = cx.global::<tl_ui::hub::GlobalHub>().0.clone();
        let subs = vec![
            cx.observe_global::<TlSettings>(|_, cx| cx.notify()),
            cx.subscribe(&hub, |_, _, _: &tl_ui::hub::SnapshotChanged, cx| {
                cx.notify()
            }),
        ];
        Self {
            page,
            label,
            _subs: subs,
        }
    }
}

impl Render for DeviceProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .children(self.page.render_custom(self.label, window, cx))
    }
}

fn disk(idx: u16, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity: DeviceIdentity::Disk {
            major: 8,
            minor: u32::from(idx) * 16,
            name: Arc::from(name),
            wwn_or_serial: None,
        },
        generation: 0,
        display_name: Arc::from(format!("Disk {name}")),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c: None,
    }
}

/// Publishes a catalog of cpu, memory and the disks `disks`.
fn publish_devices(engine: &FakeEngine, disks: &[&str], cx: &mut TestAppContext) {
    let mut entries = vec![
        DeviceEntry {
            idx: DeviceIdx(0),
            identity: DeviceIdentity::CpuPackage,
            generation: 0,
            display_name: Arc::from("Test CPU"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        },
        DeviceEntry {
            idx: DeviceIdx(1),
            identity: DeviceIdentity::Memory,
            generation: 0,
            display_name: Arc::from("Memory"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        },
    ];
    for (i, name) in disks.iter().enumerate() {
        entries.push(disk(2 + i as u16, name));
    }
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(entries),
        });
    });
    cx.run_until_parked();
}

/// Opens the probe window with a toast queue, and draws it.
fn device_probe(cx: &mut TestAppContext) -> (VisualTestContext, Entity<ToastQueue>) {
    probe_for(DEVICE_LIST, cx)
}

fn probe_for(
    label: &'static str,
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<ToastQueue>) {
    let window = cx.add_window(|window, cx| {
        let probe = cx.new(|cx| DeviceProbe::with_label(label, cx));
        Root::new(probe, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(900.), px(400.)));
    vcx.update(|window, _| window.activate_window());
    let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
    draw_probe(&mut vcx);
    (vcx, queue)
}

fn draw_probe(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The row keys from top to bottom, as drawn.
fn drawn_keys(vcx: &mut VisualTestContext) -> Vec<String> {
    let mut rows: Vec<(Pixels, String)> = ["overview", "cpu", "memory", "disk:sda", "disk:sdb"]
        .into_iter()
        .filter_map(|k| {
            let selector: &'static str = Box::leak(format!("DeviceOrderRow-{k}").into_boxed_str());
            vcx.debug_bounds(selector)
                .map(|b| (b.origin.y, k.to_owned()))
        })
        .collect();
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite"));
    rows.into_iter().map(|(_, k)| k).collect()
}

fn device_order(cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| TlSettings::settings(cx).performance.device_order.clone())
}

fn row_bounds(vcx: &mut VisualTestContext, key: &str) -> gpui_kit::Bounds<Pixels> {
    let selector: &'static str = Box::leak(format!("DeviceOrderRow-{key}").into_boxed_str());
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{key} row drawn"))
}

fn click_at(vcx: &mut VisualTestContext, at: gpui_kit::Point<Pixels>) {
    vcx.simulate_mouse_down(at, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    vcx.simulate_mouse_up(at, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    draw_probe(vcx);
}

fn click_selector(vcx: &mut VisualTestContext, selector: String) {
    let selector: &'static str = Box::leak(selector.into_boxed_str());
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} drawn"))
        .center();
    click_at(vcx, at);
}

#[gpui_kit::test]
fn device_list_initial_order(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_devices(&engine, &["sda"], cx);
    let (mut vcx, _) = device_probe(cx);
    // The Overview row comes first when the CPU package is present (D-022).
    assert_eq!(
        drawn_keys(&mut vcx),
        ["overview", "cpu", "memory", "disk:sda"]
    );
    let rows = page(cx, PageId::Performance);
    assert!(rows.keys().contains(&SettingKey::PerformanceDeviceOrder));
    assert!(
        rows.keys()
            .contains(&SettingKey::PerformanceDeviceOverrides)
    );
    // "Default" is selected on each row: no override is stored.
    assert!(cx.update(|cx| {
        TlSettings::settings(cx)
            .performance
            .device_overrides
            .is_empty()
    }));
    assert!(vcx.debug_bounds("TlButton-device-vis-cpu-0").is_some());
}

#[gpui_kit::test]
fn device_list_alt_up_moves(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_devices(&engine, &["sda"], cx);
    let (mut vcx, _) = device_probe(cx);
    // Focus the disk row with a click on its name, then Alt+Up.
    let row = row_bounds(&mut vcx, "disk:sda");
    click_at(
        &mut vcx,
        gpui_kit::point(row.origin.x + px(60.), row.center().y),
    );
    vcx.simulate_keystrokes("alt-up");
    draw_probe(&mut vcx);
    assert_eq!(device_order(cx), ["overview", "cpu", "disk:sda", "memory"]);
    assert_eq!(
        drawn_keys(&mut vcx),
        ["overview", "cpu", "disk:sda", "memory"]
    );
    // Focus stays on the moved row: Alt+Down moves it back.
    vcx.simulate_keystrokes("alt-down");
    draw_probe(&mut vcx);
    assert_eq!(device_order(cx), ["overview", "cpu", "memory", "disk:sda"]);
}

#[gpui_kit::test]
fn device_list_drag_reorders(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_devices(&engine, &["sda"], cx);
    let (mut vcx, _) = device_probe(cx);
    let from = vcx
        .debug_bounds("DeviceOrderHandle-memory")
        .expect("memory handle")
        .center();
    let to = row_bounds(&mut vcx, "cpu").center();
    vcx.simulate_mouse_down(
        from,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::none(),
    );
    for at in [
        gpui_kit::point(from.x, from.y - px(4.)),
        gpui_kit::point(to.x, to.y + px(2.)),
        to,
    ] {
        vcx.simulate_mouse_move(
            at,
            Some(gpui_kit::MouseButton::Left),
            gpui_kit::Modifiers::none(),
        );
        draw_probe(&mut vcx);
    }
    vcx.simulate_mouse_up(to, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    draw_probe(&mut vcx);
    assert_eq!(device_order(cx), ["overview", "memory", "cpu", "disk:sda"]);
}

#[gpui_kit::test]
fn device_list_override_segments(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_devices(&engine, &["sda"], cx);
    let (mut vcx, _) = device_probe(cx);
    let overrides = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            serde_json::to_value(&TlSettings::settings(cx).performance.device_overrides)
                .expect("serialize")
        })
    };
    click_selector(&mut vcx, "TlButton-device-vis-cpu-2".into());
    assert_eq!(overrides(cx), serde_json::json!({ "cpu": "hide" }));
    click_selector(&mut vcx, "TlButton-device-vis-cpu-1".into());
    assert_eq!(overrides(cx), serde_json::json!({ "cpu": "show" }));
    click_selector(&mut vcx, "TlButton-device-vis-cpu-0".into());
    assert_eq!(
        overrides(cx),
        serde_json::json!({}),
        "Default removes the key"
    );
    // The Overview row is hideable too (D-022).
    click_selector(&mut vcx, "TlButton-device-vis-overview-2".into());
    assert_eq!(overrides(cx), serde_json::json!({ "overview": "hide" }));
}

#[gpui_kit::test]
fn device_list_reset_and_show_all(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_devices(&engine, &["sda"], cx);
    cx.update(|cx| {
        TlSettings::update(cx, |d| {
            let p = &mut d.settings.performance;
            p.device_order = vec!["memory".into(), "cpu".into()];
            p.device_overrides
                .insert("cpu".into(), tl_model::settings::DeviceVisibility::Hide);
        })
    });
    let (mut vcx, queue) = device_probe(cx);
    click_selector(&mut vcx, "TlButton-device-order-reset".into());
    assert!(device_order(cx).is_empty());
    let toasts = vcx.update(|_, cx| queue.read(cx).visible_texts());
    assert_eq!(toasts, ["Sidebar order reset"]);
    assert!(
        !cx.update(|cx| TlSettings::settings(cx)
            .performance
            .device_overrides
            .is_empty()),
        "Reset order keeps the overrides"
    );
    click_selector(&mut vcx, "TlButton-device-order-show-all".into());
    assert!(cx.update(|cx| {
        TlSettings::settings(cx)
            .performance
            .device_overrides
            .is_empty()
    }));
    let toasts = vcx.update(|_, cx| queue.read(cx).visible_texts());
    assert_eq!(toasts.len(), 1, "Show all shows no toast");
}

#[gpui_kit::test]
fn device_list_hotplug_row(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_devices(&engine, &["sda"], cx);
    let (mut vcx, _) = device_probe(cx);
    assert_eq!(drawn_keys(&mut vcx).len(), 4);
    publish_devices(&engine, &["sda", "sdb"], cx);
    draw_probe(&mut vcx);
    assert_eq!(
        drawn_keys(&mut vcx),
        ["overview", "cpu", "memory", "disk:sda", "disk:sdb"]
    );
}

// PageBuilder: action switches and dropdowns, Str switches, indent, tooltip, restart toast text
// and the slider Release hook (E21-026, E21-020).

gpui_kit::actions!(tl_settings_test, [FlipAnimations]);

/// A test action carrying a picked value.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = tl_settings_test, no_json)]
struct PickName {
    name: gpui_kit::SharedString,
}

/// All toast texts of the Settings window, shown and waiting.
fn window_toasts(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, cx| {
        let queue =
            ToastQueue::of_window(window, cx).expect("the Settings window has a toast layer");
        let q = queue.read(cx);
        let mut all = q.visible_texts().to_vec();
        all.extend(q.waiting_texts().iter().cloned());
        all.iter().map(|t| t.to_string()).collect()
    })
}

#[gpui_kit::test]
fn page_builder_action_switch_dispatches(cx: &mut TestAppContext) {
    setup(cx);
    let count = Rc::new(Cell::new(0));
    let seen = count.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &FlipAnimations, _| seen.set(seen.get() + 1));
    });
    let mut page = PageBuilder::new(PageId::Advanced);
    page.action_switch("G", SettingKey::MotionAnimations, "Animations", || {
        Box::new(FlipAnimations)
    });
    assert_eq!(page.keys(), [SettingKey::MotionAnimations]);
    let now = cx.update(|cx| TlSettings::settings(cx).motion.animations);
    cx.update(|cx| page.set_bool(SettingKey::MotionAnimations, !now, cx));
    cx.run_until_parked();
    assert_eq!(count.get(), 1, "a change dispatches once");
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.animations),
        now,
        "the switch itself writes nothing"
    );
    cx.update(|cx| page.set_bool(SettingKey::MotionAnimations, now, cx));
    cx.run_until_parked();
    assert_eq!(count.get(), 1, "the stored value dispatches nothing");
}

#[gpui_kit::test]
fn page_builder_action_dropdown_dispatches(cx: &mut TestAppContext) {
    setup(cx);
    let picked = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let seen = picked.clone();
    cx.update(|cx| {
        cx.on_action(move |a: &PickName, _| seen.borrow_mut().push(a.name.to_string()));
    });
    let mut page = PageBuilder::new(PageId::Advanced);
    page.action_dropdown(
        "G",
        SettingKey::ThemeActive,
        "Theme",
        &[
            ("One".into(), SettingValue::Str("one".into())),
            ("Two (custom)".into(), SettingValue::Str("two".into())),
        ],
        |name| {
            Box::new(PickName {
                name: name.to_string().into(),
            })
        },
    );
    let before = cx.update(|cx| TlSettings::settings(cx).theme.active.clone());
    assert_eq!(
        page.dropdown_labels(SettingKey::ThemeActive),
        ["One", "Two (custom)"]
    );
    cx.update(|cx| page.pick(SettingKey::ThemeActive, "Two (custom)", cx));
    cx.run_until_parked();
    assert_eq!(*picked.borrow(), ["two"]);
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).theme.active.clone()),
        before,
        "the dropdown itself writes nothing"
    );
}

#[gpui_kit::test]
fn page_builder_indent_tooltip_disabled(cx: &mut TestAppContext) {
    setup(cx);
    let mut page = PageBuilder::new(PageId::Advanced);
    page.switch("G", SettingKey::MotionAnimations, "Animations", None)
        .indent_next(16.)
        .disable_next(true)
        .switch("G", SettingKey::MotionSmoothGraphs, "Smooth Graphs", None)
        .tooltip_next(Some("Not supported by this desktop".into()))
        .switch("G", SettingKey::MotionSmoothScroll, "Smooth scroll", None);
    assert_eq!(
        page.keys(),
        [
            SettingKey::MotionAnimations,
            SettingKey::MotionSmoothGraphs,
            SettingKey::MotionSmoothScroll
        ]
    );
    assert_eq!(page.indent_of(SettingKey::MotionAnimations), 0.);
    assert_eq!(page.indent_of(SettingKey::MotionSmoothGraphs), 16.);
    assert!(page.is_disabled(SettingKey::MotionSmoothGraphs));
    assert!(!page.is_disabled(SettingKey::MotionSmoothScroll));
    assert_eq!(page.tooltip_of(SettingKey::MotionSmoothGraphs), None);
    assert_eq!(
        page.tooltip_of(SettingKey::MotionSmoothScroll).as_deref(),
        Some("Not supported by this desktop")
    );
    // A drawn row still writes through its setter.
    cx.update(|cx| page.set_bool(SettingKey::MotionSmoothScroll, false, cx));
    assert!(!cx.update(|cx| TlSettings::settings(cx).motion.smooth_scroll));
}

#[gpui_kit::test]
fn page_builder_str_switch_restart_toast(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let mut page = PageBuilder::new(PageId::Advanced);
    vcx.update(|_, cx| {
        page.restart_note_toast(cx, "Restart to apply".into())
            .str_switch(
                "G",
                SettingKey::AppearanceDecorations,
                "Use system title bar",
                "server",
                "client",
            );
    });
    assert_eq!(
        page.description_of(SettingKey::AppearanceDecorations)
            .as_deref(),
        Some("Takes effect after restart")
    );
    vcx.update(|_, cx| page.set_bool(SettingKey::AppearanceDecorations, true, cx));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).appearance.decorations),
        Decorations::Server
    );
    assert_eq!(window_toasts(&mut vcx), ["Restart to apply"]);
    vcx.update(|_, cx| page.set_bool(SettingKey::AppearanceDecorations, false, cx));
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).appearance.decorations),
        Decorations::Client
    );
}

static COMMITTED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

fn commit_pages(_: &mut gpui_kit::App) -> Vec<PageBuilder> {
    // A key no real page has a slider for: the window keeps the first slider of a key.
    let mut page = PageBuilder::new(PageId::Advanced);
    page.slider_on_commit(|v, _| COMMITTED.store(v, std::sync::atomic::Ordering::SeqCst))
        .slider(
            "History",
            SettingKey::HistoryMaxDiskMb,
            "Max disk",
            100..=1000,
            100,
            |v| format!("{v}").into(),
        );
    vec![page]
}

#[gpui_kit::test]
fn page_builder_slider_on_commit(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| cx.set_global(TestPages(commit_pages)));
    let (frame, vcx) = open(cx, 1);
    let state = vcx
        .read(|cx| {
            frame
                .read(cx)
                .content()
                .read(cx)
                .slider_state(SettingKey::HistoryMaxDiskMb)
                .cloned()
        })
        .expect("slider state");
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Preview(400.))));
    cx.run_until_parked();
    assert_eq!(COMMITTED.load(std::sync::atomic::Ordering::SeqCst), -1);
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Commit(600.))));
    cx.run_until_parked();
    assert_eq!(COMMITTED.load(std::sync::atomic::Ordering::SeqCst), 600);
}

// Startup and window page (E21-026).

/// Publishes a windows snapshot with `validity` (the KWin bridge state keep-above reads).
fn set_kwin(engine: &FakeEngine, cx: &mut TestAppContext, validity: tl_model::validity::Validity) {
    engine.publish_with(|s| {
        let w = Arc::make_mut(&mut s.windows);
        w.header.validity = validity;
        w.header.version += 1;
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn startup_window_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::StartupWindow).keys(),
        [
            SettingKey::WindowCloseWithEsc,
            SettingKey::WindowAlwaysOnTop,
            SettingKey::TrayCloseToTray,
            SettingKey::TrayMinimizeToTray,
            SettingKey::TrayAlwaysShow,
            SettingKey::StartupStartAtLogin,
            SettingKey::StartupStartMinimized,
            SettingKey::NetworkAutoFetchPublicIp,
            SettingKey::LowmemEnabled
        ]
    );
}

#[gpui_kit::test]
fn startup_window_start_minimized_disabled(cx: &mut TestAppContext) {
    setup(cx);
    assert!(!cx.update(|cx| TlSettings::settings(cx).startup.start_at_login));
    let startup = page(cx, PageId::StartupWindow);
    assert!(startup.is_disabled(SettingKey::StartupStartMinimized));
    assert_eq!(startup.indent_of(SettingKey::StartupStartMinimized), 16.);

    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::StartupStartAtLogin,
            SettingValue::Bool(true),
        )
    });
    let startup = page(cx, PageId::StartupWindow);
    assert!(!startup.is_disabled(SettingKey::StartupStartMinimized));
    assert_eq!(startup.indent_of(SettingKey::StartupStartMinimized), 16.);
}

#[gpui_kit::test]
fn startup_window_start_at_login_dispatch(cx: &mut TestAppContext) {
    setup(cx);
    let count = Rc::new(Cell::new(0));
    let seen = count.clone();
    cx.update(|cx| {
        cx.on_action(
            move |_: &tl_ui::window::menus_settings::ToggleStartAtLogin, _| {
                seen.set(seen.get() + 1)
            },
        );
    });
    let startup = page(cx, PageId::StartupWindow);
    cx.update(|cx| startup.set_bool(SettingKey::StartupStartAtLogin, true, cx));
    cx.run_until_parked();
    assert_eq!(count.get(), 1, "ToggleStartAtLogin dispatched once");
    assert!(
        cx.update(|cx| TlSettings::settings(cx).startup.start_at_login),
        "the handler wrote the key after the autostart entry"
    );

    // Start minimized goes through its own action while Start at login is on.
    let startup = page(cx, PageId::StartupWindow);
    cx.update(|cx| startup.set_bool(SettingKey::StartupStartMinimized, true, cx));
    cx.run_until_parked();
    assert!(cx.update(|cx| TlSettings::settings(cx).startup.start_minimized));
}

#[gpui_kit::test]
fn startup_window_always_on_top_dispatch(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    set_kwin(&engine, cx, tl_model::validity::Validity::Valid);
    let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seen.clone();
    cx.update(|cx| {
        cx.on_action(
            move |a: &tl_ui::components::window_frame::ToggleAlwaysOnTop, _| {
                log.borrow_mut()
                    .push((a.window.clone(), a.caption.to_string()))
            },
        );
    });
    let startup = page(cx, PageId::StartupWindow);
    assert!(!startup.is_disabled(SettingKey::WindowAlwaysOnTop));
    assert_eq!(startup.tooltip_of(SettingKey::WindowAlwaysOnTop), None);
    cx.update(|cx| startup.set_bool(SettingKey::WindowAlwaysOnTop, true, cx));
    cx.run_until_parked();
    assert_eq!(*seen.borrow(), [(WindowKey::Main, "tasklens".to_string())]);
}

#[gpui_kit::test]
fn startup_window_always_on_top_unsupported(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    set_kwin(&engine, cx, tl_model::validity::Validity::Unsupported);
    let startup = page(cx, PageId::StartupWindow);
    assert!(startup.is_disabled(SettingKey::WindowAlwaysOnTop));
    assert_eq!(
        startup.tooltip_of(SettingKey::WindowAlwaysOnTop).as_deref(),
        Some("Not supported by this desktop")
    );
}

#[gpui_kit::test]
fn startup_window_public_ip_asks_first(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let windows = cx.update(|cx| cx.windows().len());
    let startup = page(cx, PageId::StartupWindow);
    vcx.update(|_, cx| startup.set_bool(SettingKey::NetworkAutoFetchPublicIp, true, cx));
    vcx.run_until_parked();
    assert!(
        !cx.update(|cx| TlSettings::settings(cx).network.auto_fetch_public_ip),
        "not before the notice is accepted"
    );
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        windows + 1,
        "the public IP notice opened"
    );
}

#[gpui_kit::test]
fn startup_window_public_ip_accepted_and_off(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| {
        TlSettings::set(
            cx,
            SettingKey::NetworkPublicIpNoticeAccepted,
            SettingValue::Bool(true),
        )
    });
    let (_, mut vcx) = open(cx, 1);
    let startup = page(cx, PageId::StartupWindow);
    vcx.update(|_, cx| startup.set_bool(SettingKey::NetworkAutoFetchPublicIp, true, cx));
    vcx.run_until_parked();
    assert!(cx.update(|cx| TlSettings::settings(cx).network.auto_fetch_public_ip));
    vcx.update(|_, cx| startup.set_bool(SettingKey::NetworkAutoFetchPublicIp, false, cx));
    vcx.run_until_parked();
    assert!(
        !cx.update(|cx| TlSettings::settings(cx).network.auto_fetch_public_ip),
        "off needs no notice"
    );
}

// Advanced page and P2 window invariants (E21-A2).

#[gpui_kit::test]
fn settings_no_key_twice(cx: &mut TestAppContext) {
    setup(cx);
    let mut seen = HashSet::new();
    for key in p2_pages(cx).iter().flat_map(|page| page.keys()) {
        assert!(
            seen.insert(*key),
            "{} is bound more than once",
            key.as_str()
        );
    }
}

#[gpui_kit::test]
fn settings_only_settings_toml_keys(cx: &mut TestAppContext) {
    setup(cx);
    let canonical: HashSet<SettingKey> = SettingKey::ALL.into_iter().collect();
    for key in p2_pages(cx).iter().flat_map(|page| page.keys()) {
        assert!(
            canonical.contains(key),
            "{} is not a canonical settings.toml key",
            key.as_str()
        );
    }
}

#[gpui_kit::test]
fn settings_p2_keys_present(cx: &mut TestAppContext) {
    setup(cx);
    let bound: HashSet<SettingKey> = p2_pages(cx)
        .iter()
        .flat_map(|page| page.keys())
        .copied()
        .collect();
    let missing: Vec<&str> = E21_P2_KEYS
        .iter()
        .filter(|key| !bound.contains(key))
        .map(|key| key.as_str())
        .collect();
    assert!(missing.is_empty(), "P2 keys not bound: {missing:?}");
}

/// UI-SHL-060, APP-023: every `settings.toml` key of table (a) is bound by exactly one row.
/// `SettingKey::ALL` lists `fans` once.
#[gpui_kit::test]
fn settings_every_key_once(cx: &mut TestAppContext) {
    setup(cx);
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for key in p2_pages(cx).iter().flat_map(|page| page.keys()) {
        *counts.entry(key.as_str()).or_default() += 1;
    }
    let missing: Vec<&str> = SettingKey::ALL
        .iter()
        .map(|key| key.as_str())
        .filter(|name| !counts.contains_key(name))
        .collect();
    let repeated: Vec<&str> = counts
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(name, _)| *name)
        .collect();
    assert!(
        missing.is_empty() && repeated.is_empty(),
        "keys not bound: {missing:?}; keys bound more than once: {repeated:?}"
    );
}

/// UI-SHL-060: `SettingKey` holds only table (a) keys, so a bound key outside it would be a
/// `state.toml` key (those live in `UiState`).
#[gpui_kit::test]
fn settings_no_state_keys(cx: &mut TestAppContext) {
    setup(cx);
    let canonical: HashSet<SettingKey> = SettingKey::ALL.into_iter().collect();
    for key in p2_pages(cx).iter().flat_map(|page| page.keys()) {
        assert!(
            canonical.contains(key),
            "{} is not a settings.toml key",
            key.as_str()
        );
    }
}

#[gpui_kit::test]
fn advanced_show_fps_switch(cx: &mut TestAppContext) {
    setup(cx);
    let advanced = page(cx, PageId::Advanced);
    assert!(!cx.update(|cx| TlSettings::settings(cx).debug.show_fps));
    cx.update(|cx| advanced.set_bool(SettingKey::DebugShowFps, true, cx));
    assert!(cx.update(|cx| TlSettings::settings(cx).debug.show_fps));
}

/// Rendering GPU (E21-040, UI-SHL-071, R-237): the first Advanced row, filled from the adapter
/// probe the Settings window starts when it opens.
#[gpui_kit::test]
fn advanced_rendering_gpu_row(cx: &mut TestAppContext) {
    setup(cx);
    let adapters = vec![
        tl_ui::app::AdapterInfo {
            name: "NVIDIA GeForce RTX 4070".to_owned(),
            backend: "Vulkan".to_owned(),
            device_type: "DiscreteGpu".to_owned(),
            pci_bus_id: Some("0000:01:00.0".to_owned()),
        },
        tl_ui::app::AdapterInfo {
            name: "llvmpipe".to_owned(),
            backend: "Vulkan".to_owned(),
            device_type: "Cpu".to_owned(),
            pci_bus_id: None,
        },
    ];
    cx.update(|cx| {
        cx.global_mut::<AppEnv>().list_adapters = Arc::new(move || {
            let (tx, rx) = async_channel::bounded(1);
            tx.try_send(adapters.clone()).expect("room for one answer");
            rx
        });
    });
    let before = page(cx, PageId::Advanced);
    assert_eq!(
        before.dropdown_labels(SettingKey::RenderingGpu),
        ["Auto", "Software renderer"],
        "no probe before a Settings window opens"
    );

    let (_, mut vcx) = open(cx, 1);
    let advanced = page(cx, PageId::Advanced);
    assert_eq!(advanced.keys().first(), Some(&SettingKey::RenderingGpu));
    assert_eq!(
        advanced
            .keys()
            .iter()
            .filter(|k| **k == SettingKey::RenderingGpu)
            .count(),
        1
    );
    assert_eq!(
        advanced.dropdown_labels(SettingKey::RenderingGpu),
        [
            "Auto",
            "NVIDIA GeForce RTX 4070",
            "llvmpipe",
            "Software renderer"
        ]
    );
    assert_eq!(
        advanced.description_of(SettingKey::RenderingGpu).as_deref(),
        Some("Takes effect after restart")
    );

    vcx.update(|_, cx| advanced.pick(SettingKey::RenderingGpu, "llvmpipe", cx));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).rendering.gpu.clone()),
        "name:llvmpipe"
    );
    assert_eq!(
        window_toasts(&mut vcx),
        ["Restart tasklens to use the new GPU"]
    );
    // Past the 1 s toast coalesce window, so a repeated toast shows as a second entry.
    let past_coalesce =
        tl_ui::components::toast::COALESCE_WINDOW + std::time::Duration::from_millis(500);
    vcx.executor().advance_clock(past_coalesce);
    vcx.run_until_parked();
    vcx.update(|_, cx| advanced.pick(SettingKey::RenderingGpu, "NVIDIA GeForce RTX 4070", cx));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).rendering.gpu.clone()),
        "pci:0000:01:00.0"
    );
    let toasts = window_toasts(&mut vcx);
    assert_eq!(toasts.len(), 2, "each change of GPU toasts once");

    vcx.executor().advance_clock(past_coalesce);
    vcx.run_until_parked();
    vcx.update(|_, cx| advanced.pick(SettingKey::RenderingGpu, "NVIDIA GeForce RTX 4070", cx));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).rendering.gpu.clone()),
        "pci:0000:01:00.0"
    );
    assert_eq!(
        window_toasts(&mut vcx),
        toasts,
        "picking the GPU already in use shows no restart toast"
    );
}

#[gpui_kit::test]
fn advanced_reset_opens_confirm(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let settings_id = vcx.update(|window, _| window.window_handle().window_id());
    let advanced = page(cx, PageId::Advanced);
    vcx.update(|window, cx| advanced.press("Reset Settings to Defaults", window, cx));
    vcx.run_until_parked();

    let handle = cx
        .update(|cx| {
            cx.windows()
                .into_iter()
                .filter(|window| window.window_id() != settings_id)
                .find_map(|window| window.downcast::<Root>())
        })
        .expect("reset confirmation window");
    let view = cx
        .update(|cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))
        .map(|frame| cx.read(|cx| frame.read(cx).content().clone()))
        .expect("reset confirmation dialog");
    let mut confirm = VisualTestContext::from_window(handle.into(), cx);
    confirm.update(|window, _| window.activate_window());
    confirm.run_until_parked();
    assert_eq!(
        confirm.update(|window, cx| view.read(cx).focused_button(window)),
        Some(DialogButton::Cancel)
    );
}

#[gpui_kit::test]
fn advanced_open_settings_folder(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    cx.update(|cx| {
        cx.global_mut::<AppEnv>().config_dir = PathBuf::from("/home/u/.config/tasklens");
    });
    let (_, mut vcx) = open(cx, 1);
    let advanced = page(cx, PageId::Advanced);
    let before = engine.commands().len();
    vcx.update(|window, cx| advanced.press("Open settings folder", window, cx));
    vcx.run_until_parked();
    let uris: Vec<String> = engine.commands()[before..]
        .iter()
        .filter_map(|command| match command {
            Command::Action(request) => match &request.kind {
                ActionKind::OpenUri(uri) => Some(uri.to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(uris, ["file:///home/u/.config/tasklens"]);
    assert!(uris[0].ends_with("/tasklens"));
}

#[gpui_kit::test]
fn settings_p2_page_list(cx: &mut TestAppContext) {
    setup(cx);
    let ids: Vec<PageId> = p2_pages(cx).iter().map(PageBuilder::id).collect();
    assert_eq!(
        ids,
        [
            PageId::General,
            PageId::Appearance,
            PageId::Motion,
            PageId::Units,
            PageId::Processes,
            PageId::Performance,
            PageId::Graphs,
            PageId::Services,
            PageId::StartupWindow,
            PageId::Privileges,
            PageId::Advanced,
        ]
    );
}

// Privileges page (E21-035).

#[gpui_kit::test]
fn privileges_page_position(cx: &mut TestAppContext) {
    setup(cx);
    let ids: Vec<PageId> = p2_pages(cx).iter().map(PageBuilder::id).collect();
    let at = ids
        .iter()
        .position(|id| *id == PageId::Privileges)
        .expect("Privileges listed");
    assert_eq!(ids[at - 1], PageId::StartupWindow);
    assert_eq!(ids[at + 1], PageId::Advanced);
}

#[gpui_kit::test]
fn privileges_unlock_switch(cx: &mut TestAppContext) {
    setup(cx);
    let privileges = page(cx, PageId::Privileges);
    assert!(!cx.update(|cx| TlSettings::settings(cx).privilege.unlock_at_launch));
    cx.update(|cx| privileges.set_bool(SettingKey::PrivilegeUnlockAtLaunch, true, cx));
    assert!(cx.update(|cx| TlSettings::settings(cx).privilege.unlock_at_launch));
}

#[gpui_kit::test]
fn privileges_setup_button_dispatch(cx: &mut TestAppContext) {
    setup(cx);
    let opened = Rc::new(Cell::new(0));
    let count = opened.clone();
    cx.update(|cx| {
        cx.on_action(move |a: &OpenAdditionalSetup, _| {
            if a == &(OpenAdditionalSetup { focus_row: None }) {
                count.set(count.get() + 1)
            }
        });
    });
    let (_, mut vcx) = open(cx, 1);
    let privileges = page(cx, PageId::Privileges);
    vcx.update(|window, cx| privileges.press("Additional setup…", window, cx));
    vcx.run_until_parked();
    assert_eq!(opened.get(), 1, "OpenAdditionalSetup dispatched once");
}

/// The Advanced and Privileges pages group their rows under headings (tasklens-bj6.58 b).
#[gpui_kit::test]
fn advanced_and_privileges_rows_have_group_headings(cx: &mut TestAppContext) {
    setup(cx);
    for (id, want) in [
        (
            PageId::Advanced,
            vec!["Rendering", "Network", "Debug", "Settings files"],
        ),
        (
            PageId::Privileges,
            vec!["Unlock", "Password prompts", "Administrator helper"],
        ),
    ] {
        let titles: Vec<String> = page(cx, id)
            .group_titles()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(titles, want, "{id:?}");
    }
}

/// The dropdowns are `TlSelect`s (opaque popup over its whole box), not the kit dropdown.
#[gpui_kit::test]
fn settings_dropdown_is_a_tl_select_with_a_popup(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    assert!(
        vcx.debug_bounds("TlSelect-settings-select-general.language")
            .is_some(),
        "General > Language drawn as a TlSelect"
    );
    assert!(vcx.debug_bounds("TlSelect-popup").is_none());
    click_selector(&mut vcx, "TlSelect-settings-select-general.language".into());
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(vcx.debug_bounds("TlSelect-popup").is_some());
}

#[gpui_kit::test]
fn privileges_page_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::Privileges).keys(),
        [
            SettingKey::PrivilegeUnlockAtLaunch,
            SettingKey::PrivilegePasswordless
        ]
    );
}

// Appearance page (E21-020).

#[gpui_kit::test]
fn appearance_keys(cx: &mut TestAppContext) {
    setup(cx);
    assert_eq!(
        page(cx, PageId::Appearance).keys(),
        [
            SettingKey::ThemeActive,
            SettingKey::ThemeFollowSystem,
            SettingKey::ThemeLight,
            SettingKey::ThemeDark,
            SettingKey::AppearanceUiScalePct,
            SettingKey::AppearanceRowHeightPct,
            SettingKey::AppearanceRoundedCorners,
            SettingKey::AppearanceHeatColoring,
            SettingKey::AppearanceHeatIntensityPct,
            SettingKey::AppearanceShadowIntensityPct,
            SettingKey::AppearanceDecorations
        ]
    );
    // Page order: Appearance is the second page.
    let ids: Vec<PageId> = cx.update(all_pages).iter().map(|p| p.id()).collect();
    assert_eq!(ids[..2], [PageId::General, PageId::Appearance]);
}

#[gpui_kit::test]
fn settings_restart_toast(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let appearance = page(cx, PageId::Appearance);
    assert_eq!(
        appearance
            .description_of(SettingKey::AppearanceDecorations)
            .as_deref(),
        Some("Takes effect after restart")
    );
    vcx.update(|_, cx| appearance.set_bool(SettingKey::AppearanceDecorations, true, cx));
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).appearance.decorations),
        Decorations::Server
    );
    assert_eq!(
        window_toasts(&mut vcx),
        ["Restart tasklens to apply the title bar change"]
    );
}

#[gpui_kit::test]
fn appearance_theme_dropdown_dispatches_activate(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    cx.update(|cx| {
        let builtin = cx.global::<ThemeStore>().builtin[0].clone();
        let mut dracula = builtin.clone();
        dracula.name = "Dracula".into();
        let mut shadow = builtin;
        shadow.name = "Default Dark".into();
        cx.global_mut::<ThemeStore>().user.extend([dracula, shadow]);
        TlSettings::update(cx, |d| d.settings.theme.follow_system = true);
    });
    let appearance = page(cx, PageId::Appearance);
    let labels = appearance.dropdown_labels(SettingKey::ThemeActive);
    assert_eq!(labels.first().map(|l| l.as_ref()), Some("Default Dark"));
    assert!(labels.iter().any(|l| l == "Dracula"));
    for key in [
        SettingKey::ThemeActive,
        SettingKey::ThemeLight,
        SettingKey::ThemeDark,
    ] {
        assert!(
            appearance
                .dropdown_labels(key)
                .iter()
                .any(|label| label == "Default Dark (custom)"),
            "shadowing custom theme is labelled in {key:?}"
        );
    }
    vcx.update(|_, cx| appearance.pick(SettingKey::ThemeActive, "Dracula", cx));
    vcx.run_until_parked();
    let theme = cx.update(|cx| TlSettings::settings(cx).theme.clone());
    assert_eq!(theme.active, "Dracula");
    assert!(!theme.follow_system, "an explicit choice turns sync off");
    assert_eq!(
        cx.update(|cx| tl_ui::theme::current(cx).name.to_string()),
        "Dracula"
    );
    assert_eq!(window_toasts(&mut vcx), ["Theme sync turned off"]);
}

#[gpui_kit::test]
fn appearance_ui_scale_release_toast(cx: &mut TestAppContext) {
    let (_, persists) = setup(cx);
    let (frame, mut vcx) = open(cx, 1);
    let state = vcx
        .read(|cx| {
            frame
                .read(cx)
                .content()
                .read(cx)
                .slider_state(SettingKey::AppearanceUiScalePct)
                .cloned()
        })
        .expect("UI Scale slider state");
    let before = persists.get();
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Preview(120.))));
    cx.run_until_parked();
    assert_eq!(persists.get(), before, "Change writes in memory only");
    assert!(
        window_toasts(&mut vcx).is_empty(),
        "no toast before release"
    );
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Commit(125.))));
    cx.run_until_parked();
    assert_eq!(persists.get(), before + 1, "Release persists once");
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).appearance.ui_scale_pct),
        125
    );
    assert_eq!(window_toasts(&mut vcx), ["UI scale changed to 125%"]);
}

// Performance page: Fans group (E21-037).

const FANS_GROUP: &str = "Fans";
const FAN_KEY: &str = "fan:nct6775:1";

fn entry(idx: u16, identity: DeviceIdentity, key: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(key),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
        hot_c: None,
    }
}

/// Publishes a catalog with one fan (`nct6775` fan 1) and the temperature sensor Tctl (k10temp).
fn publish_fan(engine: &FakeEngine, with_fan: bool, cx: &mut TestAppContext) {
    let mut entries = vec![entry(0, DeviceIdentity::CpuPackage, "")];
    if with_fan {
        entries.push(entry(
            1,
            DeviceIdentity::Fan {
                chip: Arc::from("nct6775"),
                index: 1,
            },
            "fan:nct6775:/devices/platform/nct6775.656:1",
        ));
    }
    entries.push(entry(
        2,
        DeviceIdentity::Sensor {
            chip: Arc::from("k10temp"),
            label: Arc::from("Tctl"),
        },
        "sensor:k10temp:/devices/pci0000:00/0000:00:18.3:temp1",
    ));
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(entries),
        });
    });
    cx.run_until_parked();
}

fn fans(
    cx: &mut TestAppContext,
) -> std::collections::BTreeMap<String, tl_model::settings::FanOverride> {
    cx.update(|cx| TlSettings::settings(cx).fans.clone())
}

fn bounds_of(vcx: &mut VisualTestContext, selector: String) -> Option<gpui_kit::Bounds<Pixels>> {
    let selector: &'static str = Box::leak(selector.into_boxed_str());
    vcx.debug_bounds(selector)
}

fn pick_sensor(vcx: &mut VisualTestContext, item: f32) {
    click_selector(vcx, format!("TlSelect-fan-temp-{FAN_KEY}"));
    let list = vcx
        .debug_bounds("TlSelect-list")
        .expect("sensor list drawn");
    // The popup lists "None" and one sensor, so each item is half the list.
    let at = gpui_kit::point(
        list.center().x,
        list.top() + list.size.height * (item / 2. + 0.25),
    );
    click_at(vcx, at);
}

fn type_label(vcx: &mut VisualTestContext, text: &str) {
    click_selector(vcx, format!("TlTextInput-fan-label-{FAN_KEY}"));
    vcx.simulate_input(text);
    vcx.simulate_keystrokes("enter");
    draw_probe(vcx);
}

#[gpui_kit::test]
fn fans_group_rows(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_fan(&engine, true, cx);
    let (mut vcx, _) = probe_for(FANS_GROUP, cx);
    assert!(bounds_of(&mut vcx, format!("FanRow-{FAN_KEY}")).is_some());
    assert!(bounds_of(&mut vcx, format!("TlTextInput-fan-label-{FAN_KEY}")).is_some());
    assert!(bounds_of(&mut vcx, format!("TlSelect-fan-temp-{FAN_KEY}")).is_some());
    // The dropdown lists "None" and the one sensor.
    click_selector(&mut vcx, format!("TlSelect-fan-temp-{FAN_KEY}"));
    let list = vcx
        .debug_bounds("TlSelect-list")
        .expect("sensor list drawn");
    assert!(list.size.height > px(0.));
    assert!(fans(cx).is_empty(), "drawing writes nothing");
    // The page lists the one key.
    assert_eq!(
        page(cx, PageId::Performance)
            .keys()
            .iter()
            .filter(|k| **k == SettingKey::Fans)
            .count(),
        1
    );
}

#[gpui_kit::test]
fn fans_group_hidden_without_fans(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_fan(&engine, false, cx);
    let (mut vcx, _) = probe_for(FANS_GROUP, cx);
    assert!(
        vcx.debug_bounds("FansGroup")
            .is_some_and(|b| b.size.height == px(0.))
    );
}

#[gpui_kit::test]
fn fans_write_entry(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_fan(&engine, true, cx);
    let (mut vcx, _) = probe_for(FANS_GROUP, cx);
    type_label(&mut vcx, "CPU fan");
    assert_eq!(
        fans(cx)[FAN_KEY].label.as_deref(),
        Some("CPU fan"),
        "label only: no temp is written"
    );
    assert_eq!(fans(cx)[FAN_KEY].temp, None);
    pick_sensor(&mut vcx, 1.);
    let entry = fans(cx)[FAN_KEY].clone();
    assert_eq!(entry.label.as_deref(), Some("CPU fan"));
    assert_eq!(
        entry.temp.as_deref(),
        Some("k10temp:/devices/pci0000:00/0000:00:18.3:temp1")
    );
}

#[gpui_kit::test]
fn fans_remove_entry(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    publish_fan(&engine, true, cx);
    cx.update(|cx| {
        tl_ui::settings_global::TlSettings::update(cx, |d| {
            d.settings.fans.insert(
                FAN_KEY.into(),
                tl_model::settings::FanOverride {
                    label: Some("CPU fan".into()),
                    temp: Some("k10temp:/devices/pci0000:00/0000:00:18.3:temp1".into()),
                },
            );
        })
    });
    let (mut vcx, _) = probe_for(FANS_GROUP, cx);
    // "None" first: the label stays, so does the entry.
    pick_sensor(&mut vcx, 0.);
    assert_eq!(fans(cx)[FAN_KEY].temp, None);
    assert_eq!(fans(cx)[FAN_KEY].label.as_deref(), Some("CPU fan"));
    // Clearing the label leaves nothing: the entry goes.
    click_selector(&mut vcx, format!("TlTextInput-fan-label-{FAN_KEY}"));
    vcx.simulate_keystrokes("ctrl-a backspace enter");
    draw_probe(&mut vcx);
    assert!(fans(cx).is_empty());
}

// Settings window P4 rows: column separators, Ctrl+Esc, TaskSlinger import (E21-041).

const CTRL_ESC_LABEL: &str = "Open with Ctrl+Esc";
const IMPORT_LABEL: &str = "Import from TaskSlinger…";

#[gpui_kit::test]
fn column_separators_switch(cx: &mut TestAppContext) {
    setup(cx);
    let processes = page(cx, PageId::Processes);
    assert!(!cx.update(|cx| TlSettings::settings(cx).tables.column_separators));
    cx.update(|cx| processes.set_bool(SettingKey::TablesColumnSeparators, true, cx));
    assert!(cx.update(|cx| TlSettings::settings(cx).tables.column_separators));
}

/// A fake KDE shortcut: `get` returns what `stored` holds, `set` returns `result` and stores
/// the value only when `apply` is set. Returns the `set` calls too.
fn fake_shortcut(
    cx: &mut TestAppContext,
    initial: Option<&str>,
    result: Result<(), String>,
    apply: bool,
) -> Arc<std::sync::Mutex<Vec<Option<String>>>> {
    let stored = Arc::new(std::sync::Mutex::new(initial.map(str::to_owned)));
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (read, write, log) = (stored.clone(), stored, calls.clone());
    cx.update(|cx| {
        cx.global_mut::<AppEnv>().global_shortcut = Some(tl_ui::app::GlobalShortcutApi {
            get: Arc::new(move || read.lock().expect("lock").clone()),
            set: Arc::new(move |value| {
                log.lock().expect("lock").push(value.map(str::to_owned));
                if result.is_ok() && apply {
                    *write.lock().expect("lock") = value.map(str::to_owned);
                }
                result.clone()
            }),
        });
    });
    calls
}

#[gpui_kit::test]
fn ctrl_esc_row_reads_shortcut(cx: &mut TestAppContext) {
    setup(cx);
    let calls = fake_shortcut(cx, Some("Ctrl+Esc"), Ok(()), true);
    let startup = page(cx, PageId::StartupWindow);
    assert!(startup.local_on(CTRL_ESC_LABEL));
    assert!(!startup.local_disabled(CTRL_ESC_LABEL));
    assert_eq!(startup.local_description(CTRL_ESC_LABEL), None);
    cx.update(|cx| startup.set_local(CTRL_ESC_LABEL, false, cx));
    assert!(!startup.local_on(CTRL_ESC_LABEL));
    cx.update(|cx| startup.set_local(CTRL_ESC_LABEL, true, cx));
    assert!(startup.local_on(CTRL_ESC_LABEL));
    assert_eq!(
        *calls.lock().expect("lock"),
        [None, Some("Ctrl+Esc".to_owned())],
        "off clears the shortcut, on sets Ctrl+Esc"
    );
    assert_eq!(startup.local_description(CTRL_ESC_LABEL), None);
}

#[gpui_kit::test]
fn ctrl_esc_not_available_without_api(cx: &mut TestAppContext) {
    setup(cx);
    let startup = page(cx, PageId::StartupWindow);
    assert!(startup.local_disabled(CTRL_ESC_LABEL));
    assert!(!startup.local_on(CTRL_ESC_LABEL));
    assert_eq!(
        startup.local_description(CTRL_ESC_LABEL).as_deref(),
        Some("Not available")
    );
}

#[gpui_kit::test]
fn ctrl_esc_relogin_description(cx: &mut TestAppContext) {
    setup(cx);
    fake_shortcut(cx, None, Ok(()), false);
    let startup = page(cx, PageId::StartupWindow);
    assert!(!startup.local_on(CTRL_ESC_LABEL));
    cx.update(|cx| startup.set_local(CTRL_ESC_LABEL, true, cx));
    assert!(startup.local_on(CTRL_ESC_LABEL));
    assert_eq!(
        startup.local_description(CTRL_ESC_LABEL).as_deref(),
        Some("Log out and back in to apply")
    );
}

#[gpui_kit::test]
fn ctrl_esc_write_error_reverts(cx: &mut TestAppContext) {
    setup(cx);
    fake_shortcut(cx, None, Err("no kglobalaccel".to_owned()), false);
    let (_, mut vcx) = open(cx, 1);
    let startup = page(cx, PageId::StartupWindow);
    vcx.update(|_, cx| startup.set_local(CTRL_ESC_LABEL, true, cx));
    vcx.run_until_parked();
    assert!(!startup.local_on(CTRL_ESC_LABEL));
    assert_eq!(startup.local_description(CTRL_ESC_LABEL), None);
    assert!(window_toasts(&mut vcx).is_empty(), "no toast");
}

/// Presses the import button, answers the file chooser with `picked` and lets the import run.
fn press_import(vcx: &mut VisualTestContext, picked: &str) {
    let advanced = vcx
        .update(|_, cx| all_pages(cx))
        .into_iter()
        .find(|p| p.id() == PageId::Advanced)
        .expect("Advanced page");
    vcx.update(|window, cx| advanced.press(IMPORT_LABEL, window, cx));
    vcx.run_until_parked();
    assert!(vcx.did_prompt_for_paths(), "the file chooser opened");
    let path = PathBuf::from(picked);
    vcx.simulate_path_prompt_response(move |_| Some(vec![path]));
    vcx.run_until_parked();
}

fn import_env(
    cx: &mut TestAppContext,
    ini: Result<(u32, u32), String>,
    themes_text: Result<String, String>,
) -> Arc<std::sync::Mutex<Vec<PathBuf>>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (ini_log, theme_log) = (seen.clone(), seen.clone());
    cx.update(|cx| {
        let env = cx.global_mut::<AppEnv>();
        env.import_taskslinger = Arc::new(move |path, settings, state| {
            ini_log.lock().expect("lock").push(path.to_owned());
            let mut settings = settings.clone();
            settings.debug.show_fps = true;
            ini.clone().map(|(s, t)| tl_ui::app::ImportedSettings {
                settings,
                state: state.clone(),
                summary: tl_ui::app::ImportSummary {
                    settings: s,
                    themes: t,
                },
            })
        });
        env.read_ts_themes = Arc::new(move |path| {
            theme_log.lock().expect("lock").push(path.to_owned());
            themes_text.clone()
        });
    });
    seen
}

#[gpui_kit::test]
fn ts_import_toast(cx: &mut TestAppContext) {
    setup(cx);
    let seen = import_env(cx, Ok((12, 2)), Ok(String::new()));
    let (_, mut vcx) = open(cx, 1);
    press_import(&mut vcx, "/home/u/UserSettings.ini");
    assert_eq!(
        *seen.lock().expect("lock"),
        [PathBuf::from("/home/u/UserSettings.ini")]
    );
    assert_eq!(
        window_toasts(&mut vcx),
        ["Imported 12 settings and 2 themes"]
    );
    assert!(
        cx.update(|cx| TlSettings::settings(cx).debug.show_fps),
        "the imported settings are applied"
    );
}

#[gpui_kit::test]
fn ts_import_failure_toast(cx: &mut TestAppContext) {
    setup(cx);
    import_env(cx, Err("unreadable".to_owned()), Ok(String::new()));
    let (_, mut vcx) = open(cx, 1);
    press_import(&mut vcx, "/home/u/UserSettings.ini");
    assert_eq!(
        window_toasts(&mut vcx),
        ["Could not import TaskSlinger settings"]
    );
    assert!(!cx.update(|cx| TlSettings::settings(cx).debug.show_fps));
}

#[gpui_kit::test]
fn ts_import_themes_file(cx: &mut TestAppContext) {
    setup(cx);
    let seen = import_env(cx, Ok((9, 9)), Ok(r#"{"themes":[]}"#.to_owned()));
    let (_, mut vcx) = open(cx, 1);
    press_import(&mut vcx, "/home/u/themes.json");
    assert_eq!(
        *seen.lock().expect("lock"),
        [PathBuf::from("/home/u/themes.json")]
    );
    assert_eq!(
        window_toasts(&mut vcx),
        ["Imported 0 settings and 0 themes"],
        "a .json pick imports themes only"
    );
    assert!(!cx.update(|cx| TlSettings::settings(cx).debug.show_fps));

    // A refused themes file is the failure toast.
    let past = tl_ui::components::toast::COALESCE_WINDOW + std::time::Duration::from_millis(500);
    vcx.executor().advance_clock(past);
    import_env(cx, Ok((0, 0)), Err("too large".to_owned()));
    press_import(&mut vcx, "/home/u/themes.json");
    assert!(window_toasts(&mut vcx).contains(&"Could not import TaskSlinger settings".to_owned()));
}

// Smoke-test polish (tasklens-bj6.56).

fn set_search(frame: &Entity<Frame>, vcx: &mut VisualTestContext, text: &str) {
    let content = vcx.read(|cx| frame.read(cx).content().clone());
    vcx.update(|window, cx| content.update(cx, |w, cx| w.set_search(text, window, cx)));
    draw_probe(vcx);
}

#[gpui_kit::test]
fn settings_search_without_match_shows_message(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open(cx, 1);
    assert!(vcx.debug_bounds("SettingsNoMatch").is_none());
    set_search(&frame, &mut vcx, "zzqqxx");
    assert!(
        vcx.debug_bounds("SettingsNoMatch").is_some(),
        "a search with no match says so"
    );
    set_search(&frame, &mut vcx, "default tab");
    assert!(
        vcx.debug_bounds("SettingsNoMatch").is_none(),
        "a title match shows the page"
    );
    set_search(&frame, &mut vcx, "");
    assert!(vcx.debug_bounds("SettingsNoMatch").is_none());
}

#[gpui_kit::test]
fn settings_two_group_page_has_no_sidebar_children(cx: &mut TestAppContext) {
    setup(cx);
    // The kit lists sub-entries (and a chevron) for a page with 2+ kit groups.
    let general = page(cx, PageId::General);
    let appearance = page(cx, PageId::Appearance);
    let (general, appearance) = cx.update(|cx| (general.kit_groups(cx), appearance.kit_groups(cx)));
    assert_eq!(general.len(), 1, "General has two groups: one kit group");
    assert!(appearance.len() > 2, "Appearance keeps its groups");
}

#[gpui_kit::test]
fn settings_groups_keep_clear_of_the_scrollbar(cx: &mut TestAppContext) {
    setup(cx);
    let groups = cx.update(|cx| {
        all_pages(cx)
            .into_iter()
            .filter(|p| !p.is_empty())
            .flat_map(|p| p.kit_groups(cx))
            .collect::<Vec<_>>()
    });
    assert!(!groups.is_empty());
    for mut group in groups {
        assert_eq!(
            group.style().padding.right,
            Some(px(tl_ui::window::settings::SCROLLBAR_GAP).into()),
            "every group leaves room for the scrollbar"
        );
    }
}
