//! A widget draws the same in a flow layout, in Edit Widgets and in a grid layout
//! (tasklens-6swdr.12, ADR-019 "Flow placement" and "Inner grid"). For each phase 2 box and each
//! preset, the sweep reads what every widget of the flow layout draws, enters Edit Widgets,
//! applies with no change, and checks that every widget kept its accent colour, its chart's
//! series count and legend, its value text, a core row's clock column, and its forms and
//! options. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, Hsla, IntoElement, ParentElement as _,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, WidgetContent, WidgetSetting, reset_to_default,
};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::ChartView;
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{PerfOverviewSetBoxMode, PerfWidgetEditBox};
use tl_ui::pages::performance::overview::parts::CoreCellFit;
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::WidgetContainer;
use tl_ui::pages::performance::widget::edit::WidgetEditor;
use tl_ui::pages::performance::widget::group_widget::GroupFlowLook;
use tl_ui::pages::performance::widget::metric_widget::{FlowLook, part_id};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

/// The first logical CPU's device index; core `n` is at `CORE0 + n`.
const CORE0: u16 = 100;
const CORES: u16 = 4;
const GIB: u64 = 1 << 30;

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
    for (idx, name) in [(3, "sda"), (4, "sdb")] {
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

/// The Overview page at 1920 × 1200 with the kernel line on, memory totals and one sample per
/// core of load (41.7%) and clock (3.94 GHz).
fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog());
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        sys.memory.validity = Validity::Valid;
        sys.memory.total = 64 * GIB;
        sys.memory.used = 23 * GIB;
        sys.memory.available = 41 * GIB;
        sys.memory.cached = 10 * GIB;
        sys.memory.swap_total = 8 * GIB;
        sys.memory.swap_free = 6 * GIB;
        sys.memory.commit_limit = 40 * GIB;
        sys.memory.committed = 20 * GIB;
        let version = 1;
        let mut views = Vec::new();
        for id in 0..CORES {
            views.push(series(CORE0 + id, MetricId::CPU_TOTAL, 41.7, version));
            views.push(series(CORE0 + id, MetricId::CPU_FREQ, 3940.0, version));
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

/// What a widget draws that must not change with its placement. `None` is a part the flow
/// look of that widget does not show in a comparable form, and is left out of the comparison.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    /// The line colour of the chart's first series.
    accent: Option<Hsla>,
    /// The chart's series count.
    series: Option<usize>,
    /// The chart's legend names.
    legend: Option<Vec<String>>,
    value: Option<String>,
    /// A core row's clock column: its text, while it shows.
    clock: Option<Option<String>>,
}

