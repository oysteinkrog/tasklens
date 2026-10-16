//! Network page: the Throughput chart, its scale and value, the disconnected gap and the demand
//! (E25, UI-PRF-031; PERF-054, PERF-055).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, DeviceCommand};
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{PublicIpState, SystemSnapshot};
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::{ChartState, ChartView, ValueFormat, YScale, format_value};
use tl_ui::components::kv_grid;
use tl_ui::components::menu::{context_menu_layer, open_context_menu};
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(1000.))
            .h(px(2000.))
            .child(self.page.clone())
            .children(context_menu_layer(window, cx))
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
    engine: Arc<FakeEngine>,
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
        tl_ui::actions::register_global_handlers(cx);
        tl_ui::components::menu::bind_keys(cx);
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
        fan_connected: None,
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
    Rig {
        engine,
        page,
        demand,
        vcx,
    }
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

/// A 1 Gbps link at full speed, 1.25e8 B/s, reaches the top of the link speed scale.
#[gpui_kit::test]
fn net_full_link_speed_reaches_top(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.performance.network_dynamic_scale = false;
    let mut r = open(cx, settings, system(1000));
    let chart = feed(&mut r, 1.25e8, 0., 0);
    let (y, top) = r
        .vcx
        .read_entity(&chart, |v, _| (v.spec.y, v.stroke_top_y()));
    assert_eq!(y, YScale::LinkSpeed(1e9));
    let top = top.expect("strokes");
    assert!(top.abs() <= 1.0, "the line sits at {top}, not the plot top");
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

    // Without an upload series the value is not the download alone.
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
    assert_eq!(validity, Validity::Missing, "shows -");
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

/// The Details grid's P1 rows and the Display unit row (E25-008, UI-PRF-031; PERF-056,
/// PERF-057).
mod details {
    use super::*;
    use tl_ui::pages::performance::details::KeyValueModel;
    use tl_ui::pages::performance::net::{
        NetDetailInputs, net_details_model, update_net_details_p1,
    };

    const GIB: u64 = 1 << 30;

    /// The interface record of [`system`] with 1.2 Mbps down, 0.3 Mbps up and the totals.
    fn record() -> NetTotals {
        let mut t = system(1000).nets[0].1;
        t.rx_bps = Metric::valid(f64::from(1.2 * MBPS));
        t.tx_bps = Metric::valid(f64::from(0.3 * MBPS));
        // 12.40 GiB
        t.rx_total = 13_314_398_618;
        t.tx_total = 2 * GIB;
        t
    }

    /// A model filled from `t` with `unit` and `auto_scale`, with translated labels and texts.
    fn model(
        cx: &mut TestAppContext,
        t: Option<&NetTotals>,
        unit: NetworkUnit,
        auto_scale: bool,
    ) -> KeyValueModel {
        let mut model = net_details_model(false);
        cx.update(|cx| {
            init_strings(cx, "en-US");
            model.refresh_strings(cx);
        });
        let inp = NetDetailInputs {
            now_boot_ns: 0,
            observed_end_ns: 0,
            totals: t,
            unit,
            auto_scale,
        };
        update_net_details_p1(&mut model, &inp);
        cx.update(|cx| model.refresh_labels(cx));
        model
    }

    fn rows(model: &KeyValueModel) -> Vec<(String, String)> {
        model
            .to_kv_rows()
            .map(|r| (r.label.to_string(), r.value.text.to_string()))
            .collect()
    }

    fn value(model: &KeyValueModel, label: &str) -> Option<String> {
        rows(model)
            .into_iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v)
    }

    #[gpui_kit::test]
    fn net_details_order_ethernet(cx: &mut TestAppContext) {
        let model = model(cx, Some(&record()), NetworkUnit::Bits, false);
        assert_eq!(model.rows.len(), 16, "Ethernet: no Wi-Fi rows");
        assert_eq!(net_details_model(true).rows.len(), 22);
        assert_eq!(
            rows(&model),
            [
                ("Download speed", "1.2 Mbps"),
                ("Upload speed", "0.3 Mbps"),
                ("Total downloaded", "12.40 GiB"),
                ("Total uploaded", "2.00 GiB"),
                ("Display unit", "Mbps"),
            ]
            .map(|(l, v)| (l.to_string(), v.to_string()))
        );
        let row = model
            .to_kv_rows()
            .find(|r| r.label.as_ref() == "Display unit")
            .expect("Display unit row");
        assert_eq!(
            row.value.tooltip.as_deref(),
            Some("Applies to all network values")
        );
    }

    #[gpui_kit::test]
    fn display_unit_values(cx: &mut TestAppContext) {
        let t = record();
        let cases = [
            (NetworkUnit::Bits, false, "Mbps"),
            (NetworkUnit::Bytes, false, "MiB/s"),
            (NetworkUnit::Bits, true, "bits/s, auto"),
            (NetworkUnit::Bytes, true, "bytes/s, auto"),
        ];
        for (unit, auto, text) in cases {
            let model = model(cx, Some(&t), unit, auto);
            assert_eq!(
                value(&model, "Display unit").as_deref(),
                Some(text),
                "{unit:?} auto {auto}"
            );
        }
    }

    #[gpui_kit::test]
    fn net_totals_iec(cx: &mut TestAppContext) {
        let model = model(cx, Some(&record()), NetworkUnit::Bits, false);
        assert_eq!(
            value(&model, "Total downloaded").as_deref(),
            Some("12.40 GiB")
        );
    }

    /// No record yet: the rates and totals are collecting; the Display unit still reads.
    #[gpui_kit::test]
    fn net_rates_collecting_before_first_record(cx: &mut TestAppContext) {
        let empty = model(cx, None, NetworkUnit::Bytes, false);
        let mut t = record();
        t.rx_bps = Metric::with(Validity::WarmingUp);
        let warming = model(cx, Some(&t), NetworkUnit::Bytes, false);
        assert_eq!(
            value(&warming, "Download speed").as_deref(),
            Some("Collecting…")
        );
        assert_eq!(
            rows(&empty),
            [
                ("Download speed", "Collecting…"),
                ("Upload speed", "Collecting…"),
                ("Total downloaded", "Collecting…"),
                ("Total uploaded", "Collecting…"),
                ("Display unit", "MiB/s"),
            ]
            .map(|(l, v)| (l.to_string(), v.to_string()))
        );
    }

    /// A click on the Display unit value (visible row 14 with the P2 rows shown) opens the unit menu under it; choosing Bytes per
    /// second sets `units.network` and the row re-reads `MiB/s`.
    #[gpui_kit::test]
    fn display_unit_menu_sets_global(cx: &mut TestAppContext) {
        let mut sys = system(1000);
        Arc::make_mut(&mut sys.nets)[0].1 = record();
        let r = open(cx, with_unit(NetworkUnit::Bits), sys);
        let link = r
            .vcx
            .debug_bounds("TlKv-value-14")
            .expect("Display unit value drawn");
        r.vcx.simulate_click(link.center(), Modifiers::none());
        draw(r.vcx);
        let menu = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("the click opened the menu");
        let entries = r
            .vcx
            .update(|window, cx| menu.read(cx).entries_for_test(window));
        let items: Vec<(String, bool)> = entries
            .iter()
            .map(|e| (e.label.as_deref().unwrap_or("").to_owned(), e.checked))
            .collect();
        assert_eq!(
            items,
            [
                ("Bits per second".to_owned(), true),
                ("Bytes per second".to_owned(), false)
            ]
        );

        // The first "down" highlights the first item.
        r.vcx.simulate_keystrokes("down down enter");
        draw(r.vcx);
        let unit = r.vcx.update(|_, cx| TlSettings::settings(cx).units.network);
        assert_eq!(unit, NetworkUnit::Bytes);
        let page = r.page.clone();
        let shown = r
            .vcx
            .update(|_, cx| value(page.read(cx).details(), "Display unit"));
        assert_eq!(shown.as_deref(), Some("MiB/s"));
    }

    /// The Display unit link is the page's focus order and its first Tab stop (no header
    /// toggles); Enter on it opens the unit menu.
    #[gpui_kit::test]
    fn display_unit_in_focus_order(cx: &mut TestAppContext) {
        let mut sys = system(1000);
        Arc::make_mut(&mut sys.nets)[0].1 = record();
        let r = open(cx, with_unit(NetworkUnit::Bits), sys);
        let page = r.page.clone();
        let order = r.vcx.update(|_, cx| page.read(cx).focus_order());
        assert_eq!(order.len(), 2, "the Display unit link and the Fetch button");
        r.vcx.update(|window, cx| window.focus_next(cx));
        let first = r.vcx.update(|window, _| order[0].is_focused(window));
        assert!(first, "the first Tab stop is the Display unit link");

        // GPUI turns Enter on the focused link into a keyboard click on key up.
        draw(r.vcx);
        let enter = gpui_kit::Keystroke::parse("enter").expect("keystroke");
        r.vcx.simulate_event(gpui_kit::KeyDownEvent {
            keystroke: enter.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        r.vcx
            .simulate_event(gpui_kit::KeyUpEvent { keystroke: enter });
        draw(r.vcx);
        let open = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx).is_some());
        assert!(open, "Enter opens the unit menu");
    }

    /// The page re-reads the rates and the Display unit when `units.network` changes.
    #[gpui_kit::test]
    fn net_details_follow_unit_setting(cx: &mut TestAppContext) {
        let mut sys = system(1000);
        Arc::make_mut(&mut sys.nets)[0].1 = record();
        let r = open(cx, with_unit(NetworkUnit::Bits), sys);
        let page = r.page.clone();
        let read = |vcx: &mut VisualTestContext| {
            // The P1 rows; the P2 rows between them and Display unit are not under test here.
            vcx.update(|_, cx| rows(page.read(cx).details()))
                .into_iter()
                .filter(|(l, _)| {
                    [
                        "Download speed",
                        "Upload speed",
                        "Total downloaded",
                        "Total uploaded",
                        "Display unit",
                    ]
                    .contains(&l.as_str())
                })
                .map(|(_, v)| v)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            read(r.vcx),
            ["1.2 Mbps", "0.3 Mbps", "12.40 GiB", "2.00 GiB", "Mbps"]
        );
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.units.network = NetworkUnit::Bytes);
        });
        draw(r.vcx);
        assert_eq!(
            read(r.vcx),
            ["0.1 MiB/s", "0 MiB/s", "12.40 GiB", "2.00 GiB", "MiB/s"]
        );
    }
}

