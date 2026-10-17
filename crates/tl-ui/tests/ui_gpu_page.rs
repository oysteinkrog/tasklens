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
use tl_model::system::{GpuStaticInfo, GpuTotals, Metric};
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
const MIB: u64 = 1 << 20;

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
        mem_reserved: Metric::valid(380 * MIB),
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
        engines: Default::default(),
        fdinfo_partial: false,
        energy_since_boot: Metric::default(),
        clock_video_mhz: Metric::default(),
        pstate: Metric::default(),
        clock_event_reasons: Metric::default(),
        fan_rpm: Metric::default(),
        pcie_gen: Metric::default(),
        pcie_width: Metric::default(),
        info: Arc::default(),
    }
}

fn text(value: &str) -> InvField<Arc<str>> {
    InvField {
        value: Arc::from(value),
        validity: Validity::Valid,
    }
}

/// The fake NVIDIA record with every P2 value of UI-PRF-033.
fn full_totals() -> GpuTotals {
    GpuTotals {
        fan_pct: Metric::valid(0.0),
        fan_rpm: Metric::valid(0),
        clock_gfx_mhz: Metric::valid(1905),
        clock_mem_mhz: Metric::valid(7001),
        clock_video_mhz: Metric::valid(1620),
        pstate: Metric::valid(3),
        clock_event_reasons: Metric::default(),
        power_w: Metric::valid(31.9),
        power_limit_w: Metric::valid(300.0),
        energy_since_boot: Metric::valid(1.24 * 3.6e6),
        pcie_gen: Metric::valid(4),
        pcie_width: Metric::valid(16),
        pcie_rx_bps: Metric::valid(12.3 * MIB as f32),
        pcie_tx_bps: Metric::valid(1.2 * MIB as f32),
        info: Arc::new(GpuStaticInfo {
            cuda_version: Some(Arc::from("13.2")),
            vbios: Some(Arc::from("95.03.3A.00.1B")),
            architecture: Some(Arc::from("Ada Lovelace")),
            pcie_max_gen: Some(5),
            pcie_max_width: Some(16),
            clock_gfx_max_mhz: Some(3105),
            clock_mem_max_mhz: Some(14001),
            power_max_limit_w: Some(350.0),
            power_min_limit_w: Some(100.0),
            slowdown_temp_c: Some(88.0),
            shutdown_temp_c: Some(95.0),
        }),
        ..totals(false)
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
                    pci_bus_id: Arc::from("0000:2d:00.0"),
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
                kernel_module: text(module),
                temperature_source: InvField {
                    value: Arc::from(temp_source),
                    validity: Validity::Valid,
                },
                dedicated_bytes: InvField {
                    value: 16 * GIB,
                    validity: Validity::Valid,
                },
                driver_version: text("615.71.09"),
                vulkan_version: text("1.4.312"),
                opengl_version: text("4.6.0 NVIDIA 615.71.09"),
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
        fan_connected: None,
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
    let mut r = open(cx, "nvidia 615.71.09", "NVML (GPU core)", false);
    publish_totals(&mut r, &full_totals());
    r
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
            let mut t = full_totals();
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
    let expected = [
        ("Utilization", "10.9%"),
        ("Dedicated memory", "3.00 GiB / 16.00 GiB"),
        ("Shared memory", "Not supported"),
        ("Hardware reserved memory", "380.00 MiB"),
        // `units::percent` writes zero as "0%" (table (d)).
        ("Video encode", "0%"),
        ("Video decode", "0%"),
        ("Temperature", "52 °C"),
        ("Temperature source", "NVML (GPU core)"),
        ("Fan speed", "0% (0 RPM)"),
        ("Slowdown temperature", "88 °C"),
        ("Shutdown temperature", "95 °C"),
        ("Memory temperature", "Not reported"),
        ("Clock speed", "1905 / 3105 MHz"),
        ("Memory speed", "7001 / 14001 MHz"),
        ("Video clock", "1620 MHz"),
        ("Power draw", "31.9 / 300 W"),
        ("PCI Express", "Gen 4 x16"),
        ("Max PCI Express", "Gen 5 x16"),
        ("PCIe throughput", "↓ 12.3 MiB/s ↑ 1.2 MiB/s"),
        ("Driver version", "615.71.09"),
        ("Kernel module", "nvidia 615.71.09"),
        ("CUDA version", "13.2"),
        ("Vulkan version", "1.4.312"),
        ("OpenGL version", "4.6.0 NVIDIA 615.71.09"),
        ("Physical location", "PCI bus 45, device 0, function 0"),
        ("VBIOS version", "95.03.3A.00.1B"),
        ("UUID", "GPU-01010101-0101-0101-0101-010101010101"),
        ("Architecture", "Ada Lovelace"),
    ];
    let got: Vec<(&str, &str)> = rows
        .iter()
        .map(|(l, v, _)| (l.as_str(), v.as_str()))
        .collect();
    assert_eq!(got, expected);
    assert_eq!(rows[2].2.as_deref(), Some("Not available on this device"));
    assert_eq!(
        find(&rows, "Clock speed").and_then(|r| r.2.as_deref()),
        Some("P3")
    );
    assert_eq!(
        find(&rows, "Power draw").and_then(|r| r.2.as_deref()),
        Some("max 350 W · 1.24 kWh since boot")
    );
    // The 28 rows and the merged encode/decode row exist in the model. Only the merged row is
    // hidden.
    r.vcx.update(|_, cx| {
        let d = r.page.read(cx).details();
        assert_eq!(d.rows.len(), 29);
        assert_eq!(d.rows[6].label, "perf-gpu-row-encode-decode");
        assert!(d.rows[6].hidden);
        assert!(d.rows.iter().filter(|row| row.hidden).count() == 1);
    });
}

/// Publishes `t` as the GPU's record and draws.
fn publish_totals(r: &mut Rig, t: &GpuTotals) {
    r.engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.gpus = Arc::from([(GPU, t.clone())].as_slice());
    });
    draw(r.vcx);
}

