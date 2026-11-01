//! A widget draws the same in a flow layout, in Edit Widgets and in a grid layout
//! (tasklens-6swdr.12, ADR-019 "Flow placement" and "Inner grid"). For each phase 2 box and each
//! preset, the sweep reads what every widget of the flow layout draws, enters Edit Widgets,
//! applies with no change (the flow layout stays), saves the pinned layout as a grid layout, and
//! checks each time that every widget kept its accent colour, its chart's series count and
//! legend, its value text, a core row's clock column, and its forms and options. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, Focusable as _, Hsla, IntoElement, Modifiers,
    ParentElement as _, Pixels, Render, ScrollDelta, ScrollWheelEvent, StyleRefinement,
    Styled as _, TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, GPU_BOX_CONTAINER, WidgetContent, WidgetSetting,
    reset_to_default,
};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::{DiskTotals, GpuTotals, LinkState, MemGroupBand, Metric, NetTotals};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::ChartView;
use tl_ui::chart::labels::LabelKind;
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{PerfOverviewSetBoxMode, PerfWidgetEditBox};
use tl_ui::pages::performance::memory_apps;
use tl_ui::pages::performance::overview::cpu_box::core_chart_id;
use tl_ui::pages::performance::overview::mem_box::MemRowsRef;
use tl_ui::pages::performance::overview::parts::{CoreCellFit, core_cell};
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::overview::{disk_box, net_box, timeline};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::WidgetContainer;
use tl_ui::pages::performance::widget::edit::{
    WidgetEditor, corner_id, editor_id, frame_id, gear_id, grip_id,
};
use tl_ui::pages::performance::widget::group_widget::GroupFlowLook;
use tl_ui::pages::performance::widget::metric_widget::{FlowLook, part_id, widget_id};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

/// The first logical CPU's device index; core `n` is at `CORE0 + n`.
const CORE0: u16 = 100;
const CORES: u16 = 4;
const GIB: u64 = 1 << 30;
/// The GPU's device index, when the rig has one.
const GPU: u16 = 7;
/// sda and sdb: device index, read and write in bytes per second.
const DISKS: [(u16, f64, f64); 2] = [(3, 1_000_000., 300_000.), (4, 200_000., 40_000.)];

struct Host {
    page: Entity<OverviewPage>,
    sidebar: Entity<DeviceSidebar>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(
                self.page
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(
                div()
                    .absolute()
                    .w(px(0.))
                    .h(px(0.))
                    .overflow_hidden()
                    .child(self.sidebar.clone()),
            )
    }
}

fn entry(idx: u16, identity: DeviceIdentity, key: &str, net_kind: Option<NetKind>) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(if key.is_empty() { "dev" } else { key }),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind,
        hot_c: None,
    }
}

/// The CPU package at 0 with `CORES` logical CPUs at `CORE0..`, memory, two disks, eth0 and
/// the aggregates. No GPU.
fn catalog() -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage, "cpu", None),
        entry(1, DeviceIdentity::Memory, "memory", None),
    ];
    for (idx, name) in [(DISKS[0].0, "sda"), (DISKS[1].0, "sdb")] {
        entries.push(entry(
            idx,
            DeviceIdentity::Disk {
                major: 8,
                minor: u32::from(idx),
                name: Arc::from(name),
                wwn_or_serial: None,
            },
            &format!("disk:{name}"),
            None,
        ));
    }
    entries.push(entry(
        5,
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: 2,
            name: Arc::from("eth0"),
        },
        "net:eth0",
        Some(NetKind::Wired),
    ));
    entries.push(entry(
        50,
        DeviceIdentity::Aggregate(AggregateKind::Disks),
        "aggregate:disk",
        None,
    ));
    entries.push(entry(
        51,
        DeviceIdentity::Aggregate(AggregateKind::Nets),
        "aggregate:net",
        None,
    ));
    for id in 0..CORES {
        entries.push(entry(
            CORE0 + id,
            DeviceIdentity::CpuLogical { id },
            &format!("cpu:{id}"),
            None,
        ));
    }
    DeviceCatalog {
        version: 1,
        entries: Arc::from(entries),
    }
}

/// A GPU at `device` at 25% busy with 4 of 16 GiB VRAM used, drawing 120 W.
fn gpu_totals(device: u16) -> GpuTotals {
    GpuTotals {
        device: DeviceIdx(device),
        util_pct: Metric::valid(25.),
        mem_util_pct: Metric::default(),
        mem_used: Metric::valid(4 * GIB),
        mem_total: Metric::valid(16 * GIB),
        shared_used: Metric::default(),
        shared_total: Metric::default(),
        mem_reserved: Metric::default(),
        enc_pct: Metric::valid(0.),
        dec_pct: Metric::valid(0.),
        enc_sessions: Metric::default(),
        enc_dec_shared: false,
        temp_c: Metric::valid(52.),
        temp_edge_c: Metric::default(),
        temp_junction_c: Metric::default(),
        temp_mem_c: Metric::default(),
        power_w: Metric::valid(120.),
        power_limit_w: Metric::default(),
        clock_gfx_mhz: Metric::default(),
        clock_mem_mhz: Metric::default(),
        fan_pct: Metric::default(),
        pcie_rx_bps: Metric::default(),
        pcie_tx_bps: Metric::default(),
        engines: Default::default(),
        throttling: Default::default(),
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

/// One series with one valid sample of `v`.
fn series(device: u16, metric: MetricId, v: f32, version: u64) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id: SeriesId {
            device: DeviceIdx(device),
            metric,
        },
        version,
        period_ns: 100_000_000,
        samples: Arc::from([Sample::valid(now_boot_ns(), v)]),
    })
}

struct Rig<'a> {
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    _engine: Arc<FakeEngine>,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// The Overview page at 1920 × 1200 with every box, the kernel line on, memory totals, one
/// sample per core of load (41.7%) and clock (3.94 GHz, BogoMIPS), and each disk's and eth0's rates.
fn open(cx: &mut TestAppContext) -> Rig<'_> {
    open_with(cx, false)
}

