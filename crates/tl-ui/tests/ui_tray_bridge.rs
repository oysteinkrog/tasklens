//! Tray bridge tests (tasklens-wox.9, UI-PRF-050, UI-PRF-051, UI-SHL-113, ADR-008).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{AppContext as _, TestAppContext};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{
    AppearanceSettings, Settings, ThemeSettings, TrayGraphState as TrayGraphEntry, UiState,
};
use tl_model::snapshot::SeriesSet;
use tl_model::tray::{
    TrayEvent, TrayGraphId, TrayGraphState, TrayModel, TrayServiceEnd, tray_channels,
};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
use tl_ui::hub::SnapshotHub;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::tray_bridge::{
    self, AddTrayGraph, RemoveTrayGraph, TRAY_TICK, TrayBridge, TrayStatus, main_window_target,
    tray_source_of_card,
};

const NOW_NS: u64 = 10_000_000_000;
const SAMPLE_NS: u64 = 9_000_000_000;

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("dev"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
    }
}

fn catalog() -> DeviceCatalog {
    DeviceCatalog {
        version: 1,
        entries: vec![
            entry(0, DeviceIdentity::CpuPackage),
            entry(1, DeviceIdentity::Memory),
            entry(2, DeviceIdentity::Aggregate(AggregateKind::Disks)),
            entry(3, DeviceIdentity::Aggregate(AggregateKind::Nets)),
            entry(
                4,
                DeviceIdentity::Gpu {
                    uuid: [0; 16],
                    pci_bus_id: Arc::from("0000:01:00.0"),
                },
            ),
        ]
        .into(),
    }
}

fn view(device: u16, metric: MetricId, value: f32) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id: SeriesId {
            device: DeviceIdx(device),
            metric,
        },
        version: 1,
        period_ns: 1_000_000_000,
        samples: Arc::from([Sample::valid(SAMPLE_NS, value)]),
    })
}

struct Rig {
    engine: Arc<FakeEngine>,
    service: TrayServiceEnd,
    bridge: gpui_kit::Entity<TrayBridge>,
}

fn graph(source: &str, color: &str) -> TrayGraphEntry {
    TrayGraphEntry {
        source: source.to_owned(),
        column: "cpu".to_owned(),
        color: color.to_owned(),
        range_s: 60,
    }
}