fn find<'a>(
    rows: &'a [(String, String, Option<String>)],
    label: &str,
) -> Option<&'a (String, String, Option<String>)> {
    rows.iter().find(|(l, _, _)| l == label)
}

#[gpui_kit::test]
fn gpu_shared_encode_decode_row(cx: &mut TestAppContext) {
    let mut r = open(cx, "i915", "hwmon i915", false);
    let mut t = totals(false);
    t.enc_dec_shared = true;
    t.enc_pct = Metric::valid(12.5);
    t.dec_pct = Metric::valid(12.5);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    let labels: Vec<&str> = rows.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(
        labels[3..6],
        [
            "Hardware reserved memory",
            "Video encode/decode",
            "Temperature"
        ]
    );
    assert_eq!(
        find(&rows, "Video encode/decode").map(|r| r.1.as_str()),
        Some("12.5%")
    );

    // Separate engines again: two rows.
    publish_totals(&mut r, &totals(false));
    let rows = detail_rows(&mut r);
    assert!(find(&rows, "Video encode/decode").is_none());
    assert!(find(&rows, "Video encode").is_some());
    assert!(find(&rows, "Video decode").is_some());
}

#[gpui_kit::test]
fn gpu_encode_sessions_tooltip(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    let mut t = totals(false);
    t.enc_sessions = Metric::valid(2);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    let encode = find(&rows, "Video encode").expect("encode row");
    assert_eq!(encode.2.as_deref(), Some("2 active sessions"));

    // One session: the singular form (o55.37).
    t.enc_sessions = Metric::valid(1);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    let encode = find(&rows, "Video encode").expect("encode row");
    assert_eq!(encode.2.as_deref(), Some("1 active session"));

    // No session count: no tooltip.
    t.enc_sessions = Metric::with(Validity::Unsupported);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    assert_eq!(find(&rows, "Video encode").and_then(|r| r.2.clone()), None);
}

