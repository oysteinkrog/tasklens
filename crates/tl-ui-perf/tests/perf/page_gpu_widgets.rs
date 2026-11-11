//! The GPU page as a widget container (ADR-019 phase 3, UI-PRF-061, `tasklens-hzire.4`): the
//! default flow layout draws the 1.3.0 parts in their 1.3.0 places, each old
//! `gpu_optional_graphs` value converts once to the widgets it stood for, each GPU has its own
//! layout, a GPU that is gone shows its widgets as unavailable and keeps its entry, every GPU
//! metric shows in any mix of forms, Edit Widgets adds and applies, and a sample renders no page
//! or container.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollDelta,
    ScrollWheelEvent, StyleRefinement, Styled as _, Subscription, TestAppContext, TouchPhase,
    VisualTestContext, Window, div, point, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{GraphicsAdapter, InvField};
use tl_model::system::{GpuTotals, Metric};
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{AppEnv, BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::{ChartGridView, ChartView};
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::widget::container::{WidgetContainer, WidgetView};
use tl_ui_overview::widget::details_widget::details_id;
use tl_ui_overview::widget::edit::saved_layout;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::group_widget::GroupWidget;
use tl_ui_overview::widget::metric_widget::{MetricWidget, widget_id};
use tl_ui_overview::widget::source::Availability;
use tl_ui_perf::performance::gpu::{GpuPage, toggle_optional};
use tl_ui_perf::performance::gpu_views::{displays_id, engines_id};
use tl_ui_perf::performance::menu::ClickedEngine;
use tl_ui_perf::performance::page::{DevicePage as _, PAGE_PAD};
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::WidgetSource;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, GroupItems, GroupKind, GroupOptions,
    GroupSelector, LAYOUT_VERSION, METRIC_ENGINE_ANY, MetricKey, SourceSelector, WidgetCell,
    WidgetContent, WidgetOptions, WidgetSetting, reset_to_default,
};
use tl_ui_settings::layout_pages::{convert_gpu_page, page_container};
use tl_ui_settings::settings::{
    AppearanceSettings, GpuOptionalGraph, PerformanceSettings, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

use super::gpu_page::{full_totals, totals};

const GIB: u64 = 1 << 30;

/// The NVIDIA GPU of `full_totals` and a second, AMD GPU with shared memory.
const GPU_A: DeviceIdx = DeviceIdx(7);
const GPU_B: DeviceIdx = DeviceIdx(8);

fn identity(n: u8) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid: [n; 16],
        pci_bus_id: Arc::from(format!("0000:{n:02x}:00.0")),
    }
}

/// The persistent key of GPU `n` (1 is A, 2 is B), as the sidebar and the page use it.
fn key(n: u8) -> DeviceKey {
    DeviceKey::from_identity(&identity(n)).expect("gpu key")
}

fn container(n: u8) -> String {
    page_container(&key(n).0)
}

fn entry(idx: DeviceIdx, n: u8, present: bool) -> DeviceEntry {
    DeviceEntry {
        idx,
        identity: identity(n),
        generation: 0,
        display_name: Arc::from(if n == 1 {
            "NVIDIA GeForce RTX 4080"
        } else {
            "AMD Radeon RX 7900"
        }),
        present,
        parent: None,
        persistent_key: key(n).0,
        net_kind: None,
        hot_c: None,
    }
}

fn adapter(idx: DeviceIdx, module: &str) -> GraphicsAdapter {
    let text = |v: &str| InvField {
        value: Arc::from(v),
        validity: Validity::Valid,
    };
    GraphicsAdapter {
        device: Some(idx),
        kernel_module: text(module),
        dedicated_bytes: InvField {
            value: 16 * GIB,
            validity: Validity::Valid,
        },
        ..GraphicsAdapter::default()
    }
}

/// GPU A's record: every value of `full_totals`.
fn totals_a() -> GpuTotals {
    full_totals()
}

/// GPU B's record: the AMD record with shared memory.
fn totals_b() -> GpuTotals {
    GpuTotals {
        device: GPU_B,
        ..totals(true)
    }
}

/// The page, cached, so it renders only when it notifies.
struct Host(Entity<GpuPage>);

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
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    /// GPU A's page, shown in the window.
    page: Entity<GpuPage>,
    /// How many times a page armed the `settings.toml.pre-layouts-v2` copy.
    copies: Arc<AtomicU32>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

fn publish_gpus(engine: &FakeEngine, a_present: bool) {
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(vec![entry(GPU_A, 1, a_present), entry(GPU_B, 2, true)]),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        let mut gpus = vec![(GPU_B, totals_b())];
        if a_present {
            gpus.insert(0, (GPU_A, totals_a()));
        }
        sys.gpus = Arc::from(gpus);
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        inv.graphics = Arc::from(vec![
            adapter(GPU_A, "nvidia 615.71.09"),
            adapter(GPU_B, "amdgpu"),
        ]);
    });
}

/// A `w × h` window with GPU A's page, built with `perf` as the loaded performance settings.
fn open(cx: &mut TestAppContext, perf: PerformanceSettings, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    publish_gpus(&engine, true);
    let client: Arc<dyn EngineClient> = engine.clone();
    let copies = Arc::new(AtomicU32::new(0));
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
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let count = Arc::clone(&copies);
        cx.set_global(AppEnv {
            keep_pre_layouts_copy: Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
            ..AppEnv::test()
        });
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let (hub2, demand2) = (hub.clone(), demand.clone());
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let page = cx.new(|cx| GpuPage::new(key(1), row(GPU_A, 1), hub2, demand2, cx));
        Host(page)
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(w), px(h)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let page = vcx.read_entity(&host, |h, _| h.0.clone());
    vcx.update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    draw(vcx);
    Rig {
        engine,
        hub,
        demand,
        page,
        copies,
        vcx,
        _driver: driver,
    }
}