fn rig(cx: &mut TestAppContext, graphs: Vec<TrayGraphEntry>, always_show: bool) -> Rig {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let mut settings = Settings::default();
    settings.tray.always_show = always_show;
    settings.units.network_auto_scale = false;
    let state = UiState {
        tray_graphs: graphs,
        ..UiState::default()
    };
    let (ui, service) = tray_channels();
    let c = Arc::clone(&client);
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog());
        s.published_ns = NOW_NS;
        let mut views = vec![
            view(0, MetricId::CPU_TOTAL, 18.6),
            view(1, MetricId::MEM_USED, 549.0),
            view(2, MetricId::DISK_ACTIVE, 1.2),
            view(3, MetricId::NET_RX, 25_000.0),
            view(3, MetricId::NET_TX, 12_500.0),
            view(4, MetricId::GPU_UTIL, 3.0),
        ];
        views.sort_by_key(|v| v.id);
        let mut sys = tl_model::snapshot::SystemSnapshot::default();
        sys.memory.total = 1000;
        s.system = Arc::new(sys);
        s.series = Arc::new(SeriesSet {
            series: views.into(),
            ..SeriesSet::default()
        });
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        let appearance = AppearanceSettings::default();
        tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
        tl_ui::theme::metrics::init_metrics(&appearance, cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_global(EngineGlobal(Arc::clone(&c)));
        cx.set_global(BootSettings {
            settings,
            state,
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let agg = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    cx.update(|cx| {
        cx.set_global(GlobalDemand(agg));
        tray_bridge::install(cx, ui);
    });
    let bridge = cx.read(TrayBridge::global);
    Rig {
        engine,
        service,
        bridge,
    }
}

impl Rig {
    /// Runs one tick and returns the models sent by it.
    fn tick(&self, cx: &mut TestAppContext) -> Vec<TrayModel> {
        cx.executor().advance_clock(TRAY_TICK);
        cx.run_until_parked();
        let mut out = Vec::new();
        while let Ok(m) = self.service.models.try_recv() {
            out.push(m);
        }
        out
    }

    fn one(&self, cx: &mut TestAppContext) -> TrayModel {
        let mut models = self.tick(cx);
        assert_eq!(models.len(), 1, "one model per tick");
        models.remove(0)
    }
}

#[gpui_kit::test]
fn loads_graphs_in_file_order(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![graph("cpu", "accent"), graph("name:firefox", "accent")],
        false,
    );
    cx.read(|cx| {
        let status = cx.global::<TrayStatus>();
        assert!(status.available && status.running);
    });
    let m = r.one(cx);
    let ids: Vec<u32> = m.graphs.iter().map(|g| g.id.0).collect();
    assert_eq!(ids, [1, 2]);
    assert_eq!(r.bridge.read_with(cx, |b, _| b.next_id().0), 3);
}

#[gpui_kit::test]
fn tray_tooltip_forms(cx: &mut TestAppContext) {
    let sources = ["cpu", "memory", "disk", "network", "gpu"];
    let r = rig(
        cx,
        sources.iter().map(|s| graph(s, "accent")).collect(),
        false,
    );
    let m = r.one(cx);
    let got: Vec<(String, String)> = m
        .graphs
        .iter()
        .map(|g| {
            (
                g.tooltip_title.to_string(),
                g.tooltip_description.to_string(),
            )
        })
        .collect();
    let want = [
        ("CPU Usage", "18.6%"),
        ("Memory Usage", "54.9%"),
        ("Disk Usage", "1.2%"),
        ("Network Usage", "0.3 Mbps"),
        ("GPU Usage", "3.0%"),
    ];
    for ((t, d), (wt, wd)) in got.iter().zip(want) {
        assert_eq!((t.as_str(), d.as_str()), (wt, wd));
    }
    assert!(m.graphs.iter().all(|g| g.state == TrayGraphState::Live));
    assert!(m.graphs.iter().all(|g| g.series.is_some()));
    assert_eq!(m.graphs[0].max, 100.0);
    assert_eq!(m.graphs[1].max, 1000.0);
}

/// Process, name and app graphs are wox.16: until then the frame is warming up, with no series.
#[gpui_kit::test]
fn no_match_and_ended_descriptions(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![
            graph("name:firefox", "accent"),
            graph("app:org.kde.kate", "accent"),
        ],
        false,
    );
    let m = r.one(cx);
    assert!(m.graphs.iter().all(|g| g.series.is_none()));
    assert!(m.graphs.iter().all(|g| g.state == TrayGraphState::Live));
    assert_eq!(&*m.graphs[0].tooltip_title, "firefox");
    assert_eq!(&*m.graphs[0].tooltip_description, "\u{2026}");
    let labels: Vec<String> = m
        .menu
        .process_graphs
        .iter()
        .map(|(_, l)| l.to_string())
        .collect();
    assert_eq!(labels, ["firefox", "org.kde.kate"]);
}

#[gpui_kit::test]
fn main_icon_visibility_rule(cx: &mut TestAppContext) {
    // Visible window, no graphs, always_show off: nothing is sent.
    let r = rig(cx, Vec::new(), false);
    assert!(r.tick(cx).is_empty());
    // Hidden window: the main icon shows.
    r.bridge
        .update(cx, |b, cx| b.set_main_window_hidden(true, cx));
    let m = r.one(cx);
    assert!(m.show_main_icon);
    assert_eq!(&*m.main_tooltip_title, "tasklens");
    // Reopened: one empty model, then nothing.
    r.bridge
        .update(cx, |b, cx| b.set_main_window_hidden(false, cx));
    let m = r.one(cx);
    assert!(!m.show_main_icon && m.graphs.is_empty());
    assert!(r.tick(cx).is_empty());

    // always_show on.
    let r = rig(cx, Vec::new(), true);
    let m = r.one(cx);
    assert!(m.show_main_icon);
    assert!(m.main_tooltip_description.starts_with("CPU: "));

    // A graph present: no main icon, even with always_show.
    let r = rig(cx, vec![graph("cpu", "accent")], true);
    assert!(!r.one(cx).show_main_icon);
}

