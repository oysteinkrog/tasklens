//! The Overview CPU box on widgets (ADR-019 phase 2 "Presets", UI-PRF-061 phase 2): each old
//! mode is a flow layout of the preset's widgets placed where 1.2.0 drew the parts, the cores
//! group fills the height (D-040), a core sample renders only the cores group, and Edit Widgets
//! adds, removes, moves and applies CPU widgets. The `apps` preset and the `cpu_by_app` widget
//! (UI-PRF-062, `tasklens-qkvs0.9`) are at the end.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, Focusable as _, IntoElement, Modifiers,
    ParentElement as _, Pixels, Render, StyleRefinement, Styled as _, Subscription, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::MemGroupBand;
use tl_model::time::now_boot_ns;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::overview::cpu_box::{
    CLOCKS_ID, CORE_CHARTS_ID, CPU_APPS_ID, CPU_CHART_ID, CpuBox,
};
use tl_ui_overview::overview::cpu_flow::{apps_of, clocks_of, cores_of, total_of};
use tl_ui_overview::overview::memory_apps::{APPS_ID, AppsChart, AppsMetric, CPU_BANDS, LEGEND_ID};
use tl_ui_overview::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui_overview::widget::container::{WidgetContainer, container_id};
use tl_ui_overview::widget::edit::WidgetEditor;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::group_widget::GroupWidget;
use tl_ui_overview::widget::metric_widget::{MetricWidget, widget_id};
use tl_ui_perf::performance::sidebar::DeviceSidebar;
use tl_ui_perf_core::core::DeviceKey;
use tl_ui_perf_core::core::actions::{
    PerfOverviewSetBoxClocks, PerfOverviewSetBoxMode, PerfWidgetEditBox, WidgetSource,
};
use tl_ui_settings::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, DeviceSelector, FixedWidget, GroupKind, WidgetContent,
};
use tl_ui_settings::layout_boxes::{METRIC_CPU_KERNEL, METRIC_FREQ};
use tl_ui_settings::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

/// The first logical CPU's device index; core `n` is at `CORE0 + n`.
const CORE0: u16 = 100;

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

/// The CPU package at 0 with `cores` logical CPUs at `CORE0..`, memory, two disks, eth0 and the
/// aggregates. No GPU.
fn catalog(cores: u16) -> DeviceCatalog {
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
    for id in 0..cores {
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

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    version: u64,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Overview page filling it and `cores` logical CPUs.
fn open(cx: &mut TestAppContext, cores: u16, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| s.devices = Arc::new(catalog(cores)));
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        let mut settings = Settings::default();
        settings.motion.smooth_graphs = false;
        // Raw core values: two samples a few ms apart barely move a smoothed bar.
        settings.performance.overview_core_smoothing = tl_ui_settings::settings::CoreSmoothing::Off;
        // Leave the Memory, Disk and Network boxes out, so the CPU box has their room. Widget
        // selectors have the container id (tasklens-6swdr.10), so other boxes do not shadow them.
        let mut overview = OverviewLayoutSetting::grid_default();
        overview
            .grid
            .retain(|b| !matches!(b.kind.as_str(), "memory" | "disk" | "net"));
        settings.performance.overview = Some(overview);
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
    vcx.simulate_resize(size(px(w), px(h)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    draw(vcx);
    let page = vcx.read_entity(&host, |h, _| h.page.clone());
    Rig {
        engine,
        page,
        vcx,
        version: 0,
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

fn near(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() < 1.
}

impl Rig<'_> {
    fn page_action(&mut self, action: &dyn gpui_kit::Action) {
        let page = self.page.clone();
        self.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(action, window, cx);
        });
        draw(self.vcx);
    }

    fn cpu(&mut self) -> Entity<CpuBox> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).cpu_box().clone())
    }

    fn container(&mut self) -> Entity<WidgetContainer> {
        let cpu = self.cpu();
        self.vcx.update(|_, cx| cpu.read(cx).container().clone())
    }

    fn editor(&mut self) -> Entity<WidgetEditor> {
        let cpu = self.cpu();
        self.vcx.update(|_, cx| cpu.read(cx).editor().clone())
    }

    fn flow(&mut self) -> ContainerLayout {
        let cpu = self.cpu();
        self.vcx.update(|_, cx| cpu.read(cx).flow().clone())
    }

    fn bounds_of(&mut self, id: String) -> gpui_kit::Bounds<Pixels> {
        let id: &'static str = Box::leak(id.into_boxed_str());
        self.vcx.debug_bounds(id).expect(id)
    }

    /// The placement of widget `id` in the container's body, px.
    fn placed(&mut self, id: u32) -> Option<gpui_kit::Bounds<Pixels>> {
        let container = self.container();
        self.vcx
            .update(|_, cx| container.read(cx).placement().bounds(id))
    }

    /// One new `CPU_TOTAL` sample per logical CPU, `pct[i]` for core `i`.
    fn publish_cores(&mut self, pct: &[f32]) {
        self.version += 1;
        let version = self.version;
        let newest = now_boot_ns();
        let views: Vec<Arc<SeriesView>> = pct
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(CORE0 + i as u16),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: 100_000_000,
                    samples: Arc::from([Sample::valid(newest, v)]),
                })
            })
            .collect();
        self.engine.publish_with(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
        });
        draw(self.vcx);
    }
}

