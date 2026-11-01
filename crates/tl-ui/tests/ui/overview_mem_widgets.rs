//! The Overview Memory box on widgets (ADR-019 phase 2 "Presets", UI-PRF-061 phase 2): each old
//! mode is a flow layout of the preset's widgets placed where 1.2.0 drew the parts, a memory
//! sample renders only the rows that changed, the By app widget keeps its legend click, Edit
//! Widgets adds, removes, moves and applies Memory widgets (Memory by app among them), and a
//! reset converted box reloads as the built-in default. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, Modifiers, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div,
    px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    ContainerLayout, FixedWidget, MEMORY_BOX_CONTAINER, WidgetContent, box_layout,
    convert_old_settings,
};
use tl_model::layout_boxes::{METRIC_MEM_SWAP, METRIC_PRESSURE};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::MemGroupBand;
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{
    PerfOverviewSetBoxArrange, PerfOverviewSetBoxMode, PerfWidgetEditBox, WidgetSource,
};
use tl_ui::pages::performance::memory_apps::LEGEND_ID;
use tl_ui::pages::performance::overview::mem_box::{
    MEM_APPS_ID, MEM_COMP_CHART_ID, MEM_USED_CHART_ID, MemBox, ROWS, cell_id, row_id, spark_id,
};
use tl_ui::pages::performance::overview::mem_flow::{
    apps_of, comp_of, row_of, rows_of, used_chart_of,
};
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::{WidgetContainer, container_id};
use tl_ui::pages::performance::widget::edit::{WidgetEditor, default_layout};
use tl_ui::pages::performance::widget::metric_widget::{FlowLook, widget_id};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: f32 = 1024. * 1024.;

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

fn entry(idx: u16, identity: DeviceIdentity, key: &str, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
        hot_c: None,
    }
}

/// The CPU package at 0, memory at 1 and the memory groups p0 to p2, "Other apps" and
/// "Shared and kernel" from 2 on.
fn catalog(version: u64) -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage, "cpu", "CPU"),
        entry(1, DeviceIdentity::Memory, "memory", "Memory"),
    ];
    for (i, (key, name)) in groups().into_iter().enumerate() {
        entries.push(entry(
            2 + i as u16,
            DeviceIdentity::MemGroup {
                key: Arc::from(key.as_str()),
            },
            &format!("memgroup:{key}"),
            &name,
        ));
    }
    DeviceCatalog {
        version,
        entries: Arc::from(entries),
    }
}

/// The memory groups: (key, name), p0 to p2, then the two fixed bands.
fn groups() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = (0..3)
        .map(|i| (format!("exe:p{i}"), format!("p{i}")))
        .collect();
    out.push(("other".to_owned(), "Other apps".to_owned()));
    out.push(("shared".to_owned(), "Shared and kernel".to_owned()));
    out
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Overview page filling it, and memory totals.
fn open(cx: &mut TestAppContext, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog(1));
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
        // Leave the Network box out, as the Memory box tests were written. Widget selectors have
        // the container id (tasklens-6swdr.10), so other boxes do not shadow them.
        let mut overview = OverviewLayoutSetting::grid_default();
        overview.grid.retain(|b| b.kind != "net");
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

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
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

    fn set_mode(&mut self, mode: &str) {
        self.page_action(&PerfOverviewSetBoxMode {
            kind: "memory".into(),
            mode: mode.into(),
        });
    }

    fn mem(&mut self) -> Entity<MemBox> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).mem_box().clone())
    }

    fn container(&mut self) -> Entity<WidgetContainer> {
        let mem = self.mem();
        self.vcx.update(|_, cx| mem.read(cx).container().clone())
    }

    fn editor(&mut self) -> Entity<WidgetEditor> {
        let mem = self.mem();
        self.vcx.update(|_, cx| mem.read(cx).editor().clone())
    }

    fn flow(&mut self) -> ContainerLayout {
        let mem = self.mem();
        self.vcx.update(|_, cx| mem.read(cx).flow().clone())
    }

    fn bounds_of(&mut self, id: String) -> gpui_kit::Bounds<Pixels> {
        let id = leak(id);
        self.vcx.debug_bounds(id).expect(id)
    }

    /// The placement of widget `id` in the container's body, px.
    fn placed(&mut self, id: u32) -> Option<gpui_kit::Bounds<Pixels>> {
        let container = self.container();
        self.vcx
            .update(|_, cx| container.read(cx).placement().bounds(id))
    }

    /// Whether the page asks for the process sweep.
    fn sweep(&mut self) -> bool {
        let page = self.page.clone();
        self.vcx
            .update(|_, cx| page.read(cx).demand_part().process_top)
    }

    /// New memory totals: `used` GiB in use of 64.
    fn publish_used(&mut self, used: u64) {
        self.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.memory.used = used * GIB;
            sys.memory.available = (64 - used) * GIB;
        });
        draw(self.vcx);
    }

    /// Groups p0 to p2 of 3 to 1 MiB, "Other apps" 1 MiB and "Shared and kernel" 2 MiB, 60
    /// one-second samples each.
    fn publish_groups(&mut self) {
        const PERIOD: u64 = 1_000_000_000;
        let now = now_boot_ns();
        let mibs = [3., 2., 1., 1., 2.];
        self.engine.publish_with(|s| {
            s.devices = Arc::new(catalog(s.devices.version + 1));
            let version = s.series.header.version + 1;
            let views: Vec<Arc<SeriesView>> = mibs
                .iter()
                .enumerate()
                .map(|(i, mib)| {
                    Arc::new(SeriesView {
                        id: SeriesId {
                            device: DeviceIdx(2 + i as u16),
                            metric: MetricId::MEM_GROUP,
                        },
                        version,
                        period_ns: PERIOD,
                        samples: (0..60u64)
                            .map(|k| Sample::valid(now - (59 - k) * PERIOD, mib * MIB))
                            .collect::<Vec<_>>()
                            .into(),
                    })
                })
                .collect();
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
            sys.memory.groups = Arc::new(
                groups()
                    .into_iter()
                    .enumerate()
                    .filter(|(_, (key, _))| key.starts_with("exe:"))
                    .map(|(i, (key, name))| MemGroupBand {
                        key: Arc::from(key),
                        name: Arc::from(name),
                        process_count: 2,
                        device: DeviceIdx(2 + i as u16),
                        wine: None,
                    })
                    .collect(),
            );
        });
        draw(self.vcx);
    }
}

