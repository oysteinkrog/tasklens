//! Performance tab goldens (UI-PRF-058, tasklens-o55.47): the Overview and the CPU page at
//! 1280 × 800, ui_scale 1.0, in the Default Dark and Default Light themes, compared with
//! `tests/golden/perf_<page>_<theme>.png`. The Overview is also checked at 1920 × 1200
//! (UI-PRF-059), in `perf_overview_1920_<theme>.png`. The whole tab, sidebar and Overview, is
//! checked with the sidebar shown (its collapse arrow on the right edge) and hidden (the expand
//! strip at the left edge), in `perf_tab_sidebar_<state>_dark.png` (tasklens-o5853). Compact
//! view (the sidebar and the status bar hidden) is checked with and without the exit button in
//! the top-right corner, in `perf_tab_compact[_exit]_dark.png` (tasklens-qdn1r).
//! `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! The Overview timeline labels its hours from CLOCK_REALTIME in the local time zone, so every
//! test runs with `TZ=UTC` and the timeline clock pinned at [`NOW`] and [`BOOT`]
//! ([`run_pinned`]). A golden then does not change with the hour or the host's zone.
//!
//! Run with `cargo test -p tl-ui-perf --features render-tests --test render`.

use std::sync::Arc;

use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::{CpuSet, CpuTopology, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::current;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::overview::timeline::set_test_clock;
use tl_ui_overview::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui_pagehost::render::{RenderHarness, run_on_lavapipe};
use tl_ui_perf::performance::cpu::CpuPage;
use tl_ui_perf::performance::memory::MemoryPage;
use tl_ui_perf::performance::page::PerfSidebar;
use tl_ui_perf::performance::sidebar::DeviceSidebar;
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow, PerformancePage};
use tl_ui_settings::settings::{CpuGraphMode, Settings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

/// Where this crate's golden PNGs live.
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// Compares `img` with `tests/golden/<name>.png`, or writes it when `TL_UPDATE_GOLDEN=1`.
fn assert_golden(name: &str, img: &tl_ui_testkit::render::RgbaImage) {
    tl_ui_pagehost::render::assert_golden(std::path::Path::new(GOLDEN_DIR), name, img);
}

const WIDTH: f32 = 1280.;
const HEIGHT: f32 = 800.;

const S: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;
/// The pinned CLOCK_REALTIME now: 2026-10-28 09:20 UTC, as in tl-ui's `overview_timeline` tests.
const NOW: i64 = 1_793_179_200 * S as i64;
/// The pinned CLOCK_BOOTTIME now: the machine booted 2 hours ago.
const BOOT: u64 = 2 * 3600 * S;
/// The time zone every test renders in.
const TEST_TZ: &str = "UTC";
const GIB: u64 = 1 << 30;
/// Sample period of every fixture series.
const PERIOD_NS: u64 = 500 * MS;
/// Samples per series: 70 s, more than the 60 s range.
const SAMPLES: u64 = 140;

#[derive(Clone, Copy)]
enum Page {
    Overview,
    Cpu,
    /// The CPU page in graph mode `mode`.
    CpuMode(CpuGraphMode),
    /// The Memory page, with a narrow compressed segment on the composition bar.
    Memory,
    /// The whole Performance tab on the Overview, with the sidebar hidden when `hidden`.
    Tab {
        hidden: bool,
    },
    /// The whole tab on the Overview in compact view, with the exit button drawn when
    /// `exit_button` (tasklens-qdn1r).
    Compact {
        exit_button: bool,
    },
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
        sys.memory.free = 2 * GIB;
        sys.memory.dirty = GIB / 4;
        sys.memory.writeback = 0;
        sys.memory.zswap = 5 << 28;
        sys.memory.swap_total = 8 * GIB;
        sys.memory.swap_free = 7 * GIB;
        sys.memory.committed = 12 * GIB;
        sys.memory.commit_limit = 24 * GIB;
    });
    let client: Arc<dyn EngineClient> = engine;
    init_strings(cx, "en-US");
    kv_grid::init(cx);
    cx.set_global(EngineGlobal(Arc::clone(&client)));
    let mut settings = Settings::default();
    if let Page::CpuMode(mode) = page {
        settings.performance.cpu_graph_mode = mode;
        // As the load does: the CPU page's old keys convert to its entry.
        settings.clamp();
    }
    let mut state = UiState::default();
    match page {
        Page::Tab { hidden } => {
            state.performance.selected_device = OVERVIEW_KEY.to_owned();
            state.performance.sidebar_hidden = hidden;
        }
        Page::Compact { .. } => {
            state.performance.selected_device = OVERVIEW_KEY.to_owned();
            state.window.compact_view = true;
        }
        _ => {}
    }
    cx.set_global(BootSettings {
        settings,
        state,
        persist: None,
        startup_toasts: Vec::new(),
    });
    TlSettings::init(cx);
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
    if let Page::Tab { .. } | Page::Compact { .. } = page {
        let host = HostContext::from_parts(
            Some(client),
            Some(hub.clone()),
            Some(demand.clone()),
            None,
            None,
        );
        let tab = cx.new(|cx| PerformancePage::new(&host, cx));
        if let Page::Compact { exit_button: true } = page {
            tab.update(cx, |p, cx| p.show_exit_button_for_test(cx));
        }
        return cx.new(|_| Shot {
            page: tab.clone().into(),
            _keep: vec![tab.into_any(), hub.into_any(), demand.into_any()],
        });
    }
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
        Page::Cpu | Page::CpuMode(_) => {
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
        Page::Memory => {
            let key = DeviceKey(Arc::from(DeviceKey::MEMORY));
            let row = DeviceRow {
                key: key.clone(),
                kind: DeviceKind::Memory,
                idx: DeviceIdx(1),
                generation: 0,
                net_class: None,
                fan_connected: None,
            };
            let page = cx.new(|cx| MemoryPage::new(key, row, hub.clone(), demand.clone(), cx));
            (page.clone().into(), vec![page.into_any()])
        }
        Page::Tab { .. } | Page::Compact { .. } => unreachable!("built above"),
    };
    keep.extend([hub.into_any(), demand.into_any(), sidebar.into_any()]);
    cx.new(|_| Shot {
        page: view,
        _keep: keep,
    })
}