/// UI-PRF-061 phase 2, ADR-019 "Presets": the default and each old mode of the CPU box are
/// flow layouts of the preset's widgets. Each widget is drawn by its own entity at the place
/// 1.2.0 drew the part, and the container places it there from the box's plan, not from a
/// paint. Edit Widgets pins the drawn widgets to the grid.
#[gpui_kit::test]
fn overview_cpu_flow_matches_modes(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.page_action(&PerfOverviewSetBoxClocks {
        kind: "cpu".into(),
        on: true,
    });
    let body = container_id(CPU_BOX_CONTAINER);
    for mode in ["chart_cores", "chart", "cores", "core_charts"] {
        r.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: mode.into(),
        });
        let flow = r.flow();
        assert_eq!(
            flow.preset.as_ref().map(|p| p.id.as_str()),
            Some(mode),
            "the mode is the preset"
        );
        let graph = matches!(mode, "chart_cores" | "chart");
        assert_eq!(total_of(&flow).is_some(), graph, "{mode}: total chart");
        assert_eq!(cores_of(&flow).is_some(), mode != "chart", "{mode}: cores");
        assert!(clocks_of(&flow).is_some(), "{mode}: Core clocks stays on");
        // (widget, the part the box drew for it)
        let mut parts: Vec<(u32, String)> = Vec::new();
        if graph {
            parts.push((total_of(&flow).unwrap(), CPU_CHART_ID.to_owned()));
            parts.push((clocks_of(&flow).unwrap(), CLOCKS_ID.to_owned()));
        } else {
            assert!(
                r.placed(clocks_of(&flow).unwrap()).is_none(),
                "{mode}: the Core clocks chart takes no space"
            );
        }
        if let Some(cores) = cores_of(&flow) {
            let part = if mode == "core_charts" {
                CORE_CHARTS_ID.to_owned()
            } else {
                widget_id(CPU_BOX_CONTAINER, cores)
            };
            parts.push((cores, part));
            // The cores group draws the 1.2.0 part from the box's model.
            let container = r.container();
            let looks = r.vcx.update(|_, cx| {
                container
                    .read(cx)
                    .group(cores)
                    .is_some_and(|g| g.read(cx).flow_look().is_some())
            });
            assert!(looks, "{mode}: the cores group draws the flow look");
        }
        let origin = r.bounds_of(body.clone()).origin;
        for (id, part) in &parts {
            let at = r
                .placed(*id)
                .unwrap_or_else(|| panic!("{mode}: {id} placed"));
            let painted = r.bounds_of(part.clone());
            let (x, y) = (painted.origin.x - origin.x, painted.origin.y - origin.y);
            assert!(
                near(at.origin.x, x)
                    && near(at.origin.y, y)
                    && near(at.size.width, painted.size.width)
                    && near(at.size.height, painted.size.height),
                "{mode}: {id} {part}: placed {at:?}, painted {painted:?} from {origin:?}"
            );
        }

        // Edit Widgets pins every drawn widget to the grid, and Cancel puts the preset back.
        r.page_action(&PerfWidgetEditBox { kind: "cpu".into() });
        let editor = r.editor();
        let working = r
            .vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .expect("Edit Widgets is on");
        assert!(working.preset.is_none(), "{mode}: pinned to a grid layout");
        let pinned: Vec<u32> = working.widgets.iter().map(|w| w.id).collect();
        let mut drawn: Vec<u32> = parts.iter().map(|p| p.0).collect();
        drawn.sort_unstable();
        let mut sorted = pinned.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, drawn, "{mode}: the drawn widgets, no more");
        assert!(working.widgets.iter().all(|w| w.cell.is_some()));
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
        draw(r.vcx);
        assert!(!r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        let after = r.flow();
        assert_eq!(after, flow, "{mode}: Cancel keeps the flow layout");
    }
}