/// The Public IP row (E25-022, UI-PRF-031; PERF-060, APP-056).
mod public_ip {
    use super::*;
    use tl_ui::pages::performance::net::{local_hh_mm, public_ip_text};

    const FETCH_BUTTON: &str = "TlButton-perf-net-public-ip-fetch";
    /// Flat index of the Public IP row: Ethernet, all rows shown.
    const ROW: usize = 15;

    fn accepted(accepted: bool) -> Settings {
        let mut s = Settings::default();
        s.network.public_ip_notice_accepted = accepted;
        s
    }

    /// Local 14:02 on a fixed day, in seconds since the epoch, so the text is the same in every
    /// time zone.
    fn at_1402() -> i64 {
        jiff::civil::date(2026, 10, 16)
            .at(14, 2, 0, 0)
            .to_zoned(jiff::tz::TimeZone::system())
            .expect("local time")
            .timestamp()
            .as_second()
    }

    fn publish(r: &mut Rig<'_>, state: PublicIpState) {
        r.engine.publish_with(|s| {
            let inv = Arc::make_mut(&mut s.inventory);
            inv.header.version += 1;
            inv.public_ip = state;
        });
        draw(r.vcx);
    }

    fn fetched(v6: Option<&str>) -> PublicIpState {
        PublicIpState::Fetched {
            v4: Some(Arc::from("203.0.113.4")),
            v6: v6.map(Arc::from),
            fetched_at_s: at_1402(),
        }
    }

