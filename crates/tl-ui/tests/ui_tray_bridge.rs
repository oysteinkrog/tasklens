//! Tray bridge tests (tasklens-wox.9, UI-PRF-050, UI-PRF-051, UI-SHL-113, ADR-008).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{AppContext as _, TestAppContext};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{
    ColumnId, DeviceIdx, GraphTarget, GroupKey, MetricId, ProcessKey, ProcessSeriesId, SeriesId,
};
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{
    AppearanceSettings, Settings, ThemeSettings, TrayGraphState as TrayGraphEntry, UiState,
};
use tl_model::snapshot::{ComponentHeader, ProcessSeriesSet, ProcessSnapshot, SeriesSet};
use tl_model::time::BootId;
use tl_model::tray::{
    TrayEvent, TrayGraphId, TrayGraphState, TrayModel, TrayServiceEnd, tray_channels,
};
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
use tl_ui::hub::SnapshotHub;
use tl_ui::settings_global::{SettingKey, TlSettings};
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

fn pkey(pid: u32, boot: u64) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid) * 10,
        boot_id: BootId(boot),
        pid,
    }
}

fn record(pid: u32, name: &str, app_id: Option<&str>) -> ProcessRecord {
    let name: Arc<str> = Arc::from(name);
    ProcessRecord {
        key: pkey(pid, 7),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: Arc::clone(&name),
            name: Arc::clone(&name),
            exe: None,
            cmdline: None,
            user: Arc::from("user"),
            cgroup: None,
            app_id: app_id.map(Arc::from),
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: Arc::clone(&name),
        }),
        group: GroupKey(0),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct: 0.0,
        rss_anon_bytes: 0,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        net_rx_bps: 0.0,
        net_tx_bps: 0.0,
        gpu_util_pct: 0.0,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.0,
        validity: FieldValidity::all(Validity::Valid),
    }
}

fn proc_graph(source: &str, column: &str, range_s: u16) -> TrayGraphEntry {
    TrayGraphEntry {
        source: source.to_owned(),
        column: column.to_owned(),
        color: "accent".to_owned(),
        range_s,
    }
}

fn cpu_id(target: GraphTarget) -> ProcessSeriesId {
    ProcessSeriesId {
        target,
        column: ColumnId::Cpu,
    }
}

fn proc_view(value: f32) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id: SeriesId {
            device: DeviceIdx(0),
            metric: MetricId::CPU_TOTAL,
        },
        version: 1,
        period_ns: 1_000_000_000,
        samples: Arc::from([Sample::valid(SAMPLE_NS, value)]),
    })
}

impl Rig {
    /// Publishes `records` as the process list of boot 7, `Valid` or not (before the first sweep).
    fn publish_processes(&self, mut records: Vec<ProcessRecord>, valid: bool) {
        records.sort_by_key(|r| r.key);
        self.engine.publish_with(|s| {
            s.boot_id = BootId(7);
            let version = s.processes.header.version + 1;
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version,
                    validity: if valid {
                        Validity::Valid
                    } else {
                        Validity::default()
                    },
                    ..ComponentHeader::default()
                },
                records: records.into(),
                ..ProcessSnapshot::default()
            });
        });
    }

    fn publish_process_series(
        &self,
        mut fast: Vec<(ProcessSeriesId, Arc<SeriesView>)>,
        mut slow: Vec<(ProcessSeriesId, Arc<SeriesView>)>,
    ) {
        fast.sort_by(|a, b| a.0.cmp(&b.0));
        slow.sort_by(|a, b| a.0.cmp(&b.0));
        self.engine.publish_with(|s| {
            let version = s.process_series.header.version + 1;
            s.process_series = Arc::new(ProcessSeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: fast.into(),
                slow: slow.into(),
            });
        });
    }
}

fn titles(m: &TrayModel) -> Vec<(String, String)> {
    m.graphs
        .iter()
        .map(|g| {
            (
                g.tooltip_title.to_string(),
                g.tooltip_description.to_string(),
            )
        })
        .collect()
}