/// UI-PRF-061 phase 2, ADR-019 "Presets": the default and each old mode of the Memory box are
/// flow layouts of the preset's widgets. Each widget is drawn by its own entity at the place
/// 1.2.0 drew the part, and the container places it there from the box's plan, not from a
/// paint. Edit Widgets pins the drawn widgets to the grid, and Cancel keeps the preset.
#[gpui_kit::test]
fn overview_mem_flow_matches_modes(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let body = container_id(MEMORY_BOX_CONTAINER);
    let modes: [(&str, Option<&str>); 7] = [
        ("rows", None),
        ("chart_rows", Some("auto")),
        ("chart_rows", Some("horizontal")),
        ("chart", None),
        ("charts", Some("auto")),
        ("charts", Some("vertical")),
        ("apps", None),
    ];
    for (mode, arrange) in modes {
        r.set_mode(mode);
        if let Some(arrange) = arrange {
            r.page_action(&PerfOverviewSetBoxArrange {
                kind: "memory".into(),
                arrange: arrange.into(),
            });
        }
        let flow = r.flow();
        assert_eq!(
            flow.preset.as_ref().map(|p| p.id.as_str()),
            Some(mode),
            "the mode is the preset"
        );
        // (widget, the part the box drew for it)
        let mut parts: Vec<(u32, String)> = Vec::new();
        match mode {
            "rows" | "chart_rows" => {
                if mode == "chart_rows" {
                    let chart = used_chart_of(&flow).expect("the used chart");
                    parts.push((chart, MEM_USED_CHART_ID.to_owned()));
                }
                for (i, id) in rows_of(&flow).into_iter().enumerate() {
                    parts.push((id.expect("a row widget"), row_id(i)));
                }
            }
            "chart" => parts.push((comp_of(&flow).expect("comp"), MEM_COMP_CHART_ID.to_owned())),
            "charts" => {
                for (i, id) in rows_of(&flow).into_iter().enumerate() {
                    parts.push((id.expect("a cell widget"), cell_id(i)));
                }
            }
            _ => parts.push((apps_of(&flow).expect("By app"), MEM_APPS_ID.to_owned())),
        }
        let origin = r.bounds_of(body.clone()).origin;
        for (id, part) in &parts {
            let at = r
                .placed(*id)
                .unwrap_or_else(|| panic!("{mode} {arrange:?}: {id} placed"));
            let painted = r.bounds_of(part.clone());
            let (x, y) = (painted.origin.x - origin.x, painted.origin.y - origin.y);
            // A row in `rows` also holds its sparkline: the text part starts the row, the
            // sparkline ends it.
            let right = if mode == "rows" {
                let i = rows_of(&flow).iter().position(|r| *r == Some(*id)).unwrap();
                r.bounds_of(spark_id(i)).right() - origin.x
            } else {
                painted.right() - origin.x
            };
            assert!(
                near(at.origin.x, x)
                    && near(at.origin.y, y)
                    && near(at.right(), right)
                    && near(at.size.height, painted.size.height),
                "{mode} {arrange:?}: {id} {part}: placed {at:?}, painted {painted:?} from {origin:?}"
            );
        }
        // Each part is its widget's own entity, in the Memory look.
        let container = r.container();
        for (id, _) in &parts {
            let look = r.vcx.update(|_, cx| {
                let c = container.read(cx);
                if let Some(w) = c.widget(*id) {
                    match w.read(cx).flow() {
                        Some(FlowLook::MemRow { .. }) => "row",
                        Some(FlowLook::MemCell { .. }) => "cell",
                        Some(FlowLook::Chart(_)) => "chart",
                        _ => "other",
                    }
                } else if c.fixed(*id).is_some() {
                    "fixed"
                } else {
                    "none"
                }
            });
            let want = match mode {
                "rows" => "row",
                "chart_rows" if Some(*id) == used_chart_of(&flow) => "chart",
                "chart_rows" => "row",
                "chart" => "chart",
                "charts" => "cell",
                _ => "fixed",
            };
            assert_eq!(look, want, "{mode}: widget {id}");
        }

        // Edit Widgets pins every drawn widget to the grid, and Cancel puts the preset back.
        r.page_action(&PerfWidgetEditBox {
            kind: "memory".into(),
        });
        let editor = r.editor();
        let working = r
            .vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .expect("Edit Widgets is on");
        assert!(working.preset.is_none(), "{mode}: pinned to a grid layout");
        let mut pinned: Vec<u32> = working.widgets.iter().map(|w| w.id).collect();
        pinned.sort_unstable();
        let mut drawn: Vec<u32> = parts.iter().map(|p| p.0).collect();
        drawn.sort_unstable();
        assert_eq!(pinned, drawn, "{mode}: the drawn widgets, no more");
        assert!(working.widgets.iter().all(|w| w.cell.is_some()));
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
        draw(r.vcx);
        assert!(!r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        assert_eq!(r.flow(), flow, "{mode}: Cancel keeps the flow layout");
    }
}

/// The render contract on widgets: a memory sample renders the widgets of the rows whose text
/// changed, not the container, and no widget entity is made or dropped. The box renders only
/// for its title value.
#[gpui_kit::test]
fn overview_mem_sample_renders_only_changed_rows(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.publish_used(20);
    let mem = r.mem();
    let container = r.container();
    let flow = r.flow();
    let row = |i: usize| row_of(&flow, i).expect("a row widget");
    let entity = |r: &mut Rig, i: usize| {
        let container = container.clone();
        r.vcx
            .update(|_, cx| container.read(cx).widget(row(i)).cloned())
            .expect("the row's entity")
    };
    let (used, cached) = (entity(&mut r, 0), entity(&mut r, 1));
    let counts = |r: &mut Rig| {
        let (mem, container, used, cached) =
            (mem.clone(), container.clone(), used.clone(), cached.clone());
        r.vcx.update(|_, cx| {
            [
                mem.read(cx).notify_count.get(),
                container.read(cx).notify_count.get(),
                used.read(cx).notify_count.get(),
                cached.read(cx).notify_count.get(),
            ]
        })
    };
    let before = counts(&mut r);
    r.publish_used(30);
    let after = counts(&mut r);
    assert_eq!(after[1], before[1], "the container does not notify");
    assert!(after[2] > before[2], "In use notifies");
    assert_eq!(
        after[3], before[3],
        "Cached did not change and does not notify"
    );
    assert_eq!(
        after[0],
        before[0] + 1,
        "the box notifies once, for its title"
    );
    assert_eq!(
        entity(&mut r, 0).entity_id(),
        used.entity_id(),
        "same entity"
    );
    let rows = r.vcx.update(|_, cx| mem.read(cx).rows());
    assert_eq!(rows[0].value.as_ref(), "30.00 GiB");
}

/// The render contract with a short body: a row the body leaves out is not placed and has no
/// entity, so a sample that changes only that row renders nothing, not even the box.
#[gpui_kit::test]
fn overview_mem_sample_skips_clipped_rows(cx: &mut TestAppContext) {
    // Below 720 px the boxes stack at their minimum height (220 px for Memory), and the text
    // part is too narrow for one-line rows: four two-line rows fit, Commit does not.
    let mut r = open(cx, 400., 900.);
    let mem = r.mem();
    let container = r.container();
    let flow = r.flow();
    let placed: Vec<_> = (0..ROWS)
        .map(|i| r.placed(row_of(&flow, i).expect("a row widget")))
        .collect();
    let commit = row_of(&flow, ROWS - 1).expect("a row widget");
    assert!(
        placed[0].is_some() && placed[ROWS - 1].is_none(),
        "the short body leaves out the Commit row: {placed:?}"
    );
    let has_entity = |r: &mut Rig| {
        let container = container.clone();
        r.vcx
            .update(|_, cx| container.read(cx).widget(commit).is_some())
    };
    assert!(!has_entity(&mut r), "the left-out row has no entity");
    let count = |r: &mut Rig| {
        let mem = mem.clone();
        r.vcx.update(|_, cx| mem.read(cx).notify_count.get())
    };
    let before = count(&mut r);
    r.engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.memory.committed = 30 * GIB;
    });
    draw(r.vcx);
    assert_eq!(count(&mut r), before, "the box does not notify");
    assert!(!has_entity(&mut r), "no entity is made for the row");
}

