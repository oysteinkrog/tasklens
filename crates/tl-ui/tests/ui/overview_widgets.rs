//! Overview box widgets (UI-PRF-061, ADR-019): the widget entity, its compositions, the
//! shared source data, the widget container on its inner grid and Edit Widgets. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, ParentElement as _, Render, SharedString, StyleRefinement, Styled as _,
    TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FormSet, GPU_BOX_CONTAINER, LAYOUT_VERSION, MetricKey,
    PresetSetting, SourceSelector, WidgetCell, WidgetOptions, WidgetSetting,
};
use tl_model::layout::{convert_old_settings, gpu_box_layout};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot, SystemSnapshot};
use tl_model::system::{GpuEngine, GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::YScale;
use tl_ui::chart::labels::LabelKind;
use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, open_context_menu};
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{PerfWidgetSetForm, WidgetSource, bind_keys_performance};
use tl_ui::pages::performance::widget::compose::{
    Composition, cell_min, row_forms, row_min, tile_min,
};
use tl_ui::pages::performance::widget::container::{WidgetContainer, container_id, scroll_id};
use tl_ui::pages::performance::widget::edit::{
    ADD_ID, APPLY_ID, CANCEL_ID, PREVIEW_ID, RESET_ID, TOOLBAR_ID, UNDO_ID, WidgetEditor,
    corner_id, default_layout, editor_id, frame_id, gear_id, grip_id,
};
use tl_ui::pages::performance::widget::grid::{self, InnerGrid, SlotDevice};
use tl_ui::pages::performance::widget::metric_widget::{MetricWidget, part_id, widget_id};
use tl_ui::pages::performance::widget::source::{
    Availability, AvailabilityChanged, SourceData, SourceStore,
};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const GIB: u64 = 1 << 30;
const GPU_A: u16 = 10;
const GPU_B: u16 = 11;
/// The uuid byte of a GPU the catalog does not list at first.
const GONE: u8 = 0xee;

/// The widgets, each in a box of its own size, one under the other, then the page with the
/// container in a box of its size.
struct Host {
    widgets: Vec<(Entity<MetricWidget>, f32, f32)>,
    container: Option<(Entity<Page>, f32, f32)>,
    /// A widget editor around its container, in a box of its size.
    editor: Option<(Entity<WidgetEditor>, f32, f32)>,
}

/// Stands in for the Overview page in the render counts: a cached view that holds the
/// container as a cached child view. The host is the window's root, which every test frame
/// renders, so it cannot count.
struct Page {
    container: Entity<WidgetContainer>,
    render_count: Cell<u32>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.render_count.set(self.render_count.get() + 1);
        div().size_full().child(
            self.container
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .children(self.widgets.iter().map(|(w, width, height)| {
                div()
                    .flex_none()
                    .w(px(*width))
                    .h(px(*height))
                    .child(w.clone().cached(StyleRefinement::default().size_full()))
            }))
            .children(self.container.iter().map(|(c, width, height)| {
                div()
                    .flex_none()
                    .w(px(*width))
                    .h(px(*height))
                    .child(c.clone().cached(StyleRefinement::default().size_full()))
            }))
            .children(self.editor.iter().map(|(e, width, height)| {
                div()
                    .flex_none()
                    .w(px(*width))
                    .h(px(*height))
                    .child(e.clone().cached(StyleRefinement::default().size_full()))
            }))
    }
}

fn gpu_id(n: u8) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid: [n; 16],
        pci_bus_id: Arc::from(format!("0000:0{n}:00.0")),
    }
}

fn entry(idx: u16, identity: DeviceIdentity, name: &str, present: bool) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c: None,
    }
}

/// The settings key of the GPU with uuid byte `n`.
fn key(n: u8) -> Arc<str> {
    DeviceKey::from_identity(&gpu_id(n)).expect("gpu key").0
}

/// Two GPUs at 10 and 11, and with `gone` the third card at 12 present or not.
fn catalog(version: u64, gone: Option<bool>) -> DeviceCatalog {
    let mut entries: Vec<DeviceEntry> = (0..10)
        .map(|i| {
            entry(
                i,
                DeviceIdentity::Aggregate(tl_model::devices::AggregateKind::Disks),
                "x",
                false,
            )
        })
        .collect();
    entries.push(entry(GPU_A, gpu_id(1), "Card A", true));
    entries.push(entry(GPU_B, gpu_id(2), "Card B", true));
    if let Some(present) = gone {
        entries.push(entry(12, gpu_id(GONE), "Old card", present));
    }
    DeviceCatalog {
        version,
        entries: Arc::from(entries),
    }
}

fn gpu_totals(device: u16, util: f32) -> GpuTotals {
    GpuTotals {
        device: DeviceIdx(device),
        util_pct: Metric::valid(util),
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
        temp_c: Metric::with(Validity::Unsupported),
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

fn selector(device: DeviceSelector, metric: &str) -> SourceSelector {
    SourceSelector {
        device,
        metric: MetricKey::new(metric),
    }
}

fn widget(id: u32, source: SourceSelector, forms: FormSet) -> WidgetSetting {
    WidgetSetting {
        id,
        source,
        forms,
        cell: None,
        options: WidgetOptions::default(),
        device_name: Some(Arc::from("Old card")),
    }
}

fn engine_gpu() -> String {
    MetricKey::engine(GpuEngine::Gpu).as_str().to_string()
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    hub: Entity<SnapshotHub>,
    store: SourceStore,
    host: Entity<Host>,
    vcx: &'a mut VisualTestContext,
    series_version: u64,
    _driver: Entity<AnimationDriver>,
}

fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog(1, None));
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.gpus = Arc::from([
            (DeviceIdx(GPU_A), gpu_totals(GPU_A, 25.)),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
        ]);
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
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let store = SourceStore::new(&hub);
    let (host, vcx) = cx.add_window_view(|_, _| Host {
        widgets: Vec::new(),
        container: None,
        editor: None,
    });
    vcx.simulate_scale_factor_change(1.0);
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    vcx.simulate_resize(size(px(500.), px(1600.)));
    vcx.run_until_parked();
    Rig {
        engine,
        hub,
        store,
        host,
        vcx,
        series_version: 0,
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

impl Rig<'_> {
    /// Adds a widget `w × h` px and draws.
    fn add(&mut self, setting: WidgetSetting, w: f32, h: f32) -> Entity<MetricWidget> {
        let hub = self.hub.clone();
        let host = self.host.clone();
        let store = &mut self.store;
        let widget = self.vcx.update(|_, cx| {
            let source = store.source(&setting.source, cx);
            let widget = cx.new(|cx| MetricWidget::new(setting, source, &hub, cx));
            host.update(cx, |host, cx| {
                host.widgets.push((widget.clone(), w, h));
                cx.notify();
            });
            widget
        });
        draw(self.vcx);
        widget
    }

    fn publish(&mut self, f: impl FnOnce(&mut Snapshot)) {
        self.engine.publish_with(f);
        self.vcx.run_until_parked();
        draw(self.vcx);
    }

    fn system(&mut self, f: impl FnOnce(&mut SystemSnapshot)) {
        self.publish(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            f(sys);
        });
    }

    /// A one-second GPU utilization series of GPU A at `values`, newest last, ending now.
    fn util_series(&mut self, values: &[f32]) {
        self.series_version += 1;
        let version = self.series_version;
        let newest = now_boot_ns();
        let n = values.len() as u64;
        let view = Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(GPU_A),
                metric: MetricId::GPU_UTIL,
            },
            version,
            period_ns: 1_000_000_000,
            samples: values
                .iter()
                .enumerate()
                .map(|(k, v)| Sample::valid(newest - (n - 1 - k as u64) * 1_000_000_000, *v))
                .collect::<Vec<_>>()
                .into(),
        });
        self.publish(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from([view]),
                slow: Arc::from([]),
            });
        });
    }

    fn read<R>(&mut self, w: &Entity<MetricWidget>, f: impl FnOnce(&MetricWidget) -> R) -> R {
        self.vcx.read_entity(w, |w, _| f(w))
    }

    fn counts(&mut self, w: &Entity<MetricWidget>) -> (u32, u32) {
        self.read(w, |w| (w.notify_count.get(), w.render_count.get()))
    }

    fn has_part(&mut self, id: u32, part: &str) -> bool {
        let p: &'static str = Box::leak(part_id(id, part).into_boxed_str());
        self.vcx.debug_bounds(p).is_some()
    }
}

#[gpui_kit::test]
fn overview_widget_compositions_min_sizes(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let a = || selector(DeviceSelector::Key(key(1)), &engine_gpu());
    let all = FormSet::all();
    let cell = cell_min(all);
    let tile_forms = FormSet::LABEL | FormSet::VALUE | FormSet::STATS | FormSet::BAR;
    let tile = tile_min(tile_forms);
    let pair = FormSet::LABEL | FormSet::VALUE;
    let row = row_min(pair);
    let cases: Vec<(u32, FormSet, f32, f32, Composition)> = vec![
        (1, all, cell.width, cell.height, Composition::Cell),
        (2, all, 400., 300., Composition::Cell),
        (3, all, cell.width - 1., cell.height, Composition::Row),
        (4, all, 400., cell.height - 1., Composition::Row),
        (5, tile_forms, tile.width, tile.height, Composition::Tile),
        (6, tile_forms, 300., 200., Composition::Tile),
        (
            7,
            tile_forms,
            tile.width,
            tile.height - 1.,
            Composition::Row,
        ),
        (8, pair, row.width, row.height, Composition::Row),
        (9, pair, 400., 18., Composition::Row),
        (10, FormSet::VALUE, 48., 18., Composition::Row),
    ];
    let mut widgets = Vec::new();
    for (id, forms, w, h, _) in &cases {
        widgets.push(r.add(widget(*id, a(), *forms), *w, *h));
    }
    for ((id, forms, w, _, want), widget) in cases.iter().zip(&widgets) {
        let composed = r.read(widget, |w| w.composed());
        assert_eq!(composed.composition, *want, "widget {id} at {w} px");
        let kept = if *want == Composition::Row {
            row_forms(*forms, *w)
        } else {
            *forms
        };
        assert_eq!(composed.forms, kept, "widget {id}");
    }
    // The Cell at its minimum draws its header, stats, bar and a chart of at least 30 px.
    for part in ["name", "value", "stats", "bar", "chart"] {
        assert!(r.has_part(1, part), "cell {part}");
    }
    let chart: &'static str = Box::leak(part_id(1, "chart").into_boxed_str());
    let bounds = r.vcx.debug_bounds(chart).expect("chart");
    assert!(bounds.size.height >= px(30.) - px(0.5), "{bounds:?}");
    assert!(r.read(&widgets[0], |w| w.chart().is_some()));
    // The narrow Row drops the stats first and keeps the value.
    let narrow = r.read(&widgets[2], |w| w.composed().forms);
    assert!(!narrow.contains(FormSet::STATS), "{narrow:?}");
    assert!(narrow.contains(FormSet::VALUE));
    assert!(!r.has_part(3, "stats"));
    assert!(r.has_part(3, "value"));
    // A Row with a name and a value keeps both at its minimum.
    assert!(r.has_part(8, "name") && r.has_part(8, "value"));
    // A Tile shows no chart.
    assert!(!r.has_part(5, "chart"));
    assert!(r.read(&widgets[4], |w| w.chart().is_none()));
    // A larger box keeps its composition: growing a Cell renders nothing new.
    let (before, _) = r.counts(&widgets[1]);
    let host = r.host.clone();
    r.vcx.update(|_, cx| {
        host.update(cx, |h, cx| {
            h.widgets[1].1 = 450.;
            h.widgets[1].2 = 320.;
            cx.notify();
        })
    });
    draw(r.vcx);
    assert_eq!(r.counts(&widgets[1]).0, before, "a resize in one class");
    assert_eq!(
        r.read(&widgets[1], |w| w.composed().composition),
        Composition::Cell
    );
}

