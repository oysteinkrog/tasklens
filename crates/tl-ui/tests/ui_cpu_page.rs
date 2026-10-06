//! CPU page: the Details grid rows (E24, UI-PRF-024; PERF-033 to PERF-036, PERF-039, PERF-040).

use std::sync::Arc;

use gpui_kit::TestAppContext;
use smallvec::smallvec;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{CacheInfo, CacheLevelInfo, CpuStaticInfo, SystemSnapshot};
use tl_model::system::{
    CoreInfo, CoreKind, CpuSet, CpuTopology, Metric, SensorKind, SensorReading,
};
use tl_model::validity::Validity;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::cpu_details::{
    CpuLiveInputs, CpuSensor, ROW_L1, ROW_L2, ROW_L3, ROW_LOAD_AVERAGE, ROW_SPEED, ROW_TEMPERATURE,
    ROW_UP_TIME, cpu_details_model, cpu_temp_sensors, update_live, update_static,
};
use tl_ui::pages::performance::cpu_grid::{cell_specs, heading_tooltip};
use tl_ui::pages::performance::details::{KeyValueModel, KvRow};

const S: u64 = 1_000_000_000;

fn row<'a>(model: &'a KeyValueModel, label: &str) -> &'a KvRow {
    model
        .rows
        .iter()
        .find(|r| r.label == label)
        .expect("row exists")
}

fn value(model: &KeyValueModel, label: &str) -> String {
    row(model, label).value.to_string()
}

fn tip(model: &KeyValueModel, label: &str) -> Option<String> {
    row(model, label).tooltip.as_ref().map(|t| t.to_string())
}

/// A snapshot with every CPU value Valid, observed at 10 s.
fn snapshot() -> SystemSnapshot {
    let mut sys = SystemSnapshot::default();
    sys.header.validity = Validity::Valid;
    sys.header.observed_end = 10 * S;
    let c = &mut sys.cpu;
    c.util_pct = Metric::valid(18.6);
    c.kernel_pct = Metric::valid(2.3);
    c.freq_mhz = Metric::valid(4631);
    c.freq_fastest_mhz = Metric::valid(5050);
    c.base_mhz = Metric::valid(4200);
    c.temp_c = Metric::valid(52.0);
    c.processes = Metric::valid(295);
    c.threads = Metric::valid(5596);
    c.handles = Metric::valid(253_248);
    c.load_avg = Metric::valid([1.24, 0.98, 0.87]);
    c.sockets = Metric::valid(1);
    sys.boot_time_s = 1_790_000_000;
    sys.sensors = Arc::from(
        [(3, 52.0), (4, 48.0), (5, 50.0)]
            .map(|(i, c)| {
                (
                    DeviceIdx(i),
                    SensorReading {
                        device: DeviceIdx(i),
                        kind: SensorKind::Temperature,
                        value: Metric::valid(c),
                        pwm_pct: Metric::with(Validity::Unsupported),
                        fan_control: None,
                        fan_connected: false,
                    },
                )
            })
            .as_slice(),
    );
    sys
}

fn sensor_entry(idx: u16, chip: &str, label: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity: DeviceIdentity::Sensor {
            chip: Arc::from(chip),
            label: Arc::from(label),
        },
        generation: 0,
        display_name: Arc::from(label),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
    }
}

/// k10temp sensors at 3, 4 and 5, and a board chip at 2 that is not the CPU's.
fn sensors() -> Vec<CpuSensor> {
    let catalog = DeviceCatalog {
        version: 1,
        entries: Arc::from(vec![
            DeviceEntry {
                idx: DeviceIdx(0),
                identity: DeviceIdentity::CpuPackage,
                generation: 0,
                display_name: Arc::from("CPU"),
                present: true,
                parent: None,
                persistent_key: Arc::from("cpu"),
                net_kind: None,
            },
            sensor_entry(1, "coretemp", "Package id 0"),
            sensor_entry(2, "nct6799", "SYSTIN"),
            sensor_entry(3, "k10temp", "Tctl"),
            sensor_entry(4, "k10temp", "Tccd1"),
            sensor_entry(5, "k10temp", "Tccd2"),
        ]),
    };
    let mut out = Vec::new();
    cpu_temp_sensors(&catalog, &mut out);
    out
}

fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
    Some(CacheLevelInfo {
        instances,
        instance_kib,
        total_kib: u64::from(instances) * instance_kib,
    })
}

#[gpui_kit::test]
fn cpu_details_order_p1(cx: &mut TestAppContext) {
    let model = cpu_details_model();
    cx.update(|cx| {
        init_strings(cx, "en-US");
        model.refresh_labels(cx);
    });
    let labels: Vec<String> = model.to_kv_rows().map(|r| r.label.to_string()).collect();
    assert_eq!(
        labels,
        [
            "Utilization",
            "Kernel time",
            "Speed",
            "Processes",
            "Threads",
            "Handles",
            "Up time",
            "Base speed",
            "Sockets",
            "Temperature",
            "Load average",
            "Cores",
            "Logical processors",
            "L1 cache",
            "L2 cache",
            "L3 cache",
        ]
    );
}

#[test]
fn cpu_uptime_and_loadavg_formats() {
    let mut model = cpu_details_model();
    let sys = snapshot();
    let s = sensors();
    let inp = |now| CpuLiveInputs {
        now_boot_ns: now,
        temp_sensors: &s,
        ..CpuLiveInputs::default()
    };
    assert!(update_live(&mut model, &sys, &inp(180_095 * S)));
    assert_eq!(value(&model, ROW_UP_TIME), "2 days, 2h 1m 35s");
    let boot = tip(&model, ROW_UP_TIME).expect("boot time tooltip");
    assert!(boot.starts_with("Boot time "), "{boot}");
    assert_eq!(boot.len(), "Boot time 2026-09-21 13:46:40".len(), "{boot}");
    update_live(&mut model, &sys, &inp(2_295 * S));
    assert_eq!(value(&model, ROW_UP_TIME), "0h 38m 15s");

    assert_eq!(value(&model, ROW_LOAD_AVERAGE), "1.24 0.98 0.87");
    assert_eq!(
        tip(&model, ROW_LOAD_AVERAGE).as_deref(),
        Some("1, 5 and 15 minutes")
    );

    // The other live rows, as UI-PRF-024 shows them.
    let shown: Vec<String> = model.rows[..9]
        .iter()
        .map(|r| r.value.to_string())
        .collect();
    assert_eq!(
        shown,
        [
            "18.6%",
            "2.3%",
            "4631 MHz",
            "295",
            "5\u{2009}596",
            "253\u{2009}248",
            "0h 38m 15s",
            "4200 MHz",
            "1",
        ]
    );
    assert_eq!(
        tip(&model, ROW_SPEED).as_deref(),
        Some("Fastest core: 5050 MHz")
    );
    assert_eq!(value(&model, ROW_TEMPERATURE), "52 °C");
    assert_eq!(
        tip(&model, ROW_TEMPERATURE).as_deref(),
        Some("Tctl 52 °C · Tccd1 48 °C · Tccd2 50 °C")
    );
}