/// Runs `body` on lavapipe with `TZ=UTC` and the timeline clock pinned at [`NOW`] and [`BOOT`].
///
/// When `TZ` is not `UTC`, the test binary runs the test `test_path` again in a child process
/// with `TZ=UTC`, because setting the variable in a running test process is not thread safe.
/// jiff reads `TZ` when it first looks up the system zone.
fn run_pinned(test_path: &str, body: impl FnOnce()) {
    if std::env::var_os("TZ").is_some_and(|tz| tz == TEST_TZ) {
        run_on_lavapipe(test_path, || {
            set_test_clock(Some((NOW, BOOT)));
            body();
        });
        return;
    }
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args([test_path, "--exact", "--nocapture", "--test-threads=1"])
        .env("TZ", TEST_TZ)
        .output()
        .expect("run the test again with TZ=UTC");
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "{test_path} failed with TZ={TEST_TZ}"
    );
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
fn blank_up_time(img: &mut tl_ui_testkit::render::RgbaImage) {
    let bg = *img.get_pixel(16, 1168);
    for y in 1168..1208 {
        for x in 32..832 {
            img.put_pixel(x, y, bg);
        }
    }
}

#[test]
fn overview_dark() {
    run_pinned("render_perf::overview_dark", || {
        check(Page::Overview, "Default Dark", "perf_overview_dark")
    });
}

#[test]
fn overview_light() {
    run_pinned("render_perf::overview_light", || {
        check(Page::Overview, "Default Light", "perf_overview_light")
    });
}

#[test]
fn overview_1920_dark() {
    run_pinned("render_perf::overview_1920_dark", || {
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
    run_pinned("render_perf::overview_1920_light", || {
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
    run_pinned("render_perf::cpu_page_dark", || {
        check(Page::Cpu, "Default Dark", "perf_cpu_dark")
    });
}

/// The composition bar labels do not overlap at the default window size (tasklens-frt94).
#[test]
fn memory_page_dark() {
    run_pinned("render_perf::memory_page_dark", || {
        check(Page::Memory, "Default Dark", "perf_memory_dark")
    });
}

#[test]
fn cpu_page_light() {
    run_pinned("render_perf::cpu_page_light", || {
        check(Page::Cpu, "Default Light", "perf_cpu_light")
    });
}

/// The all-cores charts show the CPU total ("42.0%") at the top right, as 1.3.0 did
/// (tasklens-hzire.11).
#[test]
fn cpu_all_cores_overlaid_dark() {
    run_pinned("render_perf::cpu_all_cores_overlaid_dark", || {
        check_at(
            Page::CpuMode(CpuGraphMode::AllCoresOverlaid),
            "Default Dark",
            "perf_cpu_overlaid_dark",
            WIDTH,
            480.,
        )
    });
}

#[test]
fn cpu_all_cores_stacked_dark() {
    run_pinned("render_perf::cpu_all_cores_stacked_dark", || {
        check_at(
            Page::CpuMode(CpuGraphMode::AllCoresStacked),
            "Default Dark",
            "perf_cpu_stacked_dark",
            WIDTH,
            480.,
        )
    });
}

/// The tab with the sidebar and its collapse arrow on the right edge (tasklens-o5853).
#[test]
fn tab_sidebar_shown_dark() {
    run_pinned("render_perf::tab_sidebar_shown_dark", || {
        check(
            Page::Tab { hidden: false },
            "Default Dark",
            "perf_tab_sidebar_shown_dark",
        )
    });
}

/// The tab with the sidebar hidden: the 12 px expand strip and the Overview on the rest of the
/// width (tasklens-o5853).
#[test]
fn tab_sidebar_hidden_dark() {
    run_pinned("render_perf::tab_sidebar_hidden_dark", || {
        check(
            Page::Tab { hidden: true },
            "Default Dark",
            "perf_tab_sidebar_hidden_dark",
        )
    });
}

/// Compact view: no sidebar, no status bar, no exit button while the mouse is away
/// (tasklens-qdn1r).
#[test]
fn tab_compact_dark() {
    run_pinned("render_perf::tab_compact_dark", || {
        check(
            Page::Compact { exit_button: false },
            "Default Dark",
            "perf_tab_compact_dark",
        )
    });
}

/// Compact view with the exit button in the top-right corner, as a mouse move shows it
/// (tasklens-qdn1r).
#[test]
fn tab_compact_exit_dark() {
    run_pinned("render_perf::tab_compact_exit_dark", || {
        check(
            Page::Compact { exit_button: true },
            "Default Dark",
            "perf_tab_compact_exit_dark",
        )
    });
}
