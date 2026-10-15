//! Detached graph window tests (tasklens-xdh.16, PERF-083, UI-PRF-044, R-086). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement as _,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, WindowDecorations, WindowKind,
    div, point, px, size,
};
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::time::now_boot_ns;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::ChartState;
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::pages::minigraph::GraphColor;
use tl_ui::pages::minigraph::sources::{GraphColumn, MinigraphEntry, MinigraphId, MinigraphSource};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::APP_ID;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::titlebar::TitleBarProbe;
use tl_ui::windows::detached_graph::{
    DetachedGraphWindow, detached_window_options, open_detached, window_size,
};

const MS: u64 = 1_000_000;

/// A stand-in for the main window: it owns its own driver.
struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn catalog() -> DeviceCatalog {
    DeviceCatalog {
        version: 1,
        entries: vec![DeviceEntry {
            idx: DeviceIdx(0),
            identity: DeviceIdentity::CpuPackage,
            generation: 0,
            display_name: Arc::from("cpu"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
        }]
        .into(),
    }
}

fn cpu_entry() -> MinigraphEntry {
    MinigraphEntry {
        id: MinigraphId(1),
        source: MinigraphSource::Cpu,
        column: GraphColumn::Cpu,
        range_s: 60,
        color: GraphColor::Accent,
        slot: 0,
        detached: true,
        window: 1,
    }
}

fn setup(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        let appearance = AppearanceSettings::default();
        tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
        tl_ui::theme::metrics::init_metrics(&appearance, cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    engine.publish_with(|s| s.devices = Arc::new(catalog()));
    cx.run_until_parked();
    cx.update(|cx| {
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        cx.set_global(GlobalHub(hub));
        GlobalRegistry::init(cx);
    });
}

fn open(
    cx: &mut TestAppContext,
    id: u32,
    cards: Vec<MinigraphEntry>,
) -> (VisualTestContext, Entity<DetachedGraphWindow>) {
    let handle = cx
        .update(|cx| open_detached(id, cards, None, cx))
        .expect("window opens");
    let view = cx
        .update(|cx| DetachedGraphWindow::find(handle.into(), cx))
        .expect("detached view");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (vcx, view)
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn detached_window_opens_with_caption(cx: &mut TestAppContext) {
    setup(cx);
    let (mut vcx, view) = open(cx, 1, vec![cpu_entry()]);
    assert_eq!(
        vcx.read_entity(&view, |v, _| v.title().to_string()),
        "tasklens Graph 1"
    );
    let held = cx.read(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::DetachedGraph(1))
    });
    assert!(held.is_some(), "the registry holds DetachedGraph(1)");

    let options = cx.update(|cx| detached_window_options(window_size(1, 1.), cx));
    assert!(!options.is_resizable);
    assert!(options.is_movable);
    assert_eq!(options.kind, WindowKind::Floating);
    assert!(options.titlebar.is_none());
    assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
    assert_eq!(options.app_id.as_deref(), Some(APP_ID));
    let bounds = options.window_bounds.expect("bounds").get_bounds();
    assert_eq!(bounds.size, size(px(184.), px(98.)));

    // A window id above 255 does not fit the registry key.
    let before = cx.update(|cx| cx.windows().len());
    assert!(
        cx.update(|cx| open_detached(256, vec![cpu_entry()], None, cx))
            .is_err()
    );
    assert_eq!(cx.update(|cx| cx.windows().len()), before);
}

#[gpui_kit::test]
fn detached_body_drag_moves_window(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| cx.set_global(TitleBarProbe::default()));
    let (mut vcx, _view) = open(cx, 1, vec![cpu_entry()]);
    // The card centre, then its label row.
    for y in [49., 10.] {
        vcx.simulate_mouse_down(
            point(px(94.), px(y)),
            MouseButton::Left,
            Modifiers::default(),
        );
        vcx.simulate_mouse_up(
            point(px(94.), px(y)),
            MouseButton::Left,
            Modifiers::default(),
        );
    }
    assert_eq!(cx.read(|cx| cx.global::<TitleBarProbe>().moves), 2);
}

#[gpui_kit::test]
fn detached_window_own_driver(cx: &mut TestAppContext) {
    setup(cx);
    let (_host, mut host_vcx) = cx.add_window_view(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        Host
    });
    let host_driver = host_vcx.update(|window, cx| AnimationDriver::global(window, cx));
    let (mut vcx, view) = open(cx, 1, vec![cpu_entry()]);

    let (own, chart) = vcx.read_entity(&view, |v, cx| {
        (v.driver().clone(), v.cards()[0].1.read(cx).chart().clone())
    });
    let newest = now_boot_ns() - 50 * MS;
    let samples: Vec<Sample> = (0..600u64)
        .map(|i| Sample::valid(newest - (599 - i) * 100 * MS, (i % 100) as f32))
        .collect();
    let series = Arc::new(SeriesView {
        id: SeriesId {
            device: DeviceIdx(0),
            metric: MetricId::CPU_TOTAL,
        },
        version: 1,
        period_ns: 100 * MS,
        samples: samples.into(),
    });
    chart.update(&mut vcx, |c, cx| {
        c.set_series_for_test(smallvec![Some(series)], cx);
        c.set_state(ChartState::Live, cx);
    });
    draw(&mut vcx);
    assert!(
        vcx.read_entity(&own, |d, _| d.is_armed()),
        "the window's driver is armed"
    );
    assert_eq!(
        host_vcx.read_entity(&host_driver, |d, _| d.stats.requested),
        0
    );
    assert_ne!(own, host_driver);
}