#[gpui_kit::test]
fn gpu_amd_temperature_sensors(cx: &mut TestAppContext) {
    let mut r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
    let mut t = totals(true);
    t.mem_reserved = Metric::with(Validity::Unsupported);
    t.temp_c = Metric::valid(52.0);
    t.temp_edge_c = Metric::valid(48.0);
    t.temp_junction_c = Metric::valid(52.0);
    t.temp_mem_c = Metric::valid(60.0);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    let temp = find(&rows, "Temperature").expect("temperature row");
    assert_eq!(temp.1, "52 °C");
    assert_eq!(
        temp.2.as_deref(),
        Some("edge 48 °C · junction 52 °C · mem 60 °C")
    );
    assert_eq!(
        find(&rows, "Temperature source").map(|r| r.1.as_str()),
        Some("hwmon amdgpu: junction")
    );
    assert_eq!(
        find(&rows, "Hardware reserved memory").map(|r| r.1.as_str()),
        Some("Not supported")
    );

    // No mem sensor: two in the tooltip.
    t.temp_mem_c = Metric::with(Validity::Unsupported);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    assert_eq!(
        find(&rows, "Temperature")
            .and_then(|r| r.2.clone())
            .as_deref(),
        Some("edge 48 °C · junction 52 °C")
    );
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
    // The rows the probe feeds. The driver, module, Vulkan, OpenGL, location and UUID rows come
    // from the inventory and the device list, so they keep their values.
    let inventory = [
        "Driver version",
        "Kernel module",
        "Vulkan version",
        "OpenGL version",
        "Physical location",
        "UUID",
    ];
    set_gpu_health(&mut r, SourceState::Failed, None);
    let rows = detail_rows(&mut r);
    for (label, value, _) in &rows {
        if !inventory.contains(&label.as_str()) {
            assert_eq!(value, "Unavailable", "{label}");
        }
    }
    assert_eq!(rows.len(), 28, "Max PCI Express and CUDA stay visible");
    assert_eq!(
        find(&rows, "Driver version").map(|r| r.1.as_str()),
        Some("615.71.09")
    );
    set_gpu_health(
        &mut r,
        SourceState::Unsupported,
        Some("GPU probe not installed"),
    );
    let rows = detail_rows(&mut r);
    for (label, value, _) in &rows {
        if !inventory.contains(&label.as_str()) {
            assert_eq!(value, "Not supported", "{label}");
        }
    }
}