#[gpui_kit::test]
fn overview_widget_unavailable_states(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
    let gone = r.add(
        widget(
            1,
            selector(DeviceSelector::Key(key(GONE)), &engine_gpu()),
            forms,
        ),
        240.,
        120.,
    );
    let temp = r.add(
        widget(2, selector(DeviceSelector::Key(key(1)), "temp"), forms),
        240.,
        120.,
    );
    let unknown = r.add(
        widget(3, selector(DeviceSelector::Key(key(1)), "fan_speed"), forms),
        240.,
        120.,
    );
    let empty_slot = r.add(
        widget(4, selector(DeviceSelector::Slot(1), "vram"), forms),
        240.,
        120.,
    );
    let state = |r: &mut Rig, w: &Entity<MetricWidget>| {
        r.read(w, |w| {
            let [name, _, _, reason] = w.texts();
            (
                w.availability(),
                name.to_string(),
                reason.to_string(),
                w.chart().is_some(),
            )
        })
    };
    assert_eq!(
        state(&mut r, &gone),
        (
            Availability::Gone,
            "Old card · 3D / Compute".to_string(),
            "Not connected".to_string(),
            false
        )
    );
    assert_eq!(
        state(&mut r, &temp),
        (
            Availability::NotReported,
            "Temperature".to_string(),
            "Not reported by this GPU".to_string(),
            false
        )
    );
    assert_eq!(
        state(&mut r, &unknown),
        (
            Availability::Unknown,
            "fan_speed".to_string(),
            "Needs a newer tasklens".to_string(),
            false
        )
    );
    assert_eq!(state(&mut r, &empty_slot).0, Availability::Gone);
    assert!(r.has_part(1, "reason") && r.has_part(1, "name"));
    assert!(!r.has_part(1, "chart") && !r.has_part(1, "value"));

    // The card comes back: the widget shows its data again, and the source says so once.
    let source = r.read(&gone, |w| w.source().clone());
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let _sub = r.vcx.update(|_, cx| {
        cx.subscribe(&source, move |_, ev: &AvailabilityChanged, _| {
            sink.lock().unwrap().push(ev.0);
        })
    });
    r.publish(|s| {
        s.devices = Arc::new(catalog(2, Some(true)));
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.gpus = Arc::from([
            (DeviceIdx(GPU_A), gpu_totals(GPU_A, 25.)),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
            (DeviceIdx(12), gpu_totals(12, 75.)),
        ]);
    });
    let (availability, name, reason, chart) = state(&mut r, &gone);
    assert_eq!(availability, Availability::Available);
    assert_eq!(
        name, "3D / Compute",
        "no device name while the card is here"
    );
    assert!(reason.is_empty());
    assert!(chart, "the chart is back");
    assert_eq!(r.read(&gone, |w| w.texts()[1].to_string()), "75.0%");
    assert!(r.has_part(1, "value") && !r.has_part(1, "reason"));
    assert_eq!(*events.lock().unwrap(), vec![Availability::Available]);

    // Unplugged again: the widget keeps its place and shows the gone state.
    r.publish(|s| s.devices = Arc::new(catalog(3, Some(false))));
    assert_eq!(state(&mut r, &gone).0, Availability::Gone);
    assert_eq!(
        *events.lock().unwrap(),
        vec![Availability::Available, Availability::Gone]
    );

    // A slot follows the GPUs the box shows.
    let host = r.host.clone();
    let store = &mut r.store;
    r.vcx.update(|_, cx| {
        store.set_slots(&[key(1), key(2)], cx);
        host.update(cx, |_, cx| cx.notify());
    });
    draw(r.vcx);
    assert_eq!(state(&mut r, &empty_slot).0, Availability::Available);
    let source = r.read(&empty_slot, |w| w.source().clone());
    let device = r.vcx.read_entity(&source, |s, _| s.device());
    assert_eq!(device, Some(DeviceIdx(GPU_B)));
}

#[gpui_kit::test]
fn overview_widget_shared_source(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let sel = selector(DeviceSelector::Key(key(1)), &engine_gpu());
    let one = r.add(widget(1, sel.clone(), FormSet::all()), 240., 160.);
    let two = r.add(
        widget(
            2,
            sel.clone(),
            FormSet::STATS | FormSet::VALUE | FormSet::LABEL,
        ),
        240.,
        60.,
    );
    let other = r.add(
        widget(
            3,
            selector(DeviceSelector::Key(key(1)), "vram"),
            FormSet::VALUE,
        ),
        240.,
        30.,
    );
    let (s1, s2, s3) = (
        r.read(&one, |w| w.source().clone()),
        r.read(&two, |w| w.source().clone()),
        r.read(&other, |w| w.source().clone()),
    );
    assert_eq!(s1, s2, "one source for one selector");
    assert_ne!(s1, s3);
    assert_eq!(r.store.len(), 2);

    // A new series: the shared source works the stats out once for both widgets.
    let stats_count =
        |r: &mut Rig, s: &Entity<SourceData>| r.vcx.read_entity(s, |s, _| s.stats_count.get());
    let before = stats_count(&mut r, &s1);
    let vram_before = stats_count(&mut r, &s3);
    r.util_series(&[10., 30., 20.]);
    assert_eq!(stats_count(&mut r, &s1), before + 1);
    let stats = |r: &mut Rig, w: &Entity<MetricWidget>| r.read(w, |w| w.texts()[2].to_string());
    let text = stats(&mut r, &one);
    assert_eq!(text, "min 10.0%  avg 20.0%  peak 30.0%");
    assert_eq!(stats(&mut r, &two), text);
    // The VRAM source reads no utilization series.
    assert_eq!(stats_count(&mut r, &s3), vram_before);

    // The last widget on a source drops it.
    let host = r.host.clone();
    r.vcx.update(|_, cx| {
        host.update(cx, |h, cx| {
            h.widgets.retain(|(w, _, _)| *w != one && *w != two);
            cx.notify();
        })
    });
    drop((one, two, s1, s2));
    draw(r.vcx);
    assert_eq!(r.store.len(), 1, "only the VRAM source is left");
}

#[gpui_kit::test]
fn overview_widget_sample_notifies_only_its_widgets(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let util = selector(DeviceSelector::Key(key(1)), &engine_gpu());
    let vram = selector(DeviceSelector::Key(key(1)), "vram");
    let bar = r.add(
        widget(
            1,
            util.clone(),
            FormSet::LABEL | FormSet::BAR | FormSet::VALUE,
        ),
        300.,
        18.,
    );
    let mem = r.add(widget(2, vram, FormSet::LABEL | FormSet::VALUE), 300., 18.);
    let chart = r.add(widget(3, util, FormSet::CHART | FormSet::VALUE), 300., 120.);
    let other_gpu = r.add(
        widget(
            4,
            selector(DeviceSelector::Key(key(2)), &engine_gpu()),
            FormSet::VALUE | FormSet::BAR,
        ),
        300.,
        18.,
    );
    let ids: Vec<_> = [&bar, &mem, &chart, &other_gpu]
        .iter()
        .map(|w| w.entity_id())
        .collect();
    let base: Vec<_> = [&bar, &mem, &chart, &other_gpu]
        .iter()
        .map(|w| r.counts(w))
        .collect();
    assert_eq!(r.read(&bar, |w| w.texts()[1].to_string()), "25.0%");

    // GPU A's utilization changes: only the widgets on it notify and render.
    r.system(|s| {
        s.gpus = Arc::from([
            (DeviceIdx(GPU_A), gpu_totals(GPU_A, 60.)),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
        ]);
    });
    let now: Vec<_> = [&bar, &mem, &chart, &other_gpu]
        .iter()
        .map(|w| r.counts(w))
        .collect();
    assert_eq!(now[0].0, base[0].0 + 1, "the bar widget notifies once");
    assert!(now[0].1 > base[0].1, "and renders");
    assert_eq!(now[1], base[1], "VRAM did not change");
    assert_eq!(now[2].0, base[2].0 + 1, "the chart widget's value changed");
    assert_eq!(now[3], base[3], "the other GPU did not change");
    assert_eq!(r.read(&bar, |w| w.texts()[1].to_string()), "60.0%");
    assert_eq!(r.read(&bar, |w| w.bar_fraction()), Some(0.6));

    // A sample that changes nothing any widget shows notifies none.
    let base = now;
    r.system(|s| {
        let mut a = gpu_totals(GPU_A, 60.);
        a.power_w = Metric::valid(200.);
        s.gpus = Arc::from([
            (DeviceIdx(GPU_A), a),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
        ]);
    });
    let now: Vec<_> = [&bar, &mem, &chart, &other_gpu]
        .iter()
        .map(|w| r.counts(w).0)
        .collect();
    assert_eq!(now, base.iter().map(|c| c.0).collect::<Vec<_>>());

    // No widget entity was made or dropped.
    let after: Vec<_> = r.vcx.read_entity(&r.host, |h, _| {
        h.widgets.iter().map(|(w, _, _)| w.entity_id()).collect()
    });
    assert_eq!(after, ids);
}

