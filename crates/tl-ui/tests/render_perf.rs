//! Performance tab goldens (UI-PRF-058, tasklens-o55.47): the Overview and the CPU page at
//! 1280 × 800, ui_scale 1.0, in the Default Dark and Default Light themes, compared with
//! `tests/golden/perf_<page>_<theme>.png`. `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! Run with `cargo test -p tl-ui --features render-tests,test-hooks --test render_perf`.

#![cfg(feature = "render-tests")]

#[cfg(not(feature = "test-hooks"))]
compile_error!("render_perf needs --features render-tests,test-hooks (R-291)");

use std::sync::Arc;

use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{Settings, UiState};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::cpu::CpuPage;
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::testing::render::{RenderHarness, assert_golden, run_on_lavapipe};

const WIDTH: f32 = 1280.;
const HEIGHT: f32 = 800.;

#[derive(Clone, Copy)]
enum Page {
    Overview,
    Cpu,
}

/// The page, with the entities that keep it fed alive.
struct Shot {
    page: AnyView,
    _keep: Vec<gpui_kit::AnyEntity>,
}

impl Render for Shot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.page.clone())
    }
}

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("dev"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c: None,
    }
}

fn catalog() -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage),
        entry(1, DeviceIdentity::Memory),
    ];
    entries.extend((0..8).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id })));
    DeviceCatalog {
        version: 1,
        entries: Arc::from(entries),
    }
}

fn build(page: Page, cx: &mut gpui_kit::App) -> Entity<Shot> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| s.devices = Arc::new(catalog()));
    let client: Arc<dyn EngineClient> = engine;
    init_strings(cx, "en-US");
    kv_grid::init(cx);
    cx.set_global(EngineGlobal(Arc::clone(&client)));
    cx.set_global(BootSettings {
        settings: Settings::default(),
        state: UiState::default(),
        persist: None,
        startup_toasts: Vec::new(),
    });
    TlSettings::init(cx);
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let sidebar = cx.new(|cx| DeviceSidebar::new(hub.clone(), demand.clone(), cx));
    let (view, mut keep): (AnyView, Vec<gpui_kit::AnyEntity>) = match page {
        Page::Overview => {
            let page = cx.new(|cx| {
                OverviewPage::new(
                    DeviceKey(Arc::from(OVERVIEW_KEY)),
                    hub.clone(),
                    demand.clone(),
                    &sidebar,
                    cx,
                )
            });
            (page.clone().into(), vec![page.into_any()])
        }
        Page::Cpu => {
            let key = DeviceKey(Arc::from(DeviceKey::CPU));
            let row = DeviceRow {
                key: key.clone(),
                kind: DeviceKind::Cpu,
                idx: DeviceIdx(0),
                generation: 0,
                net_class: None,
                fan_connected: None,
            };
            let page = cx.new(|cx| CpuPage::new(key, row, hub.clone(), demand.clone(), cx));
            (page.clone().into(), vec![page.into_any()])
        }
    };
    keep.extend([hub.into_any(), demand.into_any(), sidebar.into_any()]);
    cx.new(|_| Shot {
        page: view,
        _keep: keep,
    })
}

fn check(page: Page, theme: &str, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme(theme);
    harness.set_scale(1.0);
    let img = harness.render(size(px(WIDTH), px(HEIGHT)), |_, cx| build(page, cx));
    assert_golden(name, &img);
}

#[test]
fn overview_dark() {
    run_on_lavapipe("overview_dark", || {
        check(Page::Overview, "Default Dark", "perf_overview_dark")
    });
}

#[test]
fn overview_light() {
    run_on_lavapipe("overview_light", || {
        check(Page::Overview, "Default Light", "perf_overview_light")
    });
}

#[test]
fn cpu_page_dark() {
    run_on_lavapipe("cpu_page_dark", || {
        check(Page::Cpu, "Default Dark", "perf_cpu_dark")
    });
}

#[test]
fn cpu_page_light() {
    run_on_lavapipe("cpu_page_light", || {
        check(Page::Cpu, "Default Light", "perf_cpu_light")
    });
}