#[test]
fn gpu_vendor_from_kernel_module() {
    for (module, vendor) in [
        ("nvidia", GpuVendor::Nvidia),
        ("nvidia 615.71.09", GpuVendor::Nvidia),
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

mod engines {
    use std::time::Duration;

    use gpui_kit::{Focusable as _, TestAppContext};
    use tl_model::system::GpuEngine;
    use tl_ui::components::menu::{MenuEntryKind, TlMenu};
    use tl_ui::pages::performance::actions::PerfToggleGpuEngine;
    use tl_ui::pages::performance::gpu::{ENGINES_ID, MEMORY_ID};
    use tl_ui::pages::performance::menu::{PageMenuCtx, page_menu};

    use super::*;

    /// The NVIDIA totals with the memory controller reported.
    fn with_memctl() -> GpuTotals {
        let mut t = totals(false);
        t.engines.set(GpuEngine::MemCtl, Metric::valid(4.0));
        t
    }

    fn open_memctl(cx: &mut TestAppContext) -> Rig<'_> {
        let mut r = nvidia(cx);
        publish_totals(&mut r, &with_memctl());
        r
    }

    fn cell_keys(r: &mut Rig) -> Vec<String> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.read(cx)
                .engine_keys()
                .iter()
                .map(|k| k.to_string())
                .collect()
        })
    }

    fn hidden(r: &mut Rig) -> Vec<String> {
        r.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .hidden_gpu_engines
                .clone()
        })
    }

    fn toggle(r: &mut Rig, engine: &str) {
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
        r.vcx.dispatch_action(PerfToggleGpuEngine {
            key: DeviceKey(Arc::from("gpu:GPU-0100")),
            engine: Arc::from(engine),
        });
        draw(r.vcx);
    }

    fn box_height(r: &mut Rig) -> f32 {
        f32::from(
            r.vcx
                .debug_bounds(ENGINES_ID)
                .expect("engine box")
                .size
                .height,
        )
    }

    #[gpui_kit::test]
    fn gpu_memctl_cell_for_nvidia(cx: &mut TestAppContext) {
        let mut r = open_memctl(cx);
        assert_eq!(cell_keys(&mut r), ["gpu", "memctl", "enc", "dec"]);
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let grid = page.read(cx).engines().read(cx);
            let headings: Vec<&str> = grid.spec.cells.iter().map(|c| c.heading.as_ref()).collect();
            assert_eq!(
                headings,
                [
                    "3D / Compute",
                    "Memory controller",
                    "Video Encode",
                    "Video Decode"
                ]
            );
            assert_eq!(
                grid.spec.cells[1].fill_series.metric,
                MetricId::GPU_ENGINE_MEMCTL
            );
        });
    }

    #[gpui_kit::test]
    fn gpu_hidden_engine_retiles(cx: &mut TestAppContext) {
        let mut r = open_memctl(cx);
        // 4 cells in 3 columns at 1000 px: two rows.
        let two_rows = box_height(&mut r);
        assert_eq!(two_rows, 112. * 2. + 8.);
        toggle(&mut r, "memctl");
        assert_eq!(hidden(&mut r), ["memctl"]);
        assert_eq!(cell_keys(&mut r), ["gpu", "enc", "dec"]);
        assert_eq!(box_height(&mut r), 112., "3 cells fit one row");
    }

    #[gpui_kit::test]
    fn gpu_all_engines_hidden_collapses_row(cx: &mut TestAppContext) {
        let mut r = open_memctl(cx);
        for e in ["gpu", "memctl", "enc"] {
            toggle(&mut r, e);
        }
        let before = r.vcx.debug_bounds(MEMORY_ID).expect("memory").top();
        toggle(&mut r, "dec");
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
        assert!(cell_keys(&mut r).is_empty());
        assert_eq!(box_height(&mut r), 0.);
        let after = r.vcx.debug_bounds(MEMORY_ID).expect("memory").top();
        assert_eq!(f32::from(before - after), 112. + 8., "rows 2 and 3 move up");
        toggle(&mut r, "dec");
        r.vcx.executor().advance_clock(Duration::from_millis(250));
        draw(r.vcx);
        assert_eq!(box_height(&mut r), 112.);
    }

    #[gpui_kit::test]
    fn gpu_toggle_engine_setting_round_trip(cx: &mut TestAppContext) {
        let mut r = open_memctl(cx);
        toggle(&mut r, "memctl");
        assert_eq!(hidden(&mut r), ["memctl"]);
        toggle(&mut r, "memctl");
        assert!(hidden(&mut r).is_empty());
        assert_eq!(cell_keys(&mut r), ["gpu", "memctl", "enc", "dec"]);
    }

    fn menu_entries(
        r: &mut Rig,
        cell_engine: Option<&str>,
    ) -> Vec<(MenuEntryKind, String, bool, Option<bool>)> {
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            let ctx = PageMenuCtx {
                range_s: 60,
                summary_on: false,
                ranges_long_enabled: true,
                view_types: None,
                clicked_value: false,
                tray_available: None,
                cell_engine: cell_engine.map(Arc::from),
            };
            let menu = TlMenu::build(window, cx, focus, |menu, w, cx| {
                page.update(cx, |p, cx| page_menu(menu, p, &ctx, w, cx))
            });
            let list = menu.read(cx).entries_for_test(window);
            list.iter()
                .map(|e| {
                    let hide = e.action.as_ref().map(|a| {
                        a.partial_eq(&PerfToggleGpuEngine {
                            key: DeviceKey(Arc::from("gpu:GPU-0100")),
                            engine: Arc::from("memctl"),
                        })
                    });
                    (
                        e.kind,
                        e.label.as_ref().map(|l| l.to_string()).unwrap_or_default(),
                        e.checked,
                        hide,
                    )
                })
                .collect()
        })
    }

    #[gpui_kit::test]
    fn gpu_cell_menu_hide_item_first(cx: &mut TestAppContext) {
        let mut r = open_memctl(cx);
        let over = menu_entries(&mut r, Some("memctl"));
        assert_eq!(over[0].1, "Hide this engine graph");
        assert_eq!(
            over[0].3,
            Some(true),
            "dispatches PerfToggleGpuEngine memctl"
        );
        let plain = menu_entries(&mut r, None);
        assert_ne!(plain[0].1, "Hide this engine graph");
        assert!(plain.iter().any(|e| e.1 == "Select GPU engine graphs"));
    }

    #[gpui_kit::test]
    fn gpu_select_engines_submenu(cx: &mut TestAppContext) {
        let mut r = open_memctl(cx);
        toggle(&mut r, "enc");
        let page = r.page.clone();
        let items = r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            let menu = TlMenu::build(window, cx, focus, |menu, w, cx| {
                page.update(cx, |p, cx| p.menu_items(menu, w, cx))
            });
            let list = menu.read(cx).entries_for_test(window);
            let sub = list
                .iter()
                .find(|e| e.label.as_deref() == Some("Select GPU engine graphs"))
                .and_then(|e| e.submenu.clone())
                .expect("submenu");
            sub.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| {
                    (
                        e.kind,
                        e.label.map(|l| l.to_string()).unwrap_or_default(),
                        e.checked,
                        e.tooltip.map(|t| t.to_string()),
                    )
                })
                .collect::<Vec<_>>()
        });
        let names: Vec<&str> = items.iter().map(|e| e.1.as_str()).collect();
        assert_eq!(
            names,
            [
                "3D / Compute",
                "Memory controller",
                "Video Encode",
                "Video Decode",
                "",
                "Power",
                "Clocks"
            ]
        );
        let checked: Vec<bool> = items[..4].iter().map(|e| e.2).collect();
        assert_eq!(checked, [true, true, false, true], "Video Encode is hidden");
        assert_eq!(items[4].0, MenuEntryKind::Separator);
        // Power and Clocks are enabled check items, both off by default.
        for e in &items[5..] {
            assert_eq!(e.0, MenuEntryKind::Item);
            assert!(!e.2, "{} is off", e.1);
            assert_eq!(e.3, None);
        }
    }
}

