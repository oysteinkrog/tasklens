//! The Overview time range and range figures (UI-PRF-055, D-030). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, Render, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SampleFlags, SeriesView};
use tl_model::settings::{
    AppearanceSettings, OverviewRange, Settings, StatusBarPlacement, ThemeSettings, UiState,
};
use tl_model::snapshot::{ComponentHeader, DeviceHistory, HistPoint, SeriesSet};
use tl_model::time::now_boot_ns;
use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
use tl_ui::chart::{ChartView, NetFormat, ValueFormat, format_value};
use tl_ui::components::kv_grid;
use tl_ui::demand::{DemandAggregator, GlobalDemand};
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::overview::parts::body_id;
use tl_ui::pages::performance::overview::range::{
    HEADER_SWITCH, RangeText, STATUS_SWITCH, segment_id,
};
use tl_ui::pages::performance::overview::{
    BOX_ID_PREFIX, OVERVIEW_KEY, OverviewPage, RANGE_HEADER_ID,
};
use tl_ui::pages::performance::status_bar::BAR_ID;
use tl_ui::pages::performance::{DeviceKey, PerformancePage};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{
    self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
};
use tl_ui::window::titlebar::TitleBarProbe;

const S: u64 = 1_000_000_000;
const MIN: i64 = 60 * S as i64;

fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("dev"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind,
        hot_c: None,
    }
}

/// CPU, memory, a GPU, a disk and a wired interface.
fn catalog() -> Vec<DeviceEntry> {
    vec![
        entry(0, DeviceIdentity::CpuPackage, None),
        entry(1, DeviceIdentity::Memory, None),
        entry(
            2,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from("0000:01:00.0"),
            },
            None,
        ),
        entry(
            3,
            DeviceIdentity::Disk {
                major: 8,
                minor: 0,
                name: Arc::from("sda"),
                wwn_or_serial: None,
            },
            None,
        ),
        entry(
            5,
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex: 2,
                name: Arc::from("eth0"),
            },
            Some(NetKind::Wired),
        ),
    ]
}

struct FakeProcesses {
    focus: FocusHandle,
}

impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

impl Render for FakeProcesses {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{InteractiveElement as _, Styled as _};
        div().track_focus(&self.focus).size_full()
    }
}

impl shell::ShellPage for FakeProcesses {
    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }
}

pub(crate) struct Rig<'a> {
    pub(crate) page: Entity<PerformancePage>,
    pub(crate) demand: Entity<DemandAggregator>,
    pub(crate) engine: Arc<FakeEngine>,
    pub(crate) vcx: &'a mut VisualTestContext,
}

pub(crate) fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// The main window, 1600 × 1000, on the Performance tab with the Overview selected and every
/// card shown. Animations are off.
pub(crate) fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(catalog()),
        });
    });
    let fake = Arc::clone(&engine);
    let client: Arc<dyn EngineClient> = engine;
    let demand = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
        let theme_settings = ThemeSettings::default();
        tl_ui::theme::store::init(
            cx,
            &theme_settings,
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings: Settings {
                theme: theme_settings,
                ..Settings::default()
            },
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand.clone()));
        tl_ui::components::toast::init(cx);
        tl_ui::actions::bind_keys_global(cx);
        tl_ui::pages::performance::actions::bind_keys_performance(cx);
        cx.set_global(StartOptions::default());
        set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
            let page = cx.new(|cx| FakeProcesses {
                focus: cx.focus_handle().tab_stop(true),
            });
            PageSlot::new(page, cx)
        });
        cx.set_global(TitleBarProbe {
            client: Some(true),
            ..TitleBarProbe::default()
        });
        tl_ui::window::menubar::init(cx);
        shell::init(cx);
        demand
    });
    let handle = cx.update(open_main_window).expect("the main window opens");
    let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1600.), px(1000.)));
    vcx.update(|window, _| window.activate_window());
    draw(vcx);
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    vcx.dispatch_action(tl_ui::actions::SelectTab(Tab::Performance as usize));
    draw(vcx);
    let page = vcx
        .read_entity(&main, |m, _| {
            m.page_entity::<PerformancePage>(Tab::Performance)
        })
        .expect("PerformancePage built");
    let r = Rig {
        page,
        demand,
        engine: fake,
        vcx,
    };
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |s, cx| s.select(DeviceKey(Arc::from(OVERVIEW_KEY)), cx));
    });
    draw(r.vcx);
    r
}

