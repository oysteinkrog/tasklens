//! The Memory page as a widget container (ADR-019 phase 3, UI-PRF-061, `tasklens-hzire.6`): the
//! default flow layout draws the 1.3.0 parts in their 1.3.0 places, each old `memory_view`,
//! `memory_swap_graph` and `memory_composition` value converts once to the widgets it stood
//! for, the switch and the page menu edit the page's entry and leave the old keys, Memory by
//! app keeps its legend click, every memory metric shows in any mix of forms, Edit Widgets adds,
//! removes, moves and applies, and a sample renders no page or container.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, IntoElement, Modifiers,
    ParentElement as _, Pixels, Render, StyleRefinement, Styled as _, Subscription, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{
    ComponentHeader, InvField, MemoryDetails, SeriesSet, SwapDevice, SwapKindModel,
};
use tl_model::system::MemGroupBand;
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::overview::memory_apps::LEGEND_ID;
use tl_ui_overview::widget::container::{WidgetContainer, WidgetView};
use tl_ui_overview::widget::details_widget::{DetailsView, details_id};
use tl_ui_overview::widget::edit::saved_layout;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::metric_widget::{MetricWidget, part_id, widget_id};
use tl_ui_overview::widget::source::Availability;
use tl_ui_perf::performance::actions::{PerfToggleMemoryComposition, PerfToggleMemorySwapGraph};
use tl_ui_perf::performance::memory::{
    MemoryPage, VIEW_SWITCH_ID, layout_bar, layout_swap, layout_view,
};
use tl_ui_perf::performance::memory_views::{MemoryPartView, PART_BAR, PART_SWITCH};
use tl_ui_perf::performance::page::{DevicePage as _, PAGE_PAD};
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::WidgetSource;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, LAYOUT_VERSION, MetricKey,
    SourceSelector, WidgetCell, WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_ui_settings::settings::{
    AppearanceSettings, MemoryView, PerformanceSettings, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

const PAGE: &str = "page.memory";
const W: f32 = 1200.;
const H: f32 = 900.;
const GIB: u64 = 1 << 30;
const MIB: f32 = (1 << 20) as f32;

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

/// The memory groups: (key, name), p0 to p2, then the two fixed bands.
fn groups() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = (0..3)
        .map(|i| (format!("exe:p{i}"), format!("p{i}")))
        .collect();
    out.push(("other".to_owned(), "Other apps".to_owned()));
    out.push(("shared".to_owned(), "Shared and kernel".to_owned()));
    out
}

/// The CPU, memory (`DeviceIdx(1)`) and the memory groups from `DeviceIdx(2)`.
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

/// Memory totals with `used` bytes in use and 8 GiB of swap.
fn publish_memory(engine: &FakeEngine, used: u64) {
    engine.publish_with(|s| {
        if s.devices.entries.is_empty() {
            s.devices = Arc::new(catalog(1));
        }
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        let m = &mut sys.memory;
        m.validity = Validity::Valid;
        m.total = 64 * GIB;
        m.used = used;
        m.available = 64 * GIB - used;
        m.free = 20 * GIB;
        m.cached = 10 * GIB;
        m.committed = 30 * GIB;
        m.commit_limit = 80 * GIB;
        m.dirty = 120 << 20;
        m.swap_total = 8 * GIB;
        m.swap_free = 7 * GIB;
    });
}

/// The page, cached, so it renders only when it notifies.
struct Host(Entity<MemoryPage>);

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
    page: Entity<MemoryPage>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

/// A 1200 × 900 window with the Memory page, built with `perf` loaded as the load does it
/// (`Settings::clamp` converts the page's old keys).
fn open(cx: &mut TestAppContext, perf: PerformanceSettings) -> Rig<'_> {
    let engine = FakeEngine::new();
    publish_memory(&engine, 23 * GIB);
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
        let key = DeviceKey(Arc::from(DeviceKey::MEMORY));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::Memory,
            idx: DeviceIdx(1),
            generation: 0,
            net_class: None,
            fan_connected: None,
        };
        Host(cx.new(|cx| MemoryPage::new(key, row, hub, demand, cx)))
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
    // The menu items reach the page as actions, as in the window.
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

fn perf(view: MemoryView, swap: bool, bar: bool) -> PerformanceSettings {
    PerformanceSettings {
        memory_view: view,
        memory_swap_graph: swap,
        memory_composition: bar,
        ..PerformanceSettings::default()
    }
}