mod vendor_grids {
    use gpui_kit::TestAppContext;
    use tl_model::system::GpuEngine;

    use super::*;

    fn with_engines(list: &[GpuEngine]) -> GpuTotals {
        let mut t = totals(false);
        for e in list {
            t.engines.set(*e, Metric::valid(7.0));
        }
        t
    }

    fn headings(r: &mut Rig) -> (Vec<String>, Vec<String>) {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            let keys = p.engine_keys().iter().map(|k| k.to_string()).collect();
            let grid = p.engines().read(cx);
            (
                keys,
                grid.spec
                    .cells
                    .iter()
                    .map(|c| c.heading.to_string())
                    .collect(),
            )
        })
    }

    fn tooltips(r: &mut Rig) -> Vec<Option<String>> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let grid = page.read(cx).engines().read(cx);
            grid.spec
                .cells
                .iter()
                .map(|c| c.tooltip.as_ref().map(|t| t.to_string()))
                .collect()
        })
    }

    #[gpui_kit::test]
    fn gpu_engine_names_amd(cx: &mut TestAppContext) {
        let mut r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
        publish_totals(
            &mut r,
            &with_engines(&[
                GpuEngine::MemCtl,
                GpuEngine::Compute,
                GpuEngine::Jpeg,
                GpuEngine::Dma,
            ]),
        );
        let (keys, names) = headings(&mut r);
        assert_eq!(
            keys,
            ["gpu", "compute", "memctl", "enc", "dec", "jpeg", "dma"]
        );
        assert_eq!(
            names,
            [
                "Graphics",
                "Compute",
                "Memory controller",
                "Video Encode",
                "Video Decode",
                "JPEG",
                "DMA"
            ]
        );
        // A card without JPEG does not list it.
        publish_totals(&mut r, &with_engines(&[GpuEngine::Compute, GpuEngine::Dma]));
        let (keys, _) = headings(&mut r);
        assert_eq!(keys, ["gpu", "compute", "enc", "dec", "dma"]);
    }

    #[gpui_kit::test]
    fn gpu_engine_names_intel(cx: &mut TestAppContext) {
        let mut r = open(cx, "i915", "hwmon i915", true);
        publish_totals(
            &mut r,
            &with_engines(&[
                GpuEngine::Render,
                GpuEngine::Compute,
                GpuEngine::Copy,
                GpuEngine::Video,
                GpuEngine::VideoEnhance,
            ]),
        );
        let (keys, names) = headings(&mut r);
        assert_eq!(
            keys,
            ["render", "compute", "copy", "video", "video-enhance"]
        );
        assert_eq!(
            names,
            ["Render/3D", "Compute", "Copy", "Video", "Video Processing"]
        );
        // The legacy "gpu" key hides Render, and "enc" with "dec" hides Video.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.hidden_gpu_engines =
                    vec!["gpu".into(), "enc".into(), "dec".into()];
            });
        });
        draw(r.vcx);
        let (keys, _) = headings(&mut r);
        assert_eq!(keys, ["compute", "copy", "video-enhance"]);
    }

    #[gpui_kit::test]
    fn gpu_nvidia_jpeg_ofa_only_when_reported(cx: &mut TestAppContext) {
        let mut r = nvidia(cx);
        let (keys, _) = headings(&mut r);
        assert_eq!(keys, ["gpu", "enc", "dec"]);
        publish_totals(&mut r, &with_engines(&[GpuEngine::Jpeg, GpuEngine::Ofa]));
        let (keys, names) = headings(&mut r);
        assert_eq!(keys, ["gpu", "enc", "dec", "jpeg", "ofa"]);
        assert_eq!(names[3..], ["JPEG", "Optical flow"]);
    }

    #[gpui_kit::test]
    fn gpu_amd_own_processes_tooltip(cx: &mut TestAppContext) {
        let mut r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
        let mut t = with_engines(&[GpuEngine::Compute, GpuEngine::Jpeg, GpuEngine::Dma]);
        publish_totals(&mut r, &t);
        assert!(tooltips(&mut r).iter().all(Option::is_none));
        t.fdinfo_partial = true;
        publish_totals(&mut r, &t);
        // gpu, compute, enc, dec, jpeg, dma: only the fdinfo engines carry the tooltip.
        let own = Some("Own processes only".to_owned());
        assert_eq!(
            tooltips(&mut r),
            [None, own.clone(), None, None, own.clone(), own]
        );
        t.fdinfo_partial = false;
        publish_totals(&mut r, &t);
        assert!(tooltips(&mut r).iter().all(Option::is_none));
    }

    #[gpui_kit::test]
    fn gpu_intel_has_no_own_processes_tooltip(cx: &mut TestAppContext) {
        let mut r = open(cx, "i915", "hwmon i915", true);
        let mut t = with_engines(&[GpuEngine::Render, GpuEngine::Copy, GpuEngine::Video]);
        t.fdinfo_partial = true;
        publish_totals(&mut r, &t);
        assert!(tooltips(&mut r).iter().all(Option::is_none));
    }
}