/// UI-PRF-060 on widgets: in `apps` the Memory by app fixed widget draws the box's
/// memory-by-app chart, a click on a group's legend row opens that group (the
/// `processes::` legend click tests follow it into the Processes tab), and the page asks for
/// the process sweep while the widget shows.
#[gpui_kit::test]
fn overview_mem_by_app_legend_click(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1200.);
    r.publish_groups();
    assert!(!r.sweep());
    r.set_mode("apps");
    r.publish_groups();
    assert!(r.sweep(), "By app asks for the process sweep");
    let mem = r.mem();
    let container = r.container();
    let id = apps_of(&r.flow()).expect("the By app widget");
    let (chart, content) = r.vcx.update(|_, cx| {
        let chart = mem.read(cx).apps().cloned().expect("the chart");
        let fixed = container.read(cx).fixed(id).cloned().expect("the widget");
        let content = fixed.read(cx).content().map(|v| v.entity_id());
        (chart, content)
    });
    assert_eq!(
        content,
        Some(chart.entity_id()),
        "the widget draws the chart"
    );
    let widget = r.bounds_of(widget_id(MEMORY_BOX_CONTAINER, id));
    // Rows: Other apps, p2, p1, p0, then Shared and kernel under the divider.
    let row = r.bounds_of(format!("{LEGEND_ID}-1"));
    assert!(
        widget.contains(&row.center()),
        "the legend is in the widget: {row:?} in {widget:?}"
    );
    r.vcx.simulate_click(row.center(), Modifiers::none());
    draw(r.vcx);
    let opened = r
        .vcx
        .update(|_, cx| chart.read(cx).last_open.clone().map(|(k, _)| k));
    assert_eq!(opened.as_deref(), Some("exe:p2"));

    r.set_mode("rows");
    assert!(!r.sweep(), "no sweep without By app");
}