fn metric(key: &str, forms: FormSet, cell: Option<WidgetCell>) -> WidgetSetting {
    WidgetSetting {
        id: 0,
        content: WidgetContent::Metric(SourceSelector {
            device: DeviceSelector::Key(Arc::from("memory")),
            metric: MetricKey::new(key),
        }),
        forms,
        cell,
        options: WidgetOptions::default(),
        device_name: None,
    }
}

fn fixed(kind: FixedWidget, cell: Option<WidgetCell>) -> WidgetSetting {
    WidgetSetting {
        content: WidgetContent::Fixed(kind),
        ..metric("used", FormSet::empty(), cell)
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

/// A grid layout of the Memory page with `widgets`, ids from 1.
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

/// One widget as "<metric> [forms]" or "fixed <name>".
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

    fn chart(&mut self, key: &str) -> Entity<ChartView> {
        let page = self.page.clone();
        self.vcx
            .update(|_, cx| page.read(cx).chart_of(key, cx))
            .unwrap_or_else(|| panic!("the {key} chart"))
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

    fn old_keys(&mut self) -> (MemoryView, bool, bool) {
        self.vcx.update(|_, cx| {
            let p = &TlSettings::settings(cx).performance;
            (p.memory_view, p.memory_swap_graph, p.memory_composition)
        })
    }

    /// The layout the page shows now.
    fn shown(&mut self) -> ContainerLayout {
        let container = self.container();
        self.vcx.update(|_, cx| container.read(cx).layout().clone())
    }

    /// Five memory groups of 3, 2, 1, 1 and 2 MiB: p0 to p2, Other apps, Shared and kernel.
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

/// The default flow layout draws the 1.3.0 page at 1200 × 900: the Usage and By app switch at
/// the top, Physical memory, Commit and Swap 150 px `sp4` apart, the composition bar `sp6`
/// under them and `details_gap` above Details. The charts carry the 1.3.0 headings, range words,
/// legend and the Commit tooltip. Nothing is written: no old key is off its default.
#[gpui_kit::test]
fn page_mem_flow_matches_1_3_0(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    assert!(r.saved().is_none(), "no entry");
    let body_w = W - 2. * PAGE_PAD;
    let switch = r.bounds(VIEW_SWITCH_ID.to_owned());
    let switch_part = r.bounds(tl_ui_perf::performance::memory_views::part_id(
        PAGE,
        PART_SWITCH,
    ));
    let ids: Vec<u32> = ["physical", "commit_pct", "swap"]
        .iter()
        .map(|k| r.metric_id(k))
        .collect();
    let charts: Vec<Bounds<Pixels>> = ids
        .iter()
        .map(|&id| r.bounds(widget_id(PAGE, id)))
        .collect();
    let bar = r.bounds(tl_ui_perf::performance::memory_views::part_id(
        PAGE, PART_BAR,
    ));
    let details = r.bounds(details_id(PAGE));
    // The switch is right-aligned in its row above Physical memory.
    assert!(
        near(switch.right(), f32::from(charts[0].right())),
        "{switch:?}"
    );
    assert!(
        near(charts[0].top(), f32::from(switch_part.bottom())),
        "{switch_part:?}"
    );
    for c in &charts {
        assert!(
            near(c.size.height, 150.) && near(c.size.width, body_w),
            "{c:?}"
        );
    }
    assert!(near(charts[1].top() - charts[0].bottom(), 8.), "sp4");
    assert!(near(charts[2].top() - charts[1].bottom(), 8.), "sp4");
    assert!(
        near(bar.top() - charts[2].bottom(), 16.),
        "sp6 above the bar"
    );
    assert!(near(details.top() - bar.bottom(), 24.), "details_gap");
    // No zram, swap device or module data: the blocks take no space.
    for kind in [
        FixedWidget::MemoryCompressed,
        FixedWidget::SwapDevices,
        FixedWidget::MemoryModules,
    ] {
        let id = r.fixed_id(kind.clone());
        assert!(r.try_bounds(widget_id(PAGE, id)).is_none(), "{kind:?}");
    }
    // The 1.3.0 headings, range words and legend.
    for (key, heading, range) in [
        ("physical", "Physical memory", "Memory usage"),
        ("commit_pct", "Commit", "Commit"),
        ("swap", "Swap", "Swap usage"),
    ] {
        let chart = r.chart(key);
        r.vcx.read_entity(&chart, |v, _| {
            assert_eq!(v.labels.heading.as_deref(), Some(heading), "{key}");
            assert_eq!(v.labels.range_word.as_ref(), range, "{key}");
        });
    }
    let chart = r.chart("physical");
    let legend: Vec<String> = r.vcx.read_entity(&chart, |v, _| {
        v.labels.legend.iter().map(|(t, _)| t.to_string()).collect()
    });
    assert_eq!(legend, ["Used", "Committed"]);
    assert!(
        r.tip(ids[1])
            .is_some_and(|t| t.starts_with("Memory the system has promised")),
        "the Commit heading tooltip"
    );
    assert_eq!(r.tip(ids[0]), None);
}

/// Each old `memory_view`, `memory_swap_graph` and `memory_composition` value gives the
/// expected widgets: the page converts once, at load, and leaves the old keys as they were.
#[gpui_kit::test]
fn page_mem_old_toggles_give_widgets(cx: &mut TestAppContext) {
    let cvl = |k: &str| format!("{k} [chart,value,label]");
    let blocks = [
        "fixed details",
        "fixed memory_compressed",
        "fixed swap_devices",
        "fixed memory_modules",
    ];
    let mut r = open(cx, PerformanceSettings::default());
    for view in [MemoryView::Usage, MemoryView::Apps] {
        for swap in [false, true] {
            for bar in [false, true] {
                if (view, swap, bar) == (MemoryView::Usage, true, true) {
                    // Every key at its default: nothing converts.
                    let mut s = Settings {
                        performance: perf(view, swap, bar),
                        ..Settings::default()
                    };
                    s.clamp();
                    assert!(s.performance.layouts.known(PAGE).is_none());
                    continue;
                }
                let mut s = Settings {
                    performance: perf(view, swap, bar),
                    ..Settings::default()
                };
                s.clamp();
                let l = s
                    .performance
                    .layouts
                    .known(PAGE)
                    .cloned()
                    .unwrap_or_else(|| panic!("{view:?} {swap} {bar} converts"));
                let mut want = vec![match view {
                    MemoryView::Usage => cvl("physical"),
                    MemoryView::Apps => "fixed memory_by_app".to_owned(),
                }];
                want.push(cvl("commit_pct"));
                if swap {
                    want.push(cvl("swap"));
                }
                if bar {
                    want.push("fixed memory_bar".to_owned());
                }
                want.extend(blocks.iter().map(|b| b.to_string()));
                assert_eq!(shape(&l), want, "{view:?} {swap} {bar}");
                assert_eq!(layout_view(&l), Some(view));
                assert_eq!((layout_swap(&l), layout_bar(&l)), (swap, bar));
                assert_eq!(
                    (
                        s.performance.memory_view,
                        s.performance.memory_swap_graph,
                        s.performance.memory_composition
                    ),
                    (view, swap, bar),
                    "the old keys stay"
                );
                // The page shows it.
                r.vcx.update(|_, cx| {
                    TlSettings::update(cx, |d| d.settings.performance.layouts.set(l.clone()));
                });
                draw(r.vcx);
                let page = r.page.clone();
                assert_eq!(r.vcx.update(|_, cx| page.read(cx).view()), Some(view));
                assert_eq!(r.shown(), l);
                let apps = r.id_of(|c| *c == WidgetContent::Fixed(FixedWidget::MemoryByApp));
                if let Some(apps) = apps {
                    let b = r.bounds(widget_id(PAGE, apps));
                    assert!(near(b.size.height, 300.), "By app is 300 px: {b:?}");
                }
            }
        }
    }
}

/// The switch and the page menu on a page without an entry write the entry and leave
/// `memory_view`, `memory_swap_graph` and `memory_composition` as they were; Commit, Swap, the
/// bar and Details keep their ids through a view change.
#[gpui_kit::test]
fn page_mem_switch_and_menu_write_entry_only(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    assert!(r.saved().is_none());
    let commit = r.metric_id("commit_pct");
    let swap = r.metric_id("swap");
    let page = r.page.clone();
    page.update(r.vcx, |p, cx| p.set_view(MemoryView::Apps, cx));
    draw(r.vcx);
    let saved = r.saved().expect("an entry");
    assert_eq!(layout_view(&saved), Some(MemoryView::Apps));
    assert_eq!(
        saved.widgets[0].content,
        WidgetContent::Fixed(FixedWidget::MemoryByApp)
    );
    assert_eq!(r.old_keys(), (MemoryView::Usage, true, true));
    assert_eq!(r.metric_id("commit_pct"), commit, "Commit keeps its id");
    assert_eq!(r.metric_id("swap"), swap, "Swap keeps its id");
    // The switch shows the new view and the page asks for the process sweep.
    let switch = r
        .vcx
        .update(|_, cx| page.read(cx).switch_view().read(cx).shown().clone());
    assert_eq!(switch.flags, [true]);
    assert!(
        r.vcx
            .update(|_, cx| page.read(cx).demand_part(cx).process_top)
    );

    r.vcx.dispatch_action(PerfToggleMemorySwapGraph);
    draw(r.vcx);
    r.vcx.dispatch_action(PerfToggleMemoryComposition);
    draw(r.vcx);
    let saved = r.saved().expect("an entry");
    assert_eq!(
        shape(&saved)[..3],
        [
            "fixed memory_by_app",
            "commit_pct [chart,value,label]",
            "fixed details"
        ]
    );
    // Both back on: Swap after Commit, the bar before Details, as in 1.3.0.
    r.vcx.dispatch_action(PerfToggleMemoryComposition);
    draw(r.vcx);
    r.vcx.dispatch_action(PerfToggleMemorySwapGraph);
    draw(r.vcx);
    let saved = r.saved().expect("an entry");
    assert_eq!(
        shape(&saved)[..5],
        [
            "fixed memory_by_app",
            "commit_pct [chart,value,label]",
            "swap [chart,value,label]",
            "fixed memory_bar",
            "fixed details"
        ]
    );
    assert_eq!(r.old_keys(), (MemoryView::Usage, true, true));
    page.update(r.vcx, |p, cx| p.set_view(MemoryView::Usage, cx));
    draw(r.vcx);
    assert!(
        !r.vcx
            .update(|_, cx| page.read(cx).demand_part(cx).process_top)
    );
}

/// UI-PRF-060 on the page: Memory by app is the `memory_by_app` widget, 300 px in the flow, its
/// legend is inside it, and a click on a group's legend row opens that group.
#[gpui_kit::test]
fn page_mem_by_app_keeps_legend_click(cx: &mut TestAppContext) {
    let mut r = open(cx, perf(MemoryView::Apps, true, true));
    r.publish_groups();
    let page = r.page.clone();
    let container = r.container();
    let id = r.fixed_id(FixedWidget::MemoryByApp);
    let (chart, content) = r.vcx.update(|_, cx| {
        let chart = page.read(cx).apps().expect("the chart");
        let fixed = container.read(cx).fixed(id).cloned().expect("the widget");
        let content = fixed.read(cx).content().map(|v| v.entity_id());
        (chart, content)
    });
    assert_eq!(
        content,
        Some(chart.entity_id()),
        "the widget draws the chart"
    );
    let widget = r.bounds(widget_id(PAGE, id));
    assert!(near(widget.size.height, 300.), "{widget:?}");
    // Rows: Other apps, p2, p1, p0, then Shared and kernel under the divider.
    let row = r.bounds(format!("{LEGEND_ID}-1"));
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
}

/// Every memory metric (in use, available, cached, swap, commit, pressure, the composition,
/// physical memory, commit of the limit, modified) shows in the forms the layout asks for, the
/// bar and Memory by app in a grid layout, and a block 1.3.0 hides keeps its cells with
/// "Nothing to show on this device". "Source >" lists the Memory box's metrics, then the phase
/// 3 keys.
#[gpui_kit::test]
fn page_mem_every_metric_any_form(cx: &mut TestAppContext) {
    let (c, b, v, s, l) = (
        FormSet::CHART,
        FormSet::BAR,
        FormSet::VALUE,
        FormSet::STATS,
        FormSet::LABEL,
    );
    let widgets = vec![
        metric("used", c | v | l, cell(0, 0, 6, 7)),
        metric("available", b | v | s | l, cell(6, 0, 6, 3)),
        metric("cached", v | l, cell(6, 3, 6, 2)),
        metric("swap", c | b | v | l, cell(6, 5, 6, 7)),
        metric("commit", b | v | l, cell(0, 7, 6, 2)),
        metric("pressure", c | v | s | l, cell(0, 9, 6, 8)),
        metric("composition", c | l, cell(6, 12, 6, 7)),
        metric("physical", c | v | l, cell(0, 17, 6, 7)),
        metric("commit_pct", b | v | s | l, cell(6, 19, 6, 3)),
        metric("dirty", c | v | s, cell(0, 24, 6, 8)),
        fixed(FixedWidget::MemoryBar, cell(6, 22, 6, 2)),
        fixed(FixedWidget::MemoryByApp, cell(0, 32, 12, 13)),
        fixed(FixedWidget::MemoryCompressed, cell(0, 45, 12, 4)),
    ];
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(widgets.clone()));
    let mut r = open(cx, perf);
    let container = r.container();
    r.vcx.update(|_, cx| {
        let c = container.read(cx);
        for (i, w) in widgets.iter().enumerate() {
            let id = i as u32 + 1;
            match &w.content {
                WidgetContent::Metric(s) => {
                    let name = s.metric.as_str();
                    let widget = c.widget(id).unwrap_or_else(|| panic!("{name}"));
                    let widget = widget.read(cx);
                    assert_eq!(widget.availability(), Availability::Available, "{name}");
                    assert_eq!(widget.drawn_forms(), w.forms, "{name}");
                }
                WidgetContent::Fixed(kind) => {
                    let fixed = c.fixed(id).unwrap_or_else(|| panic!("{kind:?}"));
                    let content = fixed.read(cx).content().is_some();
                    let want = *kind != FixedWidget::MemoryCompressed;
                    assert_eq!(content, want, "{kind:?}");
                }
                WidgetContent::Group(_) => unreachable!(),
            }
        }
    });
    // No zram and zswap off: the block keeps its cells and says so.
    let reason = r.bounds(part_id(PAGE, 13, "reason"));
    let block = r.bounds(widget_id(PAGE, 13));
    assert!(block.contains(&reason.center()));
    // With a grid layout the switch sits in the header and the two check items go.
    assert!(
        r.try_bounds(VIEW_SWITCH_ID.to_owned()).is_some(),
        "the switch stays"
    );
    let page = r.page.clone();
    assert_eq!(r.vcx.update(|_, cx| page.read(cx).view()), None);
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
    assert!(names.iter().any(|n| n == "Groups shown"), "{names:?}");
    assert!(!names.iter().any(|n| n == "Show swap usage"), "{names:?}");
    // The source list: the Memory box's metrics, then the phase 3 keys.
    let groups = r.vcx.update(|_, cx| {
        let editor = page.read(cx).editor().clone();
        editor.read(cx).source_groups(cx)
    });
    assert_eq!(groups.len(), 1, "memory only");
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
    let n = labels.len();
    assert_eq!(
        labels[n - 3..],
        ["Physical memory", "Commit (% of limit)", "Modified"],
        "{labels:?}"
    );
}

/// Edit Widgets on the Memory page pins the flow layout, adds Available, moves it, removes
/// Commit, adds Memory by app as a part and applies: the page's entry is the grid layout, the
/// Physical memory chart keeps its entity, and the old keys stay.
#[gpui_kit::test]
fn page_mem_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    let container = r.container();
    let physical = r.metric_id("physical");
    let physical_entity = r
        .vcx
        .update(|_, cx| container.read(cx).widget(physical).map(|w| w.entity_id()));
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    // In Edit Widgets the switch is not drawn: it is no widget.
    assert!(r.try_bounds(VIEW_SWITCH_ID.to_owned()).is_none());
    let source = WidgetSource {
        device: Arc::from("memory"),
        metric: Arc::from("available"),
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
    let apps = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("memory_by_app", cx)))
        .expect("Memory by app added");
    let commit = r.metric_id("commit_pct");
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(commit, cx);
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
        !saved.widgets.iter().any(|w| w.id == commit),
        "Commit removed"
    );
    let apps_content = r.vcx.update(|_, cx| {
        container
            .read(cx)
            .fixed(apps)
            .map(|f| f.read(cx).content().is_some())
    });
    assert_eq!(apps_content, Some(true), "Memory by app has its chart");
    assert!(
        saved
            .widgets
            .iter()
            .any(|w| w.content == WidgetContent::Fixed(FixedWidget::MemoryBar)),
        "the bar is pinned with the rest"
    );
    assert_eq!(
        r.vcx
            .update(|_, cx| container.read(cx).widget(physical).map(|w| w.entity_id())),
        physical_entity,
        "Apply keeps the Physical memory widget's entity"
    );
    assert_eq!(r.old_keys(), (MemoryView::Usage, true, true));
    let shown = r.vcx.update(|_, cx| {
        container
            .read(cx)
            .widget(added)
            .map(|w| w.read(cx).availability())
    });
    assert_eq!(shown, Some(Availability::Available));
}

