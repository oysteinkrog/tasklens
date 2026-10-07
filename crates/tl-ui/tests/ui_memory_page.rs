//! Memory page: charts, the Swap chart rules, the menu checks and the demand (E25, UI-PRF-026;
//! PERF-018, PERF-026, PERF-041).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings, UiState};
use tl_model::snapshot::SystemSnapshot;
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
use tl_ui::pages::performance::actions::PerfToggleMemorySwapGraph;
use tl_ui::pages::performance::memory::{COMMIT_ID, MemoryPage, PHYSICAL_ID, SWAP_ID};
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const GIB: u64 = 1 << 30;
/// The Memory device in the fake catalog.
const MEM: DeviceIdx = DeviceIdx(1);

struct Host(Entity<MemoryPage>);

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

/// Valid memory totals: `total`, `commit_limit` and `swap_total` in bytes.
fn system(total: u64, commit_limit: u64, swap_total: u64) -> SystemSnapshot {
    let mut sys = SystemSnapshot::default();
    sys.header.validity = Validity::Valid;
    sys.header.version = 1;
    let m = &mut sys.memory;
    m.validity = Validity::Valid;
    m.total = total;
    m.used = total / 2;
    m.available = total - m.used;
    m.committed = commit_limit / 2;
    m.commit_limit = commit_limit;
    m.swap_total = swap_total;
    m.swap_free = swap_total;
    sys
}

struct Rig<'a> {
    page: Entity<MemoryPage>,
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

/// A Memory page over `sys` in a 1000 px wide window, with `settings`, its root focused.
fn open(cx: &mut TestAppContext, settings: Settings, sys: SystemSnapshot) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
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
    let key = DeviceKey(Arc::from(DeviceKey::MEMORY));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Memory,
        idx: MEM,
        generation: 0,
        net_class: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| MemoryPage::new(key, row, h, d, cx));
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

/// 60 one-second samples of `metric` on the Memory device ending now, the last one `newest`
/// and the others `older`.
fn series(metric: MetricId, older: f32, newest: f32) -> Arc<SeriesView> {
    const PERIOD: u64 = 1_000_000_000;
    let now = now_boot_ns();
    Arc::new(SeriesView {
        id: SeriesId {
            device: MEM,
            metric,
        },
        version: 1,
        period_ns: PERIOD,
        samples: (0..60u64)
            .map(|k| {
                Sample::valid(
                    now - (59 - k) * PERIOD,
                    if k == 59 { newest } else { older },
                )
            })
            .collect::<Vec<_>>()
            .into(),
    })
}

fn with_swap(on: bool) -> Settings {
    let mut s = Settings::default();
    s.performance.memory_swap_graph = on;
    s
}

#[gpui_kit::test]
fn memory_swap_graph_hidden_without_swap(cx: &mut TestAppContext) {
    let r = open(cx, with_swap(true), system(64 * GIB, 80 * GIB, 0));
    assert!(r.vcx.debug_bounds(PHYSICAL_ID).is_some());
    assert!(r.vcx.debug_bounds(COMMIT_ID).is_some());
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none(), "no swap, no chart");
    let physical = r.vcx.debug_bounds(PHYSICAL_ID).expect("physical");
    let commit = r.vcx.debug_bounds(COMMIT_ID).expect("commit");
    assert_eq!(physical.size.height, px(150.));
    assert_eq!(commit.top() - physical.bottom(), px(8.), "sp4 apart");
}

#[gpui_kit::test]
fn memory_swap_toggle_follows_setting(cx: &mut TestAppContext) {
    let r = open(cx, with_swap(false), system(64 * GIB, 80 * GIB, 32 * GIB));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none(), "setting off");

    r.vcx.dispatch_action(PerfToggleMemorySwapGraph);
    draw(r.vcx);
    r.vcx.executor().advance_clock(Duration::from_millis(250));
    draw(r.vcx);
    let swap = r.vcx.debug_bounds(SWAP_ID).expect("swap chart shown");
    assert_eq!(swap.size.height, px(150.));
    let page = r.page.clone();
    let (setting, shown) = r.vcx.update(|_, cx| {
        (
            TlSettings::settings(cx).performance.memory_swap_graph,
            page.read(cx).show_swap(),
        )
    });
    assert!(setting && shown);
}

#[gpui_kit::test]
fn memory_menu_checks_follow_settings(cx: &mut TestAppContext) {
    use tl_ui::pages::performance::actions::PerfToggleMemoryComposition;
    let r = open(
        cx,
        Settings::default(),
        system(64 * GIB, 80 * GIB, 32 * GIB),
    );
    let page = r.page.clone();
    let before = r.vcx.update(|_, cx| page.read(cx).show_composition());
    r.vcx.dispatch_action(PerfToggleMemoryComposition);
    draw(r.vcx);
    let (after, setting) = r.vcx.update(|_, cx| {
        (
            page.read(cx).show_composition(),
            TlSettings::settings(cx).performance.memory_composition,
        )
    });
    assert_eq!((after, setting), (!before, !before));
    let words = r.vcx.update(|_, cx| {
        let w = page.read(cx).words();
        (w.menu_composition.to_string(), w.menu_swap.to_string())
    });
    assert_eq!(
        words,
        (
            "Show memory composition".to_owned(),
            "Show swap usage".to_owned()
        )
    );
}