#[test]
fn cache_tooltips() {
    let mut model = cpu_details_model();
    let info = CpuStaticInfo {
        cores: 8,
        logical: 16,
        caches: CacheInfo {
            l1d: level(16, 32),
            l1i: level(16, 32),
            l2: level(8, 1024),
            l3: level(1, 98_304),
            l4: None,
        },
        ..CpuStaticInfo::default()
    };
    assert!(update_static(&mut model, &info));
    assert_eq!(value(&model, ROW_L1), "1 MiB");
    assert_eq!(
        tip(&model, ROW_L1).as_deref(),
        Some("16 × 32 KiB data + 16 × 32 KiB instruction")
    );
    assert_eq!(value(&model, ROW_L2), "8 MiB");
    assert_eq!(tip(&model, ROW_L2).as_deref(), Some("8 × 1 MiB"));
    assert_eq!(value(&model, ROW_L3), "96 MiB");
    assert_eq!(
        tip(&model, ROW_L3).as_deref(),
        Some("98\u{2009}304 KiB · 1 × 96 MiB")
    );
    assert_eq!(value(&model, "perf-cpu-row-cores"), "8");
    assert_eq!(value(&model, "perf-cpu-row-logical-processors"), "16");
    assert!(!update_static(&mut model, &info), "same info: no change");

    let with_l4 = CpuStaticInfo {
        caches: CacheInfo {
            l4: level(1, 131_072),
            ..info.caches
        },
        ..info
    };
    assert!(update_static(&mut model, &with_l4));
    assert_eq!(
        tip(&model, ROW_L3).as_deref(),
        Some("98\u{2009}304 KiB · 1 × 96 MiB · L4 128 MiB")
    );
}

#[test]
fn cpu_temperature_hidden_or_unavailable() {
    let mut model = cpu_details_model();
    let mut sys = snapshot();
    sys.cpu.temp_c = Metric::with(Validity::Unsupported);
    update_live(&mut model, &sys, &CpuLiveInputs::default());
    assert!(row(&model, ROW_TEMPERATURE).hidden);
    assert!(
        !model
            .to_kv_rows()
            .any(|r| r.label.as_ref() == ROW_TEMPERATURE)
    );

    sys.cpu.temp_c = Metric::with(Validity::Failed);
    let inp = CpuLiveInputs {
        cpu_error: Some("EIO"),
        ..CpuLiveInputs::default()
    };
    assert!(update_live(&mut model, &sys, &inp));
    assert!(!row(&model, ROW_TEMPERATURE).hidden);
    assert_eq!(value(&model, ROW_TEMPERATURE), "Unavailable");
    assert_eq!(
        tip(&model, ROW_TEMPERATURE).as_deref(),
        Some("Could not read: EIO")
    );
}

#[test]
fn cpu_details_update_is_change_only() {
    let mut model = cpu_details_model();
    let sys = snapshot();
    let s = sensors();
    let inp = CpuLiveInputs {
        now_boot_ns: 100 * S,
        temp_sensors: &s,
        ..CpuLiveInputs::default()
    };
    assert!(update_live(&mut model, &sys, &inp));
    assert!(!update_live(&mut model, &sys, &inp));

    // A new up time second changes only that row.
    let later = CpuLiveInputs {
        now_boot_ns: 101 * S,
        ..inp
    };
    assert!(update_live(&mut model, &sys, &later));
    assert!(!update_live(&mut model, &sys, &later));

    // Stale keeps the value; the age tooltip changes as time passes.
    let mut stale = sys.clone();
    stale.cpu.util_pct.validity = Validity::Stale;
    assert!(update_live(&mut model, &stale, &later));
    assert_eq!(
        row(&model, "perf-cpu-row-utilization").validity,
        Validity::Stale
    );
    assert_eq!(value(&model, "perf-cpu-row-utilization"), "18.6%");
    assert!(!update_live(&mut model, &stale, &later));
}

#[test]
fn cpu_speed_bogomips_marker() {
    let mut model = cpu_details_model();
    let mut sys = snapshot();
    sys.cpu.freq_mhz = Metric::valid(4200);
    sys.cpu.freq_is_bogomips = true;
    update_live(&mut model, &sys, &CpuLiveInputs::default());
    assert_eq!(value(&model, ROW_SPEED), "4200 MHz (?)");
    assert_eq!(
        tip(&model, ROW_SPEED).as_deref(),
        Some("Reported by the kernel at boot; the current clock is not available")
    );

    // The real clock comes back: the marker and the tooltip go.
    sys.cpu.freq_is_bogomips = false;
    update_live(&mut model, &sys, &CpuLiveInputs::default());
    assert_eq!(value(&model, ROW_SPEED), "4200 MHz");
    assert_eq!(
        tip(&model, ROW_SPEED).as_deref(),
        Some("Fastest core: 5050 MHz")
    );
}