    /// The shown value and tooltip of the Public IP row.
    fn row(r: &mut Rig<'_>) -> (String, Option<String>) {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let found = page
                .read(cx)
                .details()
                .to_kv_rows()
                .find(|row| row.label.as_ref() == "Public IP")
                .expect("Public IP row");
            (
                found.value.text.to_string(),
                found.value.tooltip.map(|t| t.to_string()),
            )
        })
    }

    fn fetches(r: &Rig<'_>) -> usize {
        r.engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::Device(DeviceCommand::FetchPublicIp)))
            .count()
    }

    fn click_fetch(r: &mut Rig<'_>) {
        let button = r
            .vcx
            .debug_bounds(FETCH_BUTTON)
            .expect("Fetch button drawn");
        r.vcx.simulate_click(button.center(), Modifiers::none());
        draw(r.vcx);
    }

    #[gpui_kit::test]
    fn public_ip_states(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(true), system(1000));
        assert_eq!(row(&mut r).0, "Not fetched");
        let value = r
            .vcx
            .debug_bounds(&format!("TlKv-value-{ROW}"))
            .expect("value drawn");
        let button = r.vcx.debug_bounds(FETCH_BUTTON).expect("Fetch button");
        assert!(button.left() > value.left(), "the button follows the text");
        assert_eq!(button.size.height, px(24.));

        publish(&mut r, PublicIpState::Fetching);
        assert_eq!(row(&mut r).0, "Fetching…");
        click_fetch(&mut r);
        assert_eq!(fetches(&r), 0, "a loading button ignores clicks");

        publish(&mut r, fetched(None));
        assert_eq!(row(&mut r).0, "203.0.113.4 (fetched 14:02)");
        assert!(
            r.vcx.debug_bounds(FETCH_BUTTON).is_none(),
            "no button after a result"
        );
    }

    #[gpui_kit::test]
    fn public_ip_first_fetch_opens_notice(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(false), system(1000));
        let before = r.vcx.update(|_, cx| cx.windows().len());
        click_fetch(&mut r);
        assert_eq!(fetches(&r), 0, "no command before Continue");
        let after = r.vcx.update(|_, cx| cx.windows().len());
        assert_eq!(after, before + 1, "the notice window opened");
    }

    #[gpui_kit::test]
    fn public_ip_accepted_sends_command(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(true), system(1000));
        click_fetch(&mut r);
        assert_eq!(fetches(&r), 1, "the command went out at once");
        publish(&mut r, PublicIpState::Fetching);
        assert_eq!(row(&mut r).0, "Fetching…");
        assert!(
            r.vcx.debug_bounds(FETCH_BUTTON).is_some(),
            "the button shows its spinner"
        );
    }

    #[gpui_kit::test]
    fn public_ip_two_lines_with_v6(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(true), system(1000));
        publish(&mut r, fetched(Some("2001:db8::4")));
        assert_eq!(
            row(&mut r).0,
            "203.0.113.4 (fetched 14:02)\n2001:db8::4 (fetched 14:02)"
        );
        let pair = r
            .vcx
            .debug_bounds(&format!("TlKv-row-{ROW}"))
            .expect("pair drawn");
        let plain = r.vcx.debug_bounds("TlKv-row-0").expect("plain pair");
        assert!(
            pair.size.height > plain.size.height,
            "the pair is taller for two lines"
        );
    }

    #[gpui_kit::test]
    fn public_ip_failure_tooltip(cx: &mut TestAppContext) {
        let mut r = open(cx, accepted(true), system(1000));
        publish(&mut r, PublicIpState::Failed(Arc::from("timed out")));
        let (text, tip) = row(&mut r);
        assert_eq!(text, "Unavailable");
        assert_eq!(
            tip.as_deref(),
            Some("Could not reach api.ipify.org: timed out")
        );
    }

    #[test]
    fn public_ip_formatter_is_shared() {
        let at = at_1402();
        assert_eq!(local_hh_mm(at), "14:02");
        let template = tl_ui::pages::performance::details::Template::en(
            "perf-net-ip-fetched",
            &["ip", "time"],
        );
        assert_eq!(
            public_ip_text(&template, Some("203.0.113.4"), None, at),
            "203.0.113.4 (fetched 14:02)"
        );
    }
}