#[gpui_kit::test]
fn overview_widget_hidden_parts_do_not_notify(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let util = || selector(DeviceSelector::Key(key(1)), &engine_gpu());
    let forms = FormSet::VALUE | FormSet::STATS;
    // Wide enough for the value and the stats; a narrow Row keeps only the value.
    let wide = r.add(widget(1, util(), forms), 300., 18.);
    let narrow = r.add(widget(2, util(), forms), 100., 18.);
    // A narrow Row that drops its sparkline has no chart.
    let spark = r.add(
        widget(3, util(), FormSet::VALUE | FormSet::STATS | FormSet::CHART),
        60.,
        18.,
    );
    assert_eq!(r.read(&wide, |w| w.composed().forms), forms);
    assert_eq!(r.read(&narrow, |w| w.composed().forms), FormSet::VALUE);
    assert_eq!(r.read(&spark, |w| w.composed().forms), FormSet::VALUE);
    assert!(
        r.read(&spark, |w| w.chart().is_none()),
        "no hidden sparkline"
    );
    assert!(!r.has_part(2, "stats") && !r.has_part(3, "chart"));

    // A new series changes only the stats: the widgets that drop them do not notify.
    let base: Vec<_> = [&wide, &narrow, &spark]
        .iter()
        .map(|w| r.counts(w))
        .collect();
    r.util_series(&[10., 30., 20.]);
    let now: Vec<_> = [&wide, &narrow, &spark]
        .iter()
        .map(|w| r.counts(w))
        .collect();
    assert_eq!(now[0].0, base[0].0 + 1, "the wide widget shows the stats");
    assert_eq!(now[1], base[1], "the narrow widget hides them");
    assert_eq!(now[2], base[2], "the sparkline widget hides them");
    let stats = |r: &mut Rig, w: &Entity<MetricWidget>| r.read(w, |w| w.texts()[2].to_string());
    assert_eq!(stats(&mut r, &wide), "min 10.0%  avg 20.0%  peak 30.0%");
    assert!(stats(&mut r, &narrow).is_empty());

    // Widened, the narrow widget keeps the stats and shows the current text.
    let host = r.host.clone();
    r.vcx.update(|_, cx| {
        host.update(cx, |h, cx| {
            h.widgets[1].1 = 300.;
            h.widgets[2].1 = 400.;
            cx.notify();
        })
    });
    draw(r.vcx);
    assert_eq!(r.read(&narrow, |w| w.composed().forms), forms);
    assert_eq!(stats(&mut r, &narrow), "min 10.0%  avg 20.0%  peak 30.0%");
    assert!(r.has_part(2, "stats"));
    assert!(
        r.read(&spark, |w| w.chart().is_some()),
        "the sparkline is back"
    );
    assert!(r.has_part(3, "chart"));
}

#[gpui_kit::test]
fn overview_widget_chart_follows_source_scale(cx: &mut TestAppContext) {
    let mut r = open(cx);
    // A chart alone on VRAM: its Y scale is the card's total.
    let chart = r.add(
        widget(
            1,
            selector(DeviceSelector::Key(key(1)), "vram"),
            FormSet::CHART,
        ),
        240.,
        120.,
    );
    let y = |r: &mut Rig| {
        let view = r.read(&chart, |w| w.chart().cloned()).expect("chart");
        r.vcx.read_entity(&view, |v, _| v.spec.y)
    };
    assert_eq!(y(&mut r), YScale::FixedTotal((16 * GIB) as f32));
    let (notified, _) = r.counts(&chart);

    // A new total keeps the series and the forms; only the chart's scale changes.
    r.system(|s| {
        let mut a = gpu_totals(GPU_A, 25.);
        a.mem_total = Metric::valid(24 * GIB);
        s.gpus = Arc::from([
            (DeviceIdx(GPU_A), a),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
        ]);
    });
    assert_eq!(y(&mut r), YScale::FixedTotal((24 * GIB) as f32));
    assert_eq!(
        r.counts(&chart).0,
        notified,
        "the widget itself shows nothing new"
    );
}

/// A source switch on a kept chart (percent to bytes) brings the new source's value format
/// to the chart's labels, not only to its spec.
#[gpui_kit::test]
fn overview_widget_source_switch_updates_chart_labels(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let first = widget(
        1,
        selector(DeviceSelector::Key(key(1)), &engine_gpu()),
        FormSet::CHART,
    );
    let w = r.add(first.clone(), 240., 120.);
    let fmt = |r: &mut Rig| {
        let view = r.read(&w, |w| w.chart().cloned()).expect("chart");
        r.vcx.read_entity(&view, |v, _| v.labels.value_fmt)
    };
    let before = fmt(&mut r);
    let view = r.read(&w, |w| w.chart().cloned()).expect("chart");

    let vram = WidgetSetting {
        source: selector(DeviceSelector::Key(key(1)), "vram"),
        ..first
    };
    let store = &mut r.store;
    r.vcx.update(|_, cx| {
        let source = store.source(&vram.source, cx);
        w.update(cx, |w, cx| w.set_setting(vram, source, cx));
    });
    draw(r.vcx);
    let after = fmt(&mut r);
    assert_ne!(before, after, "VRAM bytes, not the engine's percent");
    assert_eq!(
        r.read(&w, |w| w.chart().cloned()).map(|c| c.entity_id()),
        Some(view.entity_id()),
        "the chart entity is kept"
    );
}

// The widget container on its inner grid (tasklens-lhliq.5).

/// A grid layout of the GPU box with (id, source, forms, cell) widgets.
fn grid_layout(widgets: Vec<(u32, SourceSelector, FormSet, WidgetCell)>) -> ContainerLayout {
    let next_id = widgets.iter().map(|w| w.0 + 1).max().unwrap_or(1);
    ContainerLayout {
        container: GPU_BOX_CONTAINER.to_owned(),
        version: LAYOUT_VERSION,
        preset: None,
        widgets: widgets
            .into_iter()
            .map(|(id, source, forms, cell)| WidgetSetting {
                cell: Some(cell),
                ..widget(id, source, forms)
            })
            .collect(),
        next_id,
    }
}

fn cell(col: u16, row: u16, cols: u16, rows: u16) -> WidgetCell {
    WidgetCell {
        col,
        row,
        cols,
        rows,
    }
}

const CHART_FORMS: FormSet = FormSet::CHART.union(FormSet::VALUE).union(FormSet::LABEL);
const ROW_FORMS: FormSet = FormSet::BAR.union(FormSet::VALUE).union(FormSet::LABEL);

/// Two charts side by side on GPU A, the VRAM row under them, and GPU B's value row.
fn four_widgets() -> ContainerLayout {
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    grid_layout(vec![
        (1, a(&engine_gpu()), CHART_FORMS, cell(0, 0, 3, 4)),
        (2, a("engine:enc"), CHART_FORMS, cell(3, 0, 3, 4)),
        (3, a("vram"), ROW_FORMS, cell(0, 4, 6, 1)),
        (
            4,
            selector(DeviceSelector::Key(key(2)), &engine_gpu()),
            FormSet::VALUE | FormSet::LABEL,
            cell(0, 5, 6, 1),
        ),
    ])
}