/// UI-PRF-061 phase 2: in Edit Widgets the Memory box lists its metrics and Memory by app,
/// adds a widget and Memory by app, removes, moves, and Apply saves a grid layout that the box
/// then draws, with the process sweep for Memory by app.
#[gpui_kit::test]
fn overview_mem_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.page_action(&PerfWidgetEditBox {
        kind: "memory".into(),
    });
    let editor = r.editor();
    assert!(r.vcx.read_entity(&editor, |e, _| e.is_editing()));

    let groups = r.vcx.update(|_, cx| editor.read(cx).source_groups(cx));
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(&*groups[0].key, "memory");
    let labels: Vec<String> = groups[0]
        .items
        .iter()
        .map(|i| i.label.to_string())
        .collect();
    assert_eq!(
        labels[..5],
        ["In use", "Cached", "Available", "Swap", "Commit"],
        "{labels:?}"
    );
    assert!(groups[0].add_group.is_none(), "memory has no group");
    let fixed = r.vcx.update(|_, cx| editor.read(cx).fixed_add(cx));
    assert_eq!(
        fixed.map(|f| f.label.to_string()).as_deref(),
        Some("Memory by app")
    );

    let source = |metric: &str| WidgetSource {
        device: Arc::from("memory"),
        metric: Arc::from(metric),
    };
    let swap = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(METRIC_MEM_SWAP), cx)))
        .expect("swap added");
    let pressure = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(METRIC_PRESSURE), cx)))
        .expect("pressure added");
    let apps = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("memory_by_app", cx)))
        .expect("Memory by app added");
    draw(r.vcx);
    assert!(
        r.vcx
            .update(|_, cx| editor.read(cx).fixed_add(cx))
            .is_none(),
        "Memory by app is offered once"
    );
    assert!(
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("memory_by_app", cx)))
            .is_none()
    );
    assert!(
        r.sweep(),
        "the By app widget in Edit Widgets asks for the sweep"
    );

    // Remove Swap, then move Memory pressure one cell toward the free side.
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(swap, cx);
            assert!(e.remove_selected(cx));
            e.select(pressure, cx);
        })
    });
    let cell_of = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .and_then(|l| l.widgets.into_iter().find(|w| w.id == pressure))
            .and_then(|w| w.cell)
            .expect("pressure cell")
    };
    let before = cell_of(&mut r);
    let dx: i32 = if before.col > 0 { -1 } else { 1 };
    assert!(
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.move_selected(dx, 0, cx)))
    );
    let after = cell_of(&mut r);
    assert_eq!(
        i64::from(after.col),
        i64::from(before.col) + i64::from(dx),
        "{before:?} → {after:?}"
    );
    let saved = r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    assert!(saved, "Apply saves");
    draw(r.vcx);

    let entry = r
        .vcx
        .update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(MEMORY_BOX_CONTAINER)
                .cloned()
        })
        .expect("an overview.memory entry");
    assert!(entry.preset.is_none(), "a grid layout");
    let ids: Vec<u32> = entry.widgets.iter().map(|w| w.id).collect();
    assert!(ids.contains(&pressure) && ids.contains(&apps), "{ids:?}");
    assert!(!ids.contains(&swap), "removed: {ids:?}");
    let a = entry.widgets.iter().find(|w| w.id == apps).unwrap();
    assert_eq!(a.content, WidgetContent::Fixed(FixedWidget::MemoryByApp));

    // The box draws the user's widgets now, each by its own entity.
    let mem = r.mem();
    assert!(r.vcx.update(|_, cx| mem.read(cx).grid().is_some()));
    let container = r.container();
    let placed = r.vcx.update(|_, cx| container.read(cx).placed_ids());
    assert!(
        placed.contains(&pressure) && placed.contains(&apps),
        "{placed:?}"
    );
    assert!(
        r.vcx
            .update(|_, cx| container.read(cx).fixed(apps).is_some())
    );
    assert!(
        r.vcx
            .debug_bounds(leak(widget_id(MEMORY_BOX_CONTAINER, pressure)))
            .is_some()
    );
    assert!(
        r.vcx
            .debug_bounds(leak(widget_id(MEMORY_BOX_CONTAINER, apps)))
            .is_some()
    );
    assert!(r.sweep(), "the saved By app widget asks for the sweep");
}