/// D-040 on widgets: 32 logical CPUs in the `cores` preset fill the cores group's height, the
/// rows growing past 18 px, and the group widget is as tall as the rows' part.
#[gpui_kit::test]
fn overview_cpu_core_group_fills_height(cx: &mut TestAppContext) {
    let mut r = open(cx, 32, 1200., 900.);
    r.page_action(&PerfOverviewSetBoxMode {
        kind: "cpu".into(),
        mode: "cores".into(),
    });
    let pct: Vec<f32> = (0..32).map(|i| i as f32 * 3.).collect();
    r.publish_cores(&pct);
    let cpu = r.cpu();
    let layout = r.vcx.update(|_, cx| cpu.read(cx).core_layout());
    assert!(!layout.scrolls, "{layout:?}");
    assert!(layout.cols * layout.rows >= 32, "{layout:?}");
    assert!(layout.row_h > px(18.), "rows grow (D-040): {layout:?}");
    let cores = cores_of(&r.flow()).expect("a cores group");
    let group = r.bounds_of(widget_id(CPU_BOX_CONTAINER, cores));
    let rows_h = layout.row_h * layout.rows as f32;
    assert!(
        near(rows_h, group.size.height),
        "the rows fill the group: {rows_h:?} of {group:?}"
    );
}

/// The render contract on widgets: a core sample renders the cores group, not the box, and no
/// widget entity is made or dropped.
#[gpui_kit::test]
fn overview_cpu_core_sample_renders_only_the_group(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.publish_cores(&[10., 20., 30., 40.]);
    let cpu = r.cpu();
    let container = r.container();
    let cores = cores_of(&r.flow()).expect("a cores group");
    let group = r
        .vcx
        .update(|_, cx| container.read(cx).group(cores).cloned())
        .expect("the cores group entity");
    let counts = |r: &mut Rig| {
        let (cpu, group, container) = (cpu.clone(), group.clone(), container.clone());
        r.vcx.update(|_, cx| {
            (
                cpu.read(cx).notify_count.get(),
                group.read(cx).notify_count.get(),
                container.read(cx).notify_count.get(),
            )
        })
    };
    let before = counts(&mut r);
    r.publish_cores(&[55., 65., 75., 85.]);
    let after = counts(&mut r);
    assert_eq!(after.0, before.0, "the box does not notify");
    assert_eq!(after.2, before.2, "the container does not notify");
    assert!(after.1 > before.1, "the cores group notifies");
    let same = r
        .vcx
        .update(|_, cx| container.read(cx).group(cores).cloned())
        .expect("still placed");
    assert_eq!(same.entity_id(), group.entity_id(), "same entity");
}

/// The render contract in the `chart` mode: the box shows no core rows, so a core sample
/// renders neither the box nor its container.
#[gpui_kit::test]
fn overview_cpu_core_sample_in_chart_mode_renders_nothing(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.page_action(&PerfOverviewSetBoxMode {
        kind: "cpu".into(),
        mode: "chart".into(),
    });
    r.publish_cores(&[10., 20., 30., 40.]);
    let cpu = r.cpu();
    let container = r.container();
    let counts = |r: &mut Rig| {
        let (cpu, container) = (cpu.clone(), container.clone());
        r.vcx.update(|_, cx| {
            (
                cpu.read(cx).notify_count.get(),
                container.read(cx).notify_count.get(),
            )
        })
    };
    let before = counts(&mut r);
    r.publish_cores(&[55., 65., 75., 85.]);
    let after = counts(&mut r);
    assert_eq!(after.0, before.0, "the box does not notify");
    assert_eq!(after.1, before.1, "the container does not notify");
}

