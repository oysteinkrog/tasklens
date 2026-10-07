//! Disk page: the two charts, the header, the demand and the source failure state (E25,
//! UI-PRF-029; PERF-047, PERF-048).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{DiskDetails, SmartKind};
use tl_model::table::Total;
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::scale::{ScaleState, compute};
use tl_ui::chart::{ChartState, ValueFormat, YScale, format_value};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::disk::{ACTIVE_ID, DiskPage, TRANSFER_ID};
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const MIB: f32 = 1024. * 1024.;
/// The disk in the fake catalog.
const DISK: DeviceIdx = DeviceIdx(2);
const MODEL: &str = "WD_BLACK SN850X 1000GB";

struct Host(Entity<DiskPage>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1000.)).h(px(2000.)).child(self.0.clone())
    }
}

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("device"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
    }
}

fn nvme() -> DeviceIdentity {
    DeviceIdentity::Disk {
        major: 259,
        minor: 0,
        name: Arc::from("nvme0n1"),
        wwn_or_serial: Some(Arc::from("eui.0025388b")),
    }
}

struct Rig<'a> {
    page: Entity<DiskPage>,
    demand: Entity<DemandAggregator>,
    engine: Arc<FakeEngine>,
    /// A live chart asks the window's driver for its next frame.
    _driver: Entity<AnimationDriver>,
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

/// A Disk page for nvme0n1 (with its model in the disk details) in a 1000 px wide window.
fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
                entry(2, nvme()),
            ]),
        });
        let details = DiskDetails {
            model: Total {
                value: Arc::from(MODEL),
                validity: Validity::Valid,
            },
            smart: SmartKind::Nvme,
            ..DiskDetails::default()
        };
        Arc::make_mut(&mut s.device_details).disks = Arc::from(vec![(DISK, details)]);
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
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let key = DeviceKey(Arc::from("disk:eui.0025388b"));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Disk,
        idx: DISK,
        generation: 0,
        net_class: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| DiskPage::new(key, row, h, d, cx));
    let host = page.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| Host(host));
    vcx.simulate_scale_factor_change(1.0);
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let focus = vcx.update(|_, cx| page.read(cx).focus_handle(cx));
    vcx.update(|window, cx| focus.focus(window, cx));
    draw(vcx);
    Rig {
        page,
        demand,
        engine,
        _driver: driver,
        vcx,
    }
}

/// 60 one-second samples of `metric` on the disk ending now, all `value`.
fn series(metric: MetricId, value: f32) -> Arc<SeriesView> {
    const PERIOD: u64 = 1_000_000_000;
    let now = now_boot_ns();
    Arc::new(SeriesView {
        id: SeriesId {
            device: DISK,
            metric,
        },
        version: 1,
        period_ns: PERIOD,
        samples: (0..60u64)
            .map(|k| Sample::valid(now - (59 - k) * PERIOD, value))
            .collect::<Vec<_>>()
            .into(),
    })
}

#[gpui_kit::test]
fn disk_transfer_value_is_sum(cx: &mut TestAppContext) {
    let r = open(cx);
    let chart = r
        .vcx
        .update(|_, cx| r.page.read(cx).transfer_chart().clone());
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [
                Some(series(MetricId::DISK_READ, 8.1 * MIB)),
                Some(series(MetricId::DISK_WRITE, 4.2 * MIB)),
            ]
            .into(),
            cx,
        );
        v.set_state(ChartState::Live, cx);
    });
    draw(r.vcx);
    let (labels, value, ids) = r.vcx.read_entity(&chart, |v, _| {
        (
            v.labels.clone(),
            v.value_now(),
            v.series_ids().map(|s| s.metric).collect::<Vec<_>>(),
        )
    });
    assert_eq!(ids, [MetricId::DISK_READ, MetricId::DISK_WRITE]);
    assert_eq!(labels.value_fmt, ValueFormat::RateIec);
    let legend: Vec<&str> = labels.legend.iter().map(|(t, _)| t.as_ref()).collect();
    assert_eq!(legend, ["Read", "Write"]);
    let (value, validity) = value.expect("newest value");
    assert_eq!(validity, Validity::Valid);
    let mut text = String::new();
    format_value(labels.value_fmt, value, NetworkUnit::Bits, &mut text);
    assert_eq!(text, "12.3 MiB/s");
}