/// Counts the widget and chart entities made while it lives, and the drops of the entities
/// [`shown_entities`] watched.
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
        let (a, b, c, d) = (count(), count(), count(), count());
        let subs = r.vcx.update(|_, cx| {
            vec![
                cx.observe_new(move |_: &mut MetricWidget, _, _| a()),
                cx.observe_new(move |_: &mut FixedWidgetView, _, _| b()),
                cx.observe_new(move |_: &mut ChartView, _, _| c()),
                cx.observe_new(move |_: &mut MemoryPartView, _, _| d()),
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
/// metric widget. Each is watched for a drop.
fn shown_entities(r: &mut Rig, life: &mut Lifecycle) -> Vec<EntityId> {
    let container = r.container();
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
        ids
    })
}

/// One sample on top of the last: `used` bytes in use and `swap` bytes of swap, all free.
fn publish_swap(engine: &FakeEngine, used: u64, swap: u64) {
    engine.publish_with(move |s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        let m = &mut sys.memory;
        m.used = used;
        m.available = m.total - used;
        m.swap_total = swap;
        m.swap_free = swap;
    });
}

/// One sample that sets SwapTotal to `swap` bytes and lists one swap device when `device`,
/// none otherwise: the system and the memory details change in the same sample.
fn publish_swap_and_devices(engine: &FakeEngine, swap: u64, device: bool) {
    engine.publish_with(move |s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        let m = &mut sys.memory;
        m.swap_total = swap;
        m.swap_free = swap;
        let swaps: Vec<SwapDevice> = if device {
            vec![SwapDevice {
                name: Arc::from("/dev/nvme0n1p3"),
                kind: SwapKindModel::Partition,
                size_bytes: swap,
                used_bytes: 0,
                priority: -2,
            }]
        } else {
            Vec::new()
        };
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.memory = Arc::new(MemoryDetails {
            swaps: InvField {
                value: Arc::from(swaps),
                validity: Validity::Valid,
            },
            ..MemoryDetails::default()
        });
    });
}

