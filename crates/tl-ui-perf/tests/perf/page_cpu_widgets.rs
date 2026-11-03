//! The CPU page as a widget container (ADR-019 phase 3, UI-PRF-061, `tasklens-hzire.5`): the
//! default flow layout draws the 1.3.0 parts in their 1.3.0 places, each old `cpu_graph_mode`
//! and `cpu_secondary_graph` value converts once to the widgets it stood for, the header
//! buttons edit the page's entry and leave the old keys, the core grid fills its widget in a
//! grid layout, every CPU metric shows in any mix of forms, Edit Widgets adds, removes, moves
//! and applies, and a sample renders no page or container.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, Subscription, TestAppContext, VisualTestContext,
    Window, div, px, size,
};
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, CpuStaticInfo, RaplState, SeriesSet};
use tl_model::system::{CoreInfo, CoreKind, CpuSet, CpuTopology, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::grid_element::{grid_height, grid_layout as cell_layout};
use tl_ui_chart::chart::{ChartGridView, ChartView, ValueAlign};
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::widget::container::{WidgetContainer, WidgetView};
use tl_ui_overview::widget::details_widget::details_id;
use tl_ui_overview::widget::edit::saved_layout;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::group_widget::GroupWidget;
use tl_ui_overview::widget::metric_widget::{MetricWidget, widget_id};
use tl_ui_overview::widget::source::Availability;
use tl_ui_perf::performance::actions::{PerfSetCpuGraphMode, PerfSetCpuSecondaryGraph};
use tl_ui_perf::performance::cpu::{CpuPage, layout_mode, layout_secondary};
use tl_ui_perf::performance::cpu_board::BOARD_ID;
use tl_ui_perf::performance::cpu_grid::grid_id;
use tl_ui_perf::performance::page::{DevicePage as _, PAGE_PAD};
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::WidgetSource;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, GroupItems, GroupKind, GroupOptions,
    GroupSelector, LAYOUT_VERSION, MetricKey, SourceSelector, WidgetCell, WidgetContent,
    WidgetOptions, WidgetSetting,
};
use tl_ui_settings::settings::{
    AppearanceSettings, CpuGraphMode, CpuSecondaryGraph, PerformanceSettings, Settings,
    ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

const PAGE: &str = "page.cpu";
const W: f32 = 1200.;
const H: f32 = 900.;

fn entry(idx: u16, identity: DeviceIdentity, key: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("AMD Ryzen 9 7950X"),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
        hot_c: None,
    }
}

/// 16 logical CPUs on 8 cores, CPU `c` and `c + 8` siblings.
fn topology() -> CpuTopology {
    let mut online = CpuSet::empty();
    for id in 0..16 {
        online.insert(id);
    }
    CpuTopology {
        online,
        cores: (0..8u16)
            .map(|c| CoreInfo {
                id: c,
                logical: smallvec![c, c + 8],
                l3_group: 0,
                kind: CoreKind::Standard,
            })
            .collect(),
    }
}

/// The CPU package (`DeviceIdx(0)`) and 16 logical CPUs (`100 + id`), with every value the
/// secondary charts read. `power` is whether RAPL reads; `temp` whether a CPU sensor does.
fn publish_cpu(engine: &FakeEngine, power: bool, temp: bool, util: f32) {
    engine.publish_with(|s| {
        let mut entries = vec![entry(0, DeviceIdentity::CpuPackage, "cpu")];
        entries.extend((0..16u16).map(|id| {
            entry(
                100 + id,
                DeviceIdentity::CpuLogical { id },
                &format!("cpu:{id}"),
            )
        }));
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        sys.cpu_topology = Arc::new(topology());
        sys.cpu.util_pct = Metric::valid(util);
        sys.cpu.kernel_pct = Metric::valid(2.3);
        sys.cpu.freq_mhz = Metric::valid(4631);
        sys.cpu.temp_c = if temp {
            Metric::valid(55.)
        } else {
            Metric::with(Validity::Unsupported)
        };
        sys.cpu.package_power_w = if power {
            Metric::valid(65.)
        } else {
            Metric::with(Validity::Unsupported)
        };
        sys.cpu.core_power_w = Metric::valid(40.);
        // The static RAPL state changes only when `power` does, as on a real machine.
        let rapl = Some(if power {
            RaplState::Readable
        } else {
            RaplState::NoZone
        });
        let dd = Arc::make_mut(&mut s.device_details);
        if dd.cpu.rapl != rapl {
            dd.header.version += 1;
            dd.cpu = Arc::new(CpuStaticInfo {
                rapl,
                ..CpuStaticInfo::default()
            });
        }
    });
}

/// The page, cached, so it renders only when it notifies.
struct Host(Entity<CpuPage>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<CpuPage>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

/// A 1200 × 900 window with the CPU page, built with `perf` loaded as the load does it
/// (`Settings::clamp` converts the page's old keys). RAPL and the CPU sensor read.
fn open(cx: &mut TestAppContext, perf: PerformanceSettings) -> Rig<'_> {
    open_with(cx, perf, true, true)
}