fn row(idx: DeviceIdx, n: u8) -> DeviceRow {
    DeviceRow {
        key: key(n),
        kind: DeviceKind::Gpu,
        idx,
        generation: 0,
        net_class: None,
        fan_connected: None,
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

fn metric(n: u8, key: &str, forms: FormSet, cell: Option<WidgetCell>) -> WidgetSetting {
    WidgetSetting {
        id: 0,
        content: WidgetContent::Metric(SourceSelector {
            device: DeviceSelector::Key(self::key(n).0),
            metric: MetricKey::new(key),
        }),
        forms,
        cell,
        options: WidgetOptions::default(),
        device_name: None,
    }
}

/// A grid layout of page `n` with `widgets`, ids from 1.
fn grid_layout(n: u8, widgets: Vec<WidgetSetting>) -> ContainerLayout {
    let widgets: Vec<WidgetSetting> = widgets
        .into_iter()
        .enumerate()
        .map(|(i, w)| WidgetSetting {
            id: i as u32 + 1,
            ..w
        })
        .collect();
    ContainerLayout {
        container: container(n),
        version: LAYOUT_VERSION,
        preset: None,
        next_id: widgets.len() as u32 + 1,
        widgets,
    }
}

/// The metric keys of a layout's metric widgets, in list order.
fn metrics_of(l: &ContainerLayout) -> Vec<String> {
    l.widgets
        .iter()
        .filter_map(|w| w.content.metric_key().map(|k| k.as_str().to_owned()))
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

    /// The id of the first widget whose content matches `f`.
    fn id_of(&mut self, f: impl Fn(&WidgetContent) -> bool) -> u32 {
        let container = self.container();
        self.vcx.update(|_, cx| {
            container
                .read(cx)
                .layout()
                .widgets
                .iter()
                .find(|w| f(&w.content))
                .map(|w| w.id)
                .expect("a widget")
        })
    }

    fn metric_id(&mut self, key: &str) -> u32 {
        self.id_of(|c| c.metric_key().is_some_and(|k| k.as_str() == key))
    }

    /// Builds the page of GPU `n` again (as the window does when it first shows it), without
    /// showing it.
    fn build_page(&mut self, n: u8) -> Entity<GpuPage> {
        let (hub, demand) = (self.hub.clone(), self.demand.clone());
        let idx = if n == 1 { GPU_A } else { GPU_B };
        self.vcx
            .update(|_, cx| cx.new(|cx| GpuPage::new(key(n), row(idx, n), hub, demand, cx)))
    }

    fn saved(&mut self, n: u8) -> Option<ContainerLayout> {
        self.vcx.update(|_, cx| saved_layout(&container(n), cx))
    }

    fn update_perf(&mut self, f: impl FnOnce(&mut PerformanceSettings)) {
        self.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| f(&mut d.settings.performance));
        });
        draw(self.vcx);
    }
}

/// The default flow layout draws the 1.3.0 page at 1200 × 900: the engine grid first, the
/// Memory and Temperature charts side by side `sp4` under it, `details_gap` above Details and
/// `sp6` above Displays. Nothing is written: the page has no old key to convert.
#[gpui_kit::test]
fn page_gpu_default_flow_places_1_3_0_parts(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
    assert!(r.saved(1).is_none(), "no entry");
    assert_eq!(r.copies.load(Ordering::SeqCst), 0, "no copy armed");
    let c = container(1);
    let (memory, temp) = (r.metric_id("memory"), r.metric_id("temp"));
    let engines = r.bounds(engines_id(&c));
    let mem = r.bounds(widget_id(&c, memory));
    let tmp = r.bounds(widget_id(&c, temp));
    let details = r.bounds(details_id(&c));
    let displays = r.bounds(displays_id(&c));
    let body_w = 1200. - 2. * PAGE_PAD;
    assert!(near(engines.size.width, body_w), "{engines:?}");
    // Three engines fit one row of 112 px.
    assert!(near(engines.size.height, 112.), "{engines:?}");
    assert!(near(mem.top() - engines.bottom(), 8.), "sp4 under the grid");
    assert_eq!(mem.top(), tmp.top(), "side by side");
    assert!(near(mem.size.width, (body_w - 8.) / 2.), "{mem:?}");
    assert!(near(tmp.left() - mem.right(), 8.));
    assert!(near(mem.size.height, 150.));
    assert!(near(details.top() - mem.bottom(), 24.), "details_gap");
    assert!(near(displays.top() - details.bottom(), 16.), "sp6");
    // The charts carry the 1.3.0 headings, legends and values.
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let p = page.read(cx);
        let m = p.memory(cx).expect("memory chart");
        let m = m.read(cx);
        assert_eq!(m.labels.heading.as_deref(), Some("Memory"));
        assert_eq!(m.labels.range_word.as_ref(), "Memory usage");
        assert_eq!(
            m.value_text().map(|t| t.as_ref()),
            Some("3.00 GiB / 16.00 GiB")
        );
        let t = p.temperature(cx).expect("temperature chart");
        assert_eq!(t.read(cx).labels.heading.as_deref(), Some("Temperature"));
    });
}

/// Each old `gpu_optional_graphs` value gives the expected widgets: the page converts it once,
/// when it is first built, arms the pre-layouts copy and leaves the old key as it was. With
/// the key empty nothing is written.
#[gpui_kit::test]
fn page_gpu_old_toggles_give_widgets(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
    let cases: [(&[GpuOptionalGraph], &[&str]); 4] = [
        (&[], &["memory", "temp"]),
        (&[GpuOptionalGraph::Power], &["memory", "temp", "power"]),
        (&[GpuOptionalGraph::Clocks], &["memory", "temp", "clocks"]),
        (
            &[GpuOptionalGraph::Clocks, GpuOptionalGraph::Power],
            &["memory", "temp", "power", "clocks"],
        ),
    ];
    for (list, want) in cases {
        r.update_perf(|p| {
            p.layouts.0.clear();
            p.gpu_optional_graphs = list.to_vec();
        });
        let before = r.copies.load(Ordering::SeqCst);
        let page = r.build_page(2);
        draw(r.vcx);
        let shown = r
            .vcx
            .update(|_, cx| page.read(cx).container().read(cx).layout().clone());
        assert_eq!(metrics_of(&shown), want, "{list:?}");
        let saved = r.saved(2);
        let copies = r.copies.load(Ordering::SeqCst) - before;
        if list.is_empty() {
            assert!(saved.is_none(), "nothing to convert");
            assert_eq!(copies, 0);
        } else {
            let saved = saved.expect("converted entry");
            assert_eq!(saved, shown);
            let perf = r
                .vcx
                .update(|_, cx| TlSettings::settings(cx).performance.clone());
            assert_eq!(saved, convert_gpu_page(&perf, &container(2)));
            assert_eq!(perf.gpu_optional_graphs, list, "the old key stays");
            assert_eq!(copies, 1, "the pre-layouts copy is armed");
            // Built again: no second conversion.
            let again = r.build_page(2);
            draw(r.vcx);
            assert_eq!(r.saved(2), Some(saved));
            assert_eq!(r.copies.load(Ordering::SeqCst) - before, 1);
            drop(again);
        }
        drop(page);
    }
}

