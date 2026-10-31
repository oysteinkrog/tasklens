//! Overview box widgets (UI-PRF-061, ADR-019): the widget entity, its compositions, the
//! shared source data and the widget container on its inner grid. Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FormSet, GPU_BOX_CONTAINER, LAYOUT_VERSION, MetricKey,
    PresetSetting, SourceSelector, WidgetCell, WidgetOptions, WidgetSetting,
};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot, SystemSnapshot};
use tl_model::system::{GpuEngine, GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::YScale;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::widget::compose::{
    Composition, cell_min, row_forms, row_min, tile_min,
};
use tl_ui::pages::performance::widget::container::{WidgetContainer, container_id, scroll_id};
use tl_ui::pages::performance::widget::grid::{self, InnerGrid};
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

/// The render contract: on a sample the container never notifies, no widget entity is made or
/// dropped, and only the widgets on the changed source notify. A sample that changes nothing
/// shown renders neither the container nor the page.
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
    let (render0, notify0, page0) = r.container_counts(&c);

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

    // GPU A's utilization changes: widget 1 notifies, the container does not. (GPUI marks the
    // ancestors of a notifying view dirty, crw.34, so they may render once with fixed work.)
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
    let (_, notify, _) = r.container_counts(&c);
    assert_eq!(notify, notify0, "the container never notifies on a sample");
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