/// How many times the page asked its container to place the widgets again.
fn place_count(r: &mut Rig) -> u32 {
    let page = r.page.clone();
    r.vcx.update(|_, cx| page.read(cx).place_count.get())
}

/// Repeated samples with the same SwapTotal make and drop no entity, notify neither the page
/// nor the container and place nothing. The one sample where SwapTotal crosses 0 places the
/// widgets exactly once, as 1.3.0 did: the Swap chart goes or comes back in that sample, and the
/// next samples with the same SwapTotal change nothing (leader-notes 2026-11-03 hzire.6;
/// ADR-019 phase 3, "A listed `swap` takes no space while SwapTotal is 0").
#[gpui_kit::test]
fn page_mem_swap_crossing_places_once(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    publish_memory(&r.engine, 23 * GIB);
    draw(r.vcx);
    let container = r.container();
    let page = r.page.clone();
    let swap = r.metric_id("swap");
    let placed = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| container.read(cx).placed_ids().contains(&swap))
    };
    let counts = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            (
                page.read(cx).notify_count.get(),
                container.read(cx).notify_count.get(),
                page.read(cx).place_count.get(),
            )
        })
    };
    // Samples that keep SwapTotal: no notify, no placement, no entity made or dropped.
    let steady = |r: &mut Rig, life: &Lifecycle, swap_bytes: u64, what: &str| {
        let before = (counts(r), life.counts());
        for used in [24 * GIB, 25 * GIB, 23 * GIB] {
            publish_swap(&r.engine, used, swap_bytes);
            draw(r.vcx);
        }
        assert_eq!(
            (counts(r), life.counts()),
            before,
            "{what}: no page or container notify, no placement, no entity made or dropped"
        );
    };
    assert!(placed(&mut r), "8 GiB of swap: the Swap chart is placed");
    let mut life = Lifecycle::new(&mut r);
    let entities = shown_entities(&mut r, &mut life);
    steady(&mut r, &life, 8 * GIB, "with swap");
    assert_eq!(shown_entities(&mut r, &mut life), entities);

    let before = place_count(&mut r);
    publish_swap(&r.engine, 24 * GIB, 0);
    draw(r.vcx);
    assert_eq!(
        place_count(&mut r),
        before + 1,
        "swapoff: placed exactly once"
    );
    assert!(!placed(&mut r), "swapoff: the sample drops the Swap chart");
    assert!(r.try_bounds(widget_id(PAGE, swap)).is_none());
    let (made, dropped) = life.counts();
    assert_eq!(made, 0, "swapoff: no entity made");
    assert!(dropped > 0, "swapoff: the Swap chart's entities go");
    steady(&mut r, &life, 0, "without swap");
    assert!(!placed(&mut r));

    let before = place_count(&mut r);
    publish_swap(&r.engine, 23 * GIB, 8 * GIB);
    draw(r.vcx);
    assert_eq!(
        place_count(&mut r),
        before + 1,
        "swapon: placed exactly once"
    );
    assert!(placed(&mut r), "swapon: the sample places the Swap chart");
    assert!(r.try_bounds(widget_id(PAGE, swap)).is_some(), "drawn");
    let (made_on, dropped_on) = life.counts();
    assert!(made_on > 0, "swapon: the Swap chart's entities are made");
    assert_eq!(dropped_on, dropped, "swapon: no entity dropped");
    steady(&mut r, &life, 8 * GIB, "with swap again");
    assert!(placed(&mut r));
}