/// A second GPU gets its own layout: each GPU page converts on its own, a menu item on one
/// page changes only its entry, and a reset of one writes its default and leaves the shared
/// old key, so the other GPU keeps its Power chart.
#[gpui_kit::test]
fn page_gpu_second_gpu_own_layout(cx: &mut TestAppContext) {
    let perf = PerformanceSettings {
        gpu_optional_graphs: vec![GpuOptionalGraph::Power],
        ..PerformanceSettings::default()
    };
    let mut r = open(cx, perf, 1200., 900.);
    let page_b = r.build_page(2);
    draw(r.vcx);
    let (a, b) = (r.saved(1).expect("A"), r.saved(2).expect("B"));
    assert_eq!(metrics_of(&a), ["memory", "temp", "power"]);
    assert_eq!(metrics_of(&b), ["memory", "temp", "power"]);
    // Every widget names its own GPU.
    for (l, n) in [(&a, 1), (&b, 2)] {
        assert!(l.widgets.iter().all(|w| match &w.content {
            WidgetContent::Metric(s) => s.device == DeviceSelector::Key(key(n).0),
            _ => true,
        }));
    }
    // The page menu's Clocks item on page A: only A's entry changes.
    r.update_perf(|p| {
        let mut l = p.layouts.known(&container(1)).cloned().expect("A");
        toggle_optional(&mut l, &key(1).0, GpuOptionalGraph::Clocks);
        p.layouts.set(l);
    });
    assert_eq!(
        metrics_of(&r.saved(1).expect("A")),
        ["memory", "temp", "power", "clocks"]
    );
    assert_eq!(r.saved(2), Some(b.clone()));
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        assert!(page.read(cx).clocks(cx).is_some(), "A shows Clocks");
        assert!(page_b.read(cx).clocks(cx).is_none(), "B does not");
    });
    // Reset of A: its default, written as an entry; the key and B stay.
    r.update_perf(|p| reset_to_default(p, &container(1)));
    let a = r.saved(1).expect("A's default entry");
    assert_eq!(metrics_of(&a), ["memory", "temp"]);
    assert_eq!(r.saved(2), Some(b));
    let key_now = r.vcx.update(|_, cx| {
        TlSettings::settings(cx)
            .performance
            .gpu_optional_graphs
            .clone()
    });
    assert_eq!(key_now, [GpuOptionalGraph::Power]);
}

/// The forms of each widget of [`page_gpu_every_metric_any_form`], in list order.
fn any_form_widgets() -> Vec<WidgetSetting> {
    use FormSet as F;
    let cell = |row: u16| {
        Some(WidgetCell {
            col: 0,
            row,
            cols: 12,
            rows: 9,
        })
    };
    let list: [(&str, FormSet); 10] = [
        ("engine:gpu", F::BAR | F::VALUE | F::LABEL),
        ("engine:enc", F::CHART | F::STATS),
        ("vram", F::BAR | F::VALUE),
        ("clock_gfx", F::VALUE | F::STATS | F::LABEL),
        ("clock_mem", F::CHART | F::LABEL),
        ("power", F::CHART | F::BAR | F::VALUE | F::STATS | F::LABEL),
        ("temp", F::STATS | F::LABEL),
        ("memory", F::CHART | F::VALUE | F::LABEL),
        ("clocks", F::CHART | F::VALUE),
        ("fan", F::BAR | F::VALUE),
    ];
    let mut widgets: Vec<WidgetSetting> = list
        .iter()
        .enumerate()
        .map(|(i, (k, forms))| metric(1, k, *forms, cell(i as u16 * 9)))
        .collect();
    widgets.push(WidgetSetting {
        content: WidgetContent::Group(GroupSelector {
            device: DeviceSelector::Key(key(1).0),
            members: GroupKind::Engines,
            metric: MetricKey::new(METRIC_ENGINE_ANY),
            options: GroupOptions {
                limit: None,
                items: GroupItems::Cells,
                arrange: None,
            },
        }),
        ..metric(
            1,
            "",
            FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
            cell(90),
        )
    });
    widgets.push(WidgetSetting {
        content: WidgetContent::Fixed(FixedWidget::GpuEngineGrid),
        ..metric(1, "", FormSet::empty(), cell(99))
    });
    widgets
}

/// Every GPU metric (each engine, VRAM, the clocks, power, temperature, the memory chart,
/// clocks and fan) shows in the forms the layout asks for, all engines as a group, and the
/// engine grid in a grid layout. "Source >" and Add Widget list them by their menu labels.
#[gpui_kit::test]
fn page_gpu_every_metric_any_form(cx: &mut TestAppContext) {
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(1, any_form_widgets()));
    let mut r = open(cx, perf, 1200., 900.);
    let container = r.container();
    let want = any_form_widgets();
    r.vcx.update(|_, cx| {
        let c = container.read(cx);
        for (i, w) in want.iter().enumerate() {
            let id = i as u32 + 1;
            match &w.content {
                WidgetContent::Metric(s) => {
                    let widget = c
                        .widget(id)
                        .unwrap_or_else(|| panic!("{}", s.metric.as_str()));
                    let widget = widget.read(cx);
                    assert_eq!(
                        widget.availability(),
                        Availability::Available,
                        "{}",
                        s.metric.as_str()
                    );
                    assert_eq!(widget.drawn_forms(), w.forms, "{}", s.metric.as_str());
                }
                WidgetContent::Group(_) => assert!(c.group(id).is_some(), "all engines"),
                WidgetContent::Fixed(_) => {
                    let fixed = c.fixed(id).expect("the engine grid");
                    assert!(fixed.read(cx).content().is_some(), "the page's grid view");
                }
            }
        }
    });
    // The page menu's Power and Clocks items are hidden with a grid layout.
    let page = r.page.clone();
    let names = r.vcx.update(|window, cx| {
        let focus = gpui_kit::Focusable::focus_handle(page.read(cx), cx);
        let menu = tl_ui_kit::components::menu::TlMenu::build(window, cx, focus, |menu, w, cx| {
            page.update(cx, |p, cx| p.menu_items(menu, w, cx))
        });
        let list = menu.read(cx).entries_for_test(window);
        let sub = list
            .iter()
            .find(|e| e.label.as_deref() == Some("Select GPU engine graphs"))
            .and_then(|e| e.submenu.clone())
            .expect("the engines submenu: the page has the engine grid");
        sub.read(cx)
            .entries_for_test(window)
            .into_iter()
            .map(|e| e.label.map(|l| l.to_string()).unwrap_or_default())
            .collect::<Vec<_>>()
    });
    assert!(
        !names.iter().any(|n| n == "Power" || n == "Clocks"),
        "{names:?}"
    );
    // The source list of the page: the GPU's own metrics, box keys then phase 3 keys.
    let page = r.page.clone();
    let items = r.vcx.update(|_, cx| {
        let editor = page.read(cx).editor().clone();
        let groups = editor.read(cx).source_groups(cx);
        assert_eq!(groups.len(), 1, "only this GPU");
        groups[0]
            .items
            .iter()
            .map(|i| (i.metric.as_str().to_owned(), i.label.to_string()))
            .collect::<Vec<_>>()
    });
    let keys: Vec<&str> = items.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "engine:gpu",
            "engine:enc",
            "engine:dec",
            "vram",
            "clock_gfx",
            "clock_mem",
            "power",
            "temp",
            "memory",
            "clocks",
            "fan"
        ]
    );
    let memory = items.iter().find(|(k, _)| k == "memory").expect("memory");
    assert_eq!(memory.1, "Memory (dedicated and shared)");
}