#[gpui_kit::test]
fn commit_axis_text_above_100(cx: &mut TestAppContext) {
    let limit = 80 * GIB;
    let r = open(cx, Settings::default(), system(64 * GIB, limit, 0));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).commit().clone());
    let peak = limit as f32 * 1.12;
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [
                Some(series(MetricId::MEM_COMMIT, peak, limit as f32 * 0.69)),
                None,
            ]
            .into(),
            cx,
        );
        v.set_state(ChartState::Live, cx);
    });
    draw(r.vcx);
    let (y, labels, value) = r
        .vcx
        .read_entity(&chart, |v, _| (v.spec.y, v.labels.clone(), v.value_now()));
    assert_eq!(y, YScale::Commit(limit as f32));
    assert!(labels.show_axis_max);
    assert_eq!(labels.value_fmt, ValueFormat::PercentOf { total: limit });
    assert_eq!(labels.heading.as_deref(), Some("Commit"));

    // The axis text the chart computes for this spec and the peak in view.
    let mut axis = String::new();
    let now = now_boot_ns();
    compute(
        &y,
        0.0,
        peak,
        now,
        &mut ScaleState::default(),
        labels.value_fmt,
        NetworkUnit::Bits,
        &mut axis,
    );
    assert_eq!(axis, "112%");

    let (value, validity) = value.expect("newest value");
    assert_eq!(validity, Validity::Valid);
    let mut text = String::new();
    format_value(labels.value_fmt, value, NetworkUnit::Bits, &mut text);
    assert_eq!(text, "69.0%");
}

#[gpui_kit::test]
fn memory_physical_value_and_scale(cx: &mut TestAppContext) {
    let total = (63.16 * GIB as f64) as u64;
    let used = 34.68 * GIB as f32;
    let r = open(cx, Settings::default(), system(total, 80 * GIB, 0));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).physical().clone());
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [Some(series(MetricId::MEM_USED, used, used)), None].into(),
            cx,
        );
        v.set_state(ChartState::Live, cx);
    });
    draw(r.vcx);
    let (y, labels, value, ids) = r.vcx.read_entity(&chart, |v, _| {
        (
            v.spec.y,
            v.labels.clone(),
            v.value_now(),
            v.series_ids().map(|s| s.metric).collect::<Vec<_>>(),
        )
    });
    assert_eq!(y, YScale::FixedTotal(total as f32));
    assert_eq!(
        ids,
        [
            MetricId::MEM_USED,
            MetricId::MEM_COMMIT,
            MetricId::MEM_DIRTY
        ]
    );
    assert_eq!(labels.heading.as_deref(), Some("Physical memory"));
    let legend: Vec<&str> = labels.legend.iter().map(|(t, _)| t.as_ref()).collect();
    assert_eq!(legend, ["Used", "Committed"], "no legend entry for dirty");
    let (value, _) = value.expect("newest value");
    let mut text = String::new();
    format_value(labels.value_fmt, value, NetworkUnit::Bits, &mut text);
    assert_eq!(text, "34.68 GiB");

    let page = r.page.clone();
    let header = r.vcx.update(|_, cx| page.read(cx).header());
    assert_eq!(header.0.as_ref(), "Memory");
    assert!(header.1.is_empty());
}

#[gpui_kit::test]
fn memory_demand_only_when_active(cx: &mut TestAppContext) {
    let r = open(cx, Settings::default(), system(64 * GIB, 80 * GIB, 0));
    let (page, demand) = (r.page.clone(), r.demand.clone());
    let merged = |vcx: &mut VisualTestContext| vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(merged(r.vcx), Demand::default(), "never active");

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    let d = merged(r.vcx);
    assert!(d.any_surface_visible);
    assert!(d.memory);
    assert!(!d.disks && !d.nets && !d.gpus && !d.cpu_fast);

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(false, cx)));
    assert_eq!(merged(r.vcx), Demand::default());
}

#[gpui_kit::test]
fn memory_chart_hide_instant_under_reduce_motion(cx: &mut TestAppContext) {
    let mut settings = with_swap(true);
    settings.motion.animations = false;
    let r = open(cx, settings, system(64 * GIB, 80 * GIB, 32 * GIB));
    assert!(r.vcx.update(|_, cx| cx.reduce_motion()));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_some());

    r.vcx.dispatch_action(PerfToggleMemorySwapGraph);
    let page = r.page.clone();
    assert!(
        !r.vcx.update(|_, cx| page.read(cx).swap_animating()),
        "no animation"
    );
    // One frame, no clock advance: the chart is gone.
    r.vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none());
}

#[gpui_kit::test]
fn memory_totals_change_rebuilds_specs(cx: &mut TestAppContext) {
    let r = open(cx, with_swap(true), system(64 * GIB, 80 * GIB, 0));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none());
    let page = r.page.clone();
    // A new SwapTotal arrives with the next system snapshot.
    let (physical, swap) = r.vcx.update(|_, cx| {
        let p = page.read(cx);
        (p.physical().clone(), p.swap().clone())
    });
    let builds = r.vcx.read_entity(&physical, |v, _| v.spec.clone());
    let mut sys = system(64 * GIB, 80 * GIB, 16 * GIB);
    sys.header.version = 2;
    r.engine.publish_with(move |s| s.system = Arc::new(sys));
    draw(r.vcx);
    r.vcx.executor().advance_clock(Duration::from_millis(250));
    draw(r.vcx);
    assert!(r.vcx.debug_bounds(SWAP_ID).is_some(), "swap appeared");
    let y = r.vcx.read_entity(&swap, |v, _| v.spec.y);
    assert_eq!(y, YScale::FixedTotal((16 * GIB) as f32));
    let same = r.vcx.read_entity(&physical, |v, _| v.spec.clone());
    assert_eq!(
        same, builds,
        "an unchanged MemTotal keeps the Physical spec"
    );
}
