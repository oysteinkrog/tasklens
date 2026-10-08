//! GPU page: probe states, the Memory chart and demand (E25, UI-PRF-032; PERF-062 to PERF-065).

#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{GraphicsAdapter, InvField};
use tl_model::system::{GpuTotals, Metric};
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::{ChartState, LegendStyle};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::gpu::{GpuPage, GpuVendor, ProbeState};
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const GPU: DeviceIdx = DeviceIdx(7);
const GIB: u64 = 1 << 30;

/// The page, and the window's animation driver, which a chart with data needs.
struct Host(Entity<GpuPage>, #[allow(dead_code)] Entity<AnimationDriver>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Cached, so the page renders only when it notifies.
        div().w(px(1000.)).h(px(2000.)).child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

fn totals(shared: bool) -> GpuTotals {
    let shared_total = if shared {
        Metric::valid(8 * GIB)
    } else {
        Metric::with(Validity::Unsupported)
    };
    GpuTotals {
        device: GPU,
        util_pct: Metric::valid(10.9),
        mem_util_pct: Metric::valid(4.0),
        mem_used: Metric::valid(3 * GIB),
        mem_total: Metric::valid(16 * GIB),
        shared_used: Metric::valid(0),
        shared_total,
        enc_pct: Metric::valid(0.0),
        dec_pct: Metric::valid(0.0),
        mem_reserved: Metric::with(Validity::Unsupported),
        enc_sessions: Metric::with(Validity::Unsupported),
        enc_dec_shared: false,
        temp_c: Metric::valid(52.0),
        temp_edge_c: Metric::with(Validity::Unsupported),
        temp_junction_c: Metric::with(Validity::Unsupported),
        temp_mem_c: Metric::with(Validity::Unsupported),
        power_w: Metric::default(),
        power_limit_w: Metric::default(),
        clock_gfx_mhz: Metric::default(),
        clock_mem_mhz: Metric::default(),
        fan_pct: Metric::default(),
        pcie_rx_bps: Metric::default(),
        pcie_tx_bps: Metric::default(),
    }
}

struct Rig<'a> {
    page: Entity<GpuPage>,
    demand: Entity<DemandAggregator>,
    engine: Arc<FakeEngine>,
    vcx: &'a mut VisualTestContext,
}

/// A GPU page for `GPU` in a 1000 px wide window. The inventory adapter has kernel module
/// `module` and temperature source `temp_source`.
fn open<'a>(cx: &'a mut TestAppContext, module: &str, temp_source: &str, shared: bool) -> Rig<'a> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![DeviceEntry {
                idx: GPU,
                identity: DeviceIdentity::Gpu {
                    uuid: [1; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
                generation: 0,
                display_name: Arc::from("NVIDIA GeForce RTX 4080"),
                present: true,
                parent: None,
                persistent_key: Arc::from(""),
                net_kind: None,
            }]),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.gpus = Arc::from([(GPU, totals(shared))].as_slice());
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        inv.graphics = Arc::from(
            [GraphicsAdapter {
                device: Some(GPU),
                kernel_module: InvField {
                    value: Arc::from(module),
                    validity: Validity::Valid,
                },
                temperature_source: InvField {
                    value: Arc::from(temp_source),
                    validity: Validity::Valid,
                },
                dedicated_bytes: InvField {
                    value: 16 * GIB,
                    validity: Validity::Valid,
                },
                ..GraphicsAdapter::default()
            }]
            .as_slice(),
        );
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
    let key = DeviceKey(Arc::from("gpu:GPU-0100"));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Gpu,
        idx: GPU,
        generation: 0,
        net_class: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| GpuPage::new(key, row, h, d, cx));
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

fn nvidia(cx: &mut TestAppContext) -> Rig<'_> {
    open(cx, "nvidia", "NVML (GPU core)", false)
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