/// UI-PRF-061 phase 2: in Edit Widgets the CPU box lists its metrics and cores, adds a
/// widget, a core widget and an all-cores group, removes, moves, and Apply saves a grid layout
/// that the box then draws.
#[gpui_kit::test]
fn overview_cpu_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.page_action(&PerfWidgetEditBox { kind: "cpu".into() });
    let editor = r.editor();
    assert!(r.vcx.read_entity(&editor, |e, _| e.is_editing()));

    // The source list: the CPU's metrics, then Core load and Core speed with C0 to C3.
    let groups = r.vcx.update(|_, cx| editor.read(cx).source_groups(cx));
    assert_eq!(groups.len(), 1, "{groups:?}");
    let cpu_group = &groups[0];
    assert_eq!(&*cpu_group.key, "cpu");
    let labels: Vec<String> = cpu_group
        .items
        .iter()
        .map(|i| i.label.to_string())
        .collect();
    assert!(labels.contains(&"Utilization".to_owned()), "{labels:?}");
    assert_eq!(cpu_group.subs.len(), 2, "{:?}", cpu_group.subs);
    let core_labels: Vec<String> = cpu_group.subs[0]
        .items
        .iter()
        .map(|i| i.label.to_string())
        .collect();
    assert_eq!(core_labels, ["C0", "C1", "C2", "C3"]);
    assert!(cpu_group.add_group.is_some(), "All cores");

    let source = |device: &str, metric: &str| WidgetSource {
        device: Arc::from(device),
        metric: Arc::from(metric),
    };
    let kernel = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source("cpu", METRIC_CPU_KERNEL), cx)))
        .expect("kernel time added");
    let core = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source("cpu:2", METRIC_FREQ), cx)))
        .expect("C2 speed added");
    let group = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add_group("cpu", "cores", METRIC_FREQ, cx)))
        .expect("all cores added");
    draw(r.vcx);
    // Remove the core widget, then move the kernel widget one cell right.
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(core, cx);
            assert!(e.remove_selected(cx));
            e.select(kernel, cx);
        })
    });
    let before = r
        .vcx
        .update(|_, cx| editor.read(cx).working_layout(cx))
        .and_then(|l| l.widgets.into_iter().find(|w| w.id == kernel))
        .and_then(|w| w.cell)
        .expect("kernel cell");
    let moved = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.move_selected(1, 0, cx)));
    let after = r
        .vcx
        .update(|_, cx| editor.read(cx).working_layout(cx))
        .and_then(|l| l.widgets.into_iter().find(|w| w.id == kernel))
        .and_then(|w| w.cell)
        .expect("kernel cell");
    if moved {
        assert_eq!(after.col, before.col + 1, "{before:?} → {after:?}");
    } else {
        // A full-width widget has no room to move right.
        assert_eq!(after, before);
    }
    let saved = r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    assert!(saved, "Apply saves");
    draw(r.vcx);

    let entry = r.vcx.update(|_, cx| {
        TlSettings::settings(cx)
            .performance
            .layouts
            .known(CPU_BOX_CONTAINER)
            .cloned()
    });
    let entry = entry.expect("an overview.cpu entry");
    assert!(entry.preset.is_none(), "a grid layout");
    let ids: Vec<u32> = entry.widgets.iter().map(|w| w.id).collect();
    assert!(ids.contains(&kernel) && ids.contains(&group), "{ids:?}");
    assert!(!ids.contains(&core), "removed: {ids:?}");
    let k = entry.widgets.iter().find(|w| w.id == kernel).unwrap();
    assert!(
        matches!(&k.content, WidgetContent::Metric(s)
            if s.metric.as_str() == METRIC_CPU_KERNEL
                && matches!(&s.device, DeviceSelector::Key(d) if &**d == "cpu")),
        "{k:?}"
    );
    let g = entry.widgets.iter().find(|w| w.id == group).unwrap();
    assert!(
        matches!(&g.content, WidgetContent::Group(sel)
            if sel.members == GroupKind::Cores && sel.metric.as_str() == METRIC_FREQ),
        "{g:?}"
    );
    // The box draws the user's widgets now, each by its own entity.
    let cpu = r.cpu();
    assert!(r.vcx.update(|_, cx| cpu.read(cx).grid().is_some()));
    let container = r.container();
    let placed = r.vcx.update(|_, cx| container.read(cx).placed_ids());
    assert!(
        placed.contains(&kernel) && placed.contains(&group),
        "{placed:?}"
    );
    assert!(
        r.vcx
            .debug_bounds(Box::leak(
                widget_id(CPU_BOX_CONTAINER, kernel).into_boxed_str()
            ))
            .is_some()
    );
}

// ---- CPU by app (UI-PRF-062, D-042, tasklens-qkvs0.9) ----

/// The device of memory group `exe:p<i>`.
fn group_dev(i: u16) -> DeviceIdx {
    DeviceIdx(20 + i)
}

/// "Other apps".
const OTHER_DEV: DeviceIdx = DeviceIdx(30);

/// The device of system band `b` of [`CPU_BANDS`].
fn band_dev(b: usize) -> DeviceIdx {
    DeviceIdx(31 + b as u16)
}