/// [`open`], with `gpu` one GPU at `GPU` too, so the Overview shows its GPU box.
fn open_with(cx: &mut TestAppContext, gpu: bool) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        let mut devices = catalog();
        if gpu {
            let mut entries = devices.entries.to_vec();
            entries.push(entry(
                GPU,
                DeviceIdentity::Gpu {
                    uuid: [7; 16],
                    pci_bus_id: Arc::from("0000:07:00.0"),
                },
                "gpu:card",
                None,
            ));
            devices.entries = Arc::from(entries);
        }
        s.devices = Arc::new(devices);
        let sys = Arc::make_mut(&mut s.system);
        if gpu {
            sys.gpus = Arc::from([(DeviceIdx(GPU), gpu_totals(GPU))]);
        }
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        sys.memory.validity = Validity::Valid;
        // BogoMIPS clocks carry a mark that changes the clock column, so a widget made after
        // the first sample (Edit Widgets, Apply) must take it from the snapshot, not wait for a
        // notification.
        sys.cpu.freq_is_bogomips = true;
        sys.memory.total = 64 * GIB;
        sys.memory.used = 23 * GIB;
        sys.memory.available = 41 * GIB;
        sys.memory.cached = 10 * GIB;
        sys.memory.swap_total = 8 * GIB;
        sys.memory.swap_free = 6 * GIB;
        sys.memory.commit_limit = 40 * GIB;
        sys.memory.committed = 20 * GIB;
        // Each disk's and eth0's totals: they sum to the aggregate series below.
        sys.disks = DISKS
            .iter()
            .map(|&(idx, read, write)| {
                (
                    DeviceIdx(idx),
                    DiskTotals {
                        device: DeviceIdx(idx),
                        active_pct: Metric::valid(12.),
                        read_bps: Metric::valid(read),
                        write_bps: Metric::valid(write),
                        response_ms: Metric::valid(1.),
                        read_total: 0,
                        written_total: 0,
                        capacity: 0,
                        temp_c: Metric::default(),
                    },
                )
            })
            .collect();
        sys.nets = Arc::from([(
            DeviceIdx(5),
            NetTotals {
                device: DeviceIdx(5),
                rx_bps: Metric::valid(180_000.),
                tx_bps: Metric::valid(27_000.),
                rx_total: 0,
                tx_total: 0,
                link_speed_mbps: Metric::valid(1_000),
                state: LinkState::Up,
            },
        )]);
        let version = 1;
        let mut views = Vec::new();
        for id in 0..CORES {
            views.push(series(CORE0 + id, MetricId::CPU_TOTAL, 41.7, version));
            views.push(series(CORE0 + id, MetricId::CPU_FREQ, 3940.0, version));
        }
        // Each disk's and eth0's series, and the disk and network totals, so their widgets
        // have a series to draw.
        for &(idx, read, write) in &DISKS {
            views.push(series(idx, MetricId::DISK_READ, read as f32, version));
            views.push(series(idx, MetricId::DISK_WRITE, write as f32, version));
        }
        for (device, metric, v) in [
            (5, MetricId::NET_RX, 180_000.0),
            (5, MetricId::NET_TX, 27_000.0),
            (50, MetricId::DISK_READ, 1_200_000.0),
            (50, MetricId::DISK_WRITE, 340_000.0),
            (50, MetricId::DISK_ACTIVE, 12.0),
            (51, MetricId::NET_RX, 180_000.0),
            (51, MetricId::NET_TX, 27_000.0),
        ] {
            views.push(series(device, metric, v, version));
        }
        views.sort_by_key(|v| v.id);
        s.series = Arc::new(SeriesSet {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            series: views.into(),
            slow: Arc::from([]),
        });
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
        let mut settings = Settings::default();
        settings.motion.smooth_graphs = false;
        settings.performance.overview_core_smoothing = tl_model::settings::CoreSmoothing::Off;
        settings.performance.cpu_kernel = true;
        // Every box, the Top processes box too.
        settings.performance.overview = Some(OverviewLayoutSetting::with_top_procs(
            settings.performance.overview_row_unit_px,
        ));
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
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let sidebar = cx.new(|cx| DeviceSidebar::new(hub.clone(), demand.clone(), cx));
        let page = cx.new(|cx| {
            OverviewPage::new(
                DeviceKey(Arc::from(OVERVIEW_KEY)),
                hub,
                demand,
                &sidebar,
                cx,
            )
        });
        Host { page, sidebar }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1920.), px(1200.)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    draw(vcx);
    let page = vcx.read_entity(&host, |h, _| h.page.clone());
    Rig {
        page,
        vcx,
        _engine: engine,
        _host: host,
        _driver: driver,
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

/// What a widget, or one item of a group, draws that must not change with its placement.
/// `None` is a part the flow look does not show in a comparable form, and is left out of the
/// comparison.
#[derive(Clone, Debug, Default, PartialEq)]
struct Look {
    /// The line colour of the first series.
    accent: Option<Hsla>,
    /// The series count, over every chart the item draws.
    series: Option<usize>,
    /// Every series' line colour, in order.
    lines: Option<Vec<Hsla>>,
    /// The legend names, over every chart the item draws.
    legend: Option<Vec<String>>,
    value: Option<String>,
    /// The rate texts the value shows ("1.0 MB/s"), without their R, W, ↓ and ↑ marks.
    rates: Option<Vec<String>>,
    /// A core row's clock column: its text, while it shows.
    clock: Option<Option<String>>,
    /// A core row's parts, left to right.
    parts: Option<Vec<&'static str>>,
}

/// A widget's look: one item for a widget, one per drawn member for a group, with the group's
/// column count.
#[derive(Clone, Debug, Default, PartialEq)]
struct WidgetLook {
    cols: Option<usize>,
    items: Vec<Look>,
}

impl WidgetLook {
    fn one(look: Look) -> Self {
        Self {
            cols: None,
            items: vec![look],
        }
    }
}

/// The chart parts of a look drawn by `charts`, in order.
fn charts_look(charts: &[&ChartView]) -> Look {
    if charts.is_empty() {
        return Look::default();
    }
    let lines: Vec<Hsla> = charts
        .iter()
        .flat_map(|c| c.spec.series.iter().map(|s| s.line))
        .collect();
    Look {
        accent: lines.first().copied(),
        series: Some(lines.len()),
        legend: Some(
            charts
                .iter()
                .flat_map(|c| c.labels.legend.iter().map(|(n, _)| n.to_string()))
                .collect(),
        ),
        lines: Some(lines),
        ..Look::default()
    }
}

/// The rates of a value text: "R 1.0 MB/s  W 300 kB/s", "↓ 1.4 Mbps  ↑ 216 kbps" and
/// "1.4 Mbps / 216 kbps" each give their two rates.
fn rates(text: &str) -> Vec<String> {
    text.split("  ")
        .flat_map(|p| p.split(" / "))
        .map(|p| {
            p.trim()
                .trim_start_matches(['R', 'W', '↓', '↑'])
                .trim()
                .to_owned()
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// The parts of a member core row, in the order the flow look's `parts::core_cell` draws them.
const CORE_PARTS: [&str; 5] = ["name", "chart", "bar", "value", "clock"];

/// The phase 2 boxes: (kind, container id, presets).
const BOXES: [(&str, &str, &[&str]); 6] = [
    (
        "cpu",
        CPU_BOX_CONTAINER,
        &["chart_cores", "chart", "cores", "core_charts"],
    ),
    (
        "memory",
        "overview.memory",
        &["rows", "chart_rows", "chart", "charts", "apps"],
    ),
    (
        "disk",
        "overview.disk",
        &["disk_charts", "rows", "total_chart"],
    ),
    (
        "net",
        "overview.net",
        &["stacked", "columns", "combined", "adapter_charts"],
    ),
    ("processes", "overview.processes", &["list"]),
    ("timeline", "overview.timeline", &["bands"]),
];

impl Rig<'_> {
    fn page_action(&mut self, action: &dyn gpui_kit::Action) {
        let page = self.page.clone();
        self.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(action, window, cx);
        });
        draw(self.vcx);
    }

    /// The container and editor of box `kind`.
    fn parts(&mut self, kind: &str) -> (Entity<WidgetContainer>, Entity<WidgetEditor>) {
        let page = self.page.clone();
        self.vcx.update(|_, cx| {
            let p = page.read(cx);
            match kind {
                "cpu" => {
                    let b = p.cpu_box().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
                "memory" => {
                    let b = p.mem_box().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
                "disk" => {
                    let b = p.disk_box().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
                "net" => {
                    let b = p.net_box().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
                "processes" => {
                    let b = p.procs_frame().expect("a Top processes box").read(cx);
                    (b.container().clone(), b.editor().clone())
                }
                "gpu" => {
                    let b = p.gpu_box().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
                _ => {
                    let b = p.timeline_frame().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
            }
        })
    }

    /// The bounds of debug selector `id` at the last frame.
    fn bounds(&mut self, id: String) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        self.vcx.debug_bounds(Box::leak(id.into_boxed_str()))
    }

    /// The number of columns the drawn cells `ids` stand in: their distinct left edges.
    fn columns(&mut self, ids: impl IntoIterator<Item = String>) -> usize {
        let mut xs: Vec<f32> = ids
            .into_iter()
            .filter_map(|id| self.bounds(id).map(|b| f32::from(b.origin.x)))
            .collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        xs.len()
    }

    /// The settings and look of every placed widget of `container`, by id.
    fn looks(
        &mut self,
        container: &Entity<WidgetContainer>,
        container_id: &str,
    ) -> Vec<(WidgetSetting, WidgetLook)> {
        let ids = self.vcx.update(|_, cx| container.read(cx).placed_ids());
        let mut out = Vec::new();
        for id in ids {
            let setting = self
                .vcx
                .update(|_, cx| {
                    container
                        .read(cx)
                        .layout()
                        .widgets
                        .iter()
                        .find(|w| w.id == id)
                        .cloned()
                })
                .expect("a placed widget has a setting");
            let look = self.look(container, container_id, id);
            out.push((setting, look));
        }
        out
    }

    fn look(
        &mut self,
        container: &Entity<WidgetContainer>,
        container_id: &str,
        id: u32,
    ) -> WidgetLook {
        let (widget, group) = self.vcx.update(|_, cx| {
            let c = container.read(cx);
            (c.widget(id).cloned(), c.group(id).cloned())
        });
        if let Some(w) = widget {
            return WidgetLook::one(self.vcx.update(|_, cx| {
                let w = w.read(cx);
                let mem_value = |rows: &MemRowsRef, index: usize| {
                    Some(rows.0.borrow().rows[index].value.to_string())
                };
                match w.flow() {
                    Some(FlowLook::Chart(c)) => charts_look(&[c.read(cx)]),
                    Some(FlowLook::MemRow { rows, index, spark }) => Look {
                        value: mem_value(rows, *index),
                        ..charts_look(&spark.iter().map(|c| c.read(cx)).collect::<Vec<_>>())
                    },
                    Some(FlowLook::MemCell {
                        rows,
                        index,
                        chart: c,
                    }) => Look {
                        value: mem_value(rows, *index),
                        ..charts_look(&[c.read(cx)])
                    },
                    Some(FlowLook::NetDirection {
                        model,
                        upload,
                        chart: c,
                    }) => Look {
                        rates: Some(vec![
                            model.0.borrow().rates[usize::from(*upload)].to_string(),
                        ]),
                        ..charts_look(&[c.read(cx)])
                    },
                    Some(FlowLook::NetCombined { model, chart: c }) => Look {
                        rates: Some(
                            model
                                .0
                                .borrow()
                                .rates
                                .iter()
                                .map(|r| r.to_string())
                                .collect(),
                        ),
                        ..charts_look(&[c.read(cx)])
                    },
                    Some(_) => panic!("widget {id}: no GPU box look in these boxes"),
                    None => {
                        let value = w.texts()[1].to_string();
                        Look {
                            rates: Some(rates(&value)),
                            value: Some(value),
                            ..charts_look(
                                &w.chart()
                                    .map(|c| c.read(cx))
                                    .into_iter()
                                    .collect::<Vec<_>>(),
                            )
                        }
                    }
                }
            }));
        }
        let Some(g) = group else {
            // A fixed widget draws its own content in every layout.
            return WidgetLook::one(Look::default());
        };
        let flow = self.vcx.update(|_, cx| g.read(cx).flow_look().cloned());
        match flow {
            Some(GroupFlowLook::CoreRows {
                bars,
                sparks,
                layout,
                ..
            }) => {
                let layout = layout.get();
                // The parts `core_cell` places, left to right by where they start.
                let p = core_cell(&layout, 1.);
                let mut parts = vec![(0., "name")];
                if p.spark_w > px(0.) {
                    parts.push((f32::from(p.spark_x), "chart"));
                }
                if p.meter_w > px(0.) {
                    parts.push((f32::from(p.meter_x), "bar"));
                }
                parts.push((f32::from(p.pct_right), "value"));
                if p.ghz {
                    parts.push((f32::from(layout.cell_w), "clock"));
                }
                parts.sort_by(|a, b| a.0.total_cmp(&b.0));
                let parts: Vec<&'static str> = parts.into_iter().map(|(_, n)| n).collect();
                let bars = bars.borrow();
                let items = (0..sparks.len())
                    .map(|i| Look {
                        value: Some(bars.pct_label(i).to_owned()),
                        clock: Some(
                            (layout.fit == CoreCellFit::Full)
                                .then(|| bars.clock_labels()[i].to_string()),
                        ),
                        parts: Some(parts.clone()),
                        ..self.vcx.update(|_, cx| charts_look(&[sparks[i].read(cx)]))
                    })
                    .collect();
                WidgetLook {
                    cols: Some(layout.cols),
                    items,
                }
            }
            Some(GroupFlowLook::CoreCells { bars, sparks, .. }) => {
                let cols = self.columns((0..sparks.len()).map(core_chart_id));
                let bars = bars.borrow();
                let items = (0..sparks.len())
                    .map(|i| Look {
                        value: Some(bars.pct_label(i).to_owned()),
                        ..self.vcx.update(|_, cx| charts_look(&[sparks[i].read(cx)]))
                    })
                    .collect();
                WidgetLook {
                    cols: Some(cols),
                    items,
                }
            }
            Some(GroupFlowLook::DiskRows { model, .. }) => {
                // The sections stack in one column; a section draws read and write as two
                // sparklines.
                let rows: Vec<_> = {
                    let m = model.borrow();
                    m.rows
                        .iter()
                        .take(m.shown)
                        .map(|r| {
                            (
                                r.read_chart.clone(),
                                r.write_chart.clone(),
                                vec![r.read.to_string(), r.write.to_string()],
                            )
                        })
                        .collect()
                };
                let items = rows
                    .into_iter()
                    .map(|(read, write, rates)| Look {
                        rates: Some(rates),
                        ..self
                            .vcx
                            .update(|_, cx| charts_look(&[read.read(cx), write.read(cx)]))
                    })
                    .collect();
                WidgetLook {
                    cols: Some(1),
                    items,
                }
            }
            Some(GroupFlowLook::DiskCells { model, .. }) => {
                let rows: Vec<_> = {
                    let m = model.borrow();
                    m.rows
                        .iter()
                        .take(m.cells)
                        .map(|r| {
                            (
                                r.key.clone(),
                                r.pair_chart.clone(),
                                vec![r.read.to_string(), r.write.to_string()],
                            )
                        })
                        .collect()
                };
                let cols = self.columns(rows.iter().map(|(key, ..)| disk_box::cell_id(key)));
                let items = rows
                    .into_iter()
                    .map(|(_, chart, rates)| Look {
                        rates: Some(rates),
                        ..self.vcx.update(|_, cx| charts_look(&[chart.read(cx)]))
                    })
                    .collect();
                WidgetLook {
                    cols: Some(cols),
                    items,
                }
            }
            Some(GroupFlowLook::NetCells { model, .. }) => {
                let cells: Vec<_> = {
                    let m = model.0.borrow();
                    m.adapter_cells().into_iter().take(m.cells).collect()
                };
                let cols = self.columns(cells.iter().map(|(idx, ..)| net_box::cell_id(*idx)));
                let items = cells
                    .into_iter()
                    .map(|(_, text, chart)| Look {
                        rates: Some(rates(&text)),
                        ..self.vcx.update(|_, cx| charts_look(&[chart.read(cx)]))
                    })
                    .collect();
                WidgetLook {
                    cols: Some(cols),
                    items,
                }
            }
            None => {
                let (cols, members) = self.vcx.update(|_, cx| {
                    let g = g.read(cx);
                    (
                        g.fill().cols,
                        g.items().map(|(_, w)| w.clone()).collect::<Vec<_>>(),
                    )
                });
                let mut items = Vec::new();
                for member in members {
                    let (member_id, clock_text, look) = self.vcx.update(|_, cx| {
                        let w = member.read(cx);
                        let value = w.texts()[1].to_string();
                        let look = Look {
                            rates: Some(rates(&value)),
                            value: Some(value),
                            ..charts_look(
                                &w.chart()
                                    .map(|c| c.read(cx))
                                    .into_iter()
                                    .collect::<Vec<_>>(),
                            )
                        };
                        (w.id(), w.clock_text().to_string(), look)
                    });
                    // The parts drawn, left to right by where they start.
                    let mut parts = Vec::new();
                    for part in CORE_PARTS {
                        if let Some(b) = self.bounds(part_id(container_id, member_id, part))
                            && b.size.width > px(0.)
                        {
                            parts.push((f32::from(b.origin.x), part));
                        }
                    }
                    parts.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let drawn = parts.iter().any(|(_, p)| *p == "clock");
                    items.push(Look {
                        clock: Some(drawn.then_some(clock_text)),
                        parts: Some(parts.into_iter().map(|(_, p)| p).collect()),
                        ..look
                    });
                }
                WidgetLook {
                    cols: Some(cols),
                    items,
                }
            }
        }
    }
}

/// `grid` with the parts `flow` leaves out left out too.
fn masked(grid: &Look, flow: &Look) -> Look {
    Look {
        accent: flow.accent.and(grid.accent),
        series: flow.series.and(grid.series),
        lines: flow.lines.as_ref().and(grid.lines.clone()),
        legend: flow.legend.as_ref().and(grid.legend.clone()),
        value: flow.value.as_ref().and(grid.value.clone()),
        rates: flow.rates.as_ref().and(grid.rates.clone()),
        clock: flow.clock.as_ref().and(grid.clock.clone()),
        parts: flow.parts.as_ref().and(grid.parts.clone()),
    }
}

/// `grid` with what `flow` leaves out left out too, item by item.
fn masked_widget(grid: &WidgetLook, flow: &WidgetLook) -> WidgetLook {
    WidgetLook {
        cols: flow.cols.and(grid.cols),
        items: grid
            .items
            .iter()
            .zip(&flow.items)
            .map(|(g, f)| masked(g, f))
            .chain(grid.items.iter().skip(flow.items.len()).cloned())
            .collect(),
    }
}

/// tasklens-6swdr.12: for each phase 2 box (CPU, Memory, Disk, Network, Top processes and
/// Timeline) and each preset, Edit Widgets, Apply with no change and the pinned layout saved as
/// a grid layout keep, for every widget and every drawn member of a group, the accent, the
/// series and their line colours, the legend, the value and rate texts, a core row's clock
/// column and part order, the group's column count, and the widget's forms and options. The
/// CPU box has the kernel line on, so its total chart has two series and the Total and Kernel
/// legend in both layouts; its core rows show "41%" and "3.9 GHz (?)". Each disk and eth0 has a
/// read and a write rate, so the Disk sections and cells and the Network adapter cells have
/// charts and rates to compare.
#[gpui_kit::test]
fn overview_widget_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let graph = r.vcx.update(|_, cx| tl_ui::theme::current(cx).graph);
    for (kind, container_id, presets) in BOXES {
        for preset in presets {
            // Back to the built-in flow layout, then the preset.
            r.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| {
                    reset_to_default(&mut d.settings.performance, container_id)
                })
            });
            draw(r.vcx);
            r.page_action(&PerfOverviewSetBoxMode {
                kind: kind.into(),
                mode: (*preset).into(),
            });
            let (container, editor) = r.parts(kind);
            let flow = r.looks(&container, container_id);
            assert!(
                !flow.is_empty(),
                "{kind} {preset}: the flow layout places widgets"
            );
            for (setting, look) in &flow {
                if matches!(setting.content, WidgetContent::Group(_)) {
                    assert!(
                        !look.items.is_empty() && look.cols.is_some_and(|c| c > 0),
                        "{kind} {preset}: group {} draws members in columns: {look:?}",
                        setting.id
                    );
                }
            }

            r.page_action(&PerfWidgetEditBox { kind: kind.into() });
            assert!(
                r.vcx.read_entity(&editor, |e, _| e.is_editing()),
                "{kind} {preset}: Edit Widgets"
            );
            let edit = r.looks(&container, container_id);
            let pinned = r
                .vcx
                .update(|_, cx| editor.read(cx).working_layout(cx))
                .expect("Edit Widgets pins the flow layout");
            // Apply with no change saves nothing: the box shows its flow layout again.
            let saved = r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
            assert!(!saved, "{kind} {preset}: nothing changed, nothing saved");
            draw(r.vcx);
            let (container, _) = r.parts(kind);
            let applied = r.looks(&container, container_id);
            // The pinned layout saved as the box's grid layout, as an Apply after a move does.
            r.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.performance.layouts.set(pinned))
            });
            draw(r.vcx);
            let (container, _) = r.parts(kind);
            let grid = r.looks(&container, container_id);
            let entry: Option<ContainerLayout> = r.vcx.update(|_, cx| {
                TlSettings::settings(cx)
                    .performance
                    .layouts
                    .known(container_id)
                    .cloned()
            });
            assert!(
                entry.is_some_and(|e| e.preset.is_none()),
                "{kind} {preset}: a grid layout"
            );

            for (stage, after) in [
                ("Edit Widgets", &edit),
                ("Apply", &applied),
                ("grid layout", &grid),
            ] {
                for (setting, look) in &flow {
                    let Some((s, l)) = after.iter().find(|(s, _)| s.id == setting.id) else {
                        panic!("{kind} {preset} {stage}: widget {} is gone", setting.id);
                    };
                    assert_eq!(
                        (&s.content, s.forms, s.options),
                        (&setting.content, setting.forms, setting.options),
                        "{kind} {preset} {stage}: widget {} keeps its content, forms and options",
                        setting.id
                    );
                    assert_eq!(
                        &masked_widget(l, look),
                        look,
                        "{kind} {preset} {stage}: widget {} ({:?}) keeps its look",
                        setting.id,
                        setting.content
                    );
                }
            }

            // Each box's own facts in the grid layout, so a sweep that compares nothing cannot
            // pass.
            for (setting, look) in &grid {
                let group = matches!(setting.content, WidgetContent::Group(_));
                let metric = match &setting.content {
                    WidgetContent::Metric(m) => m.metric.as_str(),
                    _ => "",
                };
                match kind {
                    "cpu" if metric == "total" => {
                        let l = &look.items[0];
                        assert_eq!(l.series, Some(2), "{preset}: total and kernel");
                        assert_eq!(
                            l.legend.as_deref(),
                            Some(&["Total".to_owned(), "Kernel".to_owned()][..]),
                            "{preset}"
                        );
                    }
                    "cpu" if group => {
                        assert_eq!(
                            look.items.len(),
                            usize::from(CORES),
                            "{preset}: a member per core"
                        );
                        for l in &look.items {
                            assert_eq!(l.value.as_deref(), Some("41%"), "{preset}");
                        }
                        if *preset == "cores" || *preset == "chart_cores" {
                            for l in &look.items {
                                assert_eq!(
                                    l.clock,
                                    Some(Some("3.9 GHz (?)".to_owned())),
                                    "{preset}: the clock column"
                                );
                                assert_eq!(
                                    l.parts.as_deref(),
                                    Some(&CORE_PARTS[..]),
                                    "{preset}: name, sparkline, meter, percent, clock"
                                );
                            }
                        }
                    }
                    "disk" if group => {
                        assert_eq!(look.items.len(), DISKS.len(), "{preset}: a member per disk");
                        for l in &look.items {
                            assert_eq!(l.series, Some(2), "{preset}: read and write");
                            assert_eq!(
                                l.lines.as_deref(),
                                Some(&[graph.disk_line, graph.disk_line][..]),
                                "{preset}: both lines in the disk colour"
                            );
                            assert_eq!(l.rates.as_ref().map(Vec::len), Some(2), "{preset}: {l:?}");
                        }
                    }
                    "net" if group => {
                        assert_eq!(look.items.len(), 1, "{preset}: a member for eth0");
                        let l = &look.items[0];
                        assert_eq!(l.series, Some(2), "{preset}: download and upload");
                        assert_eq!(l.accent, Some(graph.net_line), "{preset}");
                        assert_eq!(l.rates.as_ref().map(Vec::len), Some(2), "{preset}: {l:?}");
                    }
                    _ => {}
                }
                let accent = match kind {
                    "cpu" => Some(graph.cpu_line),
                    "disk" => Some(graph.disk_line),
                    // The Upload widget draws the lighter upload line, as the Upload chart.
                    "net" if metric != "tx" => Some(graph.net_line),
                    _ => None,
                };
                if let Some(accent) = accent {
                    for l in &look.items {
                        if let Some(a) = l.accent {
                            assert_eq!(a, accent, "{kind} {preset}: widget {} accent", setting.id);
                        }
                    }
                }
            }
        }
    }
}