/// A GPU that is gone shows its page's widgets as unavailable ("Not connected") and its entry
/// stays; when it comes back the saved layout shows again with the same entities.
#[gpui_kit::test]
fn page_gpu_removed_shows_unavailable(cx: &mut TestAppContext) {
    let cell = |row| {
        Some(WidgetCell {
            col: 0,
            row,
            cols: 12,
            rows: 9,
        })
    };
    let layout = grid_layout(
        1,
        vec![
            metric(
                1,
                "memory",
                FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
                cell(0),
            ),
            metric(
                1,
                "engine:gpu",
                FormSet::BAR | FormSet::VALUE | FormSet::LABEL,
                cell(9),
            ),
        ],
    );
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(layout.clone());
    let mut r = open(cx, perf, 1200., 900.);
    let container = r.container();
    let state = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            let c = container.read(cx);
            [1, 2].map(|id| {
                let w = c.widget(id).expect("placed");
                let entity = w.entity_id();
                let w = w.read(cx);
                (entity, w.availability(), w.texts()[3].to_string())
            })
        })
    };
    let before = state(&mut r);
    assert!(before.iter().all(|(_, a, _)| *a == Availability::Available));
    publish_gpus(&r.engine, false);
    draw(r.vcx);
    let gone = state(&mut r);
    for (_, a, reason) in &gone {
        assert_eq!(*a, Availability::Gone);
        assert_eq!(reason, "Not connected");
    }
    assert_eq!(r.saved(1), Some(layout.clone()), "the entry is kept");
    publish_gpus(&r.engine, true);
    draw(r.vcx);
    let back = state(&mut r);
    for (b, a) in back.iter().zip(&before) {
        assert_eq!(b.1, Availability::Available);
        assert_eq!(b.0, a.0, "same entity");
    }
}

/// The engine a right-press at `at` names for "Hide this engine graph".
fn pressed_engine(r: &mut Rig, at: Point<Pixels>) -> Option<String> {
    r.vcx.update(|_, cx| cx.set_global(ClickedEngine(None)));
    r.vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    r.vcx.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    r.vcx.run_until_parked();
    r.vcx
        .update(|_, cx| cx.try_global::<ClickedEngine>().and_then(|c| c.0.clone()))
        .map(|k| k.to_string())
}

/// A right-press on a cell of an engine grid that scrolls inside its widget names the engine
/// drawn under the pointer, at the scroll offset, for "Hide this engine graph".
#[gpui_kit::test]
fn page_gpu_engine_press_follows_scroll(cx: &mut TestAppContext) {
    // Two columns wide: one cell per row, three rows of 112 px in a shorter widget.
    let grid = WidgetSetting {
        content: WidgetContent::Fixed(FixedWidget::GpuEngineGrid),
        ..metric(
            1,
            "",
            FormSet::empty(),
            Some(WidgetCell {
                col: 0,
                row: 0,
                cols: 2,
                rows: 9,
            }),
        )
    };
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(1, vec![grid]));
    let mut r = open(cx, perf, 1200., 900.);
    let id = engines_id(&container(1));
    let top = r.bounds(id.clone());
    let at = point(top.left() + px(20.), top.top() + px(20.));
    assert_eq!(pressed_engine(&mut r, at).as_deref(), Some("gpu"));

    r.vcx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(-1000.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw(r.vcx);
    let dy = f32::from(top.top() - r.bounds(id).top());
    // A cell and its 8 px gap take 120 px: the pointer is over a later row.
    let row = ((20. + dy) / 120.).floor() as usize;
    assert!(row >= 1, "the grid scrolled past its first row: {dy}");
    let want = ["gpu", "enc", "dec"][row.min(2)];
    assert_eq!(pressed_engine(&mut r, at).as_deref(), Some(want));
}

/// Edit Widgets on the GPU page pins the flow layout, adds VRAM and applies: the page's entry
/// is the grid layout, the 1.3.0 parts keep their entities.
#[gpui_kit::test]
fn page_gpu_edit_add_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
    let container = r.container();
    let memory = r.metric_id("memory");
    let entity = r
        .vcx
        .update(|_, cx| container.read(cx).widget(memory).map(|w| w.entity_id()));
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    let source = WidgetSource {
        device: key(1).0,
        metric: Arc::from("vram"),
    };
    let added = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("added");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r.saved(1).expect("the page's entry");
    assert!(saved.preset.is_none(), "a grid layout");
    assert!(saved.widgets.iter().any(|w| w.id == added));
    assert!(
        saved
            .widgets
            .iter()
            .any(|w| matches!(w.content, WidgetContent::Fixed(FixedWidget::GpuEngineGrid))),
        "the engine grid is pinned with the rest"
    );
    assert_eq!(
        r.vcx
            .update(|_, cx| container.read(cx).widget(memory).map(|w| w.entity_id())),
        entity,
        "Apply keeps the entity"
    );
    r.vcx.update(|_, cx| {
        assert!(
            TlSettings::settings(cx)
                .performance
                .gpu_optional_graphs
                .is_empty()
        );
    });
}

/// Counts the widget, chart and engine grid entities made while it lives, and the drops of the
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

    /// Counts a drop of `entity`.
    fn watch<T: 'static>(&mut self, entity: &Entity<T>, cx: &mut App) -> EntityId {
        let dropped = Rc::clone(&self.dropped);
        self.subs.push(cx.observe_release(entity, move |_, _| {
            dropped.set(dropped.get() + 1);
        }));
        entity.entity_id()
    }

    /// The entities made and dropped since the counter started.
    fn counts(&self) -> (u32, u32) {
        (self.made.get(), self.dropped.get())
    }
}

/// The entities the page shows, in placement order: every placed widget and the chart of each
/// metric widget, then the engine grid part and its chart grid. Each is watched for a drop.
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
        let engines = page.read(cx).engines_view().clone();
        let grid = engines.read(cx).grid().clone();
        ids.push(life.watch(&engines, cx));
        ids.push(life.watch(&grid, cx));
        ids
    })
}