/// [`catalog`] with 4 logical CPUs plus memory groups `exe:p0` to `exe:p3`, "Other apps" and
/// the five system bands. The version stays 2, so a repeated sample changes no device.
fn group_catalog() -> DeviceCatalog {
    let base = catalog(4);
    let group = |idx: DeviceIdx, key: &str, name: &str| DeviceEntry {
        display_name: Arc::from(name),
        ..entry(
            idx.0,
            DeviceIdentity::MemGroup {
                key: Arc::from(key),
            },
            &format!("memgroup:{key}"),
            None,
        )
    };
    let mut entries = base.entries.to_vec();
    for i in 0..4u16 {
        entries.push(group(group_dev(i), &format!("exe:p{i}"), &format!("p{i}")));
    }
    entries.push(group(OTHER_DEV, "other", "Other apps"));
    for (b, (key, _, _)) in CPU_BANDS.iter().enumerate() {
        entries.push(group(band_dev(b), key, key));
    }
    entries.sort_by_key(|e| e.idx);
    DeviceCatalog {
        version: 2,
        entries: Arc::from(entries),
    }
}

impl Rig<'_> {
    /// One `CPU_GROUP` sample a second for the last minute: groups `exe:p<i>` at `groups[i]`
    /// percent (a group past the list has no series), "Other apps" 1.5% and the system bands
    /// 1, 0.5, 0.5, 0 and 0.2%.
    fn publish_cpu_groups(&mut self, groups: &[f32]) {
        const PERIOD: u64 = 1_000_000_000;
        let now = now_boot_ns();
        let bands = [1., 0.5, 0.5, 0., 0.2];
        self.engine.publish_with(|s| {
            if s.devices.version != 2 {
                s.devices = Arc::new(group_catalog());
            }
            let version = s.series.header.version + 1;
            let line = |device: DeviceIdx, value: f32| {
                Arc::new(SeriesView {
                    id: SeriesId {
                        device,
                        metric: MetricId::CPU_GROUP,
                    },
                    version,
                    period_ns: PERIOD,
                    samples: (0..60u64)
                        .map(|k| Sample::valid(now - (59 - k) * PERIOD, value))
                        .collect::<Vec<_>>()
                        .into(),
                })
            };
            let mut views: Vec<Arc<SeriesView>> = groups
                .iter()
                .enumerate()
                .map(|(i, v)| line(group_dev(i as u16), *v))
                .collect();
            views.push(line(OTHER_DEV, 1.5));
            views.extend(bands.iter().enumerate().map(|(b, v)| line(band_dev(b), *v)));
            views.sort_by_key(|v| v.id);
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.cpu.groups = Arc::new(
                (0..groups.len() as u16)
                    .map(|i| MemGroupBand {
                        key: Arc::from(format!("exe:p{i}")),
                        name: Arc::from(format!("p{i}")),
                        process_count: 2,
                        device: group_dev(i),
                        wine: None,
                    })
                    .collect(),
            );
        });
        draw(self.vcx);
    }

    /// Whether the page asks for the process sweep.
    fn sweep(&mut self) -> bool {
        let page = self.page.clone();
        self.vcx
            .update(|_, cx| page.read(cx).demand_part().process_top)
    }

    fn set_mode(&mut self, mode: &str) {
        self.page_action(&PerfOverviewSetBoxMode {
            kind: "cpu".into(),
            mode: mode.into(),
        });
    }

    /// The box's CPU by app chart and the content the fixed widget `id` draws.
    fn apps_and_content(&mut self, id: u32) -> (Entity<AppsChart>, Option<EntityId>) {
        let (cpu, container) = (self.cpu(), self.container());
        self.vcx.update(|_, cx| {
            let chart = cpu.read(cx).apps().cloned().expect("the chart");
            let content = container
                .read(cx)
                .fixed(id)
                .and_then(|f| f.read(cx).content().map(|v| v.entity_id()));
            (chart, content)
        })
    }
}