/// wox.9 criteria 3 and 4: the process, name and app tooltip forms.
#[gpui_kit::test]
fn tray_tooltip_process_forms(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![
            proc_graph("process:0000000000000007:1234:12340", "cpu", 60),
            proc_graph("name:firefox", "cpu", 60),
            proc_graph("app:org.kde.kate", "cpu", 60),
        ],
        false,
    );
    r.publish_processes(
        vec![
            record(1234, "firefox", None),
            record(1235, "firefox", None),
            record(1236, "firefox", None),
            ProcessRecord {
                group: GroupKind::App {
                    app_id: Arc::from("org.kde.kate"),
                }
                .key(),
                ..record(2000, "kate", Some("org.kde.kate"))
            },
        ],
        true,
    );
    r.publish_process_series(
        vec![
            (cpu_id(GraphTarget::Process(pkey(1234, 7))), proc_view(3.2)),
            (
                cpu_id(GraphTarget::App(Arc::from("org.kde.kate"))),
                proc_view(3.2),
            ),
        ],
        Vec::new(),
    );
    let m = r.one(cx);
    let got = titles(&m);
    assert_eq!(
        got[0],
        ("firefox (PID 1234)".to_owned(), "CPU 3.2%".to_owned())
    );
    assert_eq!(got[1], ("3 procs".to_owned(), "firefox".to_owned()));
    assert_eq!(got[2], ("kate".to_owned(), "CPU 3.2%".to_owned()));
    assert!(m.graphs.iter().all(|g| g.state == TrayGraphState::Live));
    assert!(m.graphs[0].series.is_some() && m.graphs[2].series.is_some());
    assert!(m.graphs[1].series.is_none());
}

#[gpui_kit::test]
fn process_title_before_name_is_read(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![proc_graph("process:0000000000000007:1234:12340", "cpu", 60)],
        false,
    );
    r.publish_processes(Vec::new(), false);
    let m = r.one(cx);
    assert_eq!(&*m.graphs[0].tooltip_title, "Process 1234");
    assert_eq!(m.graphs[0].state, TrayGraphState::Live);
}

/// wox.9 criteria 5 and 6.
#[gpui_kit::test]
fn no_match_and_ended_descriptions(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![
            proc_graph("name:firefox", "cpu", 60),
            proc_graph("process:0000000000000008:1234:12340", "cpu", 60),
        ],
        false,
    );
    r.publish_processes(vec![record(5, "other", None)], true);
    let m = r.one(cx);
    assert_eq!(m.graphs[0].state, TrayGraphState::NoMatch);
    assert_eq!(titles(&m)[0], ("firefox".to_owned(), "No match".to_owned()));
    assert_eq!(m.graphs[1].state, TrayGraphState::Ended);
    assert_eq!(&*m.graphs[1].tooltip_description, "Ended");
    assert!(m.graphs.iter().all(|g| g.series.is_none()));
    let labels: Vec<String> = m
        .menu
        .process_graphs
        .iter()
        .map(|(_, l)| l.to_string())
        .collect();
    let want: Vec<String> = m
        .graphs
        .iter()
        .map(|g| g.tooltip_title.to_string())
        .collect();
    assert_eq!(labels, want);
}

#[gpui_kit::test]
fn ended_latch_never_returns_live(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![proc_graph("process:0000000000000007:1234:12340", "cpu", 60)],
        false,
    );
    r.publish_processes(vec![record(1234, "firefox", None)], true);
    let m = r.one(cx);
    assert_eq!(m.graphs[0].state, TrayGraphState::Live);
    assert_eq!(&*m.graphs[0].tooltip_title, "firefox (PID 1234)");

    r.publish_processes(Vec::new(), true);
    let m = r.one(cx);
    assert_eq!(m.graphs[0].state, TrayGraphState::Ended);
    assert_eq!(&*m.graphs[0].tooltip_title, "firefox (PID 1234)");

    r.publish_processes(vec![record(1234, "firefox", None)], true);
    let m = r.one(cx);
    assert_eq!(m.graphs[0].state, TrayGraphState::Ended);
    assert_eq!(&*m.graphs[0].tooltip_title, "firefox (PID 1234)");
}