/// A converted Memory box (old `mode`, `split` and `arrange`), reset in Edit Widgets and
/// applied, reloads as the built-in default with no `overview.memory` entry and no old keys
/// (ADR-019 "Saving the default").
#[gpui_kit::test]
fn overview_mem_reset_survives_reload(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let converted = r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            let mut layout = OverviewLayoutSetting::grid_default();
            let mem = layout
                .grid
                .iter_mut()
                .find(|b| b.kind == "memory")
                .expect("a Memory box");
            mem.mode = Some("chart_rows".to_owned());
            mem.split = Some(0.6);
            mem.arrange = Some("horizontal".to_owned());
            d.settings.performance.overview = Some(layout);
            assert!(convert_old_settings(&mut d.settings.performance));
        });
        box_layout(&TlSettings::settings(cx).performance, "memory").expect("memory")
    });
    draw(r.vcx);
    assert_eq!(
        converted.preset.as_ref().map(|p| p.id.as_str()),
        Some("chart_rows")
    );
    let mem = r.mem();
    assert_eq!(r.vcx.update(|_, cx| mem.read(cx).mode()), "chart_rows");

    r.page_action(&PerfWidgetEditBox {
        kind: "memory".into(),
    });
    let editor = r.editor();
    assert!(
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.reset_confirmed(cx)))
    );
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);

    let perf = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.clone());
    assert!(perf.layouts.entry(MEMORY_BOX_CONTAINER).is_none());
    let entry = perf
        .overview
        .as_ref()
        .and_then(|o| o.grid.iter().find(|b| b.kind == "memory"))
        .expect("the Memory box stays on the page");
    assert_eq!(entry.mode, None);
    assert_eq!(entry.split, None);
    assert_eq!(entry.arrange, None);
    let mut reloaded = perf.clone();
    assert!(!convert_old_settings(&mut reloaded), "nothing converts");
    assert_eq!(
        box_layout(&reloaded, "memory").expect("memory"),
        default_layout(MEMORY_BOX_CONTAINER)
    );
    assert_eq!(r.vcx.update(|_, cx| mem.read(cx).mode()), "rows");
}