pub(crate) fn overview(r: &mut Rig) -> Entity<OverviewPage> {
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        page.read(cx)
            .page(&DeviceKey(Arc::from(OVERVIEW_KEY)))
            .and_then(|v| v.clone().downcast::<OverviewPage>().ok())
            .expect("an OverviewPage")
    })
}

/// One chart of each device box.
pub(crate) fn box_charts(r: &mut Rig) -> Vec<Entity<ChartView>> {
    let page = overview(r);
    r.vcx.update(|_, cx| {
        let page = page.read(cx);
        vec![
            page.cpu_box().read(cx).total_chart().clone(),
            page.mem_box().read(cx).sparks()[0].clone(),
            page.gpu_box().read(cx).sections()[0].chart.clone(),
            page.disk_box().read(cx).rows()[0].read_chart.clone(),
            page.net_box().read(cx).down_chart().clone(),
        ]
    })
}

/// One chart that each device box draws in its default mode: CPU, Memory, Disk, Network. The
/// GPU box's default mode draws one chart per engine, and the fixture GPU has no engines.
fn drawn_charts(r: &mut Rig) -> Vec<Entity<ChartView>> {
    let page = overview(r);
    r.vcx.update(|_, cx| {
        let page = page.read(cx);
        vec![
            page.cpu_box().read(cx).total_chart().clone(),
            page.mem_box().read(cx).sparks()[0].clone(),
            page.disk_box().read(cx).rows()[0].pair_chart.clone(),
            page.net_box().read(cx).down_chart().clone(),
        ]
    })
}

/// The range text of each device box: CPU, Memory, GPU, Disk, Network.
fn range_texts(r: &mut Rig) -> Vec<RangeText> {
    let page = overview(r);
    r.vcx.update(|_, cx| {
        let page = page.read(cx);
        vec![
            page.cpu_box().read(cx).range_text().clone(),
            page.mem_box().read(cx).range_text().clone(),
            page.gpu_box().read(cx).range_text().clone(),
            page.disk_box().read(cx).range_text().clone(),
            page.net_box().read(cx).range_text().clone(),
        ]
    })
}

pub(crate) fn click(cx: &mut VisualTestContext, id: &'static str) {
    let at = cx.debug_bounds(id).expect(id).center();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    draw(cx);
}

pub(crate) fn set_range(r: &mut Rig, range: OverviewRange) {
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.overview_range = range);
    });
    draw(r.vcx);
}

fn cpu() -> SeriesId {
    SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    }
}

pub(crate) fn now_realtime() -> i64 {
    i64::try_from(jiff::Timestamp::now().as_nanosecond()).expect("fits")
}

/// Publishes a device history answer for `range` with the CPU total `points`, whose oldest
/// stored point is `since`.
fn publish_history(r: &mut Rig, range: DeviceRange, since: i64, points: Vec<HistPoint>) {
    publish_history_boots(r, range, since, points, &[]);
}

/// `publish_history` with boots at the CLOCK_REALTIME times `boots`.
fn publish_history_boots(
    r: &mut Rig,
    range: DeviceRange,
    since: i64,
    points: Vec<HistPoint>,
    boots: &[i64],
) {
    r.engine.publish_with(|s| {
        let version = s.device_history.header.version + 1;
        s.device_history = Arc::new(DeviceHistory {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            request: Some(DeviceHistoryRequest {
                range,
                end_realtime_ns: None,
            }),
            realtime_offset_ns: now_realtime() - now_boot_ns() as i64,
            since_realtime_ns: Some(since),
            step_ns: 60 * S,
            series: Arc::from([(cpu(), Arc::from(points))]),
            boots: Arc::from(boots),
            timeline: None,
        });
    });
    draw(r.vcx);
}

fn point(t: i64, min: f32, mean: f32, max: f32) -> HistPoint {
    HistPoint {
        t_realtime_ns: t,
        mean,
        min,
        max,
    }
}