#[gpui_kit::test]
fn process_graph_demand_targets(cx: &mut TestAppContext) {
    let r = rig(
        cx,
        vec![
            proc_graph("name:firefox", "cpu", 60),
            proc_graph("name:kate", "rss", 60),
            proc_graph("process:0000000000000007:1234:12340", "cpu", 60),
        ],
        false,
    );
    r.publish_processes(
        vec![record(1234, "firefox", None), record(9, "kate", None)],
        true,
    );
    r.one(cx);
    cx.executor().advance_clock(DEMAND_COALESCE);
    cx.run_until_parked();
    let last = r.engine.demands().pop().expect("demand sent");
    assert_eq!(
        &last.graph_targets[..],
        [
            (GraphTarget::Name(Arc::from("firefox")), ColumnId::Cpu),
            (GraphTarget::Process(pkey(1234, 7)), ColumnId::Cpu),
        ]
    );

    // The process ends: its target goes, the name graph keeps its own.
    r.publish_processes(
        vec![record(9, "kate", None), record(10, "firefox", None)],
        true,
    );
    r.one(cx);
    cx.executor().advance_clock(DEMAND_COALESCE);
    cx.run_until_parked();
    let last = r.engine.demands().pop().expect("demand sent");
    assert_eq!(
        &last.graph_targets[..],
        [(GraphTarget::Name(Arc::from("firefox")), ColumnId::Cpu)]
    );
}

#[gpui_kit::test]
fn process_graph_slow_tier_for_long_ranges(cx: &mut TestAppContext) {
    let id = cpu_id(GraphTarget::Name(Arc::from("firefox")));
    let fast = proc_view(1.0);
    let slow = proc_view(2.0);
    for (range_s, want_slow) in [(300u16, true), (60, false)] {
        let r = rig(cx, vec![proc_graph("name:firefox", "cpu", range_s)], false);
        r.publish_processes(vec![record(1, "firefox", None)], true);
        r.publish_process_series(
            vec![(id.clone(), fast.clone())],
            vec![(id.clone(), slow.clone())],
        );
        let m = r.one(cx);
        let got = m.graphs[0].series.as_ref().expect("series");
        let want = if want_slow { &slow } else { &fast };
        assert!(Arc::ptr_eq(got, want), "range {range_s}");
    }
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
use tl_ui::window::settings::{PageId, all_pages};

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
    persists: Rc<Cell<u32>>,
    engine: Arc<FakeEngine>,
}

fn full(cx: &mut TestAppContext, graphs: Vec<TrayGraphEntry>) -> Full {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let persists = Rc::new(Cell::new(0));
    let init = UiInit {
        settings: Settings::default(),
        state: UiState {
            tray_graphs: graphs,
            ..UiState::default()
        },
        themes_json: None,
        persist: Box::new({
            let persists = persists.clone();
            move |_, _| persists.set(persists.get() + 1)
        }),
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
    Full {
        vcx,
        service,
        persists,
        engine,
    }
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
    f.vcx.update(|_, cx| {
        cx.on_action(move |_: &ToggleMiniWindow, _| seen.set(seen.get() + 1));
    });
    f.event(TrayEvent::ShowMiniWindow);
    assert_eq!(mini.get(), 1);
}

// ---- E31-009: close to tray and minimize to tray ----

use tl_model::command::Tab;
use tl_ui::actions::{CloseWindow, MinimizeWindow, SelectTab};
use tl_ui::app::{UiExit, UiExitSlot};
use tl_ui::window::main::main_window_view;
use tl_ui::window::titlebar::TitleBarProbe;

impl Full {
    /// Runs one tray tick and returns the last model it sent.
    fn tick(&mut self) -> Option<TrayModel> {
        self.vcx.executor().advance_clock(TRAY_TICK);
        self.vcx.run_until_parked();
        let mut last = None;
        while let Ok(m) = self.service.models.try_recv() {
            last = Some(m);
        }
        last
    }

    fn icon_shown(&mut self) -> bool {
        self.tick().expect("a tray model").show_main_icon
    }

    /// Dispatches through the focused window, like a key press.
    fn press(&mut self, action: impl gpui_kit::Action) {
        self.vcx.dispatch_action(action);
        self.vcx.run_until_parked();
    }

    fn set_tray(&mut self, close: bool, minimize: bool) {
        self.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                d.settings.tray.close_to_tray = close;
                d.settings.tray.minimize_to_tray = minimize;
            })
        });
    }
}