/// Every box with its presets for the Edit Widgets sweep: the phase 2 boxes and the GPU box.
const EDIT_BOXES: [(&str, &str, &[&str]); 7] = [
    BOXES[0],
    BOXES[1],
    BOXES[2],
    BOXES[3],
    BOXES[4],
    BOXES[5],
    (
        "gpu",
        GPU_BOX_CONTAINER,
        &["engine_charts", "chart_engines", "chart"],
    ),
];

/// The text parts of a widget or a group member, by debug selector suffix.
const TEXT_PARTS: [&str; 5] = ["name", "value", "stats", "clock", "reason"];

/// The outer `w` px of `b` as four rectangles: top, bottom, left and right.
fn ring(b: Bounds<Pixels>, w: f32) -> [Bounds<Pixels>; 4] {
    let w = px(w);
    [
        Bounds::new(b.origin, size(b.size.width, w)),
        Bounds::new(point(b.left(), b.bottom() - w), size(b.size.width, w)),
        Bounds::new(b.origin, size(w, b.size.height)),
        Bounds::new(point(b.right() - w, b.top()), size(w, b.size.height)),
    ]
}

/// Whether `a` and `b` share more than a hairline.
fn covers(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    let eps = px(0.01);
    a.left() < b.right() - eps
        && b.left() < a.right() - eps
        && a.top() < b.bottom() - eps
        && b.top() < a.bottom() - eps
}

