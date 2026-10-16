//! Battery page: chart kind by device type, Details rows, the time label, the threshold rows,
//! the signed power value, copy text and demand (E25-028, UI-PRF-035; PERF-078).

#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId};
use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{BatteryDetails, DeviceDetailsSnapshot, InvField};
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::labels::format_value;
use tl_ui::chart::{ValueFormat, YScale};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::sensors::BatteryPage;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const BAT: DeviceIdx = DeviceIdx(30);
const BAT_KEY: &str = "bat:BAT0";

struct Host(
    Entity<BatteryPage>,
    #[allow(dead_code)] Entity<AnimationDriver>,
);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1000.)).h(px(2000.)).child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

fn valid<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn text(s: &str) -> InvField<Arc<str>> {
    valid(Arc::from(s))
}

fn record(kind: &str, power_supply: bool, state: &str) -> BatteryDetails {
    BatteryDetails {
        kind: Arc::from(kind),
        vendor: text("Logitech"),
        model: text("MX Master 3"),
        serial: text("SN-1"),
        native_path: Arc::from("BAT0"),
        percentage: valid(85.0),
        energy_wh: valid(42.1),
        energy_rate_w: valid(12.4),
        voltage_v: valid(11.8),
        state: Arc::from(state),
        time_to_full_s: valid(8040),
        time_to_empty_s: valid(3600),
        charge_cycles: valid(312),
        technology: Arc::from("Lithium ion"),
        capacity_pct: valid(92.0),
        energy_empty_wh: valid(0.0),
        energy_full_wh: valid(49.0),
        energy_full_design_wh: valid(53.2),
        voltage_min_design_v: valid(9.0),
        voltage_max_design_v: valid(12.6),
        power_supply,
        charge_threshold: Some((true, 75, 80)),
        temperature_c: valid(31.0),
        history: None,
    }
}

struct Rig<'a> {
    page: Entity<BatteryPage>,
    demand: Entity<DemandAggregator>,
    engine: Arc<FakeEngine>,
    vcx: &'a mut VisualTestContext,
}

fn publish(engine: &FakeEngine, b: BatteryDetails) {
    engine.publish_with(|s| {
        let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
        d.header.version += 1;
        d.batteries = Arc::from(vec![(BAT, b)]);
        s.device_details = Arc::new(d);
    });
}

fn open<'a>(cx: &'a mut TestAppContext, b: BatteryDetails) -> Rig<'a> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![DeviceEntry {
                idx: BAT,
                identity: DeviceIdentity::Battery {
                    native_path: Arc::from("BAT0"),
                },
                generation: 0,
                display_name: Arc::from("BAT0"),
                present: true,
                parent: None,
                persistent_key: Arc::from(BAT_KEY),
                net_kind: None,
            }]),
        });
    });
    publish(&engine, b);
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
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let key = DeviceKey(Arc::from(BAT_KEY));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Battery,
        idx: BAT,
        generation: 0,
        net_class: None,
        fan_connected: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| BatteryPage::new(key, row, h, d, cx));
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

fn update(r: &mut Rig, b: BatteryDetails) {
    publish(&r.engine, b);
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

fn has_row(rows: &[(String, String)], label: &str) -> bool {
    rows.iter().any(|(l, _)| l == label)
}

#[gpui_kit::test]
fn battery_graph_kind_by_type(cx: &mut TestAppContext) {
    let r = open(cx, record("Battery", true, "Discharging"));
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert!(p.is_system());
        let c = p.chart().read(cx);
        assert_eq!(c.labels.heading.as_deref(), Some("Power"));
        assert_eq!(c.labels.value_fmt, ValueFormat::Watts);
        assert_eq!(c.labels.range_word.as_ref(), "Power draw");
        assert!(c.labels.show_axis_max);
        assert_eq!(c.spec.y, YScale::Symmetric);
        assert_eq!(c.spec.series.len(), 1);
        assert_eq!(c.spec.series[0].id.metric, MetricId::BATTERY_POWER);
        assert_eq!(p.header().0.as_ref(), "Battery");
        assert_eq!(p.header().1.as_ref(), "Logitech MX Master 3");
    });

    let r = open(cx, record("Mouse", false, "Discharging"));
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert!(!p.is_system());
        let c = p.chart().read(cx);
        assert_eq!(c.labels.heading.as_deref(), Some("Percentage"));
        assert_eq!(c.labels.value_fmt, ValueFormat::Percent);
        assert_eq!(c.labels.range_word.as_ref(), "Percentage");
        assert_eq!(
            c.spec.y,
            YScale::Fixed {
                min: 0.0,
                max: 100.0
            }
        );
        assert_eq!(c.spec.series[0].id.metric, MetricId::BATTERY_PCT);
        assert_eq!(p.header().0.as_ref(), "Mouse");
    });
}