fn exit_of(cx: &TestAppContext) -> Option<UiExit> {
    cx.read(|cx| cx.try_global::<UiExitSlot>().and_then(|s| s.0.clone()))
}

fn window_open(cx: &TestAppContext) -> bool {
    cx.read(|cx| main_window_target(cx).is_some())
}

#[gpui_kit::test]
fn close_to_tray_keeps_app(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.set_tray(true, false);
    f.press(CloseWindow);
    assert!(!window_open(cx));
    assert_eq!(exit_of(cx), None);
    assert!(f.icon_shown());
    cx.update(|cx| cx.dispatch_action(&Quit));
    cx.run_until_parked();
    assert_eq!(exit_of(cx), Some(UiExit::Quit));
}

#[gpui_kit::test]
fn close_quits_without_tray_setting(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.set_tray(false, false);
    f.press(CloseWindow);
    assert_eq!(exit_of(cx), Some(UiExit::Quit));
}

#[gpui_kit::test]
fn close_to_tray_needs_available_tray(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.set_tray(true, false);
    f.event(TrayEvent::WatcherOffline);
    f.press(CloseWindow);
    assert_eq!(exit_of(cx), Some(UiExit::Quit));
}

#[gpui_kit::test]
fn minimize_to_tray_removes_window(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.vcx
        .update(|_, cx| cx.set_global(TitleBarProbe::default()));
    f.set_tray(false, true);
    f.press(MinimizeWindow);
    assert!(!window_open(cx));
    assert_eq!(exit_of(cx), None);
    assert_eq!(cx.read(|cx| cx.global::<TitleBarProbe>().minimizes), 0);
    assert!(f.icon_shown());
}

#[gpui_kit::test]
fn minimize_stays_without_tray_setting(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.vcx
        .update(|_, cx| cx.set_global(TitleBarProbe::default()));
    f.set_tray(false, false);
    f.press(MinimizeWindow);
    assert!(window_open(cx));
    assert_eq!(cx.read(|cx| cx.global::<TitleBarProbe>().minimizes), 1);
}

#[gpui_kit::test]
fn reopen_clears_hidden_flag(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    f.set_tray(true, false);
    f.press(SelectTab(2));
    let tab = f.vcx.update(|_, cx| {
        let main = main_window_view(cx).expect("main window");
        main.read(cx).active_tab()
    });
    f.press(CloseWindow);
    assert!(f.icon_shown());
    cx.update(|cx| cx.dispatch_action(&ShowMainWindow));
    cx.run_until_parked();
    assert!(window_open(cx));
    let reopened = cx.read(|cx| {
        main_window_view(cx)
            .expect("reopened")
            .read(cx)
            .active_tab()
    });
    assert_eq!(reopened, tab);
    assert_ne!(tab, Tab::Performance);
    assert!(!f.tick().is_some_and(|m| m.show_main_icon));
}

// ---- E31-010: Tray rows on the Startup and window settings page ----