fn open_with(
    cx: &mut TestAppContext,
    perf: PerformanceSettings,
    power: bool,
    temp: bool,
) -> Rig<'_> {
    let engine = FakeEngine::new();
    publish_cpu(&engine, power, temp, 18.6);
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
        let mut settings = Settings {
            performance: perf,
            ..Settings::default()
        };
        settings.motion.smooth_graphs = false;
        settings.clamp();
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
        let key = DeviceKey(Arc::from(DeviceKey::CPU));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::Cpu,
            idx: DeviceIdx(0),
            generation: 0,
            net_class: None,
            fan_connected: None,
        };
        Host(cx.new(|cx| CpuPage::new(key, row, hub, demand, cx)))
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(W), px(H)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let page = vcx.read_entity(&host, |h, _| h.0.clone());
    vcx.update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    // The header buttons and menu items reach the page as actions, as in the window.
    let focus = vcx.update(|_, cx| gpui_kit::Focusable::focus_handle(page.read(cx), cx));
    vcx.update(|window, cx| focus.focus(window, cx));
    draw(vcx);
    Rig {
        engine,
        page,
        vcx,
        _driver: driver,
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn near(a: Pixels, b: f32) -> bool {
    (f32::from(a) - b).abs() < 1.
}

fn perf(mode: CpuGraphMode, graph: CpuSecondaryGraph) -> PerformanceSettings {
    PerformanceSettings {
        cpu_graph_mode: mode,
        cpu_secondary_graph: graph,
        ..PerformanceSettings::default()
    }
}

fn metric(device: &str, key: &str, forms: FormSet, cell: Option<WidgetCell>) -> WidgetSetting {
    WidgetSetting {
        id: 0,
        content: WidgetContent::Metric(SourceSelector {
            device: DeviceSelector::Key(Arc::from(device)),
            metric: MetricKey::new(key),
        }),
        forms,
        cell,
        options: WidgetOptions::default(),
        device_name: None,
    }
}

fn cell(col: u16, row: u16, cols: u16, rows: u16) -> Option<WidgetCell> {
    Some(WidgetCell {
        col,
        row,
        cols,
        rows,
    })
}

/// A grid layout of the CPU page with `widgets`, ids from 1.
fn grid_layout(widgets: Vec<WidgetSetting>) -> ContainerLayout {
    let widgets: Vec<WidgetSetting> = widgets
        .into_iter()
        .enumerate()
        .map(|(i, w)| WidgetSetting {
            id: i as u32 + 1,
            ..w
        })
        .collect();
    ContainerLayout {
        container: PAGE.to_owned(),
        version: LAYOUT_VERSION,
        preset: None,
        next_id: widgets.len() as u32 + 1,
        widgets,
    }
}

/// One widget as "<metric> [forms]" or "fixed <name>" or "group <members>".
fn shape(l: &ContainerLayout) -> Vec<String> {
    l.widgets
        .iter()
        .map(|w| match &w.content {
            WidgetContent::Metric(s) => {
                let forms: Vec<&str> = w.forms.names().collect();
                format!("{} [{}]", s.metric.as_str(), forms.join(","))
            }
            WidgetContent::Fixed(f) => format!("fixed {}", f.name()),
            WidgetContent::Group(g) => format!("group {}", g.members.name()),
        })
        .collect()
}

impl Rig<'_> {
    fn container(&mut self) -> Entity<WidgetContainer> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).container().clone())
    }

    fn bounds(&mut self, id: String) -> Bounds<Pixels> {
        let id = leak(id);
        self.vcx.debug_bounds(id).expect(id)
    }

    fn try_bounds(&mut self, id: String) -> Option<Bounds<Pixels>> {
        self.vcx.debug_bounds(leak(id))
    }

    /// The id of the first widget whose content matches `f`.
    fn id_of(&mut self, f: impl Fn(&WidgetContent) -> bool) -> Option<u32> {
        let container = self.container();
        self.vcx.update(|_, cx| {
            container
                .read(cx)
                .layout()
                .widgets
                .iter()
                .find(|w| f(&w.content))
                .map(|w| w.id)
        })
    }

    fn metric_id(&mut self, key: &str) -> u32 {
        self.id_of(|c| c.metric_key().is_some_and(|k| k.as_str() == key))
            .unwrap_or_else(|| panic!("a {key} widget"))
    }

    fn fixed_id(&mut self, kind: FixedWidget) -> u32 {
        self.id_of(|c| *c == WidgetContent::Fixed(kind.clone()))
            .expect("a fixed widget")
    }

    /// The tooltip of widget `id`'s source.
    fn tip(&mut self, id: u32) -> Option<String> {
        let container = self.container();
        self.vcx.update(|_, cx| {
            container
                .read(cx)
                .source(id)
                .and_then(|s| s.read(cx).tip().map(|t| t.to_string()))
        })
    }

    fn saved(&mut self) -> Option<ContainerLayout> {
        self.vcx.update(|_, cx| saved_layout(PAGE, cx))
    }

    fn old_keys(&mut self) -> (CpuGraphMode, CpuSecondaryGraph) {
        self.vcx.update(|_, cx| {
            let p = &TlSettings::settings(cx).performance;
            (p.cpu_graph_mode, p.cpu_secondary_graph)
        })
    }

    /// The layout the page shows now.
    fn shown(&mut self) -> ContainerLayout {
        let container = self.container();
        self.vcx.update(|_, cx| container.read(cx).layout().clone())
    }
}

