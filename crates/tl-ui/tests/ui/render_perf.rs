//! Performance tab goldens (UI-PRF-058, tasklens-o55.47): the Overview and the CPU page at
//! 1280 × 800, ui_scale 1.0, in the Default Dark and Default Light themes, compared with
//! `tests/golden/perf_<page>_<theme>.png`. The Overview is also checked at 1920 × 1200
//! (UI-PRF-059), in `perf_overview_1920_<theme>.png`. `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! Run with `cargo test -p tl-ui --features render-tests,test-hooks --test ui -- render_perf::`.

#![cfg(feature = "render-tests")]

#[cfg(not(feature = "test-hooks"))]
compile_error!("render_perf needs --features render-tests,test-hooks (R-291)");

use std::sync::Arc;

use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::{CpuSet, CpuTopology, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::cpu::CpuPage;
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::page::PerfSidebar;
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::testing::render::{RenderHarness, assert_golden, run_on_lavapipe};
use tl_ui::theme::current;

const WIDTH: f32 = 1280.;
const HEIGHT: f32 = 800.;

const S: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;
const GIB: u64 = 1 << 30;
/// Sample period of every fixture series.
const PERIOD_NS: u64 = 500 * MS;
/// Samples per series: 70 s, more than the 60 s range.
const SAMPLES: u64 = 140;

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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The theme's window background and text colour, as the main window sets them.
        let theme = current(cx);
        div()
            .size_full()
            .bg(theme.window_bg)
            .text_color(theme.text_primary)
            .child(self.page.clone())
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
        entry(2, DeviceIdentity::Aggregate(AggregateKind::Nets)),
    ];
    entries.extend((0..8).map(|id| entry(100 + id, DeviceIdentity::CpuLogical { id })));
    DeviceCatalog {
        version: 1,
        entries: Arc::from(entries),
    }
}

/// Newest sample time: 3.4 s past a 60 s boundary at least a minute ahead of the clock. A chart
/// puts its right edge at its newest sample, so only the phase against the grid shows and every
/// run draws the same pixels. A chart with no samples would place its grid against the clock.
fn newest_ns() -> u64 {
    (now_boot_ns() / (60 * S) + 2) * 60 * S + 3_400 * MS
}

/// A series of `id` ending at `newest`: `base` plus a slow sine of amplitude `amp`, shifted by
/// `phase` radians.
fn series(id: SeriesId, newest: u64, base: f32, amp: f32, phase: f32) -> Arc<SeriesView> {
    let first = newest - (SAMPLES - 1) * PERIOD_NS;
    let samples: Vec<Sample> = (0..SAMPLES)
        .map(|i| {
            let x = i as f32 * 0.15 + phase;
            Sample::valid(first + i * PERIOD_NS, base + amp * x.sin())
        })
        .collect();
    Arc::new(SeriesView {
        id,
        version: 1,
        period_ns: PERIOD_NS,
        samples: Arc::from(samples),
    })
}

/// Every series the Overview boxes and the CPU page draw, so no chart is empty.
fn series_set(newest: u64) -> SeriesSet {
    let id = |device: u16, metric: MetricId| SeriesId {
        device: DeviceIdx(device),
        metric,
    };
    let mut views = vec![
        series(id(0, MetricId::CPU_TOTAL), newest, 42.0, 18.0, 0.0),
        series(id(0, MetricId::CPU_KERNEL), newest, 9.0, 4.0, 1.0),
        series(id(0, MetricId::CPU_FREQ), newest, 4200.0, 300.0, 0.5),
        series(id(0, MetricId::CPU_POWER_PKG), newest, 62.0, 15.0, 0.3),
        series(id(0, MetricId::CPU_POWER_CORE), newest, 40.0, 10.0, 0.8),
        series(id(1, MetricId::MEM_USED), newest, 9.0e9, 4.0e8, 0.2),
        series(id(1, MetricId::MEM_CACHED), newest, 4.0e9, 1.0e8, 0.4),
        series(id(1, MetricId::MEM_AVAILABLE), newest, 6.5e9, 4.0e8, 2.0),
        series(id(1, MetricId::SWAP_USED), newest, 5.0e8, 0.0, 0.0),
        series(id(1, MetricId::MEM_COMMIT), newest, 1.2e10, 3.0e8, 0.6),
        series(id(2, MetricId::NET_RX), newest, 2.5e6, 1.5e6, 0.0),
        series(id(2, MetricId::NET_TX), newest, 4.0e5, 2.0e5, 1.2),
    ];
    for core in 0..8u16 {
        let phase = f32::from(core) * 0.7;
        views.push(series(
            id(100 + core, MetricId::CPU_TOTAL),
            newest,
            40.0,
            25.0,
            phase,
        ));
        views.push(series(
            id(100 + core, MetricId::CPU_KERNEL),
            newest,
            8.0,
            4.0,
            phase,
        ));
    }
    views.sort_by_key(|v| v.id);
    SeriesSet {
        header: ComponentHeader {
            version: 1,
            ..ComponentHeader::default()
        },
        series: Arc::from(views),
        slow: Arc::from([]),
    }
}