/// UI-PRF-062 "Where": the `apps` preset at 1200 × 900 shows CPU by app filling the box body,
/// drawn by its fixed widget, with the legend at the right; Core clocks takes no space; the page
/// asks for the process sweep. Under 480 px the legend goes below the chart.
#[gpui_kit::test]
fn overview_cpu_by_app_preset(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.page_action(&PerfOverviewSetBoxClocks {
        kind: "cpu".into(),
        on: true,
    });
    r.publish_cpu_groups(&[20., 10., 5.]);
    assert!(!r.sweep(), "no sweep before By app");
    r.set_mode("apps");
    r.publish_cpu_groups(&[20., 10., 5.]);
    assert!(r.sweep(), "By app asks for the process sweep");
    let cpu = r.cpu();
    assert_eq!(r.vcx.update(|_, cx| cpu.read(cx).mode()), "apps");
    assert!(r.vcx.update(|_, cx| cpu.read(cx).apps_placed()));

    let flow = r.flow();
    assert_eq!(flow.preset.as_ref().map(|p| p.id.as_str()), Some("apps"));
    let id = apps_of(&flow).expect("the cpu_by_app widget");
    assert!(total_of(&flow).is_none() && cores_of(&flow).is_none());
    let clocks = clocks_of(&flow).expect("Core clocks stays on");
    assert!(r.placed(clocks).is_none(), "Core clocks takes no space");
    assert_eq!(
        r.vcx
            .update(|_, cx| cpu.read(cx).drawn_widgets())
            .as_slice(),
        [id]
    );

    // The widget draws the box's chart in its Cpu kind and fills the body.
    let (chart, content) = r.apps_and_content(id);
    assert_eq!(
        content,
        Some(chart.entity_id()),
        "the widget draws the chart"
    );
    assert_eq!(
        r.vcx.update(|_, cx| chart.read(cx).metric()),
        AppsMetric::Cpu
    );
    assert!(
        r.vcx.update(|_, cx| chart.read(cx).bands().len()) >= 4,
        "three groups and Other apps at least"
    );
    let body = r.bounds_of(container_id(CPU_BOX_CONTAINER));
    let part = r.bounds_of(CPU_APPS_ID.to_owned());
    let widget = r.bounds_of(widget_id(CPU_BOX_CONTAINER, id));
    let at = r.placed(id).expect("placed");
    for b in [part, widget] {
        assert!(
            near(b.origin.x, body.origin.x)
                && near(b.origin.y, body.origin.y)
                && near(b.size.width, body.size.width)
                && near(b.size.height, body.size.height),
            "fills the body: {b:?} in {body:?}"
        );
    }
    assert!(
        near(at.origin.x, px(0.))
            && near(at.origin.y, px(0.))
            && near(at.size.width, body.size.width)
            && near(at.size.height, body.size.height),
        "placed where it is drawn: {at:?} in {body:?}"
    );
    let plot = r.bounds_of(APPS_ID.to_owned());
    let legend = r.bounds_of(LEGEND_ID.to_owned());
    assert!(body.size.width >= px(480.), "{body:?}");
    assert!(
        legend.left() >= plot.right() - px(1.) && widget.contains(&legend.center()),
        "the legend is at the right: {legend:?}, chart {plot:?}"
    );

    // A narrow box: the legend goes below the chart.
    r.vcx.simulate_resize(size(px(440.), px(900.)));
    draw(r.vcx);
    let body = r.bounds_of(container_id(CPU_BOX_CONTAINER));
    assert!(body.size.width < px(480.), "{body:?}");
    let plot = r.bounds_of(APPS_ID.to_owned());
    let legend = r.bounds_of(LEGEND_ID.to_owned());
    assert!(
        legend.top() >= plot.bottom() - px(1.),
        "the legend is below: {legend:?}, chart {plot:?}"
    );
    let widget = r.bounds_of(widget_id(CPU_BOX_CONTAINER, id));
    assert!(
        widget.contains(&legend.center()),
        "{legend:?} in {widget:?}"
    );

    // Back to the default: no sweep, and the widget is gone.
    r.vcx.simulate_resize(size(px(1200.), px(900.)));
    r.set_mode("chart_cores");
    assert!(!r.sweep(), "no sweep without By app");
    assert!(r.placed(id).is_none());
}