fn chart_parts(chart: Option<&ChartView>) -> (Option<Hsla>, Option<usize>, Option<Vec<String>>) {
    match chart {
        Some(c) => (
            c.spec.series.first().map(|s| s.line),
            Some(c.spec.series.len()),
            Some(c.labels.legend.iter().map(|(n, _)| n.to_string()).collect()),
        ),
        None => (None, None, None),
    }
}

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
                _ => {
                    let b = p.timeline_frame().read(cx);
                    (b.container().clone(), b.editor().clone())
                }
            }
        })
    }

    /// The settings and look of every placed widget of `container`, by id.
    fn looks(
        &mut self,
        container: &Entity<WidgetContainer>,
        container_id: &str,
    ) -> Vec<(WidgetSetting, Look)> {
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

    fn look(&mut self, container: &Entity<WidgetContainer>, container_id: &str, id: u32) -> Look {
        let (widget, group) = self.vcx.update(|_, cx| {
            let c = container.read(cx);
            (c.widget(id).cloned(), c.group(id).cloned())
        });
        if let Some(w) = widget {
            return self.vcx.update(|_, cx| {
                let w = w.read(cx);
                let chart = |c: &Entity<ChartView>| chart_parts(Some(c.read(cx)));
                match w.flow() {
                    Some(FlowLook::Chart(c)) => {
                        let (accent, series, legend) = chart(c);
                        Look {
                            accent,
                            series,
                            legend,
                            value: None,
                            clock: None,
                        }
                    }
                    Some(FlowLook::MemRow { rows, index, spark }) => {
                        let (accent, series, legend) = match spark {
                            Some(c) => chart(c),
                            None => (None, None, None),
                        };
                        Look {
                            accent,
                            series,
                            legend,
                            value: Some(rows.0.borrow().rows[*index].value.to_string()),
                            clock: None,
                        }
                    }
                    Some(FlowLook::MemCell {
                        rows,
                        index,
                        chart: c,
                    }) => {
                        let (accent, series, legend) = chart(c);
                        Look {
                            accent,
                            series,
                            legend,
                            value: Some(rows.0.borrow().rows[*index].value.to_string()),
                            clock: None,
                        }
                    }
                    Some(FlowLook::NetDirection { chart: c, .. })
                    | Some(FlowLook::NetCombined { chart: c, .. }) => {
                        let (accent, series, legend) = chart(c);
                        Look {
                            accent,
                            series,
                            legend,
                            value: None,
                            clock: None,
                        }
                    }
                    Some(_) => panic!("widget {id}: no GPU box look in these boxes"),
                    None => {
                        let (accent, series, legend) = chart_parts(w.chart().map(|c| c.read(cx)));
                        Look {
                            accent,
                            series,
                            legend,
                            value: Some(w.texts()[1].to_string()),
                            clock: None,
                        }
                    }
                }
            });
        }
        let Some(g) = group else {
            // A fixed widget draws its own content in every layout.
            return Look {
                accent: None,
                series: None,
                legend: None,
                value: None,
                clock: None,
            };
        };
        let flow = self.vcx.update(|_, cx| g.read(cx).flow_look().cloned());
        match flow {
            Some(GroupFlowLook::CoreRows {
                bars,
                sparks,
                layout,
                ..
            }) => {
                let (accent, series, legend) = self
                    .vcx
                    .update(|_, cx| chart_parts(Some(sparks[0].read(cx))));
                let bars = bars.borrow();
                // The last core: in Edit Widgets the first row's start is the grip's.
                let last = bars.clock_labels().len().saturating_sub(1);
                let clock = (layout.get().fit == CoreCellFit::Full)
                    .then(|| bars.clock_labels()[last].to_string());
                Look {
                    accent,
                    series,
                    legend,
                    value: Some(bars.pct_label(0).to_owned()),
                    clock: Some(clock),
                }
            }
            Some(GroupFlowLook::CoreCells { bars, sparks, .. }) => {
                let (accent, series, legend) = self
                    .vcx
                    .update(|_, cx| chart_parts(Some(sparks[0].read(cx))));
                Look {
                    accent,
                    series,
                    legend,
                    value: Some(bars.borrow().pct_label(0).to_owned()),
                    clock: None,
                }
            }
            Some(GroupFlowLook::DiskRows { model, .. }) => {
                // A disk section draws read and write as two sparklines: the accent compares.
                let chart = model.borrow().rows.first().map(|r| r.read_chart.clone());
                let accent =
                    chart.and_then(|c| self.vcx.update(|_, cx| chart_parts(Some(c.read(cx))).0));
                Look {
                    accent,
                    series: None,
                    legend: None,
                    value: None,
                    clock: None,
                }
            }
            Some(GroupFlowLook::DiskCells { model, .. }) => {
                let chart = model.borrow().rows.first().map(|r| r.pair_chart.clone());
                let (accent, series, legend) = match chart {
                    Some(c) => self.vcx.update(|_, cx| chart_parts(Some(c.read(cx)))),
                    None => (None, None, None),
                };
                Look {
                    accent,
                    series,
                    legend,
                    value: None,
                    clock: None,
                }
            }
            Some(GroupFlowLook::NetCells { .. }) => Look {
                // The adapter cells' parts are the box's own: nothing compares.
                accent: None,
                series: None,
                legend: None,
                value: None,
                clock: None,
            },
            None => {
                // The group's first item.
                let item = self
                    .vcx
                    .update(|_, cx| g.read(cx).items().next().map(|(_, w)| w.clone()));
                let Some(item) = item else {
                    return Look {
                        accent: None,
                        series: None,
                        legend: None,
                        value: None,
                        clock: None,
                    };
                };
                // The clock column of the last item: in Edit Widgets the first row's start is
                // the grip's.
                let last = self
                    .vcx
                    .update(|_, cx| g.read(cx).items().last().map(|(_, w)| w.clone()))
                    .expect("a last item");
                let (item_id, clock_text) = self.vcx.update(|_, cx| {
                    let w = last.read(cx);
                    (w.id(), w.clock_text().to_string())
                });
                let look = self.vcx.update(|_, cx| {
                    let w = item.read(cx);
                    let (accent, series, legend) = chart_parts(w.chart().map(|c| c.read(cx)));
                    Look {
                        accent,
                        series,
                        legend,
                        value: Some(w.texts()[1].to_string()),
                        clock: None,
                    }
                });
                let drawn = self
                    .vcx
                    .debug_bounds(Box::leak(
                        part_id(container_id, item_id, "clock").into_boxed_str(),
                    ))
                    .is_some();
                Look {
                    clock: Some(drawn.then_some(clock_text)),
                    ..look
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
        legend: flow.legend.as_ref().and(grid.legend.clone()),
        value: flow.value.as_ref().and(grid.value.clone()),
        clock: flow.clock.as_ref().and(grid.clock.clone()),
    }
}

/// tasklens-6swdr.12: for each phase 2 box (CPU, Memory, Disk, Network, Top processes and
/// Timeline) and each preset, entering Edit Widgets and applying with no change keeps every
/// widget's accent, series count, legend, value text and core clock column, and its forms and
/// options. The CPU box has the kernel line on, so its total chart has two series and the
/// Total and Kernel legend in both layouts; its core rows show "41%" and "3.9 GHz".
#[gpui_kit::test]
fn overview_widget_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    let mut r = open(cx);
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

            r.page_action(&PerfWidgetEditBox { kind: kind.into() });
            assert!(
                r.vcx.read_entity(&editor, |e, _| e.is_editing()),
                "{kind} {preset}: Edit Widgets"
            );
            let edit = r.looks(&container, container_id);
            let saved = r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
            assert!(saved, "{kind} {preset}: Apply saves the pinned layout");
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

            for (stage, after) in [("Edit Widgets", &edit), ("Apply", &grid)] {
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
                        &masked(l, look),
                        look,
                        "{kind} {preset} {stage}: widget {} ({:?}) keeps its look",
                        setting.id,
                        setting.content
                    );
                }
            }

            // The CPU box's own facts, so a sweep that compares nothing cannot pass.
            if kind == "cpu" {
                for (setting, look) in &grid {
                    match &setting.content {
                        WidgetContent::Metric(m) if m.metric.as_str() == "total" => {
                            assert_eq!(look.series, Some(2), "{preset}: total and kernel");
                            assert_eq!(
                                look.legend.as_deref(),
                                Some(&["Total".to_owned(), "Kernel".to_owned()][..]),
                                "{preset}"
                            );
                        }
                        WidgetContent::Group(_) => {
                            assert_eq!(look.value.as_deref(), Some("41%"), "{preset}");
                            if *preset == "cores" || *preset == "chart_cores" {
                                assert_eq!(
                                    look.clock,
                                    Some(Some("3.9 GHz".to_owned())),
                                    "{preset}: the clock column"
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            let cpu_line = r
                .vcx
                .update(|_, cx| tl_ui::theme::current(cx).graph.cpu_line);
            if kind == "cpu" {
                for (setting, look) in &grid {
                    if let Some(accent) = look.accent {
                        assert_eq!(accent, cpu_line, "{preset}: widget {} is blue", setting.id);
                    }
                }
            }
        }
    }
}
