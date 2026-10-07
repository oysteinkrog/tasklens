//! Network page: the Throughput chart, its scale and value, the disconnected gap and the demand
//! (E25, UI-PRF-031; PERF-054, PERF-055).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings, UiState};
use tl_model::snapshot::SystemSnapshot;
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::{ChartState, ChartView, ValueFormat, YScale, format_value};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::net::{NetPage, THROUGHPUT_ID};
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

/// The interface in the fake catalog.
const NET: DeviceIdx = DeviceIdx(1);
const PERIOD: u64 = 1_000_000_000;
/// Bytes per second in one Mbps.
const MBPS: f32 = 125_000.;
const MIB: f32 = 1_048_576.;

/// The page in a window with an animation driver: a live chart asks it for frames.
struct Host {
    page: Entity<NetPage>,
    _driver: Entity<AnimationDriver>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1000.)).h(px(2000.)).child(self.page.clone())
    }
}

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("enp38s0"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
    }
}

/// One interface up at `speed_mbps` (sysfs -1 gives Unsupported).
fn system(speed_mbps: i64) -> SystemSnapshot {
    let mut sys = SystemSnapshot::default();
    sys.header.validity = Validity::Valid;
    sys.header.version = 1;
    let link_speed_mbps = match u32::try_from(speed_mbps) {
        Ok(v) => Metric::valid(v),
        Err(_) => Metric::with(Validity::Unsupported),
    };
    sys.nets = Arc::from(
        [(
            NET,
            NetTotals {
                device: NET,
                rx_bps: Metric::valid(0.),
                tx_bps: Metric::valid(0.),
                rx_total: 0,
                tx_total: 0,
                link_speed_mbps,
                state: LinkState::Up,
            },
        )]
        .as_slice(),
    );
    sys
}

struct Rig<'a> {
    page: Entity<NetPage>,
    demand: Entity<DemandAggregator>,
    vcx: &'a mut VisualTestContext,
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// A Network page for `enp38s0` over `sys` in a 1000 px wide window, with `settings`.
fn open(cx: &mut TestAppContext, settings: Settings, sys: SystemSnapshot) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(
                    1,
                    DeviceIdentity::Net {
                        netns_ino: 1,
                        ifindex: 2,
                        name: Arc::from("enp38s0"),
                    },
                ),
            ]),
        });
        s.system = Arc::new(sys);
    });
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let key = DeviceKey(Arc::from("net:enp38s0"));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Net,
        idx: NET,
        generation: 0,
        net_class: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| NetPage::new(key, row, h, d, cx));
    let host = page.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        Host {
            page: host,
            _driver: driver,
        }
    });
    vcx.simulate_scale_factor_change(1.0);
    draw(vcx);
    Rig { page, demand, vcx }
}

/// 60 one-second samples of `metric` ending now: `value` while valid, then `gaps` invalid ones.
fn series(metric: MetricId, value: f32, gaps: u64) -> Arc<SeriesView> {
    let now = now_boot_ns();
    Arc::new(SeriesView {
        id: SeriesId {
            device: NET,
            metric,
        },
        // A new shape gets a new version, so the chart rebuilds its paths.
        version: 1 + gaps,
        period_ns: PERIOD,
        samples: (0..60u64)
            .map(|k| {
                let t = now - (59 - k) * PERIOD;
                if k >= 60 - gaps {
                    Sample::gap(t)
                } else {
                    Sample::valid(t, value)
                }
            })
            .collect::<Vec<_>>()
            .into(),
    })
}

/// Hands the chart download `rx` and upload `tx` in bytes per second, the last `gaps` samples
/// invalid.
fn feed(r: &mut Rig<'_>, rx: f32, tx: f32, gaps: u64) -> Entity<ChartView> {
    let chart = r.vcx.update(|_, cx| r.page.read(cx).throughput().clone());
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [
                Some(series(MetricId::NET_RX, rx, gaps)),
                Some(series(MetricId::NET_TX, tx, gaps)),
            ]
            .into(),
            cx,
        );
        v.set_state(ChartState::Live, cx);
    });
    draw(r.vcx);
    chart
}

/// The chart's value text in `unit`.
fn value_text(r: &mut Rig<'_>, chart: &Entity<ChartView>, unit: NetworkUnit) -> String {
    let (fmt, value) = r
        .vcx
        .read_entity(chart, |v, _| (v.labels.value_fmt, v.value_now()));
    let (value, validity) = value.expect("newest value");
    assert_eq!(validity, Validity::Valid);
    let mut text = String::new();
    format_value(fmt, value, unit, &mut text);
    text
}

fn with_unit(unit: NetworkUnit) -> Settings {
    let mut s = Settings::default();
    s.units.network = unit;
    s
}