/// The default flow layout draws the 1.3.0 page at 1200 × 900: the per-core grid as tall as
/// its rows at the page width, Power draw 120 px `sp4` under it, `details_gap` above Details.
/// Nothing is written: no old key is off its default.
#[gpui_kit::test]
fn page_cpu_flow_matches_1_3_0(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    assert!(r.saved().is_none(), "no entry");
    let grid = r.bounds(grid_id(PAGE));
    let power = r.metric_id("power_draw");
    let power = r.bounds(widget_id(PAGE, power));
    let details = r.bounds(details_id(PAGE));
    let body_w = W - 2. * PAGE_PAD;
    assert!(near(grid.size.width, body_w), "{grid:?}");
    // As tall as its rows at the page width: 112 px rows, as in 1.3.0.
    let page = r.page.clone();
    let (cell_h, natural) = r.vcx.update(|_, cx| {
        let spec = page.read(cx).grid().view.read(cx).spec.clone();
        let layout = cell_layout(spec.cells.len(), px(body_w), spec.cell_min_w, spec.gap);
        (spec.cell_h, grid_height(&layout, px(112.), spec.gap))
    });
    assert!(near(cell_h, 112.), "{cell_h:?}");
    assert!(near(grid.size.height, f32::from(natural)), "{grid:?}");
    assert!(near(power.top() - grid.bottom(), 8.), "sp4 under the grid");
    assert!(near(power.size.height, 120.), "{power:?}");
    assert!(near(power.size.width, body_w));
    assert!(near(details.top() - power.bottom(), 24.), "details_gap");
    // The fallbacks take no space while Power draw reads.
    for key in ["temp", "clock"] {
        let id = r.metric_id(key);
        assert!(r.try_bounds(widget_id(PAGE, id)).is_none(), "{key}");
    }
    // Power draw carries the 1.3.0 heading, range word and value.
    r.vcx.update(|_, cx| {
        let chart = page.read(cx).secondary(cx).expect("power chart");
        let chart = chart.read(cx);
        assert_eq!(chart.labels.heading.as_deref(), Some("Power draw"));
        assert_eq!(chart.labels.range_word.as_ref(), "Power draw");
        assert!(chart.labels.show_axis_max);
    });

    // Overall: the Utilization chart, 300 px, with Total and Kernel.
    r.vcx.dispatch_action(PerfSetCpuGraphMode {
        mode: CpuGraphMode::Overall,
    });
    draw(r.vcx);
    let total = r.metric_id("total");
    let chart = r.bounds(widget_id(PAGE, total));
    assert!(near(chart.size.height, 300.) && near(chart.size.width, body_w));
    r.vcx.update(|_, cx| {
        let chart = page.read(cx).overall(cx).expect("overall chart");
        let chart = chart.read(cx);
        assert_eq!(chart.labels.heading.as_deref(), Some("Utilization"));
        assert_ne!(
            chart.labels.value_align,
            ValueAlign::Hidden,
            "the utilization value"
        );
    });

    // Core board: the total chart at 40% of the width, the board after a 16 px gutter.
    r.vcx.dispatch_action(PerfSetCpuGraphMode {
        mode: CpuGraphMode::Board,
    });
    draw(r.vcx);
    let total = r.metric_id("total");
    let board = r.fixed_id(FixedWidget::CpuCoreBoard);
    let chart = r.bounds(widget_id(PAGE, total));
    let board = r.bounds(widget_id(PAGE, board));
    assert!(near(chart.size.width, (body_w * 0.4).round()), "{chart:?}");
    assert!(near(
        board.left() - chart.left(),
        (body_w * 0.4).round() + 16.
    ));
    assert!(near(board.right() - chart.left(), body_w));
    assert!(near(chart.size.height, 300.) && near(board.size.height, 300.));
    assert!(r.vcx.debug_bounds(BOARD_ID).is_some(), "the board's rows");
}

/// Each old `cpu_graph_mode` and `cpu_secondary_graph` value gives the expected widgets: the
/// page converts once, at load, and leaves the old keys as they were.
#[gpui_kit::test]
fn page_cpu_old_toggles_give_widgets(cx: &mut TestAppContext) {
    use CpuGraphMode::*;
    use CpuSecondaryGraph as G;
    let cvl = |k: &str| format!("{k} [chart,value,label]");
    let cases: [(CpuGraphMode, CpuSecondaryGraph, Vec<String>); 5] = [
        (Overall, G::None, vec![cvl("total")]),
        (
            PerCore,
            G::Clock,
            vec!["fixed cpu_core_grid".into(), cvl("clock")],
        ),
        (
            AllCoresOverlaid,
            G::Clocks,
            vec![
                "cores_overlaid [chart,value,label]".into(),
                "core_clocks [chart,label]".into(),
            ],
        ),
        (
            AllCoresStacked,
            G::Temperature,
            vec!["cores_stacked [chart,value,label]".into(), cvl("temp")],
        ),
        (
            Board,
            G::Power,
            vec![
                cvl("total"),
                "fixed cpu_core_board".into(),
                cvl("power_draw"),
                cvl("temp"),
                cvl("clock"),
            ],
        ),
    ];
    let mut r = open(cx, PerformanceSettings::default());
    for (mode, graph, mut want) in cases {
        // A load with the keys set: the page's entry is the converted flow layout.
        let mut s = Settings {
            performance: perf(mode, graph),
            ..Settings::default()
        };
        s.clamp();
        let l = s
            .performance
            .layouts
            .known(PAGE)
            .cloned()
            .unwrap_or_else(|| panic!("{mode:?} {graph:?} converts"));
        want.push("fixed details".into());
        assert_eq!(shape(&l), want, "{mode:?} {graph:?}");
        assert_eq!(layout_mode(&l), Some(mode));
        assert_eq!(layout_secondary(&l), graph);
        assert_eq!(s.performance.cpu_graph_mode, mode, "the old key stays");
        // The page shows it.
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.performance.layouts.set(l.clone()));
        });
        draw(r.vcx);
        let page = r.page.clone();
        assert_eq!(r.vcx.update(|_, cx| page.read(cx).mode()), Some(mode));
        assert_eq!(r.shown(), l);
    }
}