#[gpui_kit::test]
fn main_tooltip_five_lines(cx: &mut TestAppContext) {
    let r = rig(cx, Vec::new(), true);
    let m = r.one(cx);
    assert_eq!(
        &*m.main_tooltip_description,
        "CPU: 18.6%\nRAM: 54.9%\nGPU: 3.0%\nDisk: 1.2%\nNetwork: 0.3 Mbps"
    );
    // A missing GPU shows the n/a glyph text.
    r.engine.publish_with(|s| {
        let mut cat = DeviceCatalog::clone(&s.devices);
        cat.entries = cat
            .entries
            .iter()
            .filter(|e| !matches!(e.identity, DeviceIdentity::Gpu { .. }))
            .cloned()
            .collect::<Vec<_>>()
            .into();
        s.devices = Arc::new(cat);
    });
    let m = r.one(cx);
    assert!(m.main_tooltip_description.contains("\nGPU: n/a\n"));
}

#[gpui_kit::test]
fn menu_state_page_and_process_graphs(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![graph("cpu", "accent"), graph("network", "accent")],
        false,
    );
    let m = r.one(cx);
    assert_eq!(
        m.menu.page_sources,
        [true, false, false, true, false, false, false]
    );
    assert!(m.menu.process_graphs.is_empty());
}

#[gpui_kit::test]
fn named_and_accent_colours(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![graph("cpu", "teal"), graph("cpu", "accent")],
        false,
    );
    let m = r.one(cx);
    assert_eq!(m.graphs[0].line_rgb, [0x14, 0xA0, 0x98]);
    assert_eq!(&*m.graphs[0].color, "teal");
    let theme = cx.read(|cx| tl_ui::theme::current(cx).clone());
    let c = theme.graph.cpu_line.to_rgb();
    let byte = |v: f32| (v * 255.0).round() as u8;
    assert_eq!(m.graphs[1].line_rgb, [byte(c.r), byte(c.g), byte(c.b)]);
    assert_eq!(m.graphs[1].line_rgb, [0x0A, 0x70, 0x96]);
}

#[gpui_kit::test]
fn idle_tray_sends_nothing_and_releases_demand(cx: &mut TestAppContext) {
    let r = rig(cx, vec![graph("cpu", "accent")], false);
    let m = r.one(cx);
    cx.executor().advance_clock(DEMAND_COALESCE);
    cx.run_until_parked();
    let last = r.engine.demands().pop().expect("demand sent");
    assert!(last.tray.icons_shown && last.tray.cpu);
    assert!(!last.tray.nets);

    // Remove the last graph: one empty model, then silence, and the demand is gone.
    let id = m.graphs[0].id;
    r.bridge.update(cx, |b, _| b.remove_graph(id));
    let m = r.one(cx);
    assert!(m.graphs.is_empty() && !m.show_main_icon);
    assert!(r.tick(cx).is_empty());
    cx.executor().advance_clock(DEMAND_COALESCE);
    cx.run_until_parked();
    let last = r.engine.demands().pop().expect("demand sent");
    assert!(!last.tray.icons_shown && !last.tray.cpu);
}

// ---- E31-A2: events, actions, persistence, toasts ----

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::VisualTestContext;
use tl_ui::actions::{Quit, ShowMainWindow, ToggleMainWindow, ToggleMiniWindow};
use tl_ui::app::{AppEnv, StartOptions, TasklensUi, UiInit};
use tl_ui::components::toast::ToastQueue;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::PerfAddTrayGraph;
use tl_ui::window::main::open_main_window;