#[gpui_kit::test]
fn settings_tray_rows_toggle_keys(cx: &mut TestAppContext) {
    let mut f = full(cx, Vec::new());
    let page = f.vcx.update(|_, cx| {
        all_pages(cx)
            .into_iter()
            .find(|p| p.id() == PageId::StartupWindow)
            .expect("page listed")
    });
    // Table order is the render order, so it is the Tab order.
    assert_eq!(
        page.keys(),
        [
            SettingKey::WindowCloseWithEsc,
            SettingKey::WindowAlwaysOnTop,
            SettingKey::TrayCloseToTray,
            SettingKey::TrayMinimizeToTray,
            SettingKey::TrayAlwaysShow,
            SettingKey::StartupStartAtLogin,
            SettingKey::StartupStartMinimized,
            SettingKey::NetworkAutoFetchPublicIp,
            SettingKey::LowmemEnabled,
        ]
    );
    let tray = |f: &mut Full| {
        f.vcx.update(|_, cx| {
            let t = TlSettings::settings(cx).tray.clone();
            (t.close_to_tray, t.minimize_to_tray, t.always_show)
        })
    };
    assert_eq!(tray(&mut f), (false, false, false));
    // Visible window and no graphs: no main icon yet.
    assert!(!f.tick().is_some_and(|m| m.show_main_icon));

    let mut seen = f.persists.get();
    for (i, key) in [
        SettingKey::TrayCloseToTray,
        SettingKey::TrayMinimizeToTray,
        SettingKey::TrayAlwaysShow,
    ]
    .into_iter()
    .enumerate()
    {
        f.vcx.update(|_, cx| page.set_bool(key, true, cx));
        f.vcx.run_until_parked();
        let now = tray(&mut f);
        assert_eq!(
            [now.0, now.1, now.2],
            [true, i >= 1, i >= 2],
            "after {key:?}"
        );
        assert!(f.persists.get() > seen, "{key:?} persisted");
        seen = f.persists.get();
    }
    assert!(f.tick().expect("a tray model").show_main_icon);
}

// ---- Process row menu item 18 (tasklens-wox.13, UI-PRC-013, PROC-094, PERF-088) ----