/// A sample where SwapTotal crosses 0 and the swap devices block starts or stops showing in
/// the same sample places the widgets exactly once, with both changes applied.
#[gpui_kit::test]
fn page_mem_swap_and_block_change_places_once(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    publish_memory(&r.engine, 23 * GIB);
    publish_swap_and_devices(&r.engine, 8 * GIB, true);
    draw(r.vcx);
    let container = r.container();
    let page = r.page.clone();
    let swap = r.metric_id("swap");
    let state = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            (
                container.read(cx).placed_ids().contains(&swap),
                page.read(cx).block_shown(&FixedWidget::SwapDevices),
            )
        })
    };
    assert_eq!(state(&mut r), (true, true), "swap and one swap device");

    let before = place_count(&mut r);
    publish_swap_and_devices(&r.engine, 0, false);
    draw(r.vcx);
    assert_eq!(
        place_count(&mut r),
        before + 1,
        "swapoff: placed exactly once"
    );
    assert_eq!(
        state(&mut r),
        (false, false),
        "swapoff: chart and block both go"
    );

    let before = place_count(&mut r);
    publish_swap_and_devices(&r.engine, 8 * GIB, true);
    draw(r.vcx);
    assert_eq!(
        place_count(&mut r),
        before + 1,
        "swapon: placed exactly once"
    );
    assert_eq!(
        state(&mut r),
        (true, true),
        "swapon: chart and block both come back"
    );
}