/// The labels chart `c` painted at the last frame, named after `who`.
fn chart_texts(c: &ChartView, who: &str) -> Vec<(String, Bounds<Pixels>)> {
    c.label_cache()
        .painted
        .iter()
        .map(|p| (format!("{who} {:?}", p.kind), p.bounds()))
        .collect()
}

/// The memory groups the Edit Widgets sweep publishes: (device, key, name, MiB). p0 and p1,
/// "Other apps", "Shared and kernel" and its part "Shared memory", which is all of it, so the
/// older-data band is 0 and left out. The legend lists p1, p0, "Other apps", then under a
/// divider "Shared memory" as its last row, as in the 1.3.0 screenshot.
const MEM_GROUPS: [(u16, &str, &str, f32); 5] = [
    (60, "exe:p0", "p0", 3.),
    (61, "exe:p1", "p1", 2.),
    (62, "other", "Other apps", 1.),
    (63, "shared", "Shared and kernel", 2.),
    (64, "k:shmem", "Shared memory", 2.),
];

/// The band key of the Memory by app legend row "Shared memory", the last row, which the
/// 1.3.0 Edit Widgets lost at the bottom. The rig's catalog is not indexed by device, so the
/// legend names this band by its key; the row is found by key.
const SHARED_MEMORY: &str = "k:shmem";