/// Publishes the GPU source in `state` with `error`.
fn set_gpu_health(r: &mut Rig, state: SourceState, error: Option<&str>) {
    r.engine.publish_with(|s| {
        let h = Arc::make_mut(&mut s.health);
        h.version += 1;
        h.sources[SourceId::GpuNvidia.index()] = SourceHealth {
            state,
            last_error: error.map(Arc::from),
            ..SourceHealth::default()
        };
    });
    draw(r.vcx);
}

fn set_active(r: &mut Rig, active: bool) {
    let page = r.page.clone();
    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(active, cx)));
}

fn merged(r: &mut Rig) -> Demand {
    let demand = r.demand.clone();
    r.vcx.update(|_, cx| demand.read(cx).merged())
}

#[gpui_kit::test]
fn gpu_page_header_and_engines(cx: &mut TestAppContext) {
    let r = nvidia(cx);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.vendor(), GpuVendor::Nvidia);
        let (word, model) = p.header();
        assert_eq!(word.as_ref(), "Graphics adapter");
        assert_eq!(model.as_ref(), "NVIDIA GeForce RTX 4080");
        let keys: Vec<&str> = p.engine_keys().iter().map(|k| &**k).collect();
        assert_eq!(keys, ["gpu", "enc", "dec"]);
        let grid = p.engines().read(cx);
        let headings: Vec<&str> = grid.spec.cells.iter().map(|c| c.heading.as_ref()).collect();
        assert_eq!(headings, ["3D / Compute", "Video Encode", "Video Decode"]);
        assert_eq!(grid.spec.cells[0].fill_series.metric, MetricId::GPU_UTIL);
        assert_eq!(f32::from(grid.spec.cell_h), 112.);
    });
}

#[gpui_kit::test]
fn gpu_probe_restart_glyphs(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    set_gpu_health(&mut r, SourceState::Failed, Some("probe exited: signal 11"));
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.probe(), ProbeState::Restarting);
        assert_eq!(
            p.probe_tooltip().map(|t| t.as_ref()),
            Some("GPU monitor restarting")
        );
        for view in [p.memory(), p.temperature()] {
            let v = view.read(cx);
            assert_eq!(v.state, ChartState::Failed("GPU monitor restarting".into()));
            assert_eq!(v.value_now().map(|(_, s)| s), Some(Validity::Failed), "`!`");
        }
        // The grid keeps one cell per engine, each reading `!`.
        let grid = p.engines().read(cx);
        assert_eq!(
            grid.state,
            ChartState::Failed("GPU monitor restarting".into())
        );
        assert_eq!(grid.spec.cells.len(), 3);
        for i in 0..3 {
            assert_eq!(grid.value_text(i), Some("!"), "cell {i}");
        }
    });
    // Back to running: the charts drop the owner state.
    set_gpu_health(&mut r, SourceState::Running, None);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.probe(), ProbeState::Ok);
        assert_eq!(p.probe_tooltip(), None);
        assert_eq!(p.memory().read(cx).state, ChartState::Live);
    });
}

#[gpui_kit::test]
fn gpu_probe_missing_placeholder_cell(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    set_gpu_health(
        &mut r,
        SourceState::Unsupported,
        Some("GPU probe not installed"),
    );
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.probe(), ProbeState::Missing);
        assert_eq!(
            p.probe_tooltip().map(|t| t.as_ref()),
            Some("GPU probe not installed")
        );
        let cell = p.placeholder().expect("placeholder cell").read(cx);
        assert_eq!(
            cell.labels.heading.as_ref().map(|h| h.as_ref()),
            Some("3D / Compute")
        );
        assert_eq!(
            cell.value_now().map(|(_, s)| s),
            Some(Validity::Unsupported),
            "`n/a`"
        );
        for view in [p.memory(), p.temperature()] {
            assert_eq!(view.read(cx).state, ChartState::Unsupported);
        }
    });
}