/// A click on "1 h" in the status bar's switch (the bar is on the Overview by default) saves the
/// range, redraws every chart of every box at one hour and asks for the device history. With
/// the bar elsewhere the switch moves to the Overview header.
#[gpui_kit::test]
fn overview_range_switch_redraws_all_boxes(cx: &mut TestAppContext) {
    let mut r = open(cx);
    for chart in box_charts(&mut r) {
        let range = r.vcx.read_entity(&chart, |c, _| c.spec.range_ns);
        assert_eq!(range, 60 * S, "1 min by default");
    }
    let demand = r.demand.clone();
    let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(merged.device_history_range, None, "1 min is live");

    let header: &'static str =
        Box::leak(segment_id(HEADER_SWITCH, OverviewRange::H24).into_boxed_str());
    // The bar carries the switch: the page has no header row (tasklens-o55.54).
    assert!(r.vcx.debug_bounds(header).is_none(), "one switch");
    assert!(
        r.vcx.debug_bounds(RANGE_HEADER_ID).is_none(),
        "no header row with the bar on the Overview"
    );
    let id: &'static str = Box::leak(segment_id(STATUS_SWITCH, OverviewRange::H1).into_boxed_str());
    click(r.vcx, id);
    let saved = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.overview_range);
    assert_eq!(saved, OverviewRange::H1);
    assert_eq!(
        r.vcx
            .update(|_, cx| TlSettings::settings(cx).performance.graph_range_s),
        60,
        "graph_range_s stays"
    );
    for chart in box_charts(&mut r) {
        let range = r.vcx.read_entity(&chart, |c, _| c.spec.range_ns);
        assert_eq!(range, 3_600 * S);
    }
    let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(
        merged.device_history_range,
        Some(DeviceHistoryRequest {
            range: DeviceRange::H1,
            end_realtime_ns: None,
        })
    );
    // The bar off: the Overview header holds the switch.
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings.performance.status_bar = StatusBarPlacement::Off;
        });
    });
    draw(r.vcx);
    assert!(r.vcx.debug_bounds(RANGE_HEADER_ID).is_some());
    assert!(r.vcx.debug_bounds(id).is_none(), "no status bar");
    click(r.vcx, header);
    for chart in box_charts(&mut r) {
        let range = r.vcx.read_entity(&chart, |c, _| c.spec.range_ns);
        assert_eq!(range, 86_400 * S);
    }
}

/// UI-PRF-060: with `history.device_metrics` off, 24 h and 7 d are disabled in both switches,
/// so a click keeps the range. 1 min to 1 h stay. Turned on again, 24 h can be picked.
#[gpui_kit::test]
fn overview_range_long_ranges_need_device_history(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let segment = |prefix: &str, range: OverviewRange| -> &'static str {
        Box::leak(segment_id(prefix, range).into_boxed_str())
    };
    let saved = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| TlSettings::settings(cx).performance.overview_range)
    };
    let set_kept = |r: &mut Rig, on: bool| {
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.history.device_metrics = on);
        });
        draw(r.vcx);
    };
    set_kept(&mut r, false);
    for range in [OverviewRange::H24, OverviewRange::D7] {
        click(r.vcx, segment(STATUS_SWITCH, range));
        assert_eq!(saved(&mut r), OverviewRange::M1, "{range:?} in the bar");
    }
    click(r.vcx, segment(STATUS_SWITCH, OverviewRange::H1));
    assert_eq!(saved(&mut r), OverviewRange::H1, "1 h stays enabled");

    // The bar off: the Overview header's switch has the same rule.
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings.performance.status_bar = StatusBarPlacement::Off;
        });
    });
    draw(r.vcx);
    for range in [OverviewRange::H24, OverviewRange::D7] {
        click(r.vcx, segment(HEADER_SWITCH, range));
        assert_eq!(saved(&mut r), OverviewRange::H1, "{range:?} in the header");
    }
    click(r.vcx, segment(HEADER_SWITCH, OverviewRange::M5));
    assert_eq!(saved(&mut r), OverviewRange::M5);

    set_kept(&mut r, true);
    click(r.vcx, segment(HEADER_SWITCH, OverviewRange::H24));
    assert_eq!(saved(&mut r), OverviewRange::H24, "enabled again");
    click(r.vcx, segment(HEADER_SWITCH, OverviewRange::D7));
    assert_eq!(saved(&mut r), OverviewRange::D7);
}