#[gpui_kit::test]
fn disk_page_two_charts_and_header(cx: &mut TestAppContext) {
    let r = open(cx);
    let active = r.vcx.debug_bounds(ACTIVE_ID).expect("Active time chart");
    let transfer = r
        .vcx
        .debug_bounds(TRANSFER_ID)
        .expect("Transfer rate chart");
    assert_eq!(active.size.height, px(150.));
    assert_eq!(transfer.size.height, px(150.));
    assert_eq!(transfer.top() - active.bottom(), px(8.), "sp4 apart");

    let page = r.page.clone();
    let (header, smart) = r
        .vcx
        .update(|_, cx| (page.read(cx).header(), page.read(cx).smart_available()));
    assert_eq!(header.0.as_ref(), "Disk 0");
    assert_eq!(header.1.as_ref(), "WD_BLACK SN850X 1000GB (nvme0n1)");
    assert!(smart);

    let chart = r.vcx.update(|_, cx| page.read(cx).active_chart().clone());
    let (y, heading) = r
        .vcx
        .read_entity(&chart, |v, _| (v.spec.y, v.labels.heading.clone()));
    assert_eq!(
        y,
        YScale::Fixed {
            min: 0.0,
            max: 100.0
        }
    );
    assert_eq!(heading.as_deref(), Some("Active time"));
}

#[gpui_kit::test]
fn disk_demand_only_when_active(cx: &mut TestAppContext) {
    let r = open(cx);
    let (page, demand) = (r.page.clone(), r.demand.clone());
    let merged = |vcx: &mut VisualTestContext| vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(merged(r.vcx), Demand::default(), "never active");

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    let d = merged(r.vcx);
    assert!(d.any_surface_visible && d.disks);
    assert!(!d.memory && !d.nets && !d.gpus && !d.cpu_fast);

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(false, cx)));
    assert_eq!(merged(r.vcx), Demand::default());
}

#[gpui_kit::test]
fn disk_charts_failed_on_source_failure(cx: &mut TestAppContext) {
    let r = open(cx);
    r.engine.publish_with(|s| {
        let h = Arc::make_mut(&mut s.health);
        h.version += 1;
        h.sources[SourceId::Disk.index()] = SourceHealth {
            state: SourceState::Failed,
            last_error: Some(Arc::from("EIO")),
            ..SourceHealth::default()
        };
    });
    draw(r.vcx);
    let page = r.page.clone();
    let charts = r.vcx.update(|_, cx| {
        let p = page.read(cx);
        [p.active_chart().clone(), p.transfer_chart().clone()]
    });
    for chart in charts {
        let state = r.vcx.read_entity(&chart, |v, _| v.state.clone());
        assert_eq!(state, ChartState::Failed("EIO".into()));
    }
}

#[gpui_kit::test]
fn disk_axis_pow2(cx: &mut TestAppContext) {
    let r = open(cx);
    let chart = r
        .vcx
        .update(|_, cx| r.page.read(cx).transfer_chart().clone());
    let (y, labels) = r
        .vcx
        .read_entity(&chart, |v, _| (v.spec.y, v.labels.clone()));
    assert_eq!(y, YScale::AutoRate { iec: true });
    assert!(labels.show_axis_max);
    // The axis text the chart computes for this spec and a 3 MiB/s peak in view.
    let mut axis = String::new();
    compute(
        &y,
        0.0,
        3.0 * MIB,
        now_boot_ns(),
        &mut ScaleState::default(),
        labels.value_fmt,
        NetworkUnit::Bits,
        &mut axis,
    );
    assert_eq!(axis, "4 MiB/s");
}

/// The Details grid's P1 rows (E25-006, UI-PRF-030; PERF-049, PERF-013).
mod details {
    use super::*;
    use tl_model::snapshot::SystemSnapshot;
    use tl_model::system::{DiskTotals, Metric};
    use tl_ui::pages::performance::details::KeyValueModel;
    use tl_ui::pages::performance::disk::{
        DiskDetailInputs, disk_details_model, update_disk_details_p1,
    };

    const GIB: u64 = 1 << 30;

