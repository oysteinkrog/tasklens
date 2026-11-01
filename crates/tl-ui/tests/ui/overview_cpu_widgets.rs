//! The Overview CPU box on widgets (ADR-019 phase 2 "Presets", UI-PRF-061 phase 2): each old
//! mode is a flow layout of the preset's widgets placed where 1.2.0 drew the parts, the cores
//! group fills the height (D-040), a core sample renders only the cores group, and Edit Widgets
//! adds, removes, moves and applies CPU widgets. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Pixels,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, DeviceSelector, GroupKind, WidgetContent,
};
use tl_model::layout_boxes::{METRIC_CPU_KERNEL, METRIC_FREQ};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::time::now_boot_ns;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{
    PerfOverviewSetBoxClocks, PerfOverviewSetBoxMode, PerfWidgetEditBox, WidgetSource,
};
use tl_ui::pages::performance::overview::cpu_box::{
    CLOCKS_ID, CORE_CHARTS_ID, CPU_CHART_ID, CpuBox,
};
use tl_ui::pages::performance::overview::cpu_flow::{clocks_of, cores_of, total_of};
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::{WidgetContainer, container_id};
use tl_ui::pages::performance::widget::edit::WidgetEditor;
use tl_ui::pages::performance::widget::metric_widget::widget_id;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

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
        // Raw core values: two samples a few ms apart barely move a smoothed bar.
        settings.performance.overview_core_smoothing = tl_model::settings::CoreSmoothing::Off;
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
                widget_id(cores)
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
    let group = r.bounds_of(widget_id(cores));
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
            .debug_bounds(Box::leak(widget_id(kernel).into_boxed_str()))
            .is_some()
    );
}