/// The header button and the page menu on a page without an entry write the entry and leave
/// `cpu_graph_mode` and `cpu_secondary_graph` as they were; the secondary widgets stay through
/// a mode change.
#[gpui_kit::test]
fn page_cpu_header_button_writes_entry_only(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    assert!(r.saved().is_none());
    let power = r.metric_id("power_draw");
    r.vcx.dispatch_action(PerfSetCpuGraphMode {
        mode: CpuGraphMode::Overall,
    });
    draw(r.vcx);
    let saved = r.saved().expect("an entry");
    assert_eq!(layout_mode(&saved), Some(CpuGraphMode::Overall));
    assert_eq!(
        r.old_keys(),
        (CpuGraphMode::PerCore, CpuSecondaryGraph::Power)
    );
    assert_eq!(
        r.metric_id("power_draw"),
        power,
        "the secondary widget keeps its id"
    );
    r.vcx.dispatch_action(PerfSetCpuSecondaryGraph {
        choice: CpuSecondaryGraph::Clocks,
    });
    draw(r.vcx);
    let saved = r.saved().expect("an entry");
    assert_eq!(layout_secondary(&saved), CpuSecondaryGraph::Clocks);
    assert_eq!(
        shape(&saved),
        [
            "total [chart,value,label]",
            "core_clocks [chart,label]",
            "fixed details"
        ]
    );
    assert_eq!(
        r.old_keys(),
        (CpuGraphMode::PerCore, CpuSecondaryGraph::Power)
    );
}

/// Without RAPL the Temperature chart takes Power draw's place with the 1.3.0 note on its
/// heading; without a sensor either, Clock speed does.
#[gpui_kit::test]
fn page_cpu_secondary_fallback(cx: &mut TestAppContext) {
    let mut r = open_with(cx, PerformanceSettings::default(), false, true);
    let page = r.page.clone();
    let temp = r.metric_id("temp");
    let grid = r.bounds(grid_id(PAGE));
    let shown = r.bounds(widget_id(PAGE, temp));
    assert!(
        near(shown.top() - grid.bottom(), 8.),
        "in Power draw's place"
    );
    let power = r.metric_id("power_draw");
    assert!(r.try_bounds(widget_id(PAGE, power)).is_none());
    assert_eq!(
        r.tip(temp).as_deref(),
        Some("Power draw is not available. Showing Temperature.")
    );
    assert_eq!(
        r.vcx.update(|_, cx| page.read(cx).secondary_choice(cx)),
        Some(CpuSecondaryGraph::Temperature)
    );
    // No sensor either: Clock speed, with its note; the Temperature note goes.
    publish_cpu(&r.engine, false, false, 18.6);
    draw(r.vcx);
    let clock = r.metric_id("clock");
    assert!(r.try_bounds(widget_id(PAGE, clock)).is_some());
    assert_eq!(
        r.tip(clock).as_deref(),
        Some("Power draw is not available. Showing Clock speed.")
    );
    assert_eq!(r.tip(temp), None);
    // Temperature picked on its own is no fallback: no note.
    r.vcx.dispatch_action(PerfSetCpuSecondaryGraph {
        choice: CpuSecondaryGraph::Temperature,
    });
    publish_cpu(&r.engine, false, true, 18.6);
    draw(r.vcx);
    let temp = r.metric_id("temp");
    assert_eq!(r.tip(temp), None);
}