// Per-core grid (E24, UI-PRF-002, PERF-028).

/// `n` logical CPUs on `n / 2` cores, core `c` holding CPUs `[c, c + n / 2]`, all online.
fn topology(n: u16) -> CpuTopology {
    let half = n / 2;
    let mut online = CpuSet::empty();
    for id in 0..n {
        online.insert(id);
    }
    CpuTopology {
        online,
        cores: (0..half)
            .map(|c| CoreInfo {
                id: c,
                logical: smallvec![c, c + half],
                l3_group: 0,
                kind: CoreKind::Standard,
            })
            .collect(),
    }
}

/// `DeviceIdx(100 + id)` per logical CPU id.
fn logical_devices(n: u16) -> Vec<DeviceIdx> {
    (0..n).map(|id| DeviceIdx(100 + id)).collect()
}

#[test]
fn cpu_grid_offline_cell() {
    let mut topo = topology(16);
    topo.online.remove(5);
    let mut cells = Vec::new();
    cell_specs(&topo, &logical_devices(16), false, &mut cells);
    assert_eq!(cells.len(), 16);
    assert!(cells[5].offline);
    assert_eq!(cells.iter().filter(|c| c.offline).count(), 1);
    assert_eq!(cells[5].heading.as_ref(), "CPU 5");
    assert_eq!(
        cells[5].fill_series,
        SeriesId {
            device: DeviceIdx(105),
            metric: MetricId::CPU_TOTAL
        }
    );
}

#[test]
fn cpu_grid_heading_tooltip_text() {
    let mut out = String::from("old text");
    heading_tooltip(3, 1, Some(4200), &mut out);
    assert_eq!(out, "Core 3, thread 1, 4200 MHz");
    heading_tooltip(3, 1, None, &mut out);
    assert_eq!(out, "Core 3, thread 1");

    // Core 3 holds CPUs [3, 11]: CPU 11 is its thread 1.
    let mut cells = Vec::new();
    cell_specs(&topology(16), &logical_devices(16), false, &mut cells);
    assert_eq!(cells[11].tooltip.as_deref(), Some("Core 3, thread 1"));
    assert_eq!(cells[3].tooltip.as_deref(), Some("Core 3, thread 0"));
}