#[gpui_kit::test]
fn net_chart_anatomy(cx: &mut TestAppContext) {
    let r = open(cx, Settings::default(), system(1000));
    let bounds = r.vcx.debug_bounds(THROUGHPUT_ID).expect("chart box");
    assert_eq!(bounds.size.height, px(240.));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).throughput().clone());
    let (ids, labels) = r.vcx.read_entity(&chart, |v, _| {
        (
            v.series_ids().map(|s| s.metric).collect::<Vec<_>>(),
            v.labels.clone(),
        )
    });
    assert_eq!(ids, [MetricId::NET_RX, MetricId::NET_TX]);
    assert_eq!(labels.heading.as_deref(), Some("Throughput"));
    assert_eq!(labels.value_fmt, ValueFormat::RateNetwork);
    assert!(labels.show_axis_max);
    let legend: Vec<&str> = labels.legend.iter().map(|(t, _)| t.as_ref()).collect();
    assert_eq!(legend, ["Download", "Upload"]);

    let page = r.page.clone();
    let header = r.vcx.update(|_, cx| page.read(cx).header());
    assert_eq!(header.0.as_ref(), "enp38s0");
    assert_eq!(header.1.as_ref(), "Unknown adapter", "no adapter name");
}

#[gpui_kit::test]
fn net_link_speed_scale(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.performance.network_dynamic_scale = false;
    let r = open(cx, settings, system(1000));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).throughput().clone());
    let y = r.vcx.read_entity(&chart, |v, _| v.spec.y);
    assert_eq!(y, YScale::LinkSpeed(1_000_000_000.));

    // The dynamic scale ignores the link speed.
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.network_dynamic_scale = true);
    });
    draw(r.vcx);
    let y = r.vcx.read_entity(&chart, |v, _| v.spec.y);
    assert_eq!(y, YScale::AutoRate { iec: false });
}

#[gpui_kit::test]
fn net_bytes_unit_switches_scale_and_value(cx: &mut TestAppContext) {
    let mut r = open(cx, with_unit(NetworkUnit::Bytes), system(-1));
    let chart = feed(&mut r, 1.2 * MIB, 0.3 * MIB, 0);
    let y = r.vcx.read_entity(&chart, |v, _| v.spec.y);
    assert_eq!(y, YScale::AutoRate { iec: true });
    assert_eq!(value_text(&mut r, &chart, NetworkUnit::Bytes), "1.5 MiB/s");

    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.units.network = NetworkUnit::Bits);
    });
    draw(r.vcx);
    let page = r.page.clone();
    let unit = r.vcx.update(|_, cx| page.read(cx).unit());
    assert_eq!(unit, NetworkUnit::Bits);
    let y = r.vcx.read_entity(&chart, |v, _| v.spec.y);
    assert_eq!(y, YScale::AutoRate { iec: false });
}

#[gpui_kit::test]
fn net_value_is_sum(cx: &mut TestAppContext) {
    let mut r = open(cx, with_unit(NetworkUnit::Bits), system(1000));
    let chart = feed(&mut r, 1.2 * MBPS, 0.3 * MBPS, 0);
    assert_eq!(value_text(&mut r, &chart, NetworkUnit::Bits), "1.5 Mbps");

    // Without upload samples the value is not the download alone.
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [Some(series(MetricId::NET_RX, 1.2 * MBPS, 0)), None].into(),
            cx,
        );
    });
    draw(r.vcx);
    let (_, validity) = r
        .vcx
        .read_entity(&chart, |v, _| v.value_now())
        .expect("value");
    assert_eq!(validity, Validity::WarmingUp);
}

#[gpui_kit::test]
fn net_disconnected_shows_gap(cx: &mut TestAppContext) {
    let mut r = open(cx, Settings::default(), system(1000));
    let chart = feed(&mut r, 2. * MBPS, MBPS, 0);
    let connected = r
        .vcx
        .read_entity(&chart, |v, _| v.stroke_right_x())
        .expect("strokes");
    let width = f32::from(
        r.vcx
            .debug_bounds(THROUGHPUT_ID)
            .expect("chart box")
            .size
            .width,
    );
    // The interface went down 5 s ago: the engine marked the last 5 samples invalid.
    let chart = feed(&mut r, 2. * MBPS, MBPS, 5);
    r.vcx.read_entity(&chart, |v, _| {
        assert_eq!(v.derived_state(now_boot_ns()), ChartState::Live);
        assert_eq!(v.cached_fill_stroke_counts(), (1, 2));
        // The lines stop at the last valid sample, 5 of 60 s before the right edge: the end
        // is a gap, with no line at zero and no dotted baseline. The plot is a little narrower
        // than the chart box.
        let right = v.stroke_right_x().expect("strokes");
        let lost = connected - right;
        assert!(
            lost >= width * 4. / 60. && lost <= width * 6. / 60.,
            "lines end at {right}, {connected} when connected, box {width} px"
        );
        assert!(v.last_gap_spans.borrow().is_empty());
    });
    let page = r.page.clone();
    let header = r.vcx.update(|_, cx| page.read(cx).header());
    assert_eq!(header.0.as_ref(), "enp38s0", "the header is unchanged");
}

#[gpui_kit::test]
fn net_demand_only_when_active(cx: &mut TestAppContext) {
    let r = open(cx, Settings::default(), system(1000));
    let (page, demand) = (r.page.clone(), r.demand.clone());
    let merged = |vcx: &mut VisualTestContext| vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(merged(r.vcx), Demand::default(), "never active");

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    let d = merged(r.vcx);
    assert!(d.any_surface_visible);
    assert!(d.nets);
    assert!(!d.disks && !d.memory && !d.gpus && !d.cpu_fast);

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(false, cx)));
    assert_eq!(merged(r.vcx), Demand::default());
}