fn saved(cx: &mut TestAppContext) -> Vec<(String, String, String, u16)> {
    cx.read(|cx| {
        TlSettings::state(cx)
            .tray_graphs
            .iter()
            .map(|g| {
                (
                    g.source.clone(),
                    g.column.clone(),
                    g.color.clone(),
                    g.range_s,
                )
            })
            .collect()
    })
}

fn row(source: &str, column: &str, color: &str, range: u16) -> (String, String, String, u16) {
    (source.into(), column.into(), color.into(), range)
}

/// The full UI with a main window, so toasts have a queue.
struct Full {
    vcx: VisualTestContext,
    service: TrayServiceEnd,
}

fn full(cx: &mut TestAppContext, graphs: Vec<TrayGraphEntry>) -> Full {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState {
            tray_graphs: graphs,
            ..UiState::default()
        },
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    let (ui, service) = tray_channels();
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        cx.set_global(StartOptions::default());
        cx.set_reduce_motion(true);
        tray_bridge::install(cx, ui);
    });
    cx.run_until_parked();
    let handle = cx.update(open_main_window).expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    Full { vcx, service }
}

impl Full {
    fn event(&mut self, e: TrayEvent) {
        self.service
            .events
            .try_send(e)
            .expect("event channel has room");
        self.vcx.run_until_parked();
    }

    fn act(&mut self, action: &dyn gpui_kit::Action) {
        self.vcx.update(|_, cx| cx.dispatch_action(action));
        self.vcx.run_until_parked();
    }

    fn toasts(&mut self) -> Vec<String> {
        self.vcx.update(|window, cx| {
            ToastQueue::of_window(window, cx)
                .expect("toast queue")
                .read(cx)
                .visible_texts()
                .iter()
                .map(|t| t.to_string())
                .collect()
        })
    }

    fn saved(&mut self) -> Vec<(String, String, String, u16)> {
        self.vcx.update(|_, cx| {
            TlSettings::state(cx)
                .tray_graphs
                .iter()
                .map(|g| {
                    (
                        g.source.clone(),
                        g.column.clone(),
                        g.color.clone(),
                        g.range_s,
                    )
                })
                .collect()
        })
    }
}

fn perf_add(key: &str) -> PerfAddTrayGraph {
    PerfAddTrayGraph {
        source: DeviceKey(Arc::from(key)),
    }
}

#[gpui_kit::test]
fn add_page_graph_persists_and_toasts(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.act(&perf_add("cpu"));
    assert_eq!(f.saved(), vec![row("cpu", "cpu", "accent", 60)]);
    assert_eq!(f.toasts(), vec!["Tray icon graph added".to_owned()]);
}

#[gpui_kit::test]
fn add_existing_graph_warns(cx: &mut TestAppContext) {
    let mut f = full(cx, vec![graph("cpu", "accent")]);
    f.act(&perf_add("cpu"));
    assert_eq!(f.saved(), vec![row("cpu", "cpu", "accent", 60)]);
    assert_eq!(
        f.toasts(),
        vec!["Tray icon graph already exists".to_owned()]
    );
}

#[gpui_kit::test]
fn remove_graph_and_remove_missing(cx: &mut TestAppContext) {
    let mut p = graph("process:0000000000000000:7:100", "accent");
    p.column = "memory".to_owned();
    let mut f = full(cx, vec![graph("cpu", "accent"), p]);
    f.act(&RemoveTrayGraph {
        source: "process:0000000000000000:7:100".into(),
        column: "memory".into(),
    });
    assert_eq!(f.saved(), vec![row("cpu", "cpu", "accent", 60)]);
    assert_eq!(f.toasts(), vec!["Tray icon graph removed".to_owned()]);
    f.act(&RemoveTrayGraph {
        source: "process:0000000000000000:7:100".into(),
        column: "memory".into(),
    });
    assert_eq!(f.saved().len(), 1);
    assert!(
        f.toasts()
            .contains(&"Unable to remove tray icon graph".to_owned())
    );
}