impl Rig<'_> {
    /// Puts a container showing `layout` in a box `w × h` px under the widgets, and draws.
    fn container(&mut self, layout: ContainerLayout, w: f32, h: f32) -> Entity<WidgetContainer> {
        let hub = self.hub.clone();
        let host = self.host.clone();
        let store = Rc::new(RefCell::new(SourceStore::new(&hub)));
        let c = self.vcx.update(|_, cx| {
            let c = cx.new(|cx| WidgetContainer::new(GPU_BOX_CONTAINER, layout, &hub, store, cx));
            host.update(cx, |host, cx| {
                let page = cx.new(|_| Page {
                    container: c.clone(),
                    render_count: Cell::new(0),
                });
                host.container = Some((page, w, h));
                cx.notify();
            });
            c
        });
        draw(self.vcx);
        c
    }

    fn resize_container(&mut self, w: f32, h: f32) {
        let host = self.host.clone();
        self.vcx.update(|_, cx| {
            host.update(cx, |host, cx| {
                if let Some(c) = host.container.as_mut() {
                    c.1 = w;
                    c.2 = h;
                }
                cx.notify();
            })
        });
        draw(self.vcx);
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

    /// Widget `id`'s bounds relative to the container's top left, (x, y, w, h).
    fn widget_rect(&mut self, id: u32) -> Option<(f32, f32, f32, f32)> {
        let body: &'static str = Box::leak(container_id(GPU_BOX_CONTAINER).into_boxed_str());
        let origin = self.vcx.debug_bounds(body)?.origin;
        let b = self
            .vcx
            .debug_bounds(Box::leak(widget_id(id).into_boxed_str()))?;
        Some((
            f32::from(b.origin.x - origin.x),
            f32::from(b.origin.y - origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        ))
    }

    /// The container's render and notify counts and the page's render count.
    fn container_counts(&mut self, c: &Entity<WidgetContainer>) -> (u32, u32, u32) {
        let (render, notify) = self
            .vcx
            .read_entity(c, |c, _| (c.render_count.get(), c.notify_count.get()));
        let page = self
            .vcx
            .read_entity(&self.host, |h, _| h.container.as_ref().map(|p| p.0.clone()))
            .expect("page");
        let page = self.vcx.read_entity(&page, |p, _| p.render_count.get());
        (render, notify, page)
    }

    fn entity_ids(&mut self, c: &Entity<WidgetContainer>) -> Vec<(u32, gpui_kit::EntityId)> {
        self.vcx.read_entity(c, |c, _| {
            c.placed_ids()
                .into_iter()
                .filter_map(|id| c.widget(id).map(|w| (id, w.entity_id())))
                .collect()
        })
    }
}

fn overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// The GPU box at its minimum of 200 × 220 px leaves a body of about 176 × 172 px. The four
/// widgets fill it without overlap; the charts are too narrow for a Cell and draw as Rows. A
/// taller layout keeps its spans, scrolls and leaves room for the scrollbar.
#[gpui_kit::test]
fn overview_widget_grid_small_box(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let c = r.container(four_widgets(), 176., 172.);
    let rects: Vec<_> = (1..=4)
        .map(|id| r.widget_rect(id).expect("widget drawn"))
        .collect();
    for (i, a) in rects.iter().enumerate() {
        assert!(a.0 >= 0. && a.0 + a.2 <= 176.5, "{} inside: {a:?}", i + 1);
        for b in &rects[i + 1..] {
            assert!(!overlap(*a, *b), "{a:?} {b:?}");
        }
    }
    // Six rows fill 172 px: (172 − 5 · 4) / 6 = 25.3 px each, so the last row ends at the
    // bottom of the body.
    let last = rects[3];
    assert!((last.1 + last.3 - 172.).abs() <= 1., "{last:?}");
    assert!((last.3 - 25.3).abs() <= 1., "{last:?}");
    assert!(r.vcx.read_entity(&c, |c, _| !c.placement().scrolls));
    // 3 of 6 columns are 86 px: below the Cell minimum of 120, so the chart draws as a Row.
    let w1 = r
        .vcx
        .read_entity(&c, |c, _| c.widget(1).cloned())
        .expect("w1");
    assert_eq!(r.read(&w1, |w| w.composed().composition), Composition::Row);
    let scroll: &'static str = Box::leak(scroll_id(GPU_BOX_CONTAINER).into_boxed_str());
    assert!(r.vcx.debug_bounds(scroll).is_none());

    // A fifth widget four rows high: ten rows do not fit, so the body scrolls at the unit.
    let mut taller = four_widgets();
    taller.widgets.push(WidgetSetting {
        cell: Some(cell(0, 6, 6, 4)),
        ..widget(
            5,
            selector(DeviceSelector::Key(key(1)), "vram"),
            CHART_FORMS,
        )
    });
    r.vcx
        .update(|_, cx| c.update(cx, |c, cx| c.set_layout(taller, cx)));
    draw(r.vcx);
    let p = r.vcx.read_entity(&c, |c, _| c.placement().clone());
    assert!(p.scrolls);
    assert_eq!(p.grid.row, 24.);
    // Ten rows: 10 · 24 + 9 · 4 = 276.
    assert_eq!(p.content_h, px(276.));
    assert!(r.vcx.debug_bounds(scroll).is_some());
    let b = p.bounds(5).expect("placed");
    assert_eq!(f32::from(b.right()), 160., "16 px left for the scrollbar");
    // The spans stay: a shrink changes no cell.
    let cells: Vec<_> = r.vcx.read_entity(&c, |c, _| {
        c.layout().widgets.iter().map(|w| w.cell).collect()
    });
    assert_eq!(cells[0], Some(cell(0, 0, 3, 4)));
}

/// The same layout at 1.0, 1.5 and 2.0 UI scale in a body of the same logical size draws every
/// widget at the same logical rectangle: the unit, the gutter and the minimums scale, the spans
/// do not.
#[gpui_kit::test]
fn overview_widget_grid_scale_1_5_and_2(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let c = r.container(four_widgets(), 400., 300.);
    let base: Vec<_> = (1..=4)
        .map(|id| r.widget_rect(id).expect("drawn"))
        .collect();
    let row = r.vcx.read_entity(&c, |c, _| c.placement().grid.row);
    for pct in [150u16, 200] {
        let s = f32::from(pct) / 100.;
        r.set_ui_scale(pct);
        r.resize_container(400. * s, 300. * s);
        let p = r.vcx.read_entity(&c, |c, _| c.placement().clone());
        assert!(!p.scrolls, "{pct}%");
        assert!((p.grid.gutter - 4. * s).abs() < 0.01, "{pct}%");
        assert!((p.grid.unit - 24. * s).abs() < 0.01, "{pct}%");
        assert!((p.grid.row - row * s).abs() < 0.01, "{pct}%");
        for (id, want) in (1..=4).zip(&base) {
            let got = r.widget_rect(id).expect("drawn");
            for (g, w) in [got.0, got.1, got.2, got.3]
                .into_iter()
                .zip([want.0, want.1, want.2, want.3])
            {
                assert!(
                    (g - w * s).abs() <= s + 1.,
                    "{pct}% widget {id}: {got:?} vs {want:?}"
                );
            }
        }
        // The minimum span of a Row with a name is the same number of columns.
        let g = InnerGrid::new(400. * s, 6, s);
        assert_eq!(
            g.forms_min_span(ROW_FORMS),
            InnerGrid::new(400., 6, 1.).forms_min_span(ROW_FORMS)
        );
    }
}

/// The render contract (ADR-019, UI-PRF-017): a sample that changes nothing shown renders
/// neither the container nor the page. A sample that changes a shown value notifies only the
/// widgets on that source; the container and the page never notify, no widget entity is made or
/// dropped, and no widget is placed again. GPUI marks the ancestors of a notifying view dirty
/// (crw.34, no GPUI fork), so the container and the page may render at most once in that frame,
/// with fixed work only: no placement. GPUI also re-renders every cached child of a view it
/// re-renders (`window.refreshing`, gpui-pre 0.3.7 `view.rs`), so the other widgets may render
/// once too, from text they already hold: they do not notify and their text stays the same.
#[gpui_kit::test]
fn overview_widget_container_sample_renders_no_container(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    let layout = grid_layout(vec![
        (1, a(&engine_gpu()), ROW_FORMS, cell(0, 0, 6, 1)),
        (
            2,
            a("vram"),
            FormSet::VALUE | FormSet::LABEL,
            cell(0, 1, 6, 1),
        ),
        (
            3,
            selector(DeviceSelector::Key(key(2)), &engine_gpu()),
            FormSet::VALUE,
            cell(0, 2, 6, 1),
        ),
    ]);
    let c = r.container(layout, 400., 200.);
    let ids = r.entity_ids(&c);
    assert_eq!(ids.len(), 3);
    let widgets: Vec<Entity<MetricWidget>> = r.vcx.read_entity(&c, |c, _| {
        (1..=3).filter_map(|id| c.widget(id).cloned()).collect()
    });
    let base: Vec<_> = widgets.iter().map(|w| r.counts(w).0).collect();
    let renders0: Vec<_> = widgets.iter().map(|w| r.counts(w).1).collect();
    let (render0, notify0, page0) = r.container_counts(&c);
    let relayouts0 = r.vcx.read_entity(&c, |c, _| c.relayout_count.get());
    let vram_texts = r.read(&widgets[1], |w| w.texts().map(SharedString::clone));
    let gpu_b_texts = r.read(&widgets[2], |w| w.texts().map(SharedString::clone));

    // A sample that changes nothing a widget shows: no notify, so nothing renders.
    r.system(|s| {
        let mut a = gpu_totals(GPU_A, 25.);
        a.power_w = Metric::valid(210.);
        s.gpus = Arc::from([
            (DeviceIdx(GPU_A), a),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
        ]);
    });
    assert_eq!(r.container_counts(&c), (render0, notify0, page0));
    let now: Vec<_> = widgets.iter().map(|w| r.counts(w).0).collect();
    assert_eq!(now, base);
    let now: Vec<_> = widgets.iter().map(|w| r.counts(w).1).collect();
    assert_eq!(now, renders0, "no widget renders");

    // GPU A's utilization changes: widget 1 notifies, the container does not.
    r.system(|s| {
        s.gpus = Arc::from([
            (DeviceIdx(GPU_A), gpu_totals(GPU_A, 70.)),
            (DeviceIdx(GPU_B), gpu_totals(GPU_B, 50.)),
        ]);
    });
    let now: Vec<_> = widgets.iter().map(|w| r.counts(w).0).collect();
    assert_eq!(now[0], base[0] + 1, "the widget on GPU A's utilization");
    assert_eq!(now[1], base[1], "VRAM did not change");
    assert_eq!(now[2], base[2], "GPU B did not change");
    let (render, notify, page) = r.container_counts(&c);
    assert_eq!(notify, notify0, "the container never notifies on a sample");
    // Only GPUI's ancestor marking renders the container and the page, at most once each.
    assert!(
        render - render0 <= 1,
        "container renders {}",
        render - render0
    );
    assert!(page - page0 <= 1, "page renders {}", page - page0);
    assert_eq!(
        r.vcx.read_entity(&c, |c, _| c.relayout_count.get()),
        relayouts0,
        "the container render places nothing"
    );
    let now: Vec<_> = widgets.iter().map(|w| r.counts(w).1).collect();
    assert_eq!(now[0], renders0[0] + 1, "the widget on GPU A renders once");
    assert!(
        now[1] - renders0[1] <= 1,
        "VRAM renders {}",
        now[1] - renders0[1]
    );
    assert!(
        now[2] - renders0[2] <= 1,
        "GPU B renders {}",
        now[2] - renders0[2]
    );
    assert_eq!(
        r.read(&widgets[1], |w| w.texts().map(SharedString::clone)),
        vram_texts
    );
    assert_eq!(
        r.read(&widgets[2], |w| w.texts().map(SharedString::clone)),
        gpu_b_texts
    );
    assert_eq!(r.entity_ids(&c), ids, "no widget entity made or dropped");
    assert_eq!(r.read(&widgets[0], |w| w.texts()[1].to_string()), "70.0%");
}

/// A layout change keeps the entity of every widget whose id stays: a move of one widget (and
/// the widgets it pushes down) keeps them all; a new id gets a new entity; a removed id drops
/// only its own.
#[gpui_kit::test]
fn overview_widget_container_keeps_entities_on_move(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let c = r.container(four_widgets(), 400., 300.);
    let ids = r.entity_ids(&c);
    assert_eq!(ids.len(), 4);
    let before = r.widget_rect(3).expect("drawn");
    let (_, notify0, _) = r.container_counts(&c);

    // Move the VRAM row to the top: the charts move down under it.
    let mut moved = r.vcx.read_entity(&c, |c, _| c.layout().clone());
    let g = r.vcx.read_entity(&c, |c, _| c.placement().grid);
    assert!(grid::set_cell(&mut moved, 3, cell(0, 0, 6, 1), &g));
    assert_eq!(moved.widgets[0].cell, Some(cell(0, 1, 3, 4)));
    r.vcx
        .update(|_, cx| c.update(cx, |c, cx| c.set_layout(moved.clone(), cx)));
    draw(r.vcx);
    let mut after = r.entity_ids(&c);
    after.sort_by_key(|(id, _)| *id);
    let mut want = ids.clone();
    want.sort_by_key(|(id, _)| *id);
    assert_eq!(after, want, "every widget keeps its entity");
    assert_eq!(
        r.container_counts(&c).1,
        notify0 + 1,
        "one notify for the change"
    );
    let now = r.widget_rect(3).expect("drawn");
    assert!(now.1 < before.1, "the row moved up: {now:?} {before:?}");
    // The same layout again changes nothing.
    r.vcx
        .update(|_, cx| c.update(cx, |c, cx| c.set_layout(moved.clone(), cx)));
    assert_eq!(r.container_counts(&c).1, notify0 + 1);

    // Remove widget 2 and add widget 5: only those change.
    let mut edited = moved;
    edited.widgets.retain(|w| w.id != 2);
    grid::insert(
        &mut edited,
        widget(
            0,
            selector(DeviceSelector::Key(key(1)), "temp"),
            FormSet::VALUE | FormSet::LABEL,
        ),
        &g,
    );
    assert_eq!(edited.widgets.last().map(|w| w.id), Some(5));
    r.vcx
        .update(|_, cx| c.update(cx, |c, cx| c.set_layout(edited, cx)));
    draw(r.vcx);
    let now = r.entity_ids(&c);
    for (id, entity) in &want {
        let kept = now.iter().find(|(i, _)| i == id).map(|(_, e)| *e);
        if *id == 2 {
            assert_eq!(kept, None, "widget 2 dropped");
        } else {
            assert_eq!(kept, Some(*entity), "widget {id} kept");
        }
    }
    assert!(now.iter().any(|(id, _)| *id == 5), "widget 5 made");
}

/// A flow layout leaves out a widget it cannot show, which then has no entity; a grid layout
/// keeps its cells and shows it as unavailable.
#[gpui_kit::test]
fn overview_widget_container_flow_leaves_out_unavailable(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    let mut layout = grid_layout(vec![
        (1, a(&engine_gpu()), ROW_FORMS, cell(0, 0, 6, 1)),
        (2, a("bogus"), FormSet::VALUE, cell(0, 1, 6, 1)),
    ]);
    let c = r.container(layout.clone(), 400., 200.);
    let placed = r.vcx.read_entity(&c, |c, _| c.placed_ids());
    assert_eq!(placed.as_slice(), [1, 2]);
    let w2 = r
        .vcx
        .read_entity(&c, |c, _| c.widget(2).cloned())
        .expect("w2");
    assert_eq!(r.read(&w2, |w| w.availability()), Availability::Unknown);

    layout.preset = Some(PresetSetting {
        id: "chart".to_owned(),
        arrange: None,
        split: None,
    });
    for w in &mut layout.widgets {
        w.cell = None;
    }
    r.vcx
        .update(|_, cx| c.update(cx, |c, cx| c.set_layout(layout, cx)));
    draw(r.vcx);
    let (placed, has2, demand) = r.vcx.read_entity(&c, |c, _| {
        (c.placed_ids(), c.widget(2).is_some(), c.demand_part().gpus)
    });
    assert_eq!(placed.as_slice(), [1]);
    assert!(!has2, "a widget that takes no space has no entity");
    assert!(demand);
    assert!(r.widget_rect(2).is_none());
}

// Edit Widgets (UI-PRF-061, ADR-019 "Editing").

fn cell_of(layout: &ContainerLayout, id: u32) -> Option<WidgetCell> {
    layout.widgets.iter().find(|w| w.id == id)?.cell
}

fn sorted(mut ids: Vec<(u32, gpui_kit::EntityId)>) -> Vec<(u32, gpui_kit::EntityId)> {
    ids.sort_by_key(|(id, _)| *id);
    ids
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn source(n: u8, metric: &str) -> WidgetSource {
    WidgetSource {
        device: key(n),
        metric: Arc::from(metric),
    }
}

/// The entry labelled `label`.
fn pick(entries: Vec<MenuEntryInfo>, label: &str) -> MenuEntryInfo {
    entries
        .into_iter()
        .find(|e| e.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label} entry"))
}

/// Whether `shown` is the built-in default pinned to the grid: no preset, every widget on
/// cells, and only widgets of the default.
fn is_pinned_default(shown: &ContainerLayout) -> bool {
    let default = default_layout(GPU_BOX_CONTAINER);
    shown.preset.is_none()
        && !shown.widgets.is_empty()
        && shown
            .widgets
            .iter()
            .all(|w| w.cell.is_some() && default.widgets.iter().any(|d| d.id == w.id))
}

fn labels(entries: &[MenuEntryInfo]) -> Vec<(String, MenuEntryKind, bool)> {
    entries
        .iter()
        .map(|e| {
            (
                e.label.as_deref().unwrap_or("").to_owned(),
                e.kind,
                e.checked,
            )
        })
        .collect()
}

impl Rig<'_> {
    /// Puts an editor around a container showing `layout` in a box `w × h` px, with GPU A in
    /// slot 0 and GPU B in slot 1, and draws.
    fn editor(
        &mut self,
        layout: ContainerLayout,
        w: f32,
        h: f32,
    ) -> (Entity<WidgetEditor>, Entity<WidgetContainer>) {
        let hub = self.hub.clone();
        let host = self.host.clone();
        let out = self.vcx.update(|_, cx| {
            let store = Rc::new(RefCell::new(SourceStore::new(&hub)));
            store.borrow_mut().set_slots(&[key(1), key(2)], cx);
            let c = cx.new(|cx| WidgetContainer::new(GPU_BOX_CONTAINER, layout, &hub, store, cx));
            let e = cx.new(|cx| WidgetEditor::new(c.clone(), &hub, cx));
            e.update(cx, |e, _| {
                e.set_slots(vec![
                    SlotDevice {
                        key: key(1),
                        name: Arc::from("Card A"),
                    },
                    SlotDevice {
                        key: key(2),
                        name: Arc::from("Card B"),
                    },
                ])
            });
            host.update(cx, |host, cx| {
                host.editor = Some((e.clone(), w, h));
                cx.notify();
            });
            (e, c)
        });
        draw(self.vcx);
        out
    }

    /// Runs `f` on the editor and draws.
    fn ed<R>(
        &mut self,
        e: &Entity<WidgetEditor>,
        f: impl FnOnce(&mut WidgetEditor, &mut Window, &mut Context<WidgetEditor>) -> R,
    ) -> R {
        let out = self
            .vcx
            .update(|window, cx| e.update(cx, |e, cx| f(e, window, cx)));
        draw(self.vcx);
        out
    }

    fn start(&mut self, e: &Entity<WidgetEditor>) {
        self.ed(e, |e, window, cx| e.start(window, cx));
        assert!(self.vcx.read_entity(e, |e, _| e.is_editing()));
    }

    fn working(&mut self, e: &Entity<WidgetEditor>) -> Option<ContainerLayout> {
        self.vcx
            .read_entity(e, |e, _| e.edit().and_then(|x| x.working.clone()))
    }

    fn selected(&mut self, e: &Entity<WidgetEditor>) -> Option<u32> {
        self.vcx
            .read_entity(e, |e, _| e.edit().and_then(|x| x.selected))
    }

    fn shown(&mut self, c: &Entity<WidgetContainer>) -> ContainerLayout {
        self.vcx.read_entity(c, |c, _| c.layout().clone())
    }

    fn saved_gpu(&mut self) -> Option<ContainerLayout> {
        self.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(GPU_BOX_CONTAINER)
                .cloned()
        })
    }

    fn window_list(&mut self) -> Vec<gpui_kit::AnyWindowHandle> {
        self.vcx.update(|_, cx| cx.windows())
    }

    /// The window that opened after `before` (a confirm dialog), drawn and active.
    fn new_window(&mut self, before: &[gpui_kit::AnyWindowHandle]) -> VisualTestContext {
        let any = self
            .window_list()
            .into_iter()
            .find(|w| !before.contains(w))
            .expect("a new window");
        let mut d = VisualTestContext::from_window(any, &self.vcx.cx);
        d.update(|window, _| window.activate_window());
        d.run_until_parked();
        d.update(|window, cx| window.draw(cx).clear(cx));
        d
    }

    /// Clicks the element with debug selector `id`.
    fn click(&mut self, id: &str) {
        let at = self
            .vcx
            .debug_bounds(leak(id.to_owned()))
            .unwrap_or_else(|| panic!("{id}"))
            .center();
        self.vcx.simulate_click(at, Modifiers::none());
        draw(self.vcx);
    }

    /// Runs the action of the menu entry `entry` on the editor, as choosing it does.
    fn choose(&mut self, e: &Entity<WidgetEditor>, entry: MenuEntryInfo) {
        let label = entry.label.clone();
        let action = entry
            .action
            .unwrap_or_else(|| panic!("{label:?} has an action"));
        self.vcx.update(|window, cx| {
            let focus = e.read(cx).focus_handle().clone();
            focus.dispatch_action(&*action, window, cx);
        });
        self.vcx.update(tl_ui::components::menu::close_context_menu);
        draw(self.vcx);
    }

    /// The working widget `id`.
    fn working_widget(&mut self, e: &Entity<WidgetEditor>, id: u32) -> Option<WidgetSetting> {
        self.working(e)?.widgets.into_iter().find(|w| w.id == id)
    }

    /// The entries of the open context menu.
    fn menu_entries(&mut self) -> Vec<MenuEntryInfo> {
        let menu = self
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a menu is open");
        self.vcx
            .update(|window, cx| menu.read(cx).entries_for_test(window))
    }

    fn submenu_entries(&mut self, entry: &MenuEntryInfo) -> Vec<MenuEntryInfo> {
        let sub = entry.submenu.clone().expect("a submenu");
        self.vcx
            .update(|window, cx| sub.read(cx).entries_for_test(window))
    }

    fn press(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>, click_count: usize) {
        self.vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count,
            first_mouse: false,
        });
        draw(self.vcx);
    }

    fn move_to(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>) {
        self.vcx
            .simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::none());
        draw(self.vcx);
    }

    fn release(&mut self, at: gpui_kit::Point<gpui_kit::Pixels>) {
        self.vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        draw(self.vcx);
    }
}