mod row_menu_tray {
    use super::*;
    use gpui_kit::VisualTestContext;
    use tl_model::ids::{GroupKey, NodeId, ProcessKey, PseudoKind};
    use tl_model::process::{GroupKind, GroupRow, GroupRowKind, ProcFlags};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenu};
    use tl_ui::pages::processes::actions::{CapturedTarget, CapturedTargets};
    use tl_ui::pages::processes::menu::{MenuCaps, row_menu};
    use tl_ui::table::tree_table::RowKind;

    const SOURCE: &str = "process:0000000000000007:4321:4242";

    fn key() -> ProcessKey {
        ProcessKey {
            start_ticks: 4_242,
            boot_id: BootId(7),
            pid: 4321,
        }
    }

    fn target(node: NodeId, kind: RowKind) -> CapturedTarget {
        CapturedTarget {
            node,
            kind,
            name: "target".into(),
            pid: matches!(kind, RowKind::Process).then_some(4321),
            uid: 1000,
            user: "me".into(),
            flags: ProcFlags::empty(),
            member_count: 1,
            stopped: false,
            ppid: 1,
            nice: Some(0),
            exe_deleted: false,
            unit: None,
        }
    }

    fn process() -> CapturedTarget {
        target(NodeId::Process(key()), RowKind::Process)
    }

    fn app_key() -> GroupKey {
        GroupKind::App {
            app_id: Arc::from("firefox"),
        }
        .key()
    }

    fn app_group() -> GroupRow {
        GroupRow {
            node: NodeId::Group(app_key()),
            key: app_key(),
            kind: GroupRowKind::App,
            class: GroupKind::App {
                app_id: Arc::from("firefox"),
            },
            name: Arc::from("Firefox"),
            icon: None,
            members: 1,
            pids: 1,
            note: None,
            memory_bytes: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            threads: 0,
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    /// The entries of the row menu for `targets`.
    fn entries(vcx: &mut VisualTestContext, targets: Vec<CapturedTarget>) -> Vec<MenuEntryInfo> {
        let targets = CapturedTargets::new(targets, None);
        let menu = vcx.update(|window, cx| {
            let focus = cx.focus_handle();
            TlMenu::build(window, cx, focus, |menu, window, cx| {
                row_menu(menu, &targets, &MenuCaps::default(), window, cx)
            })
        });
        vcx.update(|window, cx| menu.read(cx).entries_for_test(window))
    }

    fn tray_entry(entries: Vec<MenuEntryInfo>) -> Option<MenuEntryInfo> {
        entries.into_iter().find(|e| {
            matches!(
                e.label.as_deref(),
                Some("Add tray icon graph" | "Remove tray icon graph")
            )
        })
    }

    fn submenu_entries(f: &mut Full, entry: &MenuEntryInfo) -> Vec<MenuEntryInfo> {
        let sub = entry.submenu.clone().expect("a submenu");
        f.vcx
            .update(|window, cx| sub.read(cx).entries_for_test(window))
    }

    #[gpui_kit::test]
    fn process_menu_tray_item_add(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        let entry = tray_entry(entries(&mut f.vcx, vec![process()])).expect("item 18");
        assert_eq!(entry.label.as_deref(), Some("Add tray icon graph"));
        assert_eq!(entry.kind, MenuEntryKind::Submenu);
        let items = submenu_entries(&mut f, &entry);
        let labels: Vec<_> = items
            .iter()
            .filter_map(|e| e.label.as_deref().map(str::to_owned))
            .collect();
        assert_eq!(labels, ["CPU", "Memory", "Disk", "Network", "GPU", "VRAM"]);
        let memory = items[1].action.as_deref().expect("memory action");
        assert!(memory.partial_eq(&AddTrayGraph {
            source: SOURCE.into(),
            column: "memory".into(),
        }));
        f.act(memory);
        assert_eq!(f.saved(), vec![row(SOURCE, "memory", "accent", 60)]);
        assert_eq!(f.toasts(), ["Tray icon graph added"]);
    }

    #[gpui_kit::test]
    fn process_menu_tray_item_remove_when_exists(cx: &mut TestAppContext) {
        let mut f = full(cx, vec![graph(SOURCE, "accent")]);
        let entry = tray_entry(entries(&mut f.vcx, vec![process()])).expect("item 18");
        assert_eq!(entry.label.as_deref(), Some("Remove tray icon graph"));
        assert_eq!(entry.kind, MenuEntryKind::Item);
        let action = entry.action.as_deref().expect("action");
        assert!(action.partial_eq(&RemoveTrayGraph {
            source: SOURCE.into(),
            column: "cpu".into(),
        }));
    }

    #[gpui_kit::test]
    fn process_menu_tray_item_app_source(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        f.engine.publish_with(|snap| {
            let mut table = (*snap.table).clone();
            table.groups = Arc::from(vec![app_group()]);
            snap.table = Arc::new(table);
        });
        let app = target(NodeId::Group(app_key()), RowKind::App);
        let entry = tray_entry(entries(&mut f.vcx, vec![app])).expect("item 18");
        let items = submenu_entries(&mut f, &entry);
        f.act(items[0].action.as_deref().expect("cpu action"));
        assert_eq!(f.saved(), vec![row("app:firefox", "cpu", "accent", 60)]);
    }

    #[gpui_kit::test]
    fn process_menu_tray_item_hidden_multi_and_group(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        let other = target(
            NodeId::Process(ProcessKey { pid: 4322, ..key() }),
            RowKind::Process,
        );
        assert!(tray_entry(entries(&mut f.vcx, vec![process(), other])).is_none());
        let pseudo = target(NodeId::Pseudo(PseudoKind::Interrupts), RowKind::Pseudo);
        assert!(tray_entry(entries(&mut f.vcx, vec![pseudo])).is_none());
    }

    #[gpui_kit::test]
    fn process_menu_tray_item_disabled_without_tray(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        f.vcx
            .update(|_, cx| cx.global_mut::<TrayStatus>().available = false);
        let entry = tray_entry(entries(&mut f.vcx, vec![process()])).expect("item 18");
        assert_eq!(entry.kind, MenuEntryKind::Disabled);
        assert_eq!(entry.tooltip.as_deref(), Some("No system tray available"));
    }
}

// ---- Minigraph card menu item 7 (tasklens-wox.14, UI-PRF-042, PERF-088) ----

mod minigraph_menu_tray {
    use super::*;
    use gpui_kit::VisualTestContext;
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenu};
    use tl_ui::pages::minigraph::GraphColor;
    use tl_ui::pages::minigraph::menu::{CardMenuCtx, card_menu};
    use tl_ui::pages::minigraph::sources::{GraphColumn, MinigraphId};

    const SOURCE: &str = "process:0000000000000007:4321:4242";

    fn ctx(source: &str, is_process: bool, column: GraphColumn) -> CardMenuCtx {
        CardMenuCtx {
            id: MinigraphId(0),
            is_process,
            source: source.to_owned().into(),
            column,
            range_s: 60,
            color: GraphColor::Accent,
            detached: false,
            long_ranges: true,
            colors: true,
            detach: false,
        }
    }

    fn entries(vcx: &mut VisualTestContext, ctx: CardMenuCtx) -> Vec<MenuEntryInfo> {
        let menu = vcx.update(|window, cx| {
            let focus = cx.focus_handle();
            TlMenu::build(window, cx, focus, move |menu, window, cx| {
                card_menu(menu, &ctx, window, cx)
            })
        });
        vcx.update(|window, cx| menu.read(cx).entries_for_test(window))
    }

    fn tray_entry(entries: Vec<MenuEntryInfo>) -> MenuEntryInfo {
        entries
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Add tray icon graph"))
            .expect("item 7")
    }

    #[gpui_kit::test]
    fn minigraph_menu_tray_item_position(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        let list = entries(&mut f.vcx, ctx("cpu", false, GraphColumn::Cpu));
        let shape: Vec<_> = list.iter().map(|e| (e.kind, e.label.as_deref())).collect();
        assert_eq!(
            shape,
            [
                (MenuEntryKind::Item, Some("Hide graph")),
                (MenuEntryKind::Submenu, Some("Time range")),
                (MenuEntryKind::Submenu, Some("Graph color")),
                (MenuEntryKind::Separator, None),
                (MenuEntryKind::Item, Some("Add tray icon graph")),
                (MenuEntryKind::Separator, None),
                (MenuEntryKind::Item, Some("Reset minigraph layout")),
            ]
        );
    }

    #[gpui_kit::test]
    fn minigraph_menu_tray_item_dispatch_system_and_process(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        let entry = tray_entry(entries(&mut f.vcx, ctx("cpu", false, GraphColumn::Cpu)));
        let action = entry.action.as_deref().expect("action");
        assert!(action.partial_eq(&AddTrayGraph {
            source: "cpu".into(),
            column: "cpu".into(),
        }));
        f.act(action);
        assert_eq!(f.saved(), vec![row("cpu", "cpu", "accent", 60)]);
        assert_eq!(f.toasts(), ["Tray icon graph added"]);
        f.act(action);
        assert!(
            f.toasts()
                .iter()
                .any(|t| t == "Tray icon graph already exists")
        );

        let entry = tray_entry(entries(&mut f.vcx, ctx(SOURCE, true, GraphColumn::Memory)));
        let action = entry.action.as_deref().expect("action");
        assert!(action.partial_eq(&AddTrayGraph {
            source: SOURCE.into(),
            column: "memory".into(),
        }));
    }

    #[gpui_kit::test]
    fn minigraph_menu_tray_item_disabled_without_tray(cx: &mut TestAppContext) {
        let mut f = full(cx, Vec::new());
        f.vcx
            .update(|_, cx| cx.global_mut::<TrayStatus>().available = false);
        let entry = tray_entry(entries(&mut f.vcx, ctx("cpu", false, GraphColumn::Cpu)));
        assert_eq!(entry.kind, MenuEntryKind::Disabled);
        assert_eq!(entry.tooltip.as_deref(), Some("No system tray available"));
    }
}