#[gpui_kit::test]
fn add_tray_graph_action_with_column(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.act(&AddTrayGraph {
        source: "name:firefox".into(),
        column: "memory".into(),
    });
    assert_eq!(f.saved(), vec![row("name:firefox", "memory", "accent", 60)]);
}

#[gpui_kit::test]
fn register_failed_removes_and_errors(cx: &mut TestAppContext) {
    let mut f = full(cx, vec![graph("cpu", "accent"), graph("memory", "accent")]);
    f.event(TrayEvent::RegisterFailed(TrayGraphId(2)));
    assert_eq!(f.saved(), vec![row("cpu", "cpu", "accent", 60)]);
    assert_eq!(f.toasts(), vec!["Unable to add tray icon graph".to_owned()]);
}

#[gpui_kit::test]
fn toggle_page_source_adds_and_removes(cx: &mut TestAppContext) {
    let mut f = full(cx, vec![graph("cpu", "accent")]);
    f.event(TrayEvent::TogglePageSource { source: "cpu" });
    assert!(f.saved().is_empty());
    assert!(f.toasts().contains(&"Tray icon graph removed".to_owned()));
    f.event(TrayEvent::TogglePageSource { source: "cpu" });
    assert_eq!(f.saved(), vec![row("cpu", "cpu", "accent", 60)]);
    assert!(f.toasts().contains(&"Tray icon graph added".to_owned()));
}

#[gpui_kit::test]
fn remove_event_removes_graph(cx: &mut TestAppContext) {
    let mut f = full(cx, vec![graph("cpu", "accent")]);
    f.event(TrayEvent::Remove { id: TrayGraphId(1) });
    assert!(f.saved().is_empty());
    assert_eq!(f.toasts(), vec!["Tray icon graph removed".to_owned()]);
}

#[gpui_kit::test]
fn ids_stay_in_file_order_after_remove_and_add(cx: &mut TestAppContext) {
    let mut f = full(cx, vec![graph("cpu", "accent"), graph("memory", "accent")]);
    f.event(TrayEvent::Remove { id: TrayGraphId(1) });
    f.act(&perf_add("net:eth0"));
    assert_eq!(
        f.saved(),
        vec![
            row("memory", "cpu", "accent", 60),
            row("network", "cpu", "accent", 60)
        ]
    );
}

#[gpui_kit::test]
fn card_keys_map_to_tray_sources(_cx: &mut TestAppContext) {
    let map = |k: &str| tray_source_of_card(&DeviceKey(Arc::from(k)));
    assert_eq!(map("cpu").as_deref(), Some("cpu"));
    assert_eq!(map("memory").as_deref(), Some("memory"));
    assert_eq!(map("disk:nvme0n1").as_deref(), Some("disk"));
    assert_eq!(map("net:eth0").as_deref(), Some("network"));
    assert_eq!(
        map("gpu:pci:0000:2d:00.0").as_deref(),
        Some("gpu:gpu:pci:0000:2d:00.0")
    );
    assert_eq!(
        map("gpu:GPU-00000000-0000-0000-0000-000000000000").as_deref(),
        Some("gpu:gpu:GPU-00000000-0000-0000-0000-000000000000")
    );
    assert_eq!(map("fan:chip:0"), None);
    assert_eq!(map("bat:BAT0"), None);
    assert_eq!(map(DeviceKey::OVERVIEW), None);
}

#[gpui_kit::test]
fn card_without_tray_source_errors(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.act(&perf_add("fan:chip:0"));
    assert!(f.saved().is_empty());
    assert_eq!(f.toasts(), vec!["Unable to add tray icon graph".to_owned()]);
}

#[gpui_kit::test]
fn card_actions_store_mapped_sources(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.act(&perf_add("gpu:pci:0000:2d:00.0"));
    f.act(&perf_add("disk:nvme0n1"));
    f.act(&perf_add("net:eth0"));
    let sources: Vec<String> = f.saved().into_iter().map(|r| r.0).collect();
    assert_eq!(sources, ["gpu:gpu:pci:0000:2d:00.0", "disk", "network"]);
}