/// In a grid layout the core grid lays out its columns by its width and its rows share the
/// widget's height, at least 112 px each; a widget too short for that scrolls the grid.
#[gpui_kit::test]
fn page_cpu_core_grid_fills_widget(cx: &mut TestAppContext) {
    let fixed = |rows| WidgetSetting {
        content: WidgetContent::Fixed(FixedWidget::CpuCoreGrid),
        ..metric("cpu", "total", FormSet::empty(), cell(0, 0, 6, rows))
    };
    let mut perf = PerformanceSettings::default();
    // 6 of 12 columns, 30 rows of 24 px with 12 px gutters: 576 × 708 px.
    perf.layouts.set(grid_layout(vec![fixed(30)]));
    let mut r = open(cx, perf);
    let page = r.page.clone();
    let id = r.fixed_id(FixedWidget::CpuCoreGrid);
    let widget = r.bounds(widget_id(PAGE, id));
    let grid = r.bounds(grid_id(PAGE));
    assert!(
        near(grid.size.width, f32::from(widget.size.width)),
        "{grid:?} {widget:?}"
    );
    // The columns follow the widget's width; the rows share its height.
    let rows_at = |r: &mut Rig, width: Pixels| {
        r.vcx.update(|_, cx| {
            let spec = page.read(cx).grid().view.read(cx).spec.clone();
            let layout = cell_layout(spec.cells.len(), width, spec.cell_min_w, spec.gap);
            (layout.rows, spec.gap, spec.cell_h)
        })
    };
    let (rows, gap, cell_h) = rows_at(&mut r, widget.size.width);
    assert!(rows > 1, "{rows}");
    let want = (widget.size.height - gap * (rows - 1) as f32) / rows as f32;
    assert!(want > px(112.), "the widget is taller than the rows need");
    assert!(near(cell_h, f32::from(want)), "{cell_h:?} for {widget:?}");
    assert!(
        near(grid.size.height, f32::from(widget.size.height)),
        "fills the widget"
    );

    // A short widget: rows stay 112 px and the grid is taller than its widget.
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings
                .performance
                .layouts
                .set(grid_layout(vec![fixed(8)]));
        });
    });
    draw(r.vcx);
    let widget = r.bounds(widget_id(PAGE, id));
    let (_, _, cell_h) = rows_at(&mut r, widget.size.width);
    assert!(near(cell_h, 112.), "{cell_h:?}");
    let grid = r.bounds(grid_id(PAGE));
    assert!(
        grid.size.height > widget.size.height,
        "the grid scrolls in its widget"
    );
}

/// One `CPU_TOTAL` and one `CPU_FREQ` sample per logical CPU and the package's
/// `CPU_CLOCK_MAX`: these values come from the series, not the system snapshot. Also the
/// highest clock, which a clock bar fills to.
fn publish_cores(engine: &FakeEngine) {
    let newest = now_boot_ns();
    let one = |id: u16, metric: MetricId, v: f32| {
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(100 + id),
                metric,
            },
            version: 1,
            period_ns: 100_000_000,
            samples: Arc::from([Sample::valid(newest, v)]),
        })
    };
    // The package's fastest core clock.
    let one_pkg = |metric: MetricId, v: f32| {
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(0),
                metric,
            },
            version: 1,
            period_ns: 100_000_000,
            samples: Arc::from([Sample::valid(newest, v)]),
        })
    };
    let mut views: Vec<Arc<SeriesView>> = (0..16u16)
        .flat_map(|id| {
            [
                one(id, MetricId::CPU_TOTAL, 20. + f32::from(id)),
                one(id, MetricId::CPU_FREQ, 4000. + f32::from(id)),
            ]
        })
        .chain([one_pkg(MetricId::CPU_CLOCK_MAX, 5050.)])
        .collect();
    // The set is looked up by id, in order.
    views.sort_by_key(|v| v.id);
    engine.publish_with(|s| {
        s.series = Arc::new(SeriesSet {
            header: ComponentHeader {
                version: 1,
                ..ComponentHeader::default()
            },
            series: views.into(),
            slow: Arc::from([]),
        });
        // The clock bars' full scale, `cpuinfo_max_freq`.
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        inv.processor.max_mhz = tl_model::snapshot::InvField {
            value: 5050,
            validity: Validity::Valid,
        };
    });
}

/// The forms of each widget of [`page_cpu_every_metric_any_form`], in list order.
fn any_form_widgets() -> Vec<WidgetSetting> {
    let (c, b, v, s, l) = (
        FormSet::CHART,
        FormSet::BAR,
        FormSet::VALUE,
        FormSet::STATS,
        FormSet::LABEL,
    );
    let cores = WidgetSetting {
        content: WidgetContent::Group(GroupSelector {
            device: DeviceSelector::Key(Arc::from("cpu")),
            members: GroupKind::Cores,
            metric: MetricKey::new("load"),
            options: GroupOptions {
                limit: None,
                items: GroupItems::Rows,
                arrange: None,
            },
        }),
        ..metric(
            "cpu",
            "total",
            FormSet::BAR | FormSet::VALUE | FormSet::LABEL,
            cell(0, 38, 12, 16),
        )
    };
    vec![
        metric("cpu", "total", c | v | l, cell(0, 0, 6, 7)),
        metric("cpu", "kernel", b | v | s | l, cell(6, 0, 6, 3)),
        metric("cpu:3", "load", c | b | v | l, cell(6, 3, 6, 4)),
        metric("cpu", "freq", v | s | l, cell(0, 7, 6, 2)),
        metric("cpu", "temp", c | v | s | l, cell(6, 7, 6, 8)),
        metric("cpu", "power_draw", c | l, cell(0, 9, 6, 7)),
        metric("cpu", "cores_overlaid", c | l, cell(0, 16, 6, 7)),
        metric("cpu", "cores_stacked", c, cell(6, 16, 6, 7)),
        metric("cpu", "clock", c | v, cell(0, 23, 6, 7)),
        metric("cpu", "freq_max", b | v | l, cell(6, 23, 6, 3)),
        metric("cpu:9", "freq", v | l, cell(6, 26, 6, 2)),
        WidgetSetting {
            content: WidgetContent::Fixed(FixedWidget::CpuCoreGrid),
            ..metric("cpu", "total", FormSet::empty(), cell(0, 30, 12, 8))
        },
        cores,
    ]
}

