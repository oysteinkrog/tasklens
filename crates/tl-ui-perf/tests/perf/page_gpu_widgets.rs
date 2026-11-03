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