/// min, avg and peak of the CPU total: the valid live samples at 1 min, and the stored min,
/// mean and max of the history points at 24 h, gaps left out.
#[gpui_kit::test]
fn overview_range_figures_min_avg_peak(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let newest = now_boot_ns();
    r.engine.publish_with(|s| {
        let version = s.series.header.version + 1;
        s.series = Arc::new(SeriesSet {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            series: Arc::from([Arc::new(SeriesView {
                id: cpu(),
                version,
                period_ns: S,
                samples: Arc::from([
                    Sample::valid(newest - 3 * S, 10.),
                    Sample::gap(newest - 2 * S),
                    Sample::valid(newest - S, 30.),
                    Sample::valid(newest, 20.),
                ]),
            })]),
            slow: Arc::from([]),
        });
    });
    draw(r.vcx);
    assert_eq!(range_texts(&mut r)[0].figures, "min 10%  avg 20%  peak 30%");

    set_range(&mut r, OverviewRange::H24);
    let now = now_realtime();
    publish_history(
        &mut r,
        DeviceRange::H24,
        now - 30 * 60 * MIN,
        vec![
            point(now - 3 * MIN, 5., 20., 60.),
            point(now - 2 * MIN, f32::NAN, f32::NAN, f32::NAN),
            point(now - MIN, 2., 40., 100.),
        ],
    );
    assert_eq!(range_texts(&mut r)[0].figures, "min 2%  avg 30%  peak 100%");
}

/// The Network figures are download plus upload of the engine's aggregate of the counted
/// interfaces, the set the Network box charts, not of the first adapter.
#[gpui_kit::test]
fn overview_range_net_figures_use_aggregate(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let mut entries = catalog();
    entries.push(entry(
        6,
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: 3,
            name: Arc::from("wlan0"),
        },
        Some(NetKind::Wireless),
    ));
    entries.push(entry(
        7,
        DeviceIdentity::Aggregate(AggregateKind::Nets),
        None,
    ));
    let newest = now_boot_ns();
    r.engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 2,
            entries: Arc::from(entries),
        });
        let version = s.series.header.version + 1;
        let view = |device: u16, metric: MetricId, v: f32| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(device),
                    metric,
                },
                version,
                period_ns: S,
                samples: Arc::from([Sample::valid(newest - S, v), Sample::valid(newest, v)]),
            })
        };
        s.series = Arc::new(SeriesSet {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            series: Arc::from([
                view(5, MetricId::NET_RX, 1_000.),
                view(5, MetricId::NET_TX, 0.),
                view(6, MetricId::NET_RX, 3_000.),
                view(6, MetricId::NET_TX, 1_000.),
                view(7, MetricId::NET_RX, 4_000.),
                view(7, MetricId::NET_TX, 1_000.),
            ]),
            slow: Arc::from([]),
        });
    });
    draw(r.vcx);
    let unit = r.vcx.update(|_, cx| NetFormat::of(cx));
    let mut rate = String::new();
    format_value(ValueFormat::RateNetwork, 5_000., unit, &mut rate);
    assert_eq!(
        range_texts(&mut r)[4].figures,
        format!("min {rate}  avg {rate}  peak {rate}")
    );
}

/// At 24 h a point whose max is 100 and mean is 5 draws its max: the CPU line reaches the plot
/// top, so a spike shorter than the 10 s tier step stays visible.
#[gpui_kit::test]
fn overview_range_history_draws_point_max(cx: &mut TestAppContext) {
    let mut r = open(cx);
    set_range(&mut r, OverviewRange::H24);
    let now = now_realtime();
    publish_history(
        &mut r,
        DeviceRange::H24,
        now - 30 * 60 * MIN,
        vec![
            point(now - 300 * MIN, 5., 5., 5.),
            point(now - 299 * MIN, 5., 5., 100.),
            point(now - 298 * MIN, 5., 5., 5.),
        ],
    );
    let chart = box_charts(&mut r)[0].clone();
    let top = r
        .vcx
        .read_entity(&chart, |c, _| c.stroke_top_y())
        .expect("strokes");
    assert!(top.abs() <= 1.0, "the line tops out at {top}, not at 100");
}