/// UI-PRF-062 "Edit Widgets": Add Widget lists "CPU by app" while the box has none; added,
/// moved and applied, the layout saves, reloads and draws the chart with the process sweep;
/// removed and applied, the sweep stops and Add Widget offers it again.
#[gpui_kit::test]
fn overview_cpu_by_app_edit_add_remove_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.publish_cpu_groups(&[20., 10., 5.]);
    r.page_action(&PerfWidgetEditBox { kind: "cpu".into() });
    let editor = r.editor();
    let fixed = r.vcx.update(|_, cx| editor.read(cx).fixed_add(cx));
    assert_eq!(
        fixed.map(|f| (f.name.to_string(), f.label.to_string())),
        Some(("cpu_by_app".to_owned(), "CPU by app".to_owned()))
    );
    let apps = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("cpu_by_app", cx)))
        .expect("CPU by app added");
    draw(r.vcx);
    assert!(
        r.vcx
            .update(|_, cx| editor.read(cx).fixed_add(cx))
            .is_none(),
        "CPU by app is offered once"
    );
    assert!(
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("cpu_by_app", cx)))
            .is_none()
    );
    assert!(r.sweep(), "the widget in Edit Widgets asks for the sweep");

    // Move it one cell, whichever way the grid has room.
    let cell_of = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .and_then(|l| l.widgets.into_iter().find(|w| w.id == apps))
            .and_then(|w| w.cell)
            .expect("its cell")
    };
    r.vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.select(apps, cx)));
    let before = cell_of(&mut r);
    let moved = [(-1, 0), (1, 0), (0, -1), (0, 1)]
        .into_iter()
        .any(|(dx, dy)| {
            r.vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.move_selected(dx, dy, cx)))
        });
    assert!(moved, "it moves");
    let after = cell_of(&mut r);
    assert_ne!((after.col, after.row), (before.col, before.row));
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);

    let saved = r
        .vcx
        .update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(CPU_BOX_CONTAINER)
                .cloned()
        })
        .expect("an overview.cpu entry");
    assert!(saved.preset.is_none(), "a grid layout");
    let w = saved
        .widgets
        .iter()
        .find(|w| w.id == apps)
        .expect("the widget saved");
    assert_eq!(w.content, WidgetContent::Fixed(FixedWidget::CpuByApp));
    assert_eq!(w.cell, Some(after), "the moved cell is saved");
    let (chart, content) = r.apps_and_content(apps);
    assert_eq!(
        content,
        Some(chart.entity_id()),
        "the widget draws the chart"
    );
    assert!(r.sweep(), "the saved widget asks for the sweep");

    // Reload: the settings go through JSON and the load's clamp, and the box shows them again.
    let perf = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.clone());
    let json = serde_json::to_string(&perf).expect("serialize");
    let mut reloaded = Settings {
        performance: serde_json::from_str(&json).expect("deserialize"),
        ..Settings::default()
    };
    reloaded.clamp();
    let entry = reloaded
        .performance
        .layouts
        .known(CPU_BOX_CONTAINER)
        .cloned()
        .expect("the entry reloads");
    assert_eq!(entry, saved, "the same layout after a reload");
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings.performance.layouts.remove(CPU_BOX_CONTAINER);
        });
    });
    draw(r.vcx);
    assert!(!r.sweep(), "the default layout has no CPU by app");
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.layouts.set(entry.clone()));
    });
    draw(r.vcx);
    let (chart, content) = r.apps_and_content(apps);
    assert_eq!(
        content,
        Some(chart.entity_id()),
        "the reloaded widget draws it"
    );
    assert!(
        r.vcx
            .debug_bounds(Box::leak(
                widget_id(CPU_BOX_CONTAINER, apps).into_boxed_str()
            ))
            .is_some(),
        "it draws"
    );
    assert!(r.sweep(), "the reloaded widget asks for the sweep");

    // Remove it and apply: the sweep stops and Add Widget offers it again.
    r.page_action(&PerfWidgetEditBox { kind: "cpu".into() });
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(apps, cx);
            assert!(e.remove_selected(cx));
        })
    });
    draw(r.vcx);
    assert!(
        r.vcx
            .update(|_, cx| editor.read(cx).fixed_add(cx))
            .is_some(),
        "offered again"
    );
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r.vcx.update(|_, cx| {
        TlSettings::settings(cx)
            .performance
            .layouts
            .known(CPU_BOX_CONTAINER)
            .cloned()
    });
    assert!(
        saved.is_none_or(|l| l.widgets.iter().all(|w| w.id != apps)),
        "removed"
    );
    assert!(!r.sweep(), "no sweep once removed");
}

/// UI-PRF-062 "Interaction": a click on a group's legend row in the box opens that group (the
/// `processes::` legend click tests follow it into the Processes tab); "Other apps" opens
/// nothing.
#[gpui_kit::test]
fn overview_cpu_by_app_legend_click(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.set_mode("apps");
    r.publish_cpu_groups(&[20., 10., 5.]);
    let id = apps_of(&r.flow()).expect("the cpu_by_app widget");
    let (chart, _) = r.apps_and_content(id);
    let widget = r.bounds_of(widget_id(CPU_BOX_CONTAINER, id));
    let opened = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| chart.read(cx).last_open.clone().map(|(k, _)| k))
    };
    // Rows: Other apps, p2, p1, p0, then the system bands under the divider.
    let row = r.bounds_of(format!("{LEGEND_ID}-0"));
    r.vcx.simulate_click(row.center(), Modifiers::none());
    draw(r.vcx);
    assert_eq!(opened(&mut r), None, "Other apps opens nothing");
    let row = r.bounds_of(format!("{LEGEND_ID}-1"));
    assert!(widget.contains(&row.center()), "{row:?} in {widget:?}");
    r.vcx.simulate_click(row.center(), Modifiers::none());
    draw(r.vcx);
    assert_eq!(opened(&mut r).as_deref(), Some("exe:p2"));
    let row = r.bounds_of(format!("{LEGEND_ID}-3"));
    r.vcx.simulate_click(row.center(), Modifiers::none());
    draw(r.vcx);
    assert_eq!(opened(&mut r).as_deref(), Some("exe:p0"));
}