/// Every CPU metric (the total, the kernel time, each core's load and clock, the clock speed,
/// the fastest core, the temperature, the power, the all-cores charts) shows in the forms the
/// layout asks for, all cores as a group, and the core grid in a grid layout. "Source >" and
/// Add Widget list the CPU's metrics, the core lists and "All cores >".
#[gpui_kit::test]
fn page_cpu_every_metric_any_form(cx: &mut TestAppContext) {
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(any_form_widgets()));
    let mut r = open(cx, perf);
    publish_cores(&r.engine);
    draw(r.vcx);
    let container = r.container();
    let want = any_form_widgets();
    r.vcx.update(|_, cx| {
        let c = container.read(cx);
        for (i, w) in want.iter().enumerate() {
            let id = i as u32 + 1;
            match &w.content {
                WidgetContent::Metric(s) => {
                    let name = s.metric.as_str();
                    let widget = c.widget(id).unwrap_or_else(|| panic!("{name}"));
                    let widget = widget.read(cx);
                    assert_eq!(widget.availability(), Availability::Available, "{name}");
                    assert_eq!(widget.drawn_forms(), w.forms, "{name}");
                }
                WidgetContent::Group(_) => assert!(c.group(id).is_some(), "all cores"),
                WidgetContent::Fixed(_) => {
                    let fixed = c.fixed(id).expect("the core grid");
                    assert!(fixed.read(cx).content().is_some(), "the page's grid view");
                }
            }
        }
    });
    // With a grid layout "Secondary graph >" is hidden; "Graph >" stays.
    let page = r.page.clone();
    let names = r.vcx.update(|window, cx| {
        let focus = gpui_kit::Focusable::focus_handle(page.read(cx), cx);
        let menu = tl_ui_kit::components::menu::TlMenu::build(window, cx, focus, |menu, w, cx| {
            page.update(cx, |p, cx| p.menu_items(menu, w, cx))
        });
        menu.read(cx)
            .entries_for_test(window)
            .into_iter()
            .map(|e| e.label.map(|l| l.to_string()).unwrap_or_default())
            .collect::<Vec<_>>()
    });
    assert!(names.iter().any(|n| n == "Graph"), "{names:?}");
    assert!(!names.iter().any(|n| n == "Secondary graph"), "{names:?}");
    // The source list: the CPU box's metrics, then the phase 3 keys, the core lists and the
    // cores group.
    let groups = r.vcx.update(|_, cx| {
        let editor = page.read(cx).editor().clone();
        editor.read(cx).source_groups(cx)
    });
    assert_eq!(groups.len(), 1, "the CPU only");
    let keys: Vec<&str> = groups[0].items.iter().map(|i| i.metric.as_str()).collect();
    for key in [
        "total",
        "kernel",
        "freq",
        "temp",
        "power",
        "power_draw",
        "core_clocks",
        "cores_overlaid",
        "cores_stacked",
        "clock",
        "freq_max",
    ] {
        assert!(keys.contains(&key), "{key} in {keys:?}");
    }
    let at = |k: &str| keys.iter().position(|x| *x == k).expect(k);
    assert!(
        at("core_clocks") < at("cores_overlaid"),
        "box keys first: {keys:?}"
    );
    let subs: Vec<String> = groups[0].subs.iter().map(|s| s.label.to_string()).collect();
    assert_eq!(subs, ["Core load", "Core speed"]);
    assert_eq!(groups[0].subs[0].items.len(), 16);
    assert!(groups[0].add_group.is_some(), "All cores >");
}

