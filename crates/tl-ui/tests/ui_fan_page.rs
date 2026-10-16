//! Fan page: sensor pairing, the two charts, the Details rows and demand (E25-027, UI-PRF-034;
//! PERF-076, PERF-077).

#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{
    AppearanceSettings, FanOverride, NetworkUnit, Settings, ThemeSettings, UiState,
};
use tl_model::system::{FanControlMode, Metric, SensorKind, SensorReading};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::scale::{ScaleState, compute};
use tl_ui::chart::{LegendStyle, ValueFormat, YScale};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::sensors::FanPage;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const FAN1: DeviceIdx = DeviceIdx(10);
const TEMP1: DeviceIdx = DeviceIdx(20);
const FAN_KEY: &str = "fan:nct6797::1";
const NS: u64 = 1_000_000_000;

struct Host(Entity<FanPage>, #[allow(dead_code)] Entity<AnimationDriver>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1000.)).h(px(2000.)).child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

fn entry(idx: u16, identity: DeviceIdentity, name: &str, key: String) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
    }
}

fn fan_reading(device: DeviceIdx, rpm: f32, pwm: f32, max: Option<u32>) -> SensorReading {
    SensorReading {
        device,
        kind: SensorKind::Fan,
        value: Metric::valid(rpm),
        pwm_pct: Metric::valid(pwm),
        fan_control: Some(FanControlMode::Automatic),
        fan_connected: true,
        fan_max_rpm: max,
    }
}

fn temp_reading(device: DeviceIdx, c: f32) -> SensorReading {
    SensorReading {
        device,
        kind: SensorKind::Temperature,
        value: Metric::valid(c),
        pwm_pct: Metric::with(Validity::Unsupported),
        fan_control: None,
        fan_connected: false,
        fan_max_rpm: None,
    }
}

struct Rig<'a> {
    page: Entity<FanPage>,
    demand: Entity<DemandAggregator>,
    engine: Arc<FakeEngine>,
    vcx: &'a mut VisualTestContext,
}

/// A page for fan 1 of a chip with `fans` fans and temperatures `temp1` to `temp<fans>`.
/// `saved` is the `[fans]` temp choice of fan 1.
fn open<'a>(
    cx: &'a mut TestAppContext,
    fans: u16,
    saved: Option<&str>,
    fan_max: Option<u32>,
) -> Rig<'a> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        let mut entries = Vec::new();
        let mut sensors = Vec::new();
        for n in 1..=fans {
            entries.push(entry(
                9 + n,
                DeviceIdentity::Fan {
                    chip: Arc::from("nct6797"),
                    index: n,
                },
                "fan",
                format!("fan:nct6797::{n}"),
            ));
            sensors.push((
                DeviceIdx(9 + n),
                fan_reading(DeviceIdx(9 + n), 1200.0, 44.7, fan_max),
            ));
        }
        for n in 1..=fans {
            entries.push(entry(
                19 + n,
                DeviceIdentity::Sensor {
                    chip: Arc::from("nct6797"),
                    label: Arc::from(format!("temp{n}")),
                },
                "temp",
                format!("sensor:nct6797::temp{n}"),
            ));
            sensors.push((
                DeviceIdx(19 + n),
                temp_reading(DeviceIdx(19 + n), 45.0 + n as f32),
            ));
        }
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.sensors = Arc::from(sensors);
    });
    let mut settings = Settings::default();
    if let Some(temp) = saved {
        settings.fans.insert(
            FAN_KEY.to_owned(),
            FanOverride {
                label: None,
                temp: Some(temp.to_owned()),
            },
        );
    }
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
    let key = DeviceKey(Arc::from(FAN_KEY));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Fan,
        idx: FAN1,
        generation: 0,
        net_class: None,
        fan_connected: Some(true),
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| FanPage::new(key, row, h, d, cx));
    let host = page.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        Host(host, driver)
    });
    vcx.simulate_scale_factor_change(1.0);
    draw(vcx);
    Rig {
        page,
        demand,
        engine,
        vcx,
    }
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