#[cfg(feature = "test-hooks")]
mod grid {
    use gpui_kit::{
        App, AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use tl_model::client::EngineClient;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::chart::LegendStyle;
    use tl_ui::chart::grid_element::grid_layout;
    use tl_ui::components::tooltip::DELAY_CONTROL;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::pages::performance::cpu_grid::{CpuGrid, GRID_ID, TRIGGER_ID};
    use tl_ui::testing::fake_engine::FakeEngine;

    use super::*;

    /// A grid over a fake engine for `n` CPUs.
    fn grid(cx: &mut TestAppContext, n: u16, kernel: bool) -> CpuGrid {
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
            let client: Arc<dyn EngineClient> = FakeEngine::new();
            let hub: Entity<SnapshotHub> = cx.new(|cx| SnapshotHub::new(client, cx));
            let theme = tl_ui::theme::current(cx).clone();
            CpuGrid::new(
                &hub,
                Arc::new(topology(n)),
                &logical_devices(n),
                kernel,
                60 * S,
                false,
                &theme,
                cx,
            )
        })
    }

    fn legend_len(grid: &CpuGrid, cx: &App) -> usize {
        grid.view.read(cx).spec.legend.len()
    }

    #[gpui_kit::test]
    fn cpu_grid_16_cores_6_cols(cx: &mut TestAppContext) {
        let mut cells = Vec::new();
        cell_specs(&topology(16), &logical_devices(16), true, &mut cells);
        assert_eq!(cells.len(), 16);
        let headings: Vec<&str> = cells.iter().map(|c| c.heading.as_ref()).collect();
        let want: Vec<String> = (0..16).map(|i| format!("CPU {i}")).collect();
        assert_eq!(headings, want);
        assert!(cells.iter().all(|c| c.overlay_series.is_some()));
        assert!(cells.iter().all(|c| !c.offline));
        assert_eq!(
            cells[7].overlay_series,
            Some(SeriesId {
                device: DeviceIdx(107),
                metric: MetricId::CPU_KERNEL
            })
        );
        assert_eq!(grid_layout(16, px(1000.), px(120.), px(8.)).cols, 6);

        let grid = grid(cx, 16, true);
        cx.read(|cx| {
            let spec = &grid.view.read(cx).spec;
            assert_eq!(spec.cells, cells);
            assert_eq!(spec.legend.len(), 2);
            assert_eq!(spec.legend[0].0.as_ref(), "Total");
            assert_eq!(spec.legend[1].0.as_ref(), "Kernel");
            assert_eq!(spec.legend[1].1, LegendStyle::Dashed);
            assert_eq!(
                (spec.cell_min_w, spec.cell_h, spec.gap),
                (px(120.), px(112.), px(8.))
            );
        });
    }

    #[gpui_kit::test]
    fn cpu_grid_kernel_toggle_changes_overlays(cx: &mut TestAppContext) {
        let mut grid = grid(cx, 16, true);
        cx.update(|cx| grid.set_kernel(false, cx));
        cx.read(|cx| {
            let spec = &grid.view.read(cx).spec;
            assert!(spec.cells.iter().all(|c| c.overlay_series.is_none()));
            assert_eq!(legend_len(&grid, cx), 1);
        });
        cx.update(|cx| grid.set_kernel(true, cx));
        cx.read(|cx| {
            let spec = &grid.view.read(cx).spec;
            assert!(spec.cells.iter().all(|c| c.overlay_series.is_some()));
            assert_eq!(legend_len(&grid, cx), 2);
        });

        let mut cells = Vec::new();
        cell_specs(&topology(16), &logical_devices(16), false, &mut cells);
        assert!(cells.iter().all(|c| c.overlay_series.is_none()));
    }

    #[gpui_kit::test]
    fn cpu_grid_tooltips_reformat_only_on_change(cx: &mut TestAppContext) {
        let mut grid = grid(cx, 16, false);
        let ptrs = |grid: &CpuGrid, cx: &mut TestAppContext| -> Vec<usize> {
            cx.read(|cx| {
                grid.view
                    .read(cx)
                    .spec
                    .cells
                    .iter()
                    .map(|c| c.tooltip.as_ref().map_or(0, |t| t.as_ptr() as usize))
                    .collect()
            })
        };
        let tip = |grid: &CpuGrid, i: usize, cx: &mut TestAppContext| -> Option<String> {
            cx.read(|cx| {
                grid.view.read(cx).spec.cells[i]
                    .tooltip
                    .as_ref()
                    .map(|t| t.to_string())
            })
        };
        let clocks: Vec<(u16, f32, Validity)> = (0..16)
            .map(|id| (id, 4000.0 + f32::from(id) * 10.0, Validity::Valid))
            .collect();

        cx.update(|cx| grid.update_clocks(&clocks, cx));
        assert_eq!(
            tip(&grid, 11, cx).as_deref(),
            Some("Core 3, thread 1, 4110 MHz")
        );
        let first = ptrs(&grid, cx);
        assert!(first.iter().all(|&p| p != 0));

        // The same values again: no tooltip is re-formatted.
        cx.update(|cx| grid.update_clocks(&clocks, cx));
        assert_eq!(ptrs(&grid, cx), first);

        // One CPU changes: only its tooltip is new.
        let mut changed = clocks.clone();
        changed[11].1 = 4200.0;
        cx.update(|cx| grid.update_clocks(&changed, cx));
        let second = ptrs(&grid, cx);
        assert_eq!(
            tip(&grid, 11, cx).as_deref(),
            Some("Core 3, thread 1, 4200 MHz")
        );
        for i in 0..16 {
            assert_eq!(second[i] == first[i], i != 11, "cell {i}");
        }

        // A clock that is not Valid drops the MHz part.
        changed[11].2 = Validity::Stale;
        cx.update(|cx| grid.update_clocks(&changed, cx));
        assert_eq!(tip(&grid, 11, cx).as_deref(), Some("Core 3, thread 1"));
    }

    struct Root {
        grid: CpuGrid,
    }

    impl Render for Root {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1300.)).child(self.grid.element(window, cx))
        }
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    fn hover(vcx: &mut VisualTestContext, x: f32, y: f32) {
        vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
        vcx.run_until_parked();
        draw(vcx);
    }

    #[gpui_kit::test]
    fn cpu_grid_heading_hover_shows_tooltip(cx: &mut TestAppContext) {
        let grid = grid(cx, 16, false);
        let (root, vcx) = cx.add_window_view(move |_, _| Root { grid });
        vcx.simulate_scale_factor_change(1.0);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        root.update(vcx, |r, cx| {
            r.grid.update_clocks(&[(7, 4200.0, Validity::Valid)], cx);
        });
        draw(vcx);
        draw(vcx);

        // 16 cells in 6 columns of 210 px and 3 rows of 112 px with 8 px gaps.
        let bounds = vcx.debug_bounds(GRID_ID).expect("grid box");
        assert_eq!(bounds.size.height, px(352.));
        assert!(
            vcx.debug_bounds(TRIGGER_ID).is_none(),
            "no trigger before hover"
        );

        // Cell 7 is column 1, row 1, at (218, 120); its heading starts 6 px in.
        hover(vcx, 218. + 12., 120. + 14.);
        hover(vcx, 218. + 14., 120. + 14.);
        let trigger = vcx
            .debug_bounds(TRIGGER_ID)
            .expect("trigger over the heading");
        assert_eq!(trigger.origin, point(px(224.), px(126.)));
        assert!(
            vcx.debug_bounds("TlTooltip").is_none(),
            "not before the delay"
        );

        vcx.executor().advance_clock(DELAY_CONTROL * 2);
        vcx.run_until_parked();
        draw(vcx);
        assert!(vcx.debug_bounds("TlTooltip").is_some(), "tooltip shows");
        let text = vcx.update(|_, cx| {
            root.read(cx)
                .grid
                .view
                .read(cx)
                .hovered_tooltip()
                .map(|t| t.to_string())
        });
        assert_eq!(text.as_deref(), Some("Core 7, thread 0, 4200 MHz"));

        // Off every heading: the trigger goes.
        hover(vcx, 218. + 100., 120. + 80.);
        draw(vcx);
        assert!(vcx.debug_bounds(TRIGGER_ID).is_none());
    }
}