    /// A disk record with every P1 field valid.
    fn record() -> DiskTotals {
        DiskTotals {
            device: DISK,
            active_pct: Metric::valid(12.0),
            read_bps: Metric::valid(4.2 * f64::from(MIB)),
            write_bps: Metric::valid(1.5 * f64::from(MIB)),
            response_ms: Metric::valid(0.4237),
            read_total: 812 * GIB,
            written_total: 400 * GIB,
            // 931.51 GiB
            capacity: 1_000_204_886_016,
            temp_c: Metric::valid(41.0),
        }
    }

    /// The model updated from `t`, with translated labels.
    fn model(cx: &mut TestAppContext, t: Option<&DiskTotals>) -> KeyValueModel {
        let mut model = disk_details_model();
        let inp = DiskDetailInputs {
            now_boot_ns: 0,
            observed_end_ns: 0,
            totals: t,
            error: None,
        };
        update_disk_details_p1(&mut model, &inp);
        cx.update(|cx| {
            init_strings(cx, "en-US");
            model.refresh_strings(cx);
            model.refresh_labels(cx);
        });
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
    fn disk_details_order_p1(cx: &mut TestAppContext) {
        let model = model(cx, Some(&record()));
        assert_eq!(model.rows.len(), 19);
        assert_eq!(
            rows(&model),
            [
                ("Active time", "12.0%"),
                ("Average response time", "0.42 ms"),
                ("Read speed", "4.2 MiB/s"),
                ("Write speed", "1.5 MiB/s"),
                ("Capacity", "931.51 GiB"),
            ]
            .map(|(l, v)| (l.to_string(), v.to_string()))
        );
    }

    #[gpui_kit::test]
    fn disk_response_time_two_decimals(cx: &mut TestAppContext) {
        let model = model(cx, Some(&record()));
        assert_eq!(
            value(&model, "Average response time").as_deref(),
            Some("0.42 ms")
        );
    }

    #[gpui_kit::test]
    fn disk_rate_measured_zero(cx: &mut TestAppContext) {
        let mut t = record();
        t.read_bps = Metric::valid(0.0);
        let model = model(cx, Some(&t));
        assert_eq!(value(&model, "Read speed").as_deref(), Some("0 KiB/s"));
    }

    #[gpui_kit::test]
    fn disk_latency_missing_collecting(cx: &mut TestAppContext) {
        let mut t = record();
        t.response_ms = Metric::with(Validity::Missing);
        let m = model(cx, Some(&t));
        assert_eq!(
            value(&m, "Average response time").as_deref(),
            Some("Collecting…")
        );
        assert_eq!(value(&m, "Active time").as_deref(), Some("12.0%"));

        // No record for the disk yet: every P1 row is collecting.
        let m = model(cx, None);
        for (label, text) in rows(&m) {
            assert_eq!(text, "Collecting…", "{label}");
        }
    }

    /// The same values update nothing; a new value changes only its row.
    #[gpui_kit::test]
    fn disk_details_change_only_on_new_value(cx: &mut TestAppContext) {
        let t = record();
        let mut model = model(cx, Some(&t));
        let inp = |t| DiskDetailInputs {
            now_boot_ns: 0,
            observed_end_ns: 0,
            totals: Some(t),
            error: None,
        };
        assert!(!update_disk_details_p1(&mut model, &inp(&t)));
        let mut t2 = t;
        t2.active_pct = Metric::valid(13.5);
        assert!(update_disk_details_p1(&mut model, &inp(&t2)));
        assert_eq!(value(&model, "Active time").as_deref(), Some("13.5%"));
    }

    /// The page fills its grid from the disk's record in the system snapshot.
    #[gpui_kit::test]
    fn disk_page_details_from_snapshot(cx: &mut TestAppContext) {
        let r = open(cx);
        let mut sys = SystemSnapshot::default();
        sys.header.validity = Validity::Valid;
        sys.header.version = 1;
        sys.disks = Arc::from(vec![(DISK, record())]);
        r.engine.publish_with(move |s| s.system = Arc::new(sys));
        draw(r.vcx);
        let page = r.page.clone();
        let values: Vec<String> = r
            .vcx
            .update(|_, cx| rows(page.read(cx).details()))
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            values,
            ["12.0%", "0.42 ms", "4.2 MiB/s", "1.5 MiB/s", "931.51 GiB"]
        );
    }
}