/// Apply saves the working layout; Apply with no change saves nothing.
#[gpui_kit::test]
fn overview_widget_edit_apply_saves(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    assert!(r.vcx.debug_bounds(TOOLBAR_ID).is_none());
    r.start(&e);
    assert!(r.vcx.read_entity(&c, |c, _| c.is_editing()));
    assert!(r.vcx.debug_bounds(TOOLBAR_ID).is_some(), "the edit toolbar");
    assert!(
        r.vcx.debug_bounds(leak(frame_id(1))).is_some(),
        "the chrome"
    );
    assert_eq!(r.working(&e), Some(four_widgets()));

    // Nothing changed: nothing is saved.
    assert!(!r.ed(&e, |e, _, cx| e.apply(cx)));
    assert_eq!(r.saved_gpu(), None);
    assert!(!r.vcx.read_entity(&c, |c, _| c.is_editing()));
    assert!(r.vcx.debug_bounds(TOOLBAR_ID).is_none());

    // The VRAM row moves to the top; Apply saves the layout and the box shows it.
    r.start(&e);
    let moved = r.ed(&e, |e, _, cx| {
        e.select(3, cx);
        e.move_selected(0, -4, cx)
    });
    assert!(moved);
    let working = r.working(&e).expect("working layout");
    assert_eq!(cell_of(&working, 3), Some(cell(0, 0, 6, 1)));
    assert_eq!(r.saved_gpu(), None, "nothing is saved before Apply");
    assert!(r.ed(&e, |e, _, cx| e.apply(cx)));
    assert_eq!(r.saved_gpu(), Some(working.clone()));
    assert_eq!(r.shown(&c), working);
}