/// At 24 h a gap point and a stretch without points both draw as breaks: two runs and the
/// dotted baseline between them, never a line through zero.
#[gpui_kit::test]
fn overview_range_gap_draws_break(cx: &mut TestAppContext) {
    let mut r = open(cx);
    set_range(&mut r, OverviewRange::H24);
    let now = now_realtime();
    publish_history(
        &mut r,
        DeviceRange::H24,
        now - 30 * 60 * MIN,
        vec![
            point(now - 600 * MIN, 10., 10., 10.),
            point(now - 599 * MIN, 12., 12., 12.),
            point(now - 598 * MIN, f32::NAN, f32::NAN, f32::NAN),
            point(now - 597 * MIN, 14., 14., 14.),
            // Nothing stored for 5 hours: the app was not running.
            point(now - 300 * MIN, 50., 50., 50.),
            point(now - 299 * MIN, 52., 52., 52.),
        ],
    );
    let chart = box_charts(&mut r)[0].clone();
    let (samples, gaps) = r.vcx.read_entity(&chart, |c, _| {
        let view = c.resolved()[0].clone().expect("the CPU series");
        (view.samples.clone(), c.last_gap_spans.borrow().clone())
    });
    assert_eq!(samples.len(), 6);
    assert!(!samples[2].is_valid(), "the gap point is a gap sample");
    assert!(
        samples[4].flags.contains(SampleFlags::GAP_BEFORE),
        "five hours without points start a new run"
    );
    assert!(
        !samples[3].flags.contains(SampleFlags::GAP_BEFORE),
        "the next minute joins"
    );
    assert!(
        gaps.len() >= 2,
        "a dotted break for the gap point and one for the missing hours: {gaps:?}"
    );
}

/// A boot inside the range draws a boot marker on every drawn box chart; the live ranges draw
/// none.
#[gpui_kit::test]
fn overview_range_boot_marker(cx: &mut TestAppContext) {
    let mut r = open(cx);
    set_range(&mut r, OverviewRange::H24);
    let now = now_realtime();
    publish_history_boots(
        &mut r,
        DeviceRange::H24,
        now - 30 * 60 * MIN,
        vec![
            point(now - 400 * MIN, 10., 10., 10.),
            // The machine was off, then booted 300 minutes ago.
            point(now - 290 * MIN, 20., 20., 20.),
        ],
        &[now - 300 * MIN],
    );
    for (i, chart) in drawn_charts(&mut r).into_iter().enumerate() {
        let marks = r.vcx.read_entity(&chart, |c, _| c.last_boot_marks.get());
        assert_eq!(marks, 1, "one boot marker on drawn chart {i}");
    }
    set_range(&mut r, OverviewRange::M1);
    draw(r.vcx);
    for chart in box_charts(&mut r) {
        let marks = r.vcx.read_entity(&chart, |c, _| c.last_boot_marks.get());
        assert_eq!(marks, 0, "live charts have no boot markers");
    }
}

/// tasklens-o55.54: with the default layout and `overview_row_unit_px` 40, at 1920 × 1200 (and
/// at 1280 × 800), every box's chrome, its height less the body that holds its plots, is at
/// most 40 px, and the first box starts at most 8 px below the status bar on top.
#[gpui_kit::test]
fn overview_box_chrome_at_most_40(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let unit = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.overview_row_unit_px);
    assert_eq!(unit, 40);
    let page = overview(&mut r);
    for (w, h) in [(1920., 1200.), (1280., 800.)] {
        r.vcx.simulate_resize(size(px(w), px(h)));
        draw(r.vcx);
        let bar = r
            .vcx
            .debug_bounds(BAR_ID)
            .expect("status bar on the Overview");
        assert!(
            r.vcx.debug_bounds(RANGE_HEADER_ID).is_none(),
            "{w}x{h}: no title row"
        );
        let kinds: Vec<_> = r.vcx.update(|_, cx| {
            page.read(cx)
                .placement()
                .boxes
                .iter()
                .map(|b| b.kind)
                .collect()
        });
        assert_eq!(
            kinds.len(),
            6,
            "{w}x{h}: five device boxes and the timeline"
        );
        let mut first = px(f32::MAX);
        for kind in kinds {
            let frame_id: &'static str =
                Box::leak(format!("{BOX_ID_PREFIX}{}", kind.key()).into_boxed_str());
            let body_sel: &'static str = Box::leak(body_id(frame_id).into_boxed_str());
            let frame = r.vcx.debug_bounds(frame_id).expect(frame_id);
            let body = r.vcx.debug_bounds(body_sel).expect(body_sel);
            let chrome = frame.size.height - body.size.height;
            assert!(
                chrome <= px(40.),
                "{w}x{h} {kind:?}: chrome {chrome:?} ({frame:?}, body {body:?})"
            );
            assert!(
                body.top() >= frame.top() && body.bottom() <= frame.bottom(),
                "{w}x{h} {kind:?}: body {body:?} in {frame:?}"
            );
            first = first.min(frame.top());
        }
        let gap = first - bar.bottom();
        assert!(
            gap >= px(0.) && gap <= px(8.),
            "{w}x{h}: first box {gap:?} below the status bar"
        );
    }
}