// Details grid rows 9 to 28 (tasklens-o55.25, UI-PRF-033).

#[gpui_kit::test]
fn gpu_max_pcie_hidden_when_equal(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    assert!(find(&detail_rows(&mut r), "Max PCI Express").is_some());
    let mut t = full_totals();
    t.info = Arc::new(GpuStaticInfo {
        pcie_max_gen: Some(4),
        pcie_max_width: Some(16),
        ..(*t.info).clone()
    });
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    assert!(find(&rows, "Max PCI Express").is_none(), "{rows:?}");
    assert_eq!(
        find(&rows, "PCI Express").map(|r| r.1.as_str()),
        Some("Gen 4 x16")
    );
    // The card drops to Gen 1 while idle: the maximum shows again.
    t.pcie_gen = Metric::valid(1);
    publish_totals(&mut r, &t);
    assert_eq!(
        find(&detail_rows(&mut r), "Max PCI Express").map(|r| r.1.as_str()),
        Some("Gen 4 x16")
    );
}

#[gpui_kit::test]
fn gpu_cuda_hidden_off_nvidia(cx: &mut TestAppContext) {
    let mut r = open(cx, "amdgpu", "hwmon amdgpu: junction", true);
    publish_totals(&mut r, &full_totals());
    let rows = detail_rows(&mut r);
    assert!(find(&rows, "CUDA version").is_none(), "{rows:?}");
    assert!(find(&rows, "Vulkan version").is_some());
}