/// Cancel puts the flow layout back unsaved, and every widget keeps its entity.
#[gpui_kit::test]
fn overview_widget_edit_cancel_restores(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let slot = |m: &str| selector(DeviceSelector::Slot(0), m);
    let flow = ContainerLayout {
        container: GPU_BOX_CONTAINER.to_owned(),
        version: LAYOUT_VERSION,
        preset: Some(PresetSetting {
            id: "chart".to_owned(),
            arrange: None,
            split: None,
        }),
        widgets: vec![
            widget(1, slot(&engine_gpu()), CHART_FORMS),
            widget(2, slot("vram"), ROW_FORMS),
        ],
        next_id: 3,
    };
    let (e, c) = r.editor(flow.clone(), 400., 320.);
    let before = sorted(r.entity_ids(&c));
    assert_eq!(before.len(), 2);

    // Entering pins the flow layout: cells, device keys and names.
    r.start(&e);
    let pinned = r.working(&e).expect("pinned");
    assert!(pinned.preset.is_none());
    for w in &pinned.widgets {
        assert!(w.cell.is_some(), "{w:?}");
        assert_eq!(w.source.device, DeviceSelector::Key(key(1)));
        assert_eq!(w.device_name.as_deref(), Some("Card A"));
    }
    assert_eq!(
        sorted(r.entity_ids(&c)),
        before,
        "pinning keeps the entities"
    );

    // Move, remove and add, then Cancel.
    r.ed(&e, |e, _, cx| {
        e.select(2, cx);
        assert!(e.move_selected(0, 1, cx));
        e.select(1, cx);
        assert!(e.remove_selected(cx));
    });
    let added = r.ed(&e, |e, _, cx| e.add(&source(2, "vram"), cx));
    assert_eq!(added, Some(3));
    assert_eq!(r.selected(&e), Some(3));
    r.ed(&e, |e, _, cx| e.cancel(cx));
    assert_eq!(r.shown(&c), flow);
    assert!(!r.vcx.read_entity(&c, |c, _| c.is_editing()));
    assert_eq!(r.saved_gpu(), None);
    assert_eq!(
        sorted(r.entity_ids(&c)),
        before,
        "Cancel creates no widget entity again"
    );
}

/// Undo steps back one action at a time, also over Reset.
#[gpui_kit::test]
fn overview_widget_edit_undo(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);
    assert!(!r.ed(&e, |e, _, cx| e.undo(cx)), "nothing to undo");
    let start = r.working(&e);
    r.ed(&e, |e, _, cx| {
        e.select(3, cx);
        assert!(e.move_selected(0, -4, cx));
    });
    let one = r.working(&e);
    r.ed(&e, |e, _, cx| assert!(e.remove_selected(cx)));
    assert!(r.working(&e).is_some_and(|l| l.widgets.len() == 3));
    assert!(r.ed(&e, |e, _, cx| e.reset_confirmed(cx)));
    assert_eq!(r.working(&e), None);
    let shown = r.shown(&c);
    assert!(is_pinned_default(&shown), "{shown:?}");

    assert!(r.ed(&e, |e, _, cx| e.undo(cx)));
    assert!(r.working(&e).is_some_and(|l| l.widgets.len() == 3));
    assert!(r.ed(&e, |e, _, cx| e.undo(cx)));
    assert_eq!(r.working(&e), one);
    assert!(r.ed(&e, |e, _, cx| e.undo(cx)));
    assert_eq!(r.working(&e), start);
    assert_eq!(r.shown(&c), four_widgets());
    let (changed, can_undo) = r.vcx.read_entity(&e, |e, _| {
        let edit = e.edit().expect("editing");
        (edit.changed(), edit.can_undo())
    });
    assert!(!changed && !can_undo);
}

/// The keys of the `PerfWidgetEdit` context.
#[gpui_kit::test]
fn overview_widget_edit_keyboard(cx: &mut TestAppContext) {
    let mut r = open(cx);
    r.vcx.update(|_, cx| bind_keys_performance(cx));
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);

    // Reading order: 1 and 2 on row 0, then 3, then 4.
    r.vcx.simulate_keystrokes("tab");
    draw(r.vcx);
    assert_eq!(r.selected(&e), Some(1));
    r.vcx.simulate_keystrokes("tab tab");
    draw(r.vcx);
    assert_eq!(r.selected(&e), Some(3));
    r.vcx.simulate_keystrokes("shift-tab");
    draw(r.vcx);
    assert_eq!(r.selected(&e), Some(2));

    // Alt+Left moves widget 2 one column; Alt+Shift+Down makes it one row taller.
    r.vcx.simulate_keystrokes("alt-left");
    draw(r.vcx);
    let working = r.working(&e).expect("working");
    assert_eq!(cell_of(&working, 2).map(|c| c.col), Some(2));
    r.vcx.simulate_keystrokes("alt-shift-down");
    draw(r.vcx);
    let working = r.working(&e).expect("working");
    assert_eq!(cell_of(&working, 2).map(|c| c.rows), Some(5));

    // Delete removes it, Ctrl+Z brings it back.
    r.vcx.simulate_keystrokes("delete");
    draw(r.vcx);
    assert_eq!(cell_of(&r.working(&e).expect("working"), 2), None);
    r.vcx.simulate_keystrokes("ctrl-z");
    draw(r.vcx);
    assert_eq!(r.working(&e), Some(working));

    // Esc cancels.
    r.vcx.simulate_keystrokes("escape");
    draw(r.vcx);
    assert!(!r.vcx.read_entity(&e, |e, _| e.is_editing()));
    assert_eq!(r.shown(&c), four_widgets());
    assert_eq!(r.saved_gpu(), None);

    // Enter applies.
    r.start(&e);
    r.vcx.simulate_keystrokes("tab alt-down enter");
    draw(r.vcx);
    assert!(!r.vcx.read_entity(&e, |e, _| e.is_editing()));
    let saved = r.saved_gpu().expect("saved");
    assert_eq!(cell_of(&saved, 1).map(|c| c.row), Some(1));
}

/// Reset asks first: the dialog's Cancel changes nothing and its Reset puts the default in the
/// working layout. The default shows pinned to the grid, so its widgets can be selected with
/// Tab, opened in the widget menu, dragged and resized at once.
#[gpui_kit::test]
fn overview_widget_reset_asks(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);

    // The dialog closes without Reset: nothing changes.
    let before = r.window_list();
    r.click(&format!("TlButton-{RESET_ID}"));
    assert_eq!(r.window_list().len(), before.len() + 1, "a confirm dialog");
    let mut d = r.new_window(&before);
    d.update(|window, _| window.remove_window());
    d.run_until_parked();
    draw(r.vcx);
    assert_eq!(r.window_list().len(), before.len(), "the dialog closed");
    assert_eq!(r.working(&e), Some(four_widgets()), "nothing changed");

    // Enter on the dialog's Reset button (it has focus): the default.
    r.click(&format!("TlButton-{RESET_ID}"));
    let mut d = r.new_window(&before);
    d.simulate_keystrokes("enter");
    d.run_until_parked();
    draw(r.vcx);
    assert_eq!(r.window_list().len(), before.len(), "the dialog closed");
    assert_eq!(r.working(&e), None);
    let shown = r.shown(&c);
    assert!(is_pinned_default(&shown), "{shown:?}");
    assert!(
        r.vcx
            .read_entity(&e, |e, _| e.edit().is_some_and(|x| x.changed())),
        "Apply is on"
    );

    // Tab selects the first default widget in reading order.
    let first = r
        .ed(&e, |e, _, cx| e.select_step(true, cx))
        .expect("Tab selects a default widget");
    let order = r.vcx.read_entity(&e, |e, cx| {
        e.working_layout(cx)
            .map(|l| tl_ui::pages::performance::widget::edit::reading_order(&l).to_vec())
            .unwrap_or_default()
    });
    assert_eq!(order.first(), Some(&first));

    // Its gear opens the widget menu.
    r.click(&gear_id(first));
    let top: Vec<String> = labels(&r.menu_entries())
        .into_iter()
        .map(|(l, _, _)| l)
        .collect();
    assert_eq!(top.first().map(String::as_str), Some("Source"), "{top:?}");
    r.vcx.update(tl_ui::components::menu::close_context_menu);
    draw(r.vcx);

    // Its grip drags it down two rows.
    let pitch = r.vcx.read_entity(&c, |c, _| {
        c.placement().grid.row + c.placement().grid.gutter
    });
    let start = cell_of(&shown, first).expect("a cell");
    let grip = r.vcx.debug_bounds(leak(grip_id(first))).expect("grip");
    let from = grip.center();
    let to = point(from.x, from.y + px(pitch * 2.));
    r.press(from, 1);
    r.move_to(to);
    r.release(to);
    let dropped = r.working_widget(&e, first).and_then(|w| w.cell);
    assert_eq!(dropped.map(|c| c.row), Some(start.row + 2), "{dropped:?}");

    // Undo goes back to the default, which Alt+Shift+Down then makes taller.
    assert!(r.ed(&e, |e, _, cx| e.undo(cx)));
    assert_eq!(r.working(&e), None);
    r.ed(&e, |e, _, cx| e.select(first, cx));
    assert!(r.ed(&e, |e, _, cx| e.resize_selected(0, 1, cx)));
    let grown = r.working_widget(&e, first).and_then(|w| w.cell);
    assert_eq!(grown.map(|c| c.rows), Some(start.rows + 1), "{grown:?}");
}

/// A converted GPU box reset and applied leaves no entry and no old keys, so nothing converts
/// at the next load and the box shows the built-in default.
#[gpui_kit::test]
fn overview_widget_reset_survives_reload(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let converted = r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            let mut layout = OverviewLayoutSetting::grid_default();
            let gpu = layout
                .grid
                .iter_mut()
                .find(|b| b.kind == "gpu")
                .expect("a GPU box");
            gpu.mode = Some("chart_engines".to_owned());
            gpu.split = Some(0.6);
            gpu.hidden = vec!["Video Decode".to_owned()];
            d.settings.performance.overview = Some(layout);
            assert!(convert_old_settings(&mut d.settings.performance));
        });
        gpu_box_layout(&TlSettings::settings(cx).performance)
    });
    assert!(converted.preset.is_some());
    let (e, c) = r.editor(converted, 400., 320.);
    r.start(&e);
    assert!(r.ed(&e, |e, _, cx| e.reset_confirmed(cx)));
    assert!(r.ed(&e, |e, _, cx| e.apply(cx)));
    assert_eq!(r.shown(&c), default_layout(GPU_BOX_CONTAINER));

    let perf = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.clone());
    assert!(perf.layouts.entry(GPU_BOX_CONTAINER).is_none());
    let gpu = perf
        .overview
        .as_ref()
        .and_then(|o| o.grid.iter().find(|b| b.kind == "gpu"))
        .expect("the GPU box stays on the page");
    assert_eq!(gpu.mode, None);
    assert_eq!(gpu.split, None);
    assert_eq!(gpu.arrange, None);
    assert!(gpu.hidden.is_empty());
    let mut reloaded = perf.clone();
    assert!(!convert_old_settings(&mut reloaded), "nothing converts");
    assert_eq!(gpu_box_layout(&reloaded), default_layout(GPU_BOX_CONTAINER));
}