fn publish_sensors(r: &mut Rig, sensors: Vec<(DeviceIdx, SensorReading)>) {
    r.engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.sensors = Arc::from(sensors);
    });
    draw(r.vcx);
}

fn detail_rows(r: &mut Rig) -> Vec<(String, String)> {
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let p = page.read(cx);
        p.details().refresh_labels(cx);
        p.details()
            .to_kv_rows()
            .map(|row| (row.label.to_string(), row.value.text.to_string()))
            .collect()
    })
}

fn row(rows: &[(String, String)], label: &str) -> String {
    rows.iter()
        .find(|(l, _)| l == label)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| panic!("no row {label}: {rows:?}"))
}

#[gpui_kit::test]
fn fan_choose_a_sensor_state(cx: &mut TestAppContext) {
    let r = open(cx, 7, None, None);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.sensor(), None);
        assert_eq!(p.temperature_value_text(), Some("-"));
        assert_eq!(
            p.choose_sensor_text().map(|t| t.as_ref()),
            Some("Choose a sensor")
        );
        let t = p.temperature().read(cx);
        assert!(t.spec.series.is_empty(), "the plot is empty");
        assert_eq!(t.labels.heading.as_deref(), Some("Temperature"));
        assert_eq!(t.labels.value_fmt, ValueFormat::Celsius);
        assert_eq!(t.spec.y, YScale::StickyTemp);
    });
}

#[gpui_kit::test]
fn fan_default_pairing_single_fan_chip(cx: &mut TestAppContext) {
    let r = open(cx, 1, None, None);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.sensor(), Some("nct6797::temp1"));
        assert_eq!(p.temperature_value_text(), None);
        assert!(p.choose_sensor_text().is_none());
        let t = p.temperature().read(cx);
        assert_eq!(t.spec.series.len(), 1);
        assert_eq!(
            t.spec.series[0].id,
            SeriesId {
                device: TEMP1,
                metric: MetricId::SENSOR_TEMP
            }
        );
    });
}

#[gpui_kit::test]
fn fan_saved_sensor_wins(cx: &mut TestAppContext) {
    let r = open(cx, 3, Some("nct6797::temp3"), None);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.sensor(), Some("nct6797::temp3"));
        let t = p.temperature().read(cx);
        assert_eq!(t.spec.series[0].id.device, DeviceIdx(22));
    });
}

#[gpui_kit::test]
fn fan_axis_max_from_fan_max(cx: &mut TestAppContext) {
    let r = open(cx, 1, None, Some(1800));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).speed().clone());
    let (y, labels, series) = r.vcx.read_entity(&chart, |v, _| {
        (v.spec.y, v.labels.clone(), v.spec.series.clone())
    });
    assert_eq!(
        y,
        YScale::StickyUp {
            hw_max: Some(1800.0)
        }
    );
    assert_eq!(labels.heading.as_deref(), Some("Fan speed"));
    assert_eq!(labels.value_fmt, ValueFormat::Rpm);
    assert!(labels.show_axis_max);
    let legend: Vec<(&str, LegendStyle)> = labels
        .legend
        .iter()
        .map(|(t, s)| (t.as_ref(), *s))
        .collect();
    assert_eq!(
        legend,
        [("RPM", LegendStyle::Solid), ("PWM", LegendStyle::Dashed)]
    );
    assert_eq!(series.len(), 2);
    assert!(series[0].fill.is_some());
    assert!(series[1].fill.is_none() && series[1].dashed);
    assert_eq!(series[0].line, series[1].line, "both Violet");

    let mut axis = String::new();
    compute(
        &y,
        0.0,
        1200.0,
        now_boot_ns(),
        &mut ScaleState::default(),
        labels.value_fmt,
        NetworkUnit::Bits,
        &mut axis,
    );
    assert_eq!(axis, "1800 RPM");
}