// CPU page entity (E24, UI-PRF-023, UI-PRF-053; ADR-008, R-126).

#[cfg(feature = "test-hooks")]
mod page {
    use gpui_kit::{
        AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::client::EngineClient;
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::settings::{AppearanceSettings, CpuGraphMode, Settings, ThemeSettings, UiState};
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::chart::grid_element::grid_layout;
    use tl_ui::components::kv_grid;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::pages::performance::actions::{PerfSetCpuGraphMode, PerfToggleCpuKernel};
    use tl_ui::pages::performance::cpu::{
        CPU_EXTRA_MODES_ENABLED, CpuPage, OVERALL_ID, kernel_label, mode_items, mode_label,
    };
    use tl_ui::pages::performance::cpu_grid::GRID_ID;
    use tl_ui::pages::performance::page::DevicePage as _;
    use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;

    use super::*;

    struct Host(Entity<CpuPage>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1000.)).h(px(2000.)).child(self.0.clone())
        }
    }

    fn cpu_entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("cpu"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
        }
    }

    struct Rig<'a> {
        page: Entity<CpuPage>,
        demand: Entity<DemandAggregator>,
        vcx: &'a mut VisualTestContext,
    }

    /// A CPU page over 16 logical CPUs (package `DeviceIdx(0)`, CPU `id` at `100 + id`) in a
    /// 1000 px wide window, with `settings`.
    fn open(cx: &mut TestAppContext, settings: Settings) -> Rig<'_> {
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            let mut entries = vec![cpu_entry(0, DeviceIdentity::CpuPackage)];
            entries
                .extend((0..16).map(|id| cpu_entry(100 + id, DeviceIdentity::CpuLogical { id })));
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(entries),
            });
            Arc::make_mut(&mut s.system).cpu_topology = Arc::new(topology(16));
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
        let key = DeviceKey(Arc::from(DeviceKey::CPU));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::Cpu,
            idx: DeviceIdx(0),
            generation: 0,
            net_class: None,
        };
        let (h, d) = (hub.clone(), demand.clone());
        let page = cx.new(|cx| CpuPage::new(key, row, h, d, cx));
        let host = page.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| Host(host));
        vcx.simulate_scale_factor_change(1.0);
        let focus = vcx.update(|_, cx| page.read(cx).focus_handle(cx));
        vcx.update(|window, cx| focus.focus(window, cx));
        draw(vcx);
        Rig { page, demand, vcx }
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
    fn cpu_mode_button_labels(cx: &mut TestAppContext) {
        let mut s = Settings::default();
        s.performance.cpu_graph_mode = CpuGraphMode::PerCore;
        let r = open(cx, s);
        let page = r.page.clone();
        let mode = r.vcx.update(|_, cx| page.read(cx).mode());
        assert_eq!(mode_label(mode), "Per core");
        assert!(r.vcx.debug_bounds("TlButton-perf-cpu-mode").is_some());
        assert!(r.vcx.debug_bounds(OVERALL_ID).is_none());

        // 1000 px less 16 px padding on each side: 6 columns, 3 rows of 112 px, 8 px gaps.
        let grid = r.vcx.debug_bounds(GRID_ID).expect("grid shown");
        assert_eq!(grid.size.width, px(968.));
        assert_eq!(grid_layout(16, grid.size.width, px(120.), px(8.)).cols, 6);
        assert_eq!(grid.size.height, px(352.));

        let labels: Vec<&str> = mode_items().iter().map(|i| i.label).collect();
        assert_eq!(
            labels,
            [
                "Overall",
                "Per core",
                "All cores overlaid",
                "All cores stacked"
            ]
        );
        assert_eq!(kernel_label(true), "Kernel on");
        assert_eq!(kernel_label(false), "Kernel off");
    }

    #[gpui_kit::test]
    fn cpu_demand_percore_only_when_needed(cx: &mut TestAppContext) {
        let mut r = open(cx, Settings::default());
        set_active(&mut r, true);
        assert_eq!(merged(&mut r).cpu, CpuDemand::PerCore);

        r.vcx.dispatch_action(PerfSetCpuGraphMode {
            mode: CpuGraphMode::Overall,
        });
        draw(r.vcx);
        let page = r.page.clone();
        let (mode, setting) = r.vcx.update(|_, cx| {
            (
                page.read(cx).mode(),
                TlSettings::settings(cx).performance.cpu_graph_mode,
            )
        });
        assert_eq!(mode, CpuGraphMode::Overall);
        assert_eq!(setting, CpuGraphMode::Overall);
        assert!(r.vcx.debug_bounds(OVERALL_ID).is_some(), "overall chart");
        assert!(r.vcx.debug_bounds(GRID_ID).is_none(), "no grid");
        assert_eq!(merged(&mut r).cpu, CpuDemand::Total);

        r.vcx.dispatch_action(PerfSetCpuGraphMode {
            mode: CpuGraphMode::PerCore,
        });
        assert_eq!(merged(&mut r).cpu, CpuDemand::PerCore);
    }

    #[gpui_kit::test]
    fn cpu_kernel_toggle_updates_specs(cx: &mut TestAppContext) {
        let r = open(cx, Settings::default());
        let page = r.page.clone();
        let read = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                let p = page.read(cx);
                (
                    p.kernel(),
                    p.overall().read(cx).spec.series.len(),
                    p.overall().read(cx).labels.legend.len(),
                    p.grid()
                        .view
                        .read(cx)
                        .spec
                        .cells
                        .iter()
                        .all(|c| c.overlay_series.is_none()),
                )
            })
        };
        assert_eq!(read(r.vcx), (true, 2, 2, false));

        r.vcx.dispatch_action(PerfToggleCpuKernel);
        let (kernel, series, legend, no_overlays) = read(r.vcx);
        assert_eq!(kernel_label(kernel), "Kernel off");
        assert_eq!((series, legend), (1, 1));
        assert!(no_overlays);
        let setting = r
            .vcx
            .update(|_, cx| TlSettings::settings(cx).performance.cpu_kernel);
        assert!(!setting);

        r.vcx.dispatch_action(PerfToggleCpuKernel);
        assert_eq!(read(r.vcx), (true, 2, 2, false));
    }

    #[gpui_kit::test]
    fn cpu_fast_on_with_smooth_off(cx: &mut TestAppContext) {
        let mut s = Settings::default();
        s.motion.smooth_graphs = false;
        let mut r = open(cx, s);
        set_active(&mut r, true);
        let d = merged(&mut r);
        assert!(d.cpu_fast);
        assert!(d.any_surface_visible);
        assert!(!d.memory);
    }

    #[gpui_kit::test]
    fn cpu_page_inactive_contributes_no_demand(cx: &mut TestAppContext) {
        let mut r = open(cx, Settings::default());
        assert_eq!(merged(&mut r), Demand::default(), "never active");
        set_active(&mut r, true);
        assert_ne!(merged(&mut r), Demand::default());
        set_active(&mut r, false);
        assert_eq!(merged(&mut r), Demand::default());
    }

    #[gpui_kit::test]
    fn bench_show_both_layout(cx: &mut TestAppContext) {
        let r = open(cx, Settings::default());
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.update(cx, |p, cx| {
                p.bench_show_both = true;
                cx.notify();
            })
        });
        draw(r.vcx);
        let overall = r.vcx.debug_bounds(OVERALL_ID).expect("overall chart");
        let grid = r.vcx.debug_bounds(GRID_ID).expect("grid");
        assert_eq!(overall.size.height, px(300.));
        assert!(grid.top() > overall.bottom(), "grid below the chart");
    }

    #[gpui_kit::test]
    fn cpu_extra_modes_disabled_before_p2(cx: &mut TestAppContext) {
        const { assert!(!CPU_EXTRA_MODES_ENABLED) };
        let items = mode_items();
        for item in &items[2..] {
            assert!(!item.enabled, "{:?}", item.mode);
            assert_eq!(item.tooltip, Some("Available in a later version"));
        }
        assert!(items[..2].iter().all(|i| i.enabled && i.tooltip.is_none()));

        // The action is ignored for a disabled mode.
        let r = open(cx, Settings::default());
        r.vcx.dispatch_action(PerfSetCpuGraphMode {
            mode: CpuGraphMode::AllCoresOverlaid,
        });
        let page = r.page.clone();
        let mode = r.vcx.update(|_, cx| page.read(cx).mode());
        assert_eq!(mode, CpuGraphMode::PerCore);
    }
}