/// Edit Widgets on the CPU page pins the flow layout, adds the kernel time, moves it, removes
/// the Power draw widget and applies: the page's entry is the grid layout, the grid keeps its
/// entities, and the old keys stay.
#[gpui_kit::test]
fn page_cpu_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    let container = r.container();
    let grid = r.fixed_id(FixedWidget::CpuCoreGrid);
    let grid_entity = r
        .vcx
        .update(|_, cx| container.read(cx).fixed(grid).map(|w| w.entity_id()));
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    let source = WidgetSource {
        device: Arc::from("cpu"),
        metric: Arc::from("kernel"),
    };
    let added = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("added");
    draw(r.vcx);
    let before = r
        .vcx
        .update(|_, cx| editor.read(cx).working_layout(cx))
        .and_then(|l| l.widgets.iter().find(|w| w.id == added)?.cell)
        .expect("the added widget's cell");
    // Move it one row down: it was added last, so the row below is free.
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(added, cx);
            assert!(e.move_selected(0, 1, cx), "moved");
        })
    });
    // The core board, added as a part: the page gives it its content.
    let board = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("cpu_core_board", cx)))
        .expect("the board added");
    let power = r.metric_id("power_draw");
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(power, cx);
            assert!(e.remove_selected(cx), "removed");
        })
    });
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r.saved().expect("the page's entry");
    assert!(saved.preset.is_none(), "a grid layout");
    let moved = saved
        .widgets
        .iter()
        .find(|w| w.id == added)
        .and_then(|w| w.cell)
        .expect("the added widget");
    assert_eq!(moved.row, before.row + 1, "moved one row down");
    assert_eq!(moved.col, before.col);
    assert!(
        saved.widgets.iter().any(|w| w.id == board),
        "the board kept"
    );
    let board_content = r.vcx.update(|_, cx| {
        container
            .read(cx)
            .fixed(board)
            .map(|f| f.read(cx).content().is_some())
    });
    assert_eq!(board_content, Some(true), "the board has its rows");
    assert!(r.vcx.debug_bounds(BOARD_ID).is_some(), "the board draws");
    assert!(
        !saved.widgets.iter().any(|w| w.id == power),
        "Power draw removed"
    );
    assert!(
        saved
            .widgets
            .iter()
            .any(|w| matches!(w.content, WidgetContent::Fixed(FixedWidget::CpuCoreGrid))),
        "the core grid is pinned with the rest"
    );
    assert_eq!(
        r.vcx
            .update(|_, cx| container.read(cx).fixed(grid).map(|w| w.entity_id())),
        grid_entity,
        "Apply keeps the grid's entity"
    );
    assert_eq!(
        r.old_keys(),
        (CpuGraphMode::PerCore, CpuSecondaryGraph::Power)
    );
    // The kernel widget draws the page's kernel time.
    let page = r.page.clone();
    let shown = r.vcx.update(|_, cx| {
        page.read(cx)
            .container()
            .read(cx)
            .widget(added)
            .map(|w| w.read(cx).availability())
    });
    assert_eq!(shown, Some(Availability::Available));
}

/// Counts the widget, chart and grid entities made while it lives, and the drops of the
/// entities [`shown_entities`] watched.
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
                cx.observe_new(move |_: &mut ChartGridView, _, _| e()),
            ]
        });
        Self {
            made,
            dropped: Rc::new(Cell::new(0)),
            subs,
        }
    }

    fn watch<T: 'static>(&mut self, entity: &Entity<T>, cx: &mut App) -> EntityId {
        let dropped = Rc::clone(&self.dropped);
        self.subs.push(cx.observe_release(entity, move |_, _| {
            dropped.set(dropped.get() + 1);
        }));
        entity.entity_id()
    }

    fn counts(&self) -> (u32, u32) {
        (self.made.get(), self.dropped.get())
    }
}

/// The entities the page shows, in placement order: every placed widget and the chart of each
/// metric widget, then the grid part and its chart grid. Each is watched for a drop.
fn shown_entities(r: &mut Rig, life: &mut Lifecycle) -> Vec<EntityId> {
    let (page, container) = (r.page.clone(), r.container());
    r.vcx.update(|_, cx| {
        let mut ids = Vec::new();
        for id in container.read(cx).placed_ids() {
            match container.read(cx).view(id).cloned() {
                Some(WidgetView::Metric(w)) => {
                    ids.push(life.watch(&w, cx));
                    if let Some(chart) = w.read(cx).chart().cloned() {
                        ids.push(life.watch(&chart, cx));
                    }
                }
                Some(WidgetView::Group(g)) => ids.push(life.watch(&g, cx)),
                Some(WidgetView::Fixed(f)) => ids.push(life.watch(&f, cx)),
                None => {}
            }
        }
        let view = page.read(cx).grid_view().clone();
        let grid = view.read(cx).grid().clone();
        ids.push(life.watch(&view, cx));
        ids.push(life.watch(&grid, cx));
        ids
    })
}

/// A sample that changes nothing the page shows renders no page or container; a sample that
/// moves the CPU's values notifies neither the page nor the container. No sample makes or drops
/// a widget, chart or grid entity (ADR-019 phase 3 render contract).
#[gpui_kit::test]
fn page_cpu_sample_renders_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(cx, perf(CpuGraphMode::Board, CpuSecondaryGraph::Power));
    let container = r.container();
    let page = r.page.clone();
    let counts = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            let c = container.read(cx);
            (
                p.render_count.get(),
                p.notify_count.get(),
                c.render_count.get(),
                c.notify_count.get(),
            )
        })
    };
    let publish = |r: &mut Rig, util: f32| {
        publish_cpu(&r.engine, true, true, util);
        draw(r.vcx);
    };
    publish(&mut r, 18.6);
    let mut life = Lifecycle::new(&mut r);
    let entities = shown_entities(&mut r, &mut life);
    // The total chart, the board, Power draw, Details, the two charts and the grid part.
    assert!(entities.len() >= 7, "{entities:?}");
    let before = counts(&mut r);
    for _ in 0..3 {
        publish(&mut r, 18.6);
    }
    assert_eq!(counts(&mut r), before, "nothing shown changed");
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    publish(&mut r, 50.);
    let after = counts(&mut r);
    assert_eq!(after.1, before.1, "the page never notifies");
    assert_eq!(after.3, before.3, "the container never notifies");
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    assert_eq!(
        shown_entities(&mut r, &mut life),
        entities,
        "the same entities"
    );
}

/// The page's widgets draw the same in the flow layout, in Edit Widgets and after Apply
/// (tasklens-hzire.10, the look sweep).
#[gpui_kit::test]
fn page_cpu_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    let container = r.container();
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    super::page_look::sweep(r.vcx, &container, &editor, "cpu");
}