/// Counts the widget, chart and by-app entities made, and the watched ones dropped.
struct Lifecycle {
    made: Rc<Cell<u32>>,
    dropped: Rc<Cell<u32>>,
    subs: Vec<Subscription>,
}

impl Lifecycle {
    fn new(r: &mut Rig) -> Self {
        let made = Rc::new(Cell::new(0));
        let count = || {
            let made = Rc::clone(&made);
            move || made.set(made.get() + 1)
        };
        let (a, b, c, d, e) = (count(), count(), count(), count(), count());
        let subs = r.vcx.update(|_, cx| {
            vec![
                cx.observe_new(move |_: &mut MetricWidget, _, _| a()),
                cx.observe_new(move |_: &mut GroupWidget, _, _| b()),
                cx.observe_new(move |_: &mut FixedWidgetView, _, _| c()),
                cx.observe_new(move |_: &mut ChartView, _, _| d()),
                cx.observe_new(move |_: &mut AppsChart, _, _| e()),
            ]
        });
        Self {
            made,
            dropped: Rc::new(Cell::new(0)),
            subs,
        }
    }

    fn watch<T: 'static>(&mut self, entity: &Entity<T>, cx: &mut App) {
        let dropped = Rc::clone(&self.dropped);
        self.subs.push(cx.observe_release(entity, move |_, _| {
            dropped.set(dropped.get() + 1);
        }));
    }

    fn counts(&self) -> (u32, u32) {
        (self.made.get(), self.dropped.get())
    }
}

/// Render contract (ADR-019, leader note 2026-11-03 hzire.6): in the `apps` preset repeated
/// samples with the same bands make and drop no entity and never notify the page, the box or
/// its container; a sample in which a group enters the top N notifies only the CPU by app chart,
/// once.
#[gpui_kit::test]
fn overview_cpu_by_app_render_contract(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, 1200., 900.);
    r.set_mode("apps");
    r.publish_cpu_groups(&[20., 10., 5.]);
    r.publish_cpu_groups(&[20., 10., 5.]);
    let id = apps_of(&r.flow()).expect("the cpu_by_app widget");
    let (chart, _) = r.apps_and_content(id);
    let (page, cpu, container) = (r.page.clone(), r.cpu(), r.container());
    let fixed = r
        .vcx
        .update(|_, cx| container.read(cx).fixed(id).cloned())
        .expect("the widget");
    let bands = |r: &mut Rig| r.vcx.update(|_, cx| chart.read(cx).bands().len());
    let shown = bands(&mut r);
    let notified = Rc::new(Cell::new(0u32));
    let _watch = {
        let notified = Rc::clone(&notified);
        r.vcx
            .update(|_, cx| cx.observe(&chart, move |_, _| notified.set(notified.get() + 1)))
    };
    let counts = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            (
                page.read(cx).notify_count.get(),
                cpu.read(cx).notify_count.get(),
                container.read(cx).notify_count.get(),
                fixed.read(cx).notify_count.get(),
            )
        })
    };
    let mut life = Lifecycle::new(&mut r);
    let inner = r.vcx.update(|_, cx| chart.read(cx).chart().clone());
    r.vcx.update(|_, cx| {
        life.watch(&chart, cx);
        life.watch(&inner, cx);
        life.watch(&fixed, cx);
    });
    let before = counts(&mut r);
    notified.set(0);
    for _ in 0..3 {
        r.publish_cpu_groups(&[20., 10., 5.]);
    }
    assert_eq!(
        counts(&mut r),
        before,
        "the page, box, container and widget never notify"
    );
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    assert_eq!(bands(&mut r), shown, "the same bands");

    // p3 gets CPU: it enters the top N.
    notified.set(0);
    r.publish_cpu_groups(&[20., 10., 5., 4.]);
    assert_eq!(bands(&mut r), shown + 1, "p3 has its own band");
    assert_eq!(notified.get(), 1, "the chart notifies once");
    assert_eq!(
        counts(&mut r),
        before,
        "the page, box, container and widget never notify"
    );
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    let same = r
        .vcx
        .update(|_, cx| container.read(cx).fixed(id).map(|f| f.entity_id()));
    assert_eq!(same, Some(fixed.entity_id()), "the same widget entity");
}
