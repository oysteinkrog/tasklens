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
use tl_model::tray::{TrayGraphState, TrayModel, TrayServiceEnd, tray_channels};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
use tl_ui::hub::SnapshotHub;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::tray_bridge::{self, TRAY_TICK, TrayBridge, TrayStatus};

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
