//! Overview box widgets (UI-PRF-061, ADR-019): the widget entity, its compositions and the
//! shared source data. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    DeviceSelector, FormSet, MetricKey, SourceSelector, WidgetOptions, WidgetSetting,
};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{ComponentHeader, SeriesSet, Snapshot, SystemSnapshot};
use tl_model::system::{GpuEngine, GpuTotals, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::widget::compose::{
    Composition, cell_min, row_forms, row_min, tile_min,
};
use tl_ui::pages::performance::widget::metric_widget::{MetricWidget, part_id};
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

/// The widgets, each in a box of its own size, one under the other.
struct Host {
    widgets: Vec<(Entity<MetricWidget>, f32, f32)>,
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