#[gpui_kit::test]
fn gpu_power_needs_setup_link(cx: &mut TestAppContext) {
    let mut r = open(cx, "i915", "hwmon i915", false);
    // An iGPU has no dedicated memory.
    r.engine.publish_with(|s| {
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        let graphics: Vec<GraphicsAdapter> = inv
            .graphics
            .iter()
            .cloned()
            .map(|a| GraphicsAdapter {
                dedicated_bytes: InvField::default(),
                ..a
            })
            .collect();
        inv.graphics = Arc::from(graphics);
    });
    let mut t = full_totals();
    t.mem_total = Metric::with(Validity::Unsupported);
    t.power_w = Metric::with(Validity::Unsupported);
    publish_totals(&mut r, &t);
    let rows = detail_rows(&mut r);
    assert_eq!(
        find(&rows, "Power draw").map(|r| r.1.as_str()),
        Some("Needs setup")
    );
    r.vcx.update(|_, cx| {
        let d = r.page.read(cx).details();
        let power = d.rows.iter().find(|row| row.label == "perf-gpu-row-power");
        let power = power.expect("power row");
        assert!(power.link, "Needs setup is a link");
        assert!(power.action.is_some());
    });
}

#[gpui_kit::test]
fn gpu_power_not_supported_on_discrete_intel(cx: &mut TestAppContext) {
    let mut r = open(cx, "xe", "hwmon xe: pkg", false);
    let mut t = full_totals();
    t.power_w = Metric::with(Validity::Unsupported);
    publish_totals(&mut r, &t);
    assert_eq!(
        find(&detail_rows(&mut r), "Power draw").map(|r| r.1.as_str()),
        Some("Not supported")
    );
}

#[gpui_kit::test]
fn gpu_uuid_truncates_with_tooltip(cx: &mut TestAppContext) {
    let mut r = nvidia(cx);
    let rows = detail_rows(&mut r);
    let uuid = "GPU-01010101-0101-0101-0101-010101010101";
    let row = find(&rows, "UUID").expect("UUID row");
    assert_eq!(row.1, uuid);
    // The grid cuts the cell to the column width with "…"; the tooltip holds the whole UUID.
    assert_eq!(row.2.as_deref(), Some(uuid));
}

/// The optional Power and Clocks row (E25-023, UI-PRF-032 row 3).
mod optional {
    use gpui_kit::{Focusable as _, TestAppContext};
    use tl_model::settings::GpuOptionalGraph;
    use tl_model::system::ClockEventReasons;
    use tl_ui::pages::performance::actions::PerfToggleGpuOptionalGraph;
    use tl_ui::pages::performance::gpu::{CLOCKS_ID, ENGINES_ID, POWER_ID};

    use super::*;