/// The widget menu lists Source, Show and Chart, then Remove Widget; Show lists only the forms
/// the metric offers; sources list only what the GPU reports.
#[gpui_kit::test]
fn overview_widget_menu_offers_metric_forms(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    let layout = grid_layout(vec![
        (1, a("vram"), FormSet::all(), cell(0, 0, 6, 4)),
        (2, a("power"), FormSet::VALUE, cell(0, 4, 6, 1)),
    ]);
    let (e, _) = r.editor(layout, 400., 320.);
    r.start(&e);

    let gear = r.vcx.debug_bounds(leak(gear_id(1))).expect("gear");
    r.vcx.simulate_click(gear.center(), Modifiers::none());
    draw(r.vcx);
    assert_eq!(r.selected(&e), Some(1));
    let entries = r.menu_entries();
    let top: Vec<(String, MenuEntryKind)> = labels(&entries)
        .into_iter()
        .map(|(l, k, _)| (l, k))
        .collect();
    assert_eq!(
        top,
        [
            ("Source".to_owned(), MenuEntryKind::Submenu),
            ("Show".to_owned(), MenuEntryKind::Submenu),
            ("Chart".to_owned(), MenuEntryKind::Submenu),
            (String::new(), MenuEntryKind::Separator),
            ("Remove Widget".to_owned(), MenuEntryKind::Item),
        ]
    );
    let show = r.submenu_entries(&entries[1]);
    let show: Vec<(String, bool)> = labels(&show).into_iter().map(|(l, _, c)| (l, c)).collect();
    assert_eq!(
        show,
        [
            ("Chart".to_owned(), true),
            ("Load bar".to_owned(), true),
            ("Value".to_owned(), true),
            ("Min, average and peak".to_owned(), true),
            ("Name".to_owned(), true),
        ]
    );
    let chart = r.submenu_entries(&entries[2]);
    let chart: Vec<(String, bool)> = labels(&chart).into_iter().map(|(l, _, c)| (l, c)).collect();
    assert_eq!(
        chart,
        [
            ("Fill under the line".to_owned(), true),
            ("Dashed line".to_owned(), false)
        ]
    );
    // Two GPUs: one submenu each. GPU A reports VRAM and power, not the temperature.
    let source = r.submenu_entries(&entries[0]);
    let gpus: Vec<String> = labels(&source).into_iter().map(|(l, _, _)| l).collect();
    assert_eq!(gpus, ["Card A", "Card B"]);
    let card_a = r.submenu_entries(&source[0]);
    let items = labels(&card_a);
    assert!(items.iter().any(|(l, _, c)| l == "VRAM" && *c), "{items:?}");
    assert!(
        items.iter().any(|(l, _, c)| l == "Power draw" && !*c),
        "{items:?}"
    );
    assert!(
        !items.iter().any(|(l, _, _)| l == "Temperature"),
        "{items:?}"
    );

    // Power without a power limit has no bar to offer.
    r.vcx.update(tl_ui::components::menu::close_context_menu);
    draw(r.vcx);
    let gear = r.vcx.debug_bounds(leak(gear_id(2))).expect("gear");
    r.vcx.simulate_click(gear.center(), Modifiers::none());
    draw(r.vcx);
    let entries = r.menu_entries();
    let top: Vec<String> = labels(&entries).into_iter().map(|(l, _, _)| l).collect();
    assert_eq!(
        top,
        ["Source", "Show", "", "Remove Widget"],
        "no Chart without a chart"
    );
    let show = r.submenu_entries(&entries[1]);
    let show: Vec<String> = labels(&show).into_iter().map(|(l, _, _)| l).collect();
    assert_eq!(show, ["Chart", "Value", "Min, average and peak", "Name"]);
}

/// The last checked form is disabled, so the form set is never empty.
#[gpui_kit::test]
fn overview_widget_last_form_disabled(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    let layout = grid_layout(vec![(2, a("power"), FormSet::VALUE, cell(0, 0, 6, 1))]);
    let (e, _) = r.editor(layout, 400., 320.);
    r.start(&e);
    let gear = r.vcx.debug_bounds(leak(gear_id(2))).expect("gear");
    r.vcx.simulate_click(gear.center(), Modifiers::none());
    draw(r.vcx);
    let entries = r.menu_entries();
    let show = r.submenu_entries(&entries[1]);
    let value = labels(&show)
        .into_iter()
        .find(|(l, _, _)| l == "Value")
        .expect("Value");
    assert_eq!(value.1, MenuEntryKind::Disabled);
    assert!(value.2, "checked");
    assert!(!r.ed(&e, |e, _, cx| e.set_form(2, "value", false, cx)));

    // With a second form on, Value can go.
    r.vcx.update(|window, cx| {
        let focus = e.read(cx).focus_handle().clone();
        focus.dispatch_action(
            &PerfWidgetSetForm {
                id: 2,
                form: Arc::from("label"),
                on: true,
            },
            window,
            cx,
        );
    });
    draw(r.vcx);
    assert!(r.ed(&e, |e, _, cx| e.set_form(2, "value", false, cx)));
    let forms = r
        .working(&e)
        .and_then(|l| l.widgets.first().map(|w| w.forms));
    assert_eq!(forms, Some(FormSet::LABEL));
}

/// A drag by the grip shows the preview of the target cells and drops the widget on them; the
/// corner resizes; Add Widget places and selects a new widget.
#[gpui_kit::test]
fn overview_widget_grid_insert_move_resize(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);
    let pitch = r.vcx.read_entity(&c, |c, _| {
        let g = c.placement().grid;
        (g.cw + g.gutter, g.row + g.gutter)
    });

    // Widget 4 (GPU B's value row, the full width) moves to the top by its grip.
    let grip = r.vcx.debug_bounds(leak(grip_id(4))).expect("grip");
    let from = grip.center();
    r.press(from, 1);
    let to = point(from.x + px(pitch.0 * 3.), from.y - px(pitch.1 * 5.));
    r.move_to(to);
    let target = r.vcx.read_entity(&e, |e, _| e.drag_cell());
    assert_eq!(target.map(|t| (t.col, t.row)), Some((0, 0)), "{target:?}");
    assert!(r.vcx.debug_bounds(PREVIEW_ID).is_some(), "the preview");
    assert_eq!(
        r.working(&e),
        Some(four_widgets()),
        "nothing changes before the drop"
    );
    r.release(to);
    assert!(r.vcx.debug_bounds(PREVIEW_ID).is_none());
    let working = r.working(&e).expect("working");
    assert_eq!(cell_of(&working, 4).map(|c| c.row), Some(0));
    let cells: Vec<WidgetCell> = working.widgets.iter().filter_map(|w| w.cell).collect();
    for (i, a) in cells.iter().enumerate() {
        for b in &cells[i + 1..] {
            assert!(!a.overlaps(b), "{a:?} {b:?}");
        }
    }

    // The corner of widget 1 makes it one column wider.
    let corner = r
        .vcx
        .debug_bounds(leak(tl_ui::pages::performance::widget::edit::corner_id(1)))
        .expect("corner");
    let before = cell_of(&working, 1).expect("cell");
    let from = corner.center();
    r.press(from, 1);
    let to = point(from.x + px(pitch.0), from.y);
    r.move_to(to);
    r.release(to);
    let after = cell_of(&r.working(&e).expect("working"), 1).expect("cell");
    assert_eq!(after.cols, before.cols + 1, "{before:?} {after:?}");

    // Add Widget: a VRAM widget of GPU B, selected, with Chart, Value and Name.
    let id = r
        .ed(&e, |e, _, cx| e.add(&source(2, "vram"), cx))
        .expect("added");
    assert_eq!(id, 5);
    assert_eq!(r.selected(&e), Some(5));
    let working = r.working(&e).expect("working");
    let w = working
        .widgets
        .iter()
        .find(|w| w.id == 5)
        .expect("widget 5");
    assert_eq!(w.forms, FormSet::CHART | FormSet::VALUE | FormSet::LABEL);
    assert_eq!(w.source.device, DeviceSelector::Key(key(2)));
    assert_eq!(w.device_name.as_deref(), Some("Card B"));
    assert!(w.cell.is_some());
}

/// The widget menu changes the source, the forms and the chart options of the working widget,
/// and removes it.
#[gpui_kit::test]
fn overview_widget_menu_changes_source_and_options(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    let layout = grid_layout(vec![(1, a("vram"), FormSet::all(), cell(0, 0, 6, 4))]);
    let (e, _) = r.editor(layout, 400., 320.);
    r.start(&e);

    // Source > Card A > Power draw: power keeps the forms it offers; it has no bar.
    r.click(&gear_id(1));
    let entries = r.menu_entries();
    let source = r.submenu_entries(&entries[0]);
    let card_a = r.submenu_entries(&pick(source, "Card A"));
    r.choose(&e, pick(card_a, "Power draw"));
    let w = r.working_widget(&e, 1).expect("widget 1");
    assert_eq!(w.source, a("power"));
    assert_eq!(w.device_name.as_deref(), Some("Card A"));
    assert_eq!(w.forms, FormSet::all() - FormSet::BAR);

    // Chart > Dashed line on, then Fill under the line off.
    r.click(&gear_id(1));
    let chart = {
        let entries = r.menu_entries();
        r.submenu_entries(&pick(entries, "Chart"))
    };
    r.choose(&e, pick(chart, "Dashed line"));
    let w = r.working_widget(&e, 1).expect("widget 1");
    assert!(w.options.dashed && w.options.fill, "{:?}", w.options);
    r.click(&gear_id(1));
    let chart = {
        let entries = r.menu_entries();
        r.submenu_entries(&pick(entries, "Chart"))
    };
    r.choose(&e, pick(chart, "Fill under the line"));
    let w = r.working_widget(&e, 1).expect("widget 1");
    assert!(w.options.dashed && !w.options.fill, "{:?}", w.options);

    // Show > Name off.
    r.click(&gear_id(1));
    let show = {
        let entries = r.menu_entries();
        r.submenu_entries(&pick(entries, "Show"))
    };
    r.choose(&e, pick(show, "Name"));
    let w = r.working_widget(&e, 1).expect("widget 1");
    assert!(!w.forms.contains(FormSet::LABEL), "{:?}", w.forms);

    // Remove Widget.
    r.click(&gear_id(1));
    let entries = r.menu_entries();
    r.choose(&e, pick(entries, "Remove Widget"));
    assert_eq!(r.working_widget(&e, 1), None);
    assert_eq!(r.selected(&e), None);
}

/// A double-click on the resize corner sets the widget to its minimum span.
#[gpui_kit::test]
fn overview_widget_corner_double_click_minimum(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);
    let min = r
        .vcx
        .read_entity(&c, |c, _| c.placement().grid.forms_min_span(CHART_FORMS));
    assert!(
        min.0 < 3 || min.1 < 4,
        "widget 1 is above its minimum: {min:?}"
    );
    let corner = r.vcx.debug_bounds(leak(corner_id(1))).expect("corner");
    r.press(corner.center(), 1);
    r.release(corner.center());
    assert_eq!(
        r.working(&e),
        Some(four_widgets()),
        "a click changes nothing"
    );
    r.press(corner.center(), 2);
    r.release(corner.center());
    let after = r.working_widget(&e, 1).and_then(|w| w.cell);
    assert_eq!(after, Some(cell(0, 0, min.0, min.1)));
    assert_eq!(r.selected(&e), Some(1));
}