/// A sample that changes nothing the page shows renders no page or container; a sample that
/// moves memory's values notifies neither the page nor the container, and Details and the bar
/// notify for their own changes. No sample makes or drops a widget, chart or part entity
/// (ADR-019 phase 3 render contract).
#[gpui_kit::test]
fn page_mem_sample_renders_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    let container = r.container();
    let page = r.page.clone();
    let details: Entity<DetailsView> = r.vcx.update(|_, cx| page.read(cx).details_view().clone());
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
    let publish = |r: &mut Rig, used: u64| {
        publish_memory(&r.engine, used);
        draw(r.vcx);
    };
    publish(&mut r, 23 * GIB);
    let mut life = Lifecycle::new(&mut r);
    let entities = shown_entities(&mut r, &mut life);
    // Physical memory, Commit and Swap with their charts, the bar and Details.
    assert!(entities.len() >= 8, "{entities:?}");
    let before = counts(&mut r);
    for _ in 0..3 {
        publish(&mut r, 23 * GIB);
    }
    assert_eq!(counts(&mut r), before, "nothing shown changed");
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    let details_before = r.vcx.read_entity(&details, |d, _| d.notify_count.get());
    publish(&mut r, 40 * GIB);
    let after = counts(&mut r);
    assert_eq!(after.1, before.1, "the page never notifies");
    assert_eq!(after.3, before.3, "the container never notifies");
    assert!(
        r.vcx.read_entity(&details, |d, _| d.notify_count.get()) > details_before,
        "Details notifies for its rows"
    );
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    assert_eq!(
        shown_entities(&mut r, &mut life),
        entities,
        "the same entities"
    );
}