    fn set_optional(r: &mut Rig, list: &[GpuOptionalGraph]) {
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.performance.gpu_optional_graphs = list.to_vec();
            });
        });
        draw(r.vcx);
    }

    fn width(r: &mut Rig, id: &'static str) -> f32 {
        f32::from(r.vcx.debug_bounds(id).expect("chart box").size.width)
    }

    #[gpui_kit::test]
    fn gpu_optional_row_full_width_when_one(cx: &mut TestAppContext) {
        let mut r = nvidia(cx);
        set_optional(&mut r, &[GpuOptionalGraph::Power]);
        let engines = width(&mut r, ENGINES_ID);
        assert_eq!(width(&mut r, POWER_ID), engines, "one chart spans row 3");
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_none());
        let height = f32::from(r.vcx.debug_bounds(POWER_ID).expect("power").size.height);
        assert_eq!(height, 150.);
    }

    #[gpui_kit::test]
    fn gpu_optional_row_split(cx: &mut TestAppContext) {
        let mut r = nvidia(cx);
        set_optional(&mut r, &[GpuOptionalGraph::Power, GpuOptionalGraph::Clocks]);
        let engines = width(&mut r, ENGINES_ID);
        let half = (engines - 8.) / 2.;
        assert_eq!(width(&mut r, POWER_ID), half);
        assert_eq!(width(&mut r, CLOCKS_ID), half);
    }

    #[gpui_kit::test]
    fn gpu_optional_row_absent(cx: &mut TestAppContext) {
        let mut r = nvidia(cx);
        assert!(r.vcx.debug_bounds(POWER_ID).is_none());
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_none());
        r.vcx.update(|_, cx| {
            let page = r.page.read(cx);
            assert!(page.power().is_none() && page.clocks().is_none());
        });
        set_optional(&mut r, &[GpuOptionalGraph::Power]);
        set_optional(&mut r, &[]);
        assert!(r.vcx.debug_bounds(POWER_ID).is_none(), "dropped again");
        r.vcx
            .update(|_, cx| assert!(r.page.read(cx).power().is_none()));
    }

    #[gpui_kit::test]
    fn gpu_power_value_and_tooltip(cx: &mut TestAppContext) {
        let mut r = nvidia(cx);
        set_optional(&mut r, &[GpuOptionalGraph::Power]);
        r.vcx.update(|_, cx| {
            let page = r.page.read(cx);
            let view = page.power().expect("power chart").read(cx);
            assert_eq!(view.value_text().map(|t| t.as_ref()), Some("31.9 / 300 W"));
            assert_eq!(
                page.power_tooltip().map(|t| t.as_ref()),
                Some("Limit range 100 to 350 W · 1.24 kWh since boot")
            );
        });
        // Without the lowest limit the PERF-067 form shows.
        let mut t = full_totals();
        let mut info = (*t.info).clone();
        info.power_min_limit_w = None;
        t.info = Arc::new(info);
        publish_totals(&mut r, &t);
        r.vcx.update(|_, cx| {
            assert_eq!(
                r.page.read(cx).power_tooltip().map(|t| t.as_ref()),
                Some("max 350 W · 1.24 kWh since boot")
            );
        });
    }

    #[gpui_kit::test]
    fn gpu_clocks_tooltip(cx: &mut TestAppContext) {
        let mut r = nvidia(cx);
        set_optional(&mut r, &[GpuOptionalGraph::Clocks]);
        r.vcx.update(|_, cx| {
            let page = r.page.read(cx);
            let view = page.clocks().expect("clocks chart").read(cx);
            assert_eq!(
                view.value_text().map(|t| t.as_ref()),
                Some("1905 / 3105 MHz")
            );
            assert_eq!(page.clocks_tooltip().map(|t| t.as_ref()), Some("P3"));
        });
        let mut t = full_totals();
        t.clock_event_reasons = Metric::valid(ClockEventReasons::POWER_CAP);
        publish_totals(&mut r, &t);
        r.vcx.update(|_, cx| {
            assert_eq!(
                r.page.read(cx).clocks_tooltip().map(|t| t.as_ref()),
                Some("P3 · Throttled: power cap")
            );
        });
        t.clock_event_reasons =
            Metric::valid(ClockEventReasons::POWER_CAP | ClockEventReasons::HW_THERMAL);
        publish_totals(&mut r, &t);
        r.vcx.update(|_, cx| {
            assert_eq!(
                r.page.read(cx).clocks_tooltip().map(|t| t.as_ref()),
                Some("P3 · Throttled: power cap, hardware thermal")
            );
        });
    }

    #[gpui_kit::test]
    fn gpu_toggle_optional_graph(cx: &mut TestAppContext) {
        let r = nvidia(cx);
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
        r.vcx.dispatch_action(PerfToggleGpuOptionalGraph {
            key: DeviceKey(Arc::from("gpu:GPU-0100")),
            which: GpuOptionalGraph::Clocks,
        });
        draw(r.vcx);
        r.vcx.update(|_, cx| {
            let list = TlSettings::settings(cx)
                .performance
                .gpu_optional_graphs
                .clone();
            assert_eq!(list, [GpuOptionalGraph::Clocks]);
            assert!(r.page.read(cx).clocks().is_some());
        });
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_some());
        r.vcx.dispatch_action(PerfToggleGpuOptionalGraph {
            key: DeviceKey(Arc::from("gpu:GPU-0100")),
            which: GpuOptionalGraph::Clocks,
        });
        draw(r.vcx);
        assert!(r.vcx.debug_bounds(CLOCKS_ID).is_none());
    }
}