/// A sample that changes nothing the page shows renders no page or container and notifies no
/// part; a sample that moves the GPU's values notifies neither the page, the container nor the
/// engine grid part. No sample makes or drops a widget, chart or engine grid entity: the page
/// shows the same entities throughout (ADR-019 phase 3 render contract).
#[gpui_kit::test]
fn page_gpu_sample_renders_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
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
                p.engines_view().read(cx).notify_count.get(),
            )
        })
    };
    let publish = |r: &mut Rig, util: f32, temp: f32| {
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let mut a = totals_a();
            a.util_pct = Metric::valid(util);
            a.temp_c = Metric::valid(temp);
            sys.gpus = Arc::from(vec![(GPU_A, a), (GPU_B, totals_b())]);
        });
        draw(r.vcx);
    };
    publish(&mut r, 10.9, 52.);
    let mut life = Lifecycle::new(&mut r);
    let entities = shown_entities(&mut r, &mut life);
    // The engine grid, Memory, Temperature, Details and Displays, and the two charts.
    assert!(entities.len() >= 9, "{entities:?}");
    let before = counts(&mut r);
    for _ in 0..3 {
        publish(&mut r, 10.9, 52.);
    }
    assert_eq!(counts(&mut r), before, "nothing shown changed");
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    publish(&mut r, 50., 60.);
    let after = counts(&mut r);
    assert_eq!(after.1, before.1, "the page never notifies");
    assert_eq!(after.3, before.3, "the container never notifies");
    // The grid's cells draw the new samples; the part around them never notifies. Its one
    // render in that frame comes from GPUI marking the ancestors of the grid view dirty, as for
    // the page and the container (leader decision on tasklens-hzire.3).
    assert_eq!(after.4, before.4, "the engine grid part never notifies");
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
fn page_gpu_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
    let container = r.container();
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    super::page_look::sweep(r.vcx, &container, &editor, "gpu");
}

/// A gpu that goes away with the default flow layout writes no entry; when it comes back the
/// page places the same widgets as before (tasklens-hzire.10, hotplug).
#[gpui_kit::test]
fn page_gpu_hotplug_default_flow_keeps_settings(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
    let container = r.container();
    let placed = |r: &mut Rig| r.vcx.update(|_, cx| container.read(cx).placed_ids());
    let before = placed(&mut r);
    assert!(!before.is_empty());
    publish_gpus(&r.engine, false);
    draw(r.vcx);
    assert!(r.saved(1).is_none(), "a hotplug writes no entry");
    publish_gpus(&r.engine, true);
    draw(r.vcx);
    assert_eq!(placed(&mut r), before, "the same widgets come back");
    assert!(r.saved(1).is_none(), "a hotplug writes no entry");
}

/// tasklens-g00wx.4: the Usage, Load by app and VRAM by app switch, the `gpu_by_app` and
/// `vram_by_app` widgets, their "Groups shown" items, legend click, demand, Add Widget, render
/// contract and the rebuild sweep.
mod by_app {
    use gpui_kit::Focusable as _;
    use tl_model::ids::{MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::snapshot::{ComponentHeader, SeriesSet};
    use tl_model::system::MemGroupBand;
    use tl_model::time::now_boot_ns;
    use tl_ui_overview::overview::memory_apps::{AppsChart, AppsMetric, LEGEND_ID};
    use tl_ui_perf::performance::gpu::{GpuView, VIEW_SWITCH_ID, layout_view};
    use tl_ui_perf_core::core::actions::{PerfSetGpuByAppTopN, PerfSetVramByAppTopN};
    use tl_ui_settings::layout_pages::gpu_page_for_preset;

    use super::*;

    const MIB: f32 = 1_048_576.;
    /// GPU A's PCI bus id: the prefix of its group bands' device keys.
    const PCI_A: &str = "0000:01:00.0";
    /// Groups `exe:p0` to `exe:p3`.
    const GROUPS: u16 = 4;
    const OTHER_DEV: DeviceIdx = DeviceIdx(30);
    const DRIVER_DEV: DeviceIdx = DeviceIdx(31);

    fn group_dev(i: u16) -> DeviceIdx {
        DeviceIdx(20 + i)
    }

    /// A band device of GPU A: `gpu:<pci>:<key>`.
    fn band_entry(idx: DeviceIdx, key: &str, name: &str) -> DeviceEntry {
        let key = format!("gpu:{PCI_A}:{key}");
        DeviceEntry {
            idx,
            identity: DeviceIdentity::MemGroup {
                key: Arc::from(key.as_str()),
            },
            generation: 0,
            display_name: Arc::from(name),
            present: true,
            parent: None,
            persistent_key: Arc::from(format!("memgroup:{key}")),
            net_kind: None,
            hot_c: None,
        }
    }

    /// The two GPUs and GPU A's bands, indexed by position as the catalog reads them, the gaps
    /// filled with inert bands.
    fn group_catalog(version: u64) -> DeviceCatalog {
        let mut entries = vec![entry(GPU_A, 1, true), entry(GPU_B, 2, true)];
        for i in 0..GROUPS {
            entries.push(band_entry(
                group_dev(i),
                &format!("exe:p{i}"),
                &format!("p{i}"),
            ));
        }
        entries.push(band_entry(OTHER_DEV, "other", "Other apps"));
        entries.push(band_entry(DRIVER_DEV, "driver", "Other / driver"));
        entries.sort_by_key(|e| e.idx);
        let mut dense: Vec<DeviceEntry> = Vec::new();
        for e in entries {
            while dense.len() < usize::from(e.idx.0) {
                let n = dense.len() as u16;
                dense.push(DeviceEntry {
                    idx: DeviceIdx(n),
                    identity: DeviceIdentity::MemGroup {
                        key: Arc::from(format!("filler:{n}")),
                    },
                    generation: 0,
                    display_name: Arc::from("filler"),
                    present: false,
                    parent: None,
                    persistent_key: Arc::from(format!("filler:{n}")),
                    net_kind: None,
                    hot_c: None,
                });
            }
            dense.push(e);
        }
        DeviceCatalog {
            version,
            entries: dense.into(),
        }
    }

    /// One GPU load and one VRAM sample a second for the last minute on GPU A: group `exe:p<i>`
    /// at `gpu[i]` percent and `vram[i]` MiB (a group past a list has no series of it), "Other
    /// apps" at 1% and 64 MiB, "Other / driver" at 2% and 128 MiB. The catalog changes once.
    fn publish_groups(engine: &FakeEngine, gpu: &[f32], vram: &[f32]) {
        const PERIOD: u64 = 1_000_000_000;
        let now = now_boot_ns();
        engine.publish_with(|s| {
            if s.devices.entries.len() < 20 {
                s.devices = Arc::new(group_catalog(s.devices.version + 1));
            }
            let version = s.series.header.version + 1;
            let line = |device: DeviceIdx, metric: MetricId, value: f32| {
                Arc::new(SeriesView {
                    id: SeriesId { device, metric },
                    version,
                    period_ns: PERIOD,
                    samples: (0..60u64)
                        .map(|k| Sample::valid(now - (59 - k) * PERIOD, value))
                        .collect::<Vec<_>>()
                        .into(),
                })
            };
            let mut views = Vec::new();
            for (list, metric, scale) in [
                (gpu, MetricId::GPU_GROUP, 1.),
                (vram, MetricId::VRAM_GROUP, MIB),
            ] {
                for (i, v) in list.iter().enumerate() {
                    views.push(line(group_dev(i as u16), metric, v * scale));
                }
            }
            for (device, g, v) in [(OTHER_DEV, 1., 64.), (DRIVER_DEV, 2., 128.)] {
                views.push(line(device, MetricId::GPU_GROUP, g));
                views.push(line(device, MetricId::VRAM_GROUP, v * MIB));
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
            let bands = |n: usize| {
                Arc::new(
                    (0..n as u16)
                        .map(|i| MemGroupBand {
                            key: Arc::from(format!("exe:p{i}")),
                            name: Arc::from(format!("p{i}")),
                            process_count: 2,
                            device: group_dev(i),
                            wine: None,
                        })
                        .collect::<Vec<_>>(),
                )
            };
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let mut a = totals_a();
            a.groups = bands(gpu.len());
            a.vram_groups = bands(vram.len());
            sys.gpus = Arc::from(vec![(GPU_A, a), (GPU_B, totals_b())]);
        });
    }

    /// Performance settings whose GPU A entry is the flow layout of `view`.
    fn view_perf(view: GpuView) -> PerformanceSettings {
        let mut perf = PerformanceSettings::default();
        let layout = gpu_page_for_preset(&perf, &container(1), view.preset());
        perf.layouts.set(layout);
        perf
    }

    fn page_view(r: &mut Rig) -> Option<GpuView> {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).view())
    }