#[gpui_kit::test]
fn gpu_probe_states_only_on_nvidia(cx: &mut TestAppContext) {
    let mut r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
    set_gpu_health(&mut r, SourceState::Failed, None);
    r.vcx.update(|_, cx| {
        let p = r.page.read(cx);
        assert_eq!(p.vendor(), GpuVendor::Amd);
        assert_eq!(p.probe(), ProbeState::Ok);
        let state = &p.memory().read(cx).state;
        assert!(
            !matches!(state, ChartState::Failed(_) | ChartState::Unsupported),
            "{state:?}"
        );
    });
}

#[gpui_kit::test]
fn gpu_memory_legend_without_shared(cx: &mut TestAppContext) {
    let r = nvidia(cx);
    r.vcx.update(|_, cx| {
        let v = r.page.read(cx).memory().read(cx);
        assert_eq!(v.spec.series.len(), 1);
        assert_eq!(v.spec.series[0].id.metric, MetricId::GPU_MEM);
        let legend: Vec<(&str, LegendStyle)> = v
            .labels
            .legend
            .iter()
            .map(|(t, s)| (t.as_ref(), *s))
            .collect();
        assert_eq!(legend, [("Dedicated", LegendStyle::Solid)]);
        assert_eq!(
            v.value_text().map(|t| t.as_ref()),
            Some("3.00 GiB / 16.00 GiB"),
            "owner value text"
        );
        assert_eq!(
            v.labels.heading.as_ref().map(|h| h.as_ref()),
            Some("Memory")
        );
    });
}

#[gpui_kit::test]
fn gpu_memory_shared_series_when_reported(cx: &mut TestAppContext) {
    let r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
    r.vcx.update(|_, cx| {
        let v = r.page.read(cx).memory().read(cx);
        assert_eq!(v.spec.series.len(), 2);
        assert_eq!(v.spec.series[1].id.metric, MetricId::GPU_MEM_SHARED);
        assert!(v.spec.series[1].dashed);
        assert!(v.spec.series[1].fill.is_none());
        assert_eq!(v.labels.legend.len(), 2);
        assert_eq!(v.labels.legend[1].1, LegendStyle::Dashed);
    });
}

#[gpui_kit::test]
fn gpu_demand_only_when_active(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    assert_eq!(merged(&mut r), Demand::default(), "never active");
    set_active(&mut r, true);
    let d = merged(&mut r);
    assert!(d.any_surface_visible);
    assert!(d.gpus);
    set_active(&mut r, false);
    assert_eq!(merged(&mut r), Demand::default());
}

#[gpui_kit::test]
fn gpu_samples_rerender_page_only_on_change(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    fn count(r: &mut Rig) -> u32 {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).render_count.get())
    }
    let publish = |r: &Rig, util: f32| {
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let mut t = totals(false);
            t.util_pct = Metric::valid(util);
            sys.gpus = Arc::from([(GPU, t)].as_slice());
        });
    };
    let before = count(&mut r);
    // The same values: the engine list, Memory shape and Details rows all stay.
    for _ in 0..3 {
        publish(&r, 10.9);
        draw(r.vcx);
    }
    assert_eq!(count(&mut r), before);
    // A new utilization changes the Details row once.
    for _ in 0..3 {
        publish(&r, 50.0);
        draw(r.vcx);
    }
    assert_eq!(count(&mut r), before + 1);
}

// Details grid rows 1 to 8 (tasklens-o55.10, UI-PRF-033).

fn detail_rows(r: &mut Rig) -> Vec<(String, String, Option<String>)> {
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let p = page.read(cx);
        p.details().refresh_labels(cx);
        p.details()
            .to_kv_rows()
            .map(|row| {
                (
                    row.label.to_string(),
                    row.value.text.to_string(),
                    row.value.tooltip.as_ref().map(|t| t.to_string()),
                )
            })
            .collect()
    })
}