#[gpui_kit::test]
fn battery_time_label_follows_state(cx: &mut TestAppContext) {
    let mut r = open(cx, record("Battery", true, "Charging"));
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Time to full"), "2h 14m");
    assert!(!has_row(&rows, "Time to empty"));
    assert_eq!(row(&rows, "State"), "Charging");

    update(&mut r, record("Battery", true, "Discharging"));
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Time to empty"), "1h 0m");
    assert!(!has_row(&rows, "Time to full"));
    assert_eq!(row(&rows, "State"), "Discharging");
}

#[gpui_kit::test]
fn battery_time_unknown_when_zero(cx: &mut TestAppContext) {
    let mut b = record("Battery", true, "Discharging");
    b.time_to_empty_s = InvField {
        value: 0,
        validity: Validity::Unsupported,
    };
    let mut r = open(cx, b);
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Time to empty"), "Unknown");
}

#[gpui_kit::test]
fn battery_thresholds_hidden_when_unsupported(cx: &mut TestAppContext) {
    let mut r = open(cx, record("Battery", true, "Discharging"));
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Charge threshold"), "Enabled");
    assert_eq!(row(&rows, "Charge start threshold"), "75%");
    assert_eq!(row(&rows, "Charge end threshold"), "80%");

    let mut b = record("Battery", true, "Discharging");
    b.charge_threshold = None;
    update(&mut r, b);
    let rows = detail_rows(&mut r);
    for label in [
        "Charge threshold",
        "Charge start threshold",
        "Charge end threshold",
    ] {
        assert!(!has_row(&rows, label), "{label} is hidden");
    }
}

#[gpui_kit::test]
fn battery_details_rows(cx: &mut TestAppContext) {
    let mut r = open(cx, record("Mouse", false, "Fully charged"));
    let rows = detail_rows(&mut r);
    assert_eq!(row(&rows, "Percentage"), "85.0%");
    assert_eq!(row(&rows, "Energy"), "42.10 Wh");
    assert_eq!(row(&rows, "Voltage"), "11.80 V");
    assert_eq!(row(&rows, "Power"), "12.4 W");
    assert_eq!(row(&rows, "State"), "Fully charged");
    assert_eq!(row(&rows, "Charge cycles"), "312");
    assert_eq!(row(&rows, "Serial"), "SN-1");
    assert_eq!(row(&rows, "Type"), "Mouse");
    assert_eq!(row(&rows, "Technology"), "Lithium ion");
    assert_eq!(row(&rows, "Powering system"), "No");
    assert_eq!(row(&rows, "Capacity"), "92.0%");
    assert_eq!(row(&rows, "Energy full (design)"), "53.20 Wh");
    assert_eq!(row(&rows, "Voltage max (design)"), "12.60 V");
    assert_eq!(row(&rows, "Temperature"), "31 °C");
}

#[gpui_kit::test]
fn battery_symmetric_fill_below_zero(cx: &mut TestAppContext) {
    let r = open(cx, record("Battery", true, "Discharging"));
    r.vcx.update(|_, cx| {
        let c = r.page.read(cx).chart().read(cx);
        assert_eq!(c.spec.y, YScale::Symmetric);
        assert!(c.spec.series[0].fill.is_some(), "filled");
    });
    let mut out = String::new();
    format_value(ValueFormat::Watts, -12.4, NetworkUnit::Bits, &mut out);
    assert_eq!(out, "-12.4 W");
    out.clear();
    format_value(ValueFormat::Watts, 12.4, NetworkUnit::Bits, &mut out);
    assert_eq!(out, "12.4 W");
    out.clear();
    format_value(ValueFormat::Watts, -0.0, NetworkUnit::Bits, &mut out);
    assert_eq!(out, "0 W");
}

#[gpui_kit::test]
fn battery_copy_text(cx: &mut TestAppContext) {
    let r = open(cx, record("Mouse", false, "Discharging"));
    let text = r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        p.details().refresh_labels(cx);
        let mut out = String::new();
        p.copy_text(&mut out);
        out
    });
    assert!(
        text.starts_with("Mouse\nLogitech MX Master 3\n"),
        "{text:?}"
    );
    assert!(text.contains("    Percentage: 85.0%"), "{text:?}");
    assert!(text.contains("    Time to empty: 1h 0m"), "{text:?}");
    assert!(!text.contains("Time to full"), "{text:?}");
}

#[gpui_kit::test]
fn battery_demand_only_when_active(cx: &mut TestAppContext) {
    let mut r = open(cx, record("Battery", true, "Discharging"));
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