#[gpui_kit::test]
fn set_color_and_range_persist(cx: &mut TestAppContext) {
    let r = rig(cx, vec![graph("cpu", "accent")], false);
    r.service
        .events
        .try_send(TrayEvent::SetColor {
            id: TrayGraphId(1),
            color: "rose".into(),
        })
        .unwrap();
    r.service
        .events
        .try_send(TrayEvent::SetRange {
            id: TrayGraphId(1),
            secs: 300,
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(saved(cx), vec![row("cpu", "cpu", "rose", 300)]);
    let m = r.one(cx);
    assert_eq!(m.graphs[0].line_rgb, [0xF4, 0x3F, 0x5E]);
    r.service
        .events
        .try_send(TrayEvent::SetRange {
            id: TrayGraphId(1),
            secs: 45,
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(saved(cx), vec![row("cpu", "cpu", "rose", 300)]);
}

#[gpui_kit::test]
fn watcher_events_set_status(cx: &mut TestAppContext) {
    let r = rig(cx, Vec::new(), false);
    let available = |cx: &mut TestAppContext| cx.read(|cx| cx.global::<TrayStatus>().available);
    r.service
        .events
        .try_send(TrayEvent::WatcherOffline)
        .unwrap();
    cx.run_until_parked();
    assert!(!available(cx));
    r.service.events.try_send(TrayEvent::WatcherOnline).unwrap();
    cx.run_until_parked();
    assert!(available(cx));
}

#[gpui_kit::test]
fn closed_channel_marks_not_running(cx: &mut TestAppContext) {
    let r = rig(cx, Vec::new(), false);
    assert!(cx.read(|cx| cx.global::<TrayStatus>().running));
    drop(r.service);
    cx.run_until_parked();
    assert!(!cx.read(|cx| cx.global::<TrayStatus>().running));
}

/// Counts each shell action that reaches the app.
fn count_shell_actions(cx: &mut TestAppContext) -> [Rc<Cell<u32>>; 4] {
    let counters: [Rc<Cell<u32>>; 4] = Default::default();
    let [toggle, show, mini, quit] = counters.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &ToggleMainWindow, _| toggle.set(toggle.get() + 1));
        cx.on_action(move |_: &ShowMainWindow, _| show.set(show.get() + 1));
        cx.on_action(move |_: &ToggleMiniWindow, _| mini.set(mini.get() + 1));
        cx.on_action(move |_: &Quit, _| quit.set(quit.get() + 1));
    });
    counters
}

#[gpui_kit::test]
fn window_events_dispatch_shell_actions(cx: &mut TestAppContext) {
    // No main window: every action goes to the app.
    let r = rig(cx, Vec::new(), false);
    let [toggle, show, mini, quit] = count_shell_actions(cx);
    assert!(cx.read(|cx| main_window_target(cx).is_none()));
    for e in [
        TrayEvent::ToggleMainWindow,
        TrayEvent::ShowMainWindow,
        TrayEvent::ShowMiniWindow,
        TrayEvent::Exit,
    ] {
        r.service.events.try_send(e).unwrap();
        cx.run_until_parked();
    }
    assert_eq!(
        [toggle.get(), show.get(), mini.get(), quit.get()],
        [1, 1, 1, 1]
    );
}

#[gpui_kit::test]
fn shell_action_goes_to_open_main_window(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    assert!(f.vcx.update(|_, cx| main_window_target(cx).is_some()));
    let mini = Rc::new(Cell::new(0));
    let seen = Rc::clone(&mini);
    f.vcx
        .update(|_, cx| cx.on_action(move |_: &ToggleMiniWindow, _| seen.set(seen.get() + 1)));
    f.event(TrayEvent::ShowMiniWindow);
    assert_eq!(mini.get(), 1);
}