#[gpui_kit::test]
fn fan_pwm_series_on_rpm_axis(cx: &mut TestAppContext) {
    let r = open(cx, 1, None, Some(1800));
    r.engine.publish_with(|s| {
        let view = |metric, value: f32| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: FAN1,
                    metric,
                },
                version: 1,
                period_ns: NS,
                samples: (0..5)
                    .map(|i| Sample::valid(now_boot_ns() - (4 - i) * NS, value))
                    .collect(),
            })
        };
        let set = Arc::make_mut(&mut s.series);
        set.header.version += 1;
        set.series = Arc::from(vec![
            view(MetricId::FAN_RPM, 1200.0),
            view(MetricId::FAN_PWM, 50.0),
        ]);
    });
    draw(r.vcx);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        let derived = p.pwm_series().expect("derived series");
        assert_eq!(derived.samples.last().map(|s| s.value), Some(900.0));
        let chart = p.speed().read(cx);
        let slot = chart.resolved()[1].as_ref().expect("second slot");
        assert!(
            Arc::ptr_eq(slot, derived),
            "the chart draws the derived series"
        );
        assert_eq!(
            chart.resolved()[0]
                .as_ref()
                .and_then(|v| v.samples.last())
                .map(|s| s.value),
            Some(1200.0)
        );
    });
}

#[gpui_kit::test]
fn fan_control_mode_words(cx: &mut TestAppContext) {
    let mut r = open(cx, 1, None, None);
    for (mode, word) in [
        (FanControlMode::Automatic, "Automatic"),
        (FanControlMode::Manual, "Manual"),
        (FanControlMode::FullSpeed, "Full speed"),
    ] {
        let mut fan = fan_reading(FAN1, 1200.0, 44.7, None);
        fan.fan_control = Some(mode);
        publish_sensors(
            &mut r,
            vec![(FAN1, fan), (TEMP1, temp_reading(TEMP1, 46.0))],
        );
        let rows = detail_rows(&mut r);
        assert_eq!(row(&rows, "Control mode"), word);
    }
}

#[gpui_kit::test]
fn fan_details_rows(cx: &mut TestAppContext) {
    let mut r = open(cx, 1, None, None);
    let rows = detail_rows(&mut r);
    let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        labels,
        ["Fan speed", "PWM", "Control mode", "Temperature", "Chip"]
    );
    assert_eq!(row(&rows, "Fan speed"), "1200 RPM");
    assert_eq!(row(&rows, "PWM"), "45%");
    assert_eq!(row(&rows, "Temperature"), "46 °C");
    assert_eq!(row(&rows, "Chip"), "nct6797");

    // A stopped fan is a measured zero, not a glyph.
    publish_sensors(
        &mut r,
        vec![
            (FAN1, fan_reading(FAN1, 0.0, 0.0, None)),
            (TEMP1, temp_reading(TEMP1, 46.0)),
        ],
    );
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Fan speed"), "0 RPM");
    assert_eq!(row(&rows, "PWM"), "0%");

    let (header, chip) = r.vcx.update(|_, cx| r.page.read(cx).header());
    assert_eq!((header.as_ref(), chip.as_ref()), ("Fan 0", "nct6797"));
}

#[gpui_kit::test]
fn fan_temperature_row_without_sensor(cx: &mut TestAppContext) {
    let mut r = open(cx, 2, None, None);
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Temperature"), "Not reported");
}

#[gpui_kit::test]
fn fan_demand_only_when_active(cx: &mut TestAppContext) {
    let mut r = open(cx, 1, None, None);
    let demand = r.demand.clone();
    let merged = |r: &mut Rig| -> Demand { r.vcx.update(|_, cx| demand.read(cx).merged()) };
    assert!(!merged(&mut r).sensors, "inactive page");
    let page = r.page.clone();
    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    assert!(merged(&mut r).sensors, "active page");
    let page = r.page.clone();
    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(false, cx)));
    assert!(!merged(&mut r).sensors, "inactive again");
}