#[gpui_kit::test]
fn gpu_details_order(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    let rows = detail_rows(&mut r);
    let labels: Vec<&str> = rows.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Utilization",
            "Dedicated memory",
            "Shared memory",
            "Hardware reserved memory",
            "Video encode",
            "Video decode",
            "Temperature",
            "Temperature source",
        ]
    );
    let values: Vec<&str> = rows.iter().map(|(_, v, _)| v.as_str()).collect();
    assert_eq!(
        values,
        [
            "10.9%",
            "3.00 GiB / 16.00 GiB",
            "Not supported",
            "Not reported",
            // `units::percent` writes zero as "0%" (table (d)).
            "0%",
            "0%",
            "52 °C",
            "NVML (GPU core)",
        ]
    );
    assert_eq!(rows[2].2.as_deref(), Some("Not available on this device"));
    // All 28 rows exist in the model, rows 9 to 28 hidden.
    r.vcx.update(|_, cx| {
        let d = r.page.read(cx).details();
        assert_eq!(d.rows.len(), 28);
        assert!(d.rows[8..].iter().all(|row| row.hidden));
    });
}

#[gpui_kit::test]
fn gpu_temperature_source_amd(cx: &mut TestAppContext) {
    let mut r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
    let rows = detail_rows(&mut r);
    let source = rows.iter().find(|(l, _, _)| l == "Temperature source");
    assert_eq!(
        source.map(|(_, v, _)| v.as_str()),
        Some("hwmon amdgpu: junction")
    );
    let shared = rows.iter().find(|(l, _, _)| l == "Shared memory");
    assert_eq!(
        shared.map(|(_, v, _)| v.as_str()),
        Some("0 B / 8.00 GiB"),
        "AMD reports a shared pool"
    );
}

#[gpui_kit::test]
fn gpu_details_probe_states(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    set_gpu_health(&mut r, SourceState::Failed, None);
    let rows = detail_rows(&mut r);
    assert!(rows.iter().all(|(_, v, _)| v == "Unavailable"), "{rows:?}");
    set_gpu_health(
        &mut r,
        SourceState::Unsupported,
        Some("GPU probe not installed"),
    );
    let rows = detail_rows(&mut r);
    assert!(
        rows.iter().all(|(_, v, _)| v == "Not supported"),
        "{rows:?}"
    );
}

#[test]
fn gpu_vendor_from_kernel_module() {
    for (module, vendor) in [
        ("nvidia", GpuVendor::Nvidia),
        ("amdgpu", GpuVendor::Amd),
        ("radeon", GpuVendor::Amd),
        ("i915", GpuVendor::Intel),
        ("xe", GpuVendor::Intel),
        ("nouveau", GpuVendor::Unknown),
        ("", GpuVendor::Unknown),
    ] {
        assert_eq!(GpuVendor::from_kernel_module(module), vendor, "{module}");
    }
}

#[gpui_kit::test]
fn gpu_memory_value_text_painted(cx: &mut TestAppContext) {
    use smallvec::smallvec;
    use tl_model::ids::SeriesId;
    use tl_model::series::{Sample, SeriesView};
    use tl_model::time::now_boot_ns;

    let r = nvidia(cx);
    let memory = r.vcx.update(|_, cx| r.page.read(cx).memory().clone());
    let now = now_boot_ns();
    let view = Arc::new(SeriesView {
        id: SeriesId {
            device: GPU,
            metric: MetricId::GPU_MEM,
        },
        version: 1,
        period_ns: 1_000_000_000,
        samples: vec![Sample::valid(now, (3 * GIB) as f32)].into(),
    });
    r.vcx.update(|_, cx| {
        memory.update(cx, |v, cx| v.set_series_for_test(smallvec![Some(view)], cx))
    });
    draw(r.vcx);
    r.vcx.update(|_, cx| {
        assert_eq!(
            memory.read(cx).painted_value_text(),
            Some("3.00 GiB / 16.00 GiB")
        );
    });
    // Without the owner text the chart formats series 0 alone.
    r.vcx
        .update(|_, cx| memory.update(cx, |v, cx| v.set_value_text(None, cx)));
    draw(r.vcx);
    r.vcx.update(|_, cx| {
        assert_eq!(memory.read(cx).painted_value_text(), Some("3.00 GiB"));
    });
}