    /// Clicks the switch button of `id` (`usage`, `gpu-apps` or `vram-apps`).
    fn click_switch(r: &mut Rig, id: &str) {
        let at = r.bounds(format!("{VIEW_SWITCH_ID}-{id}")).center();
        r.vcx.simulate_click(at, Modifiers::none());
        draw(r.vcx);
    }

    fn fixed_id(r: &mut Rig, kind: FixedWidget) -> Option<u32> {
        let container = r.container();
        r.vcx.update(|_, cx| {
            container
                .read(cx)
                .layout()
                .widgets
                .iter()
                .find(|w| w.content == WidgetContent::Fixed(kind.clone()))
                .map(|w| w.id)
        })
    }

    /// The entity id of the content view fixed widget `id` draws.
    fn content_of(r: &mut Rig, id: u32) -> Option<EntityId> {
        let container = r.container();
        r.vcx.update(|_, cx| {
            let fixed = container.read(cx).fixed(id).cloned()?;
            fixed.read(cx).content().map(|v| v.entity_id())
        })
    }

    fn charts(r: &mut Rig) -> (Option<Entity<AppsChart>>, Option<Entity<AppsChart>>) {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            (p.gpu_apps(), p.vram_apps())
        })
    }

    fn demand(r: &mut Rig) -> (bool, bool) {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let d = page.read(cx).demand_part(cx);
            (d.process_top, d.gpu_process)
        })
    }

    /// The page menu's item labels.
    fn menu_names(r: &mut Rig) -> Vec<String> {
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            let menu =
                tl_ui_kit::components::menu::TlMenu::build(window, cx, focus, |menu, w, cx| {
                    page.update(cx, |p, cx| p.menu_items(menu, w, cx))
                });
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| e.label.map(|l| l.to_string()).unwrap_or_default())
                .collect()
        })
    }

    /// Each segment of the switch sets the page's preset and its parts: GPU load by app in
    /// the engine grid's place, VRAM by app in the Memory chart's place, each for this page's
    /// GPU. Temperature, Power, Clocks, Details and Displays keep their ids, and the old key
    /// stays.
    #[gpui_kit::test]
    fn page_gpu_view_switch(cx: &mut TestAppContext) {
        let perf = PerformanceSettings {
            gpu_optional_graphs: vec![GpuOptionalGraph::Power, GpuOptionalGraph::Clocks],
            ..PerformanceSettings::default()
        };
        let mut r = open(cx, perf, 1200., 1400.);
        publish_groups(&r.engine, &[20., 10., 5.], &[512., 256., 128.]);
        draw(r.vcx);
        assert_eq!(page_view(&mut r), Some(GpuView::Usage));
        let kept = [
            r.metric_id("temp"),
            r.metric_id("power"),
            r.metric_id("clocks"),
            fixed_id(&mut r, FixedWidget::Details).expect("details"),
            fixed_id(&mut r, FixedWidget::GpuDisplays).expect("displays"),
        ];
        let words: Vec<String> = {
            let page = r.page.clone();
            r.vcx.update(|_, cx| {
                GpuView::ALL
                    .map(|v| page.read(cx).words().view(v).to_string())
                    .to_vec()
            })
        };
        assert_eq!(words, ["Usage", "Load by app", "VRAM by app"]);
        for (id, view, first, second) in [
            (
                "gpu-apps",
                GpuView::GpuApps,
                "fixed gpu_by_app",
                "metric memory",
            ),
            (
                "vram-apps",
                GpuView::VramApps,
                "fixed gpu_engine_grid",
                "fixed vram_by_app",
            ),
            (
                "usage",
                GpuView::Usage,
                "fixed gpu_engine_grid",
                "metric memory",
            ),
        ] {
            click_switch(&mut r, id);
            let saved = r.saved(1).expect("the page's entry");
            assert_eq!(layout_view(&saved), Some(view), "{id}");
            let describe = |w: &WidgetSetting| match &w.content {
                WidgetContent::Fixed(k) => format!("fixed {}", k.name()),
                c => format!("metric {}", c.metric_key().map_or("", |k| k.as_str())),
            };
            assert_eq!(describe(&saved.widgets[0]), first, "{id}");
            assert_eq!(describe(&saved.widgets[1]), second, "{id}");
            assert_eq!(
                metrics_of(&saved)[metrics_of(&saved).len() - 3..],
                ["temp", "power", "clocks"],
                "{id}: Power and Clocks stay"
            );
            for want in kept {
                assert!(
                    saved.widgets.iter().any(|w| w.id == want),
                    "{id}: id {want}"
                );
            }
            assert_eq!(page_view(&mut r), Some(view), "{id}");
            let (gpu, vram) = charts(&mut r);
            match view {
                GpuView::GpuApps => {
                    let gpu = gpu.expect("GPU load by app");
                    let metric = r.vcx.update(|_, cx| gpu.read(cx).metric());
                    assert_eq!(metric, AppsMetric::Gpu(GPU_A));
                    let w = fixed_id(&mut r, FixedWidget::GpuByApp).expect("the widget");
                    assert_eq!(content_of(&mut r, w), Some(gpu.entity_id()));
                    let apps = r.bounds(widget_id(&container(1), w));
                    assert!(near(apps.size.height, 300.), "{apps:?}");
                }
                GpuView::VramApps => {
                    let vram = vram.expect("VRAM by app");
                    let metric = r.vcx.update(|_, cx| vram.read(cx).metric());
                    assert_eq!(metric, AppsMetric::Vram(GPU_A));
                    let w = fixed_id(&mut r, FixedWidget::VramByApp).expect("the widget");
                    assert_eq!(content_of(&mut r, w), Some(vram.entity_id()));
                    // Temperature alone takes the full width under it.
                    let apps = r.bounds(widget_id(&container(1), w));
                    let temp = r.bounds(widget_id(&container(1), kept[0]));
                    assert!(
                        near(temp.size.width, f32::from(apps.size.width)),
                        "{temp:?}"
                    );
                    assert!(near(temp.top() - apps.bottom(), 8.), "{temp:?}");
                }
                GpuView::Usage => {
                    assert!(fixed_id(&mut r, FixedWidget::GpuByApp).is_none());
                    assert!(fixed_id(&mut r, FixedWidget::VramByApp).is_none());
                }
            }
        }
        r.vcx.update(|_, cx| {
            assert_eq!(
                TlSettings::settings(cx).performance.gpu_optional_graphs,
                [GpuOptionalGraph::Power, GpuOptionalGraph::Clocks],
                "the old key stays"
            );
        });
    }

    /// The view is saved per GPU: GPU A set to VRAM by app is read by a new page of GPU A,
    /// and GPU B stays on Usage with no entry.
    #[gpui_kit::test]
    fn page_gpu_view_saved_per_device(cx: &mut TestAppContext) {
        let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.update(cx, |p, cx| p.set_view(GpuView::VramApps, cx)));
        draw(r.vcx);
        let saved = r.saved(1).expect("GPU A's entry");
        assert_eq!(layout_view(&saved), Some(GpuView::VramApps));
        assert!(r.saved(2).is_none(), "GPU B has no entry");
        let (a, b) = (r.build_page(1), r.build_page(2));
        let views = r.vcx.update(|_, cx| (a.read(cx).view(), b.read(cx).view()));
        assert_eq!(views, (Some(GpuView::VramApps), Some(GpuView::Usage)));
    }

    /// With a grid layout the switch sits in the header with no segment on, and a pick asks
    /// "Replace your widgets?" first: nothing is written until the dialog is confirmed.
    #[gpui_kit::test]
    fn page_gpu_view_grid_asks(cx: &mut TestAppContext) {
        let cell = WidgetCell {
            col: 0,
            row: 0,
            cols: 12,
            rows: 6,
        };
        let mut perf = PerformanceSettings::default();
        let grid = grid_layout(
            1,
            vec![metric(
                1,
                "memory",
                FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
                Some(cell),
            )],
        );
        perf.layouts.set(grid.clone());
        let mut r = open(cx, perf, 1200., 900.);
        assert_eq!(page_view(&mut r), None);
        let switch = r.bounds(VIEW_SWITCH_ID.to_owned());
        let body = r.bounds(widget_id(&container(1), 1));
        assert!(
            switch.bottom() <= body.top(),
            "in the header: {switch:?} {body:?}"
        );
        let windows = r.vcx.update(|_, cx| cx.windows().len());
        click_switch(&mut r, "gpu-apps");
        assert_eq!(
            r.vcx.update(|_, cx| cx.windows().len()),
            windows + 1,
            "the dialog opens"
        );
        assert_eq!(r.saved(1), Some(grid), "nothing written yet");
        assert_eq!(page_view(&mut r), None);
    }

    /// "Groups shown" sets `top_n` of each placed chart: one item while one chart is placed,
    /// "GPU by app groups shown" and "VRAM by app groups shown" while both are. Without a
    /// by-app chart there is none.
    #[gpui_kit::test]
    fn page_gpu_by_app_groups_shown(cx: &mut TestAppContext) {
        let mut r = open(cx, PerformanceSettings::default(), 1200., 1400.);
        let names = menu_names(&mut r);
        assert!(
            !names.iter().any(|n| n.contains("roups shown")),
            "{names:?}"
        );
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.update(cx, |p, cx| p.set_view(GpuView::GpuApps, cx)));
        draw(r.vcx);
        let names = menu_names(&mut r);
        assert_eq!(names.first().map(String::as_str), Some("Groups shown"));
        // Both charts: VRAM by app in the Memory chart's place of the gpu_apps layout.
        r.update_perf(|perf| {
            let mut l = gpu_page_for_preset(perf, &container(1), GpuView::GpuApps.preset());
            l.widgets[1].content = WidgetContent::Fixed(FixedWidget::VramByApp);
            perf.layouts.set(l);
        });
        let names = menu_names(&mut r);
        assert_eq!(
            names[..2],
            ["GPU by app groups shown", "VRAM by app groups shown"],
            "{names:?}"
        );
        let focus = r.vcx.update(|_, cx| page.read(cx).focus_handle(cx));
        r.vcx.update(|window, cx| focus.focus(window, cx));
        r.vcx.dispatch_action(PerfSetGpuByAppTopN { count: 5 });
        r.vcx.dispatch_action(PerfSetVramByAppTopN { count: 15 });
        draw(r.vcx);
        let top = r.vcx.update(|_, cx| {
            let p = &TlSettings::settings(cx).performance;
            (p.gpu_by_app.top_n, p.vram_by_app.top_n)
        });
        assert_eq!(top, (5, 15));
    }

    /// Clicking a group's legend row opens Processes filtered to that group; "Other apps" and
    /// "Other / driver" open nothing.
    #[gpui_kit::test]
    fn page_gpu_by_app_legend_click(cx: &mut TestAppContext) {
        let mut r = open(cx, view_perf(GpuView::GpuApps), 1200., 900.);
        publish_groups(&r.engine, &[20., 10., 5.], &[]);
        draw(r.vcx);
        let apps = charts(&mut r).0.expect("GPU load by app");
        let rows = r.vcx.update(|_, cx| apps.read(cx).bands().len());
        let click = |r: &mut Rig, row: usize| {
            let at = r.bounds(format!("{LEGEND_ID}-{row}")).center();
            r.vcx.simulate_click(at, Modifiers::none());
            draw(r.vcx);
            r.vcx.read_entity(&apps, |a, _| {
                a.last_open
                    .as_ref()
                    .map(|(key, name)| (key.to_string(), name.to_string()))
            })
        };
        let opened: Vec<Option<(String, String)>> =
            (0..rows).map(|row| click(&mut r, row)).collect();
        let groups: Vec<&(String, String)> = opened.iter().flatten().collect();
        assert!(
            groups.contains(&&("exe:p2".to_owned(), "p2".to_owned())),
            "{opened:?}"
        );
        assert!(
            !groups.iter().any(|(k, _)| k == "other" || k == "driver"),
            "{opened:?}"
        );
    }

    /// A placed by-app chart asks for the process sweep and the per-process GPU rows; Usage
    /// asks for neither.
    #[gpui_kit::test]
    fn page_gpu_by_app_demand(cx: &mut TestAppContext) {
        let mut r = open(cx, PerformanceSettings::default(), 1200., 900.);
        assert_eq!(demand(&mut r), (false, false), "Usage");
        let page = r.page.clone();
        for (view, want) in [
            (GpuView::GpuApps, (true, true)),
            (GpuView::VramApps, (true, true)),
            (GpuView::Usage, (false, false)),
        ] {
            r.vcx
                .update(|_, cx| page.update(cx, |p, cx| p.set_view(view, cx)));
            draw(r.vcx);
            assert_eq!(demand(&mut r), want, "{view:?}");
        }
    }

    /// Edit Widgets on the GPU page lists GPU load by app and VRAM by app; added and applied
    /// they save with their cells, draw the page's charts and ask for the per-process GPU rows.
    #[gpui_kit::test]
    fn page_gpu_by_app_add_widget(cx: &mut TestAppContext) {
        let mut r = open(cx, PerformanceSettings::default(), 1200., 1400.);
        let page = r.page.clone();
        let editor = r.vcx.update(|_, cx| page.read(cx).editor().clone());
        r.vcx
            .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
        draw(r.vcx);
        let offered = |r: &mut Rig| {
            r.vcx.update(|_, cx| {
                editor
                    .read(cx)
                    .fixed_adds(cx)
                    .iter()
                    .map(|a| (a.name.to_string(), a.label.to_string()))
                    .collect::<Vec<_>>()
            })
        };
        let adds = offered(&mut r);
        for (name, label) in [
            ("gpu_by_app", "GPU load by app"),
            ("vram_by_app", "VRAM by app"),
        ] {
            assert!(
                adds.contains(&(name.to_owned(), label.to_owned())),
                "{adds:?}"
            );
        }
        let mut added = Vec::new();
        for name in ["gpu_by_app", "vram_by_app"] {
            added.push(
                r.vcx
                    .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed(name, cx)))
                    .expect(name),
            );
            draw(r.vcx);
        }
        let adds = offered(&mut r);
        assert!(
            !adds.iter().any(|(n, _)| n.ends_with("_by_app")),
            "a second copy is not offered: {adds:?}"
        );
        assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
        draw(r.vcx);
        let saved = r.saved(1).expect("the page's entry");
        assert!(saved.preset.is_none(), "a grid layout");
        for (id, kind) in added
            .iter()
            .zip([FixedWidget::GpuByApp, FixedWidget::VramByApp])
        {
            let w = saved.widgets.iter().find(|w| w.id == *id).expect("saved");
            assert_eq!(w.content, WidgetContent::Fixed(kind));
            assert!(w.cell.is_some(), "it has its cells");
        }
        let (gpu, vram) = charts(&mut r);
        let (gpu, vram) = (gpu.expect("gpu chart"), vram.expect("vram chart"));
        assert_eq!(content_of(&mut r, added[0]), Some(gpu.entity_id()));
        assert_eq!(content_of(&mut r, added[1]), Some(vram.entity_id()));
        assert_eq!(demand(&mut r), (true, true));
    }

    /// Render contract: repeated samples with the same bands never notify the page or its
    /// container and make or drop no entity; a sample in which a group enters the top N
    /// notifies only the chart, once.
    #[gpui_kit::test]
    fn page_gpu_by_app_render_contract(cx: &mut TestAppContext) {
        let mut r = open(cx, view_perf(GpuView::GpuApps), 1200., 900.);
        let container = r.container();
        let page = r.page.clone();
        publish_groups(&r.engine, &[20., 10., 5.], &[]);
        draw(r.vcx);
        let chart = charts(&mut r).0.expect("the chart");
        let bands = |r: &mut Rig| r.vcx.update(|_, cx| chart.read(cx).bands().len());
        let shown_bands = bands(&mut r);
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
                    container.read(cx).notify_count.get(),
                )
            })
        };
        let mut life = Lifecycle::new(&mut r);
        let entities = shown_entities(&mut r, &mut life);
        let before = counts(&mut r);
        for _ in 0..3 {
            publish_groups(&r.engine, &[20., 10., 5.], &[]);
            draw(r.vcx);
        }
        assert_eq!(
            counts(&mut r),
            before,
            "the page and container never notify"
        );
        assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
        assert_eq!(bands(&mut r), shown_bands, "the same bands");

        notified.set(0);
        publish_groups(&r.engine, &[20., 10., 5., 4.], &[]);
        draw(r.vcx);
        assert_eq!(bands(&mut r), shown_bands + 1, "p3 has its own band");
        assert_eq!(notified.get(), 1, "the chart notifies once");
        assert_eq!(
            counts(&mut r),
            before,
            "the page and container never notify"
        );
        assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
        assert_eq!(
            shown_entities(&mut r, &mut life),
            entities,
            "the same entities"
        );
    }

    /// Every band rebuild keeps the values and process counts (the g00wx.2 decision): after a
    /// window resize and after a view switch away and back, the bands of both charts are the
    /// same right after the trigger, before any new samples.
    #[gpui_kit::test]
    fn gpu_by_app_rebuild_keeps_values(cx: &mut TestAppContext) {
        let mut r = open(cx, view_perf(GpuView::GpuApps), 1200., 900.);
        publish_groups(&r.engine, &[20., 10., 5.], &[512., 256., 128.]);
        draw(r.vcx);
        let page = r.page.clone();
        r.vcx
            .update(|_, cx| page.update(cx, |p, cx| p.set_view(GpuView::VramApps, cx)));
        draw(r.vcx);
        r.vcx
            .update(|_, cx| page.update(cx, |p, cx| p.set_view(GpuView::GpuApps, cx)));
        draw(r.vcx);
        let (gpu, vram) = charts(&mut r);
        let (gpu, vram) = (gpu.expect("gpu chart"), vram.expect("vram chart"));
        let facts = |r: &mut Rig| {
            [&gpu, &vram].map(|apps| {
                r.vcx.read_entity(apps, |a, _| {
                    a.bands()
                        .iter()
                        .map(|b| (b.key.clone(), b.value, b.processes))
                        .collect::<Vec<_>>()
                })
            })
        };
        let before = facts(&mut r);
        for bands in &before {
            assert!(
                bands.len() >= 4,
                "three groups and Other apps at least: {bands:?}"
            );
            assert!(bands.iter().any(|b| b.1 > 0.), "{bands:?}");
        }
        assert!(
            before[0]
                .iter()
                .any(|b| &*b.0 == "exe:p0" && b.2 == Some(2)),
            "{:?}",
            before[0]
        );

        // Resize.
        r.vcx.simulate_resize(size(px(900.), px(900.)));
        draw(r.vcx);
        assert_eq!(facts(&mut r), before, "after the resize");

        // View switch away and back: the same chart entities, the same bands.
        for view in [GpuView::VramApps, GpuView::Usage, GpuView::GpuApps] {
            r.vcx
                .update(|_, cx| page.update(cx, |p, cx| p.set_view(view, cx)));
            draw(r.vcx);
            assert_eq!(facts(&mut r), before, "after the switch to {view:?}");
        }
        let (gpu_now, vram_now) = charts(&mut r);
        assert_eq!(gpu_now.map(|c| c.entity_id()), Some(gpu.entity_id()));
        assert_eq!(vram_now.map(|c| c.entity_id()), Some(vram.entity_id()));
    }
}