/// The all-cores charts show the CPU total at the top right, as 1.3.0 did, in the page's own
/// layout and in a widget placed by Edit Widgets (tasklens-hzire.11), and look the same in
/// Edit Widgets and after Apply.
#[gpui_kit::test]
fn page_cpu_all_cores_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    for (mode, key) in [
        (CpuGraphMode::AllCoresOverlaid, "cores_overlaid"),
        (CpuGraphMode::AllCoresStacked, "cores_stacked"),
    ] {
        let mut r = open(cx, perf(mode, CpuSecondaryGraph::None));
        publish_cpu(&r.engine, false, false, 21.6);
        publish_cores(&r.engine);
        draw(r.vcx);
        let container = r.container();
        let id = r.metric_id(key);
        let chart = r.vcx.update(|_, cx| {
            let c = container.read(cx);
            c.widget(id)
                .and_then(|w| w.read(cx).chart().cloned())
                .expect("the cores chart")
        });
        let painted = r
            .vcx
            .update(|_, cx| chart.read(cx).painted_value_text().map(str::to_owned));
        assert_eq!(painted.as_deref(), Some("21.6%"), "{key}");
        let editor = {
            let page = r.page.clone();
            r.vcx.update(|_, cx| page.read(cx).editor().clone())
        };
        super::page_look::sweep(r.vcx, &container, &editor, "cpu");
    }

    // A widget placed by Edit Widgets, with the forms Add Widget gives it.
    let forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(vec![metric(
        "cpu",
        "cores_overlaid",
        forms,
        cell(0, 0, 12, 6),
    )]));
    let mut r = open(cx, perf);
    publish_cpu(&r.engine, false, false, 21.6);
    publish_cores(&r.engine);
    draw(r.vcx);
    let container = r.container();
    let chart = r.vcx.update(|_, cx| {
        let c = container.read(cx);
        c.widget(1)
            .and_then(|w| w.read(cx).chart().cloned())
            .expect("the placed chart")
    });
    let painted = r
        .vcx
        .update(|_, cx| chart.read(cx).painted_value_text().map(str::to_owned));
    assert_eq!(painted.as_deref(), Some("21.6%"), "placed widget");
}

/// True when the engine got `cpu_fast` in its latest demand, after the page's send delay.
fn fast_sent(r: &mut Rig) -> bool {
    r.vcx.executor().advance_clock(Duration::from_millis(200));
    draw(r.vcx);
    r.engine
        .demands()
        .last()
        .expect("a demand was sent")
        .cpu_fast
}

/// The page asks for 10 Hz samples in `mode` while it shows, and not when it is hidden
/// (R-126, hzire.12).
fn check_fast_mode(cx: &mut TestAppContext, mode: CpuGraphMode) {
    let mut r = open(cx, perf(mode, CpuSecondaryGraph::default()));
    let page = r.page.clone();
    assert!(
        r.vcx.update(|_, cx| page.read(cx).demand_part(cx).cpu_fast),
        "{mode:?}: demand part"
    );
    assert!(fast_sent(&mut r), "{mode:?}: shown");
    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(false, cx)));
    assert!(!fast_sent(&mut r), "{mode:?}: hidden");
    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    assert!(fast_sent(&mut r), "{mode:?}: shown again");
}

#[gpui_kit::test]
fn page_cpu_fast_overall(cx: &mut TestAppContext) {
    check_fast_mode(cx, CpuGraphMode::Overall);
}

#[gpui_kit::test]
fn page_cpu_fast_per_core(cx: &mut TestAppContext) {
    check_fast_mode(cx, CpuGraphMode::PerCore);
}

#[gpui_kit::test]
fn page_cpu_fast_all_cores_overlaid(cx: &mut TestAppContext) {
    check_fast_mode(cx, CpuGraphMode::AllCoresOverlaid);
}

#[gpui_kit::test]
fn page_cpu_fast_all_cores_stacked(cx: &mut TestAppContext) {
    check_fast_mode(cx, CpuGraphMode::AllCoresStacked);
}

#[gpui_kit::test]
fn page_cpu_fast_board(cx: &mut TestAppContext) {
    check_fast_mode(cx, CpuGraphMode::Board);
}

/// A page with a value widget only asks for no 10 Hz samples. A CPU chart widget added by
/// Edit Widgets turns them on, with no core grid on the page.
#[gpui_kit::test]
fn page_cpu_fast_chart_widget_added(cx: &mut TestAppContext) {
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(vec![metric(
        "cpu",
        "total",
        FormSet::VALUE,
        cell(0, 0, 4, 2),
    )]));
    let mut r = open(cx, perf);
    assert!(!fast_sent(&mut r), "a value widget alone");
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    let source = WidgetSource {
        device: Arc::from("cpu"),
        metric: Arc::from("kernel"),
    };
    r.vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("added");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r.saved().expect("the page's entry");
    assert!(
        !saved
            .widgets
            .iter()
            .any(|w| matches!(w.content, WidgetContent::Fixed(FixedWidget::CpuCoreGrid))),
        "no core grid"
    );
    assert!(fast_sent(&mut r), "a CPU chart widget");
}