/// Add Widget lists the sources by GPU; choosing one adds and selects the widget.
#[gpui_kit::test]
fn overview_widget_add_menu_adds(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, _) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);
    r.click(&format!("TlButton-{ADD_ID}"));
    let entries = r.menu_entries();
    let groups: Vec<(String, MenuEntryKind, bool)> = labels(&entries);
    assert_eq!(
        groups,
        [
            ("Card A".to_owned(), MenuEntryKind::Submenu, false),
            ("Card B".to_owned(), MenuEntryKind::Submenu, false),
        ]
    );
    let card_b = r.submenu_entries(&entries[1]);
    let vram = pick(card_b, "VRAM");
    assert_eq!(vram.kind, MenuEntryKind::Item, "a plain item");
    assert!(!vram.checked);
    r.choose(&e, vram);
    let w = r.working_widget(&e, 5).expect("widget 5");
    assert_eq!(w.source, selector(DeviceSelector::Key(key(2)), "vram"));
    assert_eq!(w.forms, FormSet::CHART | FormSet::VALUE | FormSet::LABEL);
    assert!(w.cell.is_some());
    assert_eq!(r.selected(&e), Some(5));
}

/// Add Widget in a full box puts the widget under the lowest one; the body scrolls so the
/// widget lies inside the visible body (ADR-019 "Insert").
#[gpui_kit::test]
fn overview_widget_add_scrolls_new_widget_into_view(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 250.);
    r.start(&e);
    let id = r
        .ed(&e, |e, _, cx| e.add(&source(2, "vram"), cx))
        .expect("added");
    draw(r.vcx);
    let body_after = r
        .vcx
        .debug_bounds(leak(format!("{}-scroll", editor_id(GPU_BOX_CONTAINER))))
        .expect("scrolling body");
    let below = r.vcx.read_entity(&c, |c, _| {
        c.placement().bounds(id).map(|b| f32::from(b.bottom()))
    });
    assert!(
        below.is_some_and(|b| b > f32::from(body_after.size.height)),
        "the widget starts below the visible body, so the body had to scroll: {below:?}"
    );
    let w = r
        .vcx
        .debug_bounds(leak(widget_id(id)))
        .expect("the new widget is drawn");
    assert!(
        w.top() >= body_after.top() - px(0.5) && w.bottom() <= body_after.bottom() + px(0.5),
        "widget {w:?} lies inside the body {body_after:?}"
    );
}

/// A widget taller than the body shows its top.
#[gpui_kit::test]
fn overview_widget_add_taller_than_body_shows_top(cx: &mut TestAppContext) {
    let mut r = open(cx);
    // The one-line toolbar leaves 108 px of body, under the new widget's 113 px.
    let (e, _) = r.editor(four_widgets(), 176., 140.);
    r.start(&e);
    let id = r
        .ed(&e, |e, _, cx| e.add(&source(2, "vram"), cx))
        .expect("added");
    draw(r.vcx);
    let body = r
        .vcx
        .debug_bounds(leak(format!("{}-scroll", editor_id(GPU_BOX_CONTAINER))))
        .expect("scrolling body");
    let w = r.vcx.debug_bounds(leak(widget_id(id))).expect("drawn");
    assert!((w.top() - body.top()).abs() <= px(0.5), "{w:?} {body:?}");
}

/// The edit toolbar stays one row at the GPU box minimum width (200 px) at every scale, with
/// Cancel and Apply inside the strip (tasklens-lhliq.10).
#[gpui_kit::test]
fn overview_widget_edit_toolbar_one_row_at_min_width(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, _) = r.editor(four_widgets(), 200., 220.);
    r.start(&e);
    for pct in [100u16, 150, 200] {
        let s = f32::from(pct) / 100.;
        r.set_ui_scale(pct);
        r.ed(&e, |_, _, cx| cx.notify());
        let host = r.host.clone();
        r.vcx.update(|_, cx| {
            host.update(cx, |h, cx| {
                if let Some(x) = h.editor.as_mut() {
                    x.1 = 200. * s;
                    x.2 = 220. * s;
                }
                cx.notify();
            })
        });
        draw(r.vcx);
        let bar = r.vcx.debug_bounds(TOOLBAR_ID).expect("toolbar");
        assert!(
            (f32::from(bar.size.height) - 32. * s).abs() < 0.5,
            "{pct}%: one 32 px row, got {:?}",
            bar.size
        );
        let mid = bar.center().y;
        for id in [ADD_ID, UNDO_ID, RESET_ID, CANCEL_ID, APPLY_ID] {
            let b = r
                .vcx
                .debug_bounds(leak(format!("TlButton-{id}")))
                .unwrap_or_else(|| panic!("{pct}%: {id} is drawn"));
            assert!(
                b.left() >= bar.left() && b.right() <= bar.right(),
                "{pct}%: {id} {b:?} inside {bar:?}"
            );
            assert!(
                b.top() >= bar.top() && b.bottom() <= bar.bottom(),
                "{pct}%: {id} {b:?} on the row of {bar:?}"
            );
            assert!(
                (f32::from(b.center().y - mid)).abs() < 1.5,
                "{pct}%: {id} shares the row"
            );
        }
    }
}

/// The gear covers no chart label: not the scale label in the plot's top-right corner, and it
/// sits clear of the grip (tasklens-lhliq.10).
#[gpui_kit::test]
fn overview_widget_edit_gear_clear_of_chart_labels(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(four_widgets(), 400., 320.);
    r.start(&e);
    for id in [1u32, 2] {
        let gear = r.vcx.debug_bounds(leak(gear_id(id))).expect("gear");
        let grip = r.vcx.debug_bounds(leak(grip_id(id))).expect("grip");
        assert!(!gear.intersects(&grip), "{gear:?} vs {grip:?}");
        let chart = r
            .vcx
            .read_entity(&c, |c, cx| {
                c.widget(id).and_then(|w| w.read(cx).chart().cloned())
            })
            .expect("chart");
        let labels = r.vcx.read_entity(&chart, |v, _| {
            [LabelKind::AxisMax, LabelKind::Heading, LabelKind::Value]
                .into_iter()
                .filter_map(|k| v.label_cache().painted(k))
                .map(|p| (p.kind, p.bounds()))
                .collect::<Vec<_>>()
        });
        assert!(
            labels.iter().any(|(k, _)| *k == LabelKind::AxisMax),
            "widget {id} paints its scale label"
        );
        for (k, b) in labels {
            assert!(
                !gear.intersects(&b),
                "widget {id}: gear {gear:?} on {k:?} {b:?}"
            );
        }
    }
}

/// One widget of each composition in a 6-column grid, for the edit-mode layout checks.
fn compositions() -> ContainerLayout {
    let a = |m: &str| selector(DeviceSelector::Key(key(1)), m);
    let tile = FormSet::LABEL | FormSet::VALUE | FormSet::STATS | FormSet::BAR;
    grid_layout(vec![
        (1, a(&engine_gpu()), FormSet::all(), cell(0, 0, 3, 4)),
        (2, a("vram"), tile, cell(3, 0, 3, 3)),
        (3, a("vram"), ROW_FORMS, cell(3, 3, 3, 1)),
        (4, a("vram"), ROW_FORMS, cell(0, 4, 6, 1)),
        (
            5,
            a("vram"),
            FormSet::VALUE | FormSet::LABEL,
            cell(0, 5, 6, 1),
        ),
        (6, a("vram"), FormSet::VALUE, cell(0, 6, 3, 1)),
        (
            7,
            a("vram"),
            FormSet::LABEL | FormSet::STATS,
            cell(3, 6, 3, 1),
        ),
        (8, a(&engine_gpu()), CHART_FORMS, cell(0, 7, 6, 3)),
    ])
}

/// In Edit Widgets the grip and the gear cover no text of their widget, and every widget lies
/// inside the body's content area: the outline is drawn inside the cell and the scrollbar's
/// width stays free, as in the applied view (tasklens-lhliq.11).
#[gpui_kit::test]
fn overview_widget_edit_chrome_covers_no_text_and_fits_the_body(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let (e, c) = r.editor(compositions(), 420., 360.);
    for pct in [100u16, 150, 200] {
        r.set_ui_scale(pct);
        r.start(&e);
        draw(r.vcx);
        draw(r.vcx);
        let scale = f32::from(pct) / 100.;
        let scroll = r
            .vcx
            .debug_bounds(leak(format!("{}-scroll", editor_id(GPU_BOX_CONTAINER))))
            .expect("body");
        // The rows are taller than the body: the body scrolls, so the scrollbar's width is free.
        let content_h = r.vcx.read_entity(&c, |c, _| c.placement().content_h);
        let scrolls = content_h > scroll.size.height + px(0.5);
        assert!(
            pct != 200 || scrolls,
            "the 200 % case must scroll to cover the scrollbar"
        );
        let right = scroll.right()
            - if scrolls {
                px(grid::SCROLLBAR_W * scale)
            } else {
                px(0.)
            };
        for id in 1u32..=8 {
            let at = format!("{pct} % widget {id}");
            let w = r.vcx.debug_bounds(leak(widget_id(id))).expect("widget");
            assert!(
                w.left() >= scroll.left() - px(0.5) && w.right() <= right + px(0.5),
                "{at}: {w:?} outside the body content {scroll:?} (right edge {right:?})"
            );
            let gear = r.vcx.debug_bounds(leak(gear_id(id))).expect("gear");
            let grip = r.vcx.debug_bounds(leak(grip_id(id))).expect("grip");
            let mut texts = Vec::new();
            for part in ["name", "value", "stats", "reason"] {
                if let Some(b) = r.vcx.debug_bounds(leak(part_id(id, part))) {
                    texts.push((part.to_owned(), b));
                }
            }
            let chart = r.vcx.read_entity(&c, |c, cx| {
                c.widget(id).and_then(|w| w.read(cx).chart().cloned())
            });
            if let Some(chart) = chart {
                for (k, b) in r.vcx.read_entity(&chart, |v, _| {
                    [LabelKind::AxisMax, LabelKind::Heading, LabelKind::Value]
                        .into_iter()
                        .filter_map(|k| v.label_cache().painted(k))
                        .map(|p| (format!("{:?}", p.kind), p.bounds()))
                        .collect::<Vec<_>>()
                }) {
                    texts.push((k, b));
                }
            }
            for (part, b) in texts {
                assert!(!gear.intersects(&b), "{at}: gear {gear:?} on {part} {b:?}");
                assert!(!grip.intersects(&b), "{at}: grip {grip:?} on {part} {b:?}");
                assert!(
                    b.right() <= w.right() + px(0.5),
                    "{at}: {part} {b:?} is cut off by the widget {w:?}"
                );
            }
        }
        r.ed(&e, |e, _, cx| e.cancel(cx));
    }
}