/// Whether `b` lies inside `outer`, to half a pixel.
fn inside(b: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let eps = px(0.5);
    b.left() >= outer.left() - eps
        && b.right() <= outer.right() + eps
        && b.top() >= outer.top() - eps
        && b.bottom() <= outer.bottom() + eps
}

impl Rig<'_> {
    /// Publishes [`MEM_GROUPS`] with 60 one-second samples each, beside the rig's devices and
    /// series, so the Memory by app chart has bands, a legend and a time axis.
    fn publish_mem_groups(&mut self) {
        const PERIOD: u64 = 1_000_000_000;
        const MIB: f32 = 1_048_576.;
        let now = now_boot_ns();
        self._engine.publish_with(|s| {
            let mut devices = (*s.devices).clone();
            let mut entries = devices.entries.to_vec();
            for &(idx, key, name, _) in &MEM_GROUPS {
                let mut e = entry(
                    idx,
                    DeviceIdentity::MemGroup {
                        key: Arc::from(key),
                    },
                    &format!("memgroup:{key}"),
                    None,
                );
                e.display_name = Arc::from(name);
                entries.push(e);
            }
            devices.entries = Arc::from(entries);
            devices.version += 1;
            s.devices = Arc::new(devices);
            let version = s.series.header.version + 1;
            let mut views: Vec<Arc<SeriesView>> = s.series.series.iter().cloned().collect();
            for &(idx, _, _, mib) in &MEM_GROUPS {
                views.push(Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(idx),
                        metric: MetricId::MEM_GROUP,
                    },
                    version,
                    period_ns: PERIOD,
                    samples: (0..60u64)
                        .map(|k| Sample::valid(now - (59 - k) * PERIOD, mib * MIB))
                        .collect::<Vec<_>>()
                        .into(),
                }));
            }
            views.sort_by_key(|v| v.id);
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..s.series.header
                },
                series: views.into(),
                slow: Arc::clone(&s.series.slow),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.memory.groups = Arc::new(
                MEM_GROUPS
                    .iter()
                    .filter(|(_, key, _, _)| key.starts_with("exe:"))
                    .map(|&(idx, key, name, _)| MemGroupBand {
                        key: Arc::from(key),
                        name: Arc::from(name),
                        process_count: 2,
                        device: DeviceIdx(idx),
                        wine: None,
                    })
                    .collect(),
            );
        });
        draw(self.vcx);
    }

    /// One mouse wheel step of `dy` px (negative scrolls down) at `at`, and draws.
    fn wheel(&mut self, at: gpui_kit::Point<Pixels>, dy: f32) {
        self.vcx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        draw(self.vcx);
    }

    /// The Memory by app chart's time axis labels painted at the last frame.
    fn by_app_ticks(&mut self) -> Vec<Bounds<Pixels>> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| {
            page.read(cx)
                .mem_box()
                .read(cx)
                .apps()
                .map(|a| {
                    a.read(cx)
                        .chart()
                        .read(cx)
                        .label_cache()
                        .painted
                        .iter()
                        .filter(|p| matches!(p.kind, LabelKind::XTick(_)))
                        .map(|p| p.bounds())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// The bounds of the Memory by app legend row of band `key`, when the legend draws it.
    fn by_app_row(&mut self, key: &str) -> Option<Bounds<Pixels>> {
        let page = self.page.clone();
        let row = self.vcx.update(|_, cx| {
            let apps = page.read(cx).mem_box().read(cx).apps()?.read(cx);
            let bands = apps.bands();
            memory_apps::legend_order(bands)
                .iter()
                .position(|&i| &*bands[i].key == key)
        })?;
        self.bounds(format!("{}-{row}", memory_apps::LEGEND_ID))
    }

    /// The Memory by app bands in legend order and the legend rows drawn, for a failure
    /// message.
    fn by_app_legend(&mut self) -> String {
        let page = self.page.clone();
        let names: Vec<String> = self.vcx.update(|_, cx| {
            page.read(cx)
                .mem_box()
                .read(cx)
                .apps()
                .map(|a| {
                    let bands = a.read(cx).bands();
                    memory_apps::legend_order(bands)
                        .iter()
                        .map(|&i| bands[i].key.to_string())
                        .collect()
                })
                .unwrap_or_default()
        });
        let rows: Vec<String> = (0..names.len().max(1) + 1)
            .map(|n| format!("{}-{n}", memory_apps::LEGEND_ID))
            .chain([format!("{}-more", memory_apps::LEGEND_ID)])
            .filter_map(|sel| self.bounds(sel.clone()).map(|b| format!("{sel} {b:?}")))
            .collect();
        format!("bands {names:?}, rows drawn {rows:?}")
    }

    /// Scrolls the edit body `body_sel` with the mouse wheel until the text `name` of widget
    /// `id` lies inside it, as a user does to read it, or the body scrolls no further. Returns
    /// the text's bounds (`None` when it is no longer drawn) and the body's.
    fn scroll_to_text(
        &mut self,
        container: &Entity<WidgetContainer>,
        container_id: &str,
        id: u32,
        name: &str,
        body_sel: &str,
    ) -> (Option<Bounds<Pixels>>, Bounds<Pixels>) {
        let mut last = None;
        for _ in 0..8 {
            let body = self.bounds(body_sel.to_owned()).expect("the edit body");
            let text = self
                .edit_texts(container, container_id, id)
                .into_iter()
                .find(|(n, _)| n == name)
                .map(|(_, b)| b);
            let Some(b) = text.filter(|b| !inside(*b, body) && last != Some(*b)) else {
                return (text, body);
            };
            last = Some(b);
            let dy = if b.bottom() > body.bottom() {
                f32::from(body.bottom() - b.bottom()) - 1.
            } else {
                f32::from(body.top() - b.top()) + 1.
            };
            self.wheel(body.center(), dy);
        }
        let body = self.bounds(body_sel.to_owned()).expect("the edit body");
        let text = self
            .edit_texts(container, container_id, id)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b);
        (text, body)
    }

    fn set_ui_scale(&mut self, pct: u16) {
        self.vcx.update(|_, cx| {
            let appearance = AppearanceSettings {
                ui_scale_pct: pct,
                ..AppearanceSettings::default()
            };
            let _ = tl_ui::theme::metrics::recompute(&appearance, cx);
        });
        draw(self.vcx);
    }

    /// The text parts drawn with debug selectors `part_id(container_id, id, part)`.
    fn parts_of(
        &mut self,
        container_id: &str,
        id: u32,
        who: &str,
    ) -> Vec<(String, Bounds<Pixels>)> {
        let mut out = Vec::new();
        for part in TEXT_PARTS.iter().chain(&["more"]) {
            if let Some(b) = self.bounds(part_id(container_id, id, part))
                && b.size.width > px(0.)
                && b.size.height > px(0.)
            {
                out.push((format!("{who} {part}"), b));
            }
        }
        out
    }

    /// Every text widget `id` of `container` draws in Edit Widgets: a widget's text parts and
    /// chart labels; each group member's; the Memory by app legend rows and chart labels; the
    /// timeline's tick and marker rows.
    fn edit_texts(
        &mut self,
        container: &Entity<WidgetContainer>,
        container_id: &str,
        id: u32,
    ) -> Vec<(String, Bounds<Pixels>)> {
        let (widget, group, fixed) = self.vcx.update(|_, cx| {
            let c = container.read(cx);
            (
                c.widget(id).cloned(),
                c.group(id).cloned(),
                c.fixed(id).is_some(),
            )
        });
        let mut texts = self.parts_of(container_id, id, &format!("widget {id}"));
        if let Some(w) = widget {
            texts.extend(self.vcx.update(|_, cx| {
                w.read(cx)
                    .chart()
                    .map(|c| chart_texts(c.read(cx), &format!("widget {id}")))
                    .unwrap_or_default()
            }));
        }
        if let Some(g) = group {
            let members: Vec<_> = self
                .vcx
                .update(|_, cx| g.read(cx).items().map(|(_, w)| w.clone()).collect());
            for member in members {
                let (member_id, labels) = self.vcx.update(|_, cx| {
                    let w = member.read(cx);
                    let who = format!("widget {id} member {:x}", w.id());
                    (
                        w.id(),
                        w.chart()
                            .map(|c| chart_texts(c.read(cx), &who))
                            .unwrap_or_default(),
                    )
                });
                texts.extend(self.parts_of(
                    container_id,
                    member_id,
                    &format!("widget {id} member {member_id:x}"),
                ));
                texts.extend(labels);
            }
        }
        if fixed {
            // The by app legend is the Memory box's: another box's fixed widget does not
            // own the rows the Memory box draws beside it.
            for n in 0..64 {
                if container_id == "overview.memory"
                    && let Some(b) = self.bounds(format!("{}-{n}", memory_apps::LEGEND_ID))
                {
                    texts.push((format!("by app legend row {n}"), b));
                }
            }
            if container_id == "overview.memory"
                && let Some(b) = self.bounds(format!("{}-more", memory_apps::LEGEND_ID))
            {
                texts.push(("by app legend more".to_owned(), b));
            }
            let page = self.page.clone();
            texts.extend(self.vcx.update(|_, cx| {
                page.read(cx)
                    .mem_box()
                    .read(cx)
                    .apps()
                    .filter(|_| container_id == "overview.memory")
                    .map(|a| chart_texts(a.read(cx).chart().read(cx), "by app chart"))
                    .unwrap_or_default()
            }));
            for row in [timeline::TICK_ROW_ID, timeline::MARKER_ROW_ID] {
                if container_id == "overview.timeline"
                    && let Some(b) = self.bounds(row.to_owned())
                    && b.size.height > px(0.)
                {
                    texts.push((row.to_owned(), b));
                }
            }
        }
        texts
    }
}

/// tasklens-mo5t5: in Edit Widgets, for every preset of every box (CPU, Memory, Disk, Network,
/// GPU, Top processes and Timeline) at scale 1.0, 1.5 and 2.0, no edit decoration (grip, gear,
/// border, selected outline, resize corner, the preview's outline) covers any text of a widget:
/// its parts, its chart's legend and axis labels, a group member's label and parts, the Memory
/// by app legend and time axis. No text is clipped, for every widget placed, with no exception:
/// each text lies inside its widget, and inside the body under the toolbar, at once or after
/// the user scrolls the body to it with the wheel. The Memory by app widget keeps its time axis
/// (as many ticks as the applied view, which has one at 100 % and 150 %) and its last legend
/// row, "Shared memory", both in the body under the toolbar with no scroll. The CPU box's Total chart has the Total and Kernel
/// legend at its top left, under the grip and the gear without the inset; the Memory by app
/// widget filled the box body and was cut off by the toolbar.
#[gpui_kit::test]
fn overview_widget_edit_covers_and_clips_no_text(cx: &mut TestAppContext) {
    let mut r = open_with(cx, true);
    r.publish_mem_groups();
    let mut legends = 0;
    for pct in [100u16, 150, 200] {
        r.set_ui_scale(pct);
        for (kind, container_id, presets) in EDIT_BOXES {
            for preset in presets {
                let at = format!("{pct} % {kind} {preset}");
                r.vcx.update(|_, cx| {
                    TlSettings::update(cx, |d| {
                        reset_to_default(&mut d.settings.performance, container_id)
                    })
                });
                draw(r.vcx);
                r.page_action(&PerfOverviewSetBoxMode {
                    kind: kind.into(),
                    mode: (*preset).into(),
                });
                let (container, editor) = r.parts(kind);
                let by_app = kind == "memory" && *preset == "apps";
                // The applied view's time axis and last legend row, which Edit Widgets keeps.
                // At 200 % the 1920 × 1200 window is 960 × 600 logical px, and the applied
                // by app chart is too short for its time axis: there Edit Widgets draws the
                // same none.
                let applied_ticks = if by_app {
                    let ticks = r.by_app_ticks().len();
                    if pct < 200 {
                        assert!(ticks > 0, "{at}: the applied view has a time axis");
                    }
                    assert!(
                        r.by_app_row(SHARED_MEMORY).is_some(),
                        "{at}: the applied legend shows {SHARED_MEMORY}: {}",
                        r.by_app_legend()
                    );
                    ticks
                } else {
                    0
                };
                r.page_action(&PerfWidgetEditBox { kind: kind.into() });
                draw(r.vcx);
                assert!(
                    r.vcx.read_entity(&editor, |e, _| e.is_editing()),
                    "{at}: Edit Widgets"
                );
                let body_sel = format!("{}-scroll", editor_id(container_id));
                if by_app {
                    let body = r.bounds(body_sel.clone()).expect("the edit body");
                    let ticks = r.by_app_ticks();
                    assert_eq!(
                        ticks.len(),
                        applied_ticks,
                        "{at}: the by app chart keeps its time axis in Edit Widgets"
                    );
                    for t in &ticks {
                        assert!(
                            inside(*t, body),
                            "{at}: time axis label {t:?} shows in the body {body:?}"
                        );
                    }
                    let row = r.by_app_row(SHARED_MEMORY).unwrap_or_else(|| {
                        panic!(
                            "{at}: the legend keeps its {SHARED_MEMORY} row: {}",
                            r.by_app_legend()
                        )
                    });
                    assert!(
                        inside(row, body),
                        "{at}: the {SHARED_MEMORY} row {row:?} shows in the body {body:?}"
                    );
                }
                let ids = r.vcx.read_entity(&container, |c, _| c.placed_ids());
                assert!(!ids.is_empty(), "{at}: widgets placed");
                for id in ids {
                    let w = r
                        .bounds(widget_id(container_id, id))
                        .unwrap_or_else(|| panic!("{at}: widget {id} drawn"));
                    let frame = r.bounds(frame_id(container_id, id)).expect("frame");
                    let mut decorations: Vec<(String, Bounds<Pixels>)> = Vec::new();
                    for (name, sel) in [
                        ("grip", grip_id(container_id, id)),
                        ("gear", gear_id(container_id, id)),
                        ("corner", corner_id(container_id, id)),
                    ] {
                        let b = r
                            .bounds(sel)
                            .unwrap_or_else(|| panic!("{at}: widget {id} {name}"));
                        decorations.push((name.to_owned(), b));
                    }
                    decorations.extend(ring(frame, 1.).map(|b| ("border".to_owned(), b)));
                    // The selected outline and a drag's preview lie in the widget's outer 2 px.
                    decorations.extend(ring(w, 2.).map(|b| ("outline".to_owned(), b)));
                    let texts = r.edit_texts(&container, container_id, id);
                    legends += texts
                        .iter()
                        .filter(|(n, _)| n.contains("Legend") || n.contains("legend row"))
                        .count();
                    for (part, b) in &texts {
                        for (d, db) in &decorations {
                            assert!(
                                !covers(*db, *b),
                                "{at}: widget {id} {d} {db:?} covers {part} {b:?}"
                            );
                        }
                        assert!(
                            inside(*b, w),
                            "{at}: {part} {b:?} is cut off by widget {id} {w:?}"
                        );
                    }
                    // Every text shows in the body under the toolbar, at once or after the
                    // wheel scrolls the body to it.
                    let mut now = texts.clone();
                    let mut body = r.bounds(body_sel.clone()).expect("the edit body");
                    for (part, _) in &texts {
                        let shown = now.iter().find(|(n, _)| n == part).map(|(_, b)| *b);
                        if shown.is_some_and(|b| inside(b, body)) {
                            continue;
                        }
                        let (b, after) =
                            r.scroll_to_text(&container, container_id, id, part, &body_sel);
                        let b = b.unwrap_or_else(|| panic!("{at}: {part} still drawn"));
                        assert!(
                            inside(b, after),
                            "{at}: {part} {b:?} is cut off by the body {after:?}"
                        );
                        now = r.edit_texts(&container, container_id, id);
                        body = after;
                    }
                }
                r.vcx
                    .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
                draw(r.vcx);
            }
        }
    }
    assert!(legends > 0, "the sweep saw chart legends");
}