fn build(page: Page, cx: &mut gpui_kit::App) -> Entity<Shot> {
    let engine = FakeEngine::new();
    let newest = newest_ns();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog());
        s.series = Arc::new(series_set(newest));
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.cpu.util_pct = Metric::valid(42.0);
        sys.cpu.kernel_pct = Metric::valid(9.0);
        sys.cpu.freq_mhz = Metric::valid(4200);
        sys.cpu.temp_c = Metric::valid(61.0);
        let mut online = CpuSet::default();
        for cpu in 0..8 {
            online.insert(cpu);
        }
        sys.cpu_topology = Arc::new(CpuTopology {
            online,
            cores: Arc::from([]),
        });
        sys.memory.validity = Validity::Valid;
        sys.memory.total = 16 * GIB;
        sys.memory.used = 9 * GIB;
        sys.memory.available = 6 * GIB;
        sys.memory.cached = 4 * GIB;
        sys.memory.swap_total = 8 * GIB;
        sys.memory.swap_free = 7 * GIB;
        sys.memory.committed = 12 * GIB;
        sys.memory.commit_limit = 24 * GIB;
    });
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
    // The page header reads its value from the sidebar card, as in `PerformancePage`.
    cx.set_global(PerfSidebar(sidebar.downgrade()));
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
    check_at(page, theme, name, WIDTH, HEIGHT);
}

fn check_at(page: Page, theme: &str, name: &str, width: f32, height: f32) {
    let mut harness = RenderHarness::new();
    harness.set_theme(theme);
    harness.set_scale(1.0);
    let mut img = harness.render(size(px(width), px(height)), |_, cx| build(page, cx));
    if matches!(page, Page::Cpu) {
        blank_up_time(&mut img);
    }
    assert_golden(name, &img);
}

/// The CPU page's Up time value, in device pixels (2 × logical): x 16..416 and y 584..604
/// logical. The page reads CLOCK_BOOTTIME itself, so this value differs on every run. It is
/// painted over with the page background taken from the padding left of it.
fn blank_up_time(img: &mut image::RgbaImage) {
    let bg = *img.get_pixel(16, 1168);
    for y in 1168..1208 {
        for x in 32..832 {
            img.put_pixel(x, y, bg);
        }
    }
}

#[test]
fn overview_dark() {
    run_on_lavapipe("render_perf::overview_dark", || {
        check(Page::Overview, "Default Dark", "perf_overview_dark")
    });
}

#[test]
fn overview_light() {
    run_on_lavapipe("render_perf::overview_light", || {
        check(Page::Overview, "Default Light", "perf_overview_light")
    });
}

#[test]
fn overview_1920_dark() {
    run_on_lavapipe("render_perf::overview_1920_dark", || {
        check_at(
            Page::Overview,
            "Default Dark",
            "perf_overview_1920_dark",
            1920.,
            1200.,
        )
    });
}

#[test]
fn overview_1920_light() {
    run_on_lavapipe("render_perf::overview_1920_light", || {
        check_at(
            Page::Overview,
            "Default Light",
            "perf_overview_1920_light",
            1920.,
            1200.,
        )
    });
}

#[test]
fn cpu_page_dark() {
    run_on_lavapipe("render_perf::cpu_page_dark", || {
        check(Page::Cpu, "Default Dark", "perf_cpu_dark")
    });
}

#[test]
fn cpu_page_light() {
    run_on_lavapipe("render_perf::cpu_page_light", || {
        check(Page::Cpu, "Default Light", "perf_cpu_light")
    });
}
