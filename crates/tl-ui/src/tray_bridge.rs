//! Bridge between the UI and the tl-tray StatusNotifierItem service (P3). tl-ui never links zbus or ksni.
//!
//! Once per second, while the tray is in use, [`TrayBridge`] builds a `TrayModel` from the latest
//! snapshot and the `[[tray_graphs]]` of `state.toml` and sends it to the service (UI-PRF-050,
//! UI-PRF-051, UI-SHL-113, ADR-008). Events coming back, and the add and remove actions, are
//! E31-A2. Process, name and app graphs are E31 wox.16: until then their frame is in the
//! warming-up state, with no series.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    Action, App, AppContext as _, AsyncApp, Context, Entity, Global, SharedString, WeakEntity,
    WindowHandle, gpui,
};
use serde::Deserialize;
use tl_model::client::EngineClient;
use tl_model::demand::{Demand, TrayDemand};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{NetworkUnit, TrayGraphState as TrayGraphEntry};
use tl_model::snapshot::Snapshot;
use tl_model::time::TimeNs;
use tl_model::tray::{
    TRAY_PAGE_SOURCES, TrayEvent, TrayGraphFrame, TrayGraphId, TrayGraphState, TrayMenuState,
    TrayModel, TrayUiEnd,
};
use tl_model::units;

use crate::actions::{Quit, ShowMainWindow, ToggleMainWindow, ToggleMiniWindow};
use crate::app::EngineGlobal;
use crate::chart::scale::{self, ScaleState};
use crate::chart::view::find_tiered;
use crate::chart::{ValueFormat, YScale, format_value};
use crate::components::toast::ToastKind;
use crate::demand::{DemandAggregator, GlobalDemand};
use crate::i18n::tr;
use crate::pages::minigraph::GraphColor;
use crate::pages::minigraph::card::{chart_spec_for, device_for, resolve_colors};
use crate::pages::minigraph::sources::MinigraphSource;
use crate::pages::performance::actions::PerfAddTrayGraph;
use crate::pages::performance::{DeviceKey, DeviceKind};
use crate::settings_global::TlSettings;
use crate::theme::current;
use crate::window::main::{ShellState, push_main_toast};

/// Time between two models.
pub const TRAY_TICK: Duration = Duration::from_millis(1000);

/// The tooltip text of a value that has no valid sample yet (WarmingUp glyph, ADR-003).
const WARMING_UP_TEXT: &str = "\u{2026}";
/// The tooltip text of a value the hardware does not provide (Unsupported glyph, ADR-003).
const UNSUPPORTED_TEXT: &str = "n/a";
/// Title of the main icon's tooltip.
const MAIN_TITLE: &str = "tasklens";

/// Adds a tray graph for `source` (a `[[minigraphs]]` source string) and `column`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct AddTrayGraph {
    pub source: SharedString,
    pub column: SharedString,
}

/// Removes the tray graph with this `source` and `column`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct RemoveTrayGraph {
    pub source: SharedString,
    pub column: SharedString,
}

/// GPUI Global read by menus ("No system tray available") and the close decision.
pub struct TrayStatus {
    pub available: bool,
    pub running: bool,
}

impl Global for TrayStatus {}

/// Keeps the bridge entity alive for the app's lifetime.
struct GlobalTrayBridge(Entity<TrayBridge>);

impl Global for GlobalTrayBridge {}

/// One tray graph (`[[tray_graphs]]` entry plus its scale memory).
pub struct TrayGraph {
    pub id: TrayGraphId,
    pub source: SharedString,
    pub column: SharedString,
    pub color: SharedString,
    pub range_s: u32,
    pub scale: ScaleState,
}

/// The tray's one entity per app.
pub struct TrayBridge {
    end: TrayUiEnd,
    client: Arc<dyn EngineClient>,
    graphs: Vec<TrayGraph>,
    next_id: u32,
    main_window_hidden: bool,
    /// The last tick found the tray in use, so a model went out and a demand part is set.
    active: bool,
    /// The tray part last given to the aggregator.
    demand_sent: Option<TrayDemand>,
}

/// Loads the graphs, sets [`TrayStatus`] and starts the 1 Hz timer. Called once by tl-app after
/// `TasklensUi::init` (E31-008).
pub fn install(cx: &mut App, end: TrayUiEnd) {
    let client = Arc::clone(&cx.global::<EngineGlobal>().0);
    let entries: Vec<TrayGraphEntry> = if cx.has_global::<TlSettings>() {
        TlSettings::state(cx).tray_graphs.clone()
    } else {
        Vec::new()
    };
    cx.set_global(TrayStatus {
        available: true,
        running: true,
    });
    let events = end.events.clone();
    let bridge = cx.new(|cx| TrayBridge::new(end, client, &entries, cx));
    cx.set_global(GlobalTrayBridge(bridge));
    cx.on_action(
        |a: &PerfAddTrayGraph, cx| match tray_source_of_card(&a.source) {
            Some(source) => add_graph(cx, source.into(), "cpu".into()),
            None => push_main_toast(
                cx,
                ToastKind::Error,
                tr(cx, "tray-toast-add-failed"),
                None,
                None,
            ),
        },
    );
    cx.on_action(|a: &AddTrayGraph, cx| add_graph(cx, a.source.clone(), a.column.clone()));
    cx.on_action(|a: &RemoveTrayGraph, cx| remove_matching(cx, &a.source, &a.column));
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(event) = events.recv().await {
            cx.update(|cx| handle_event(cx, event));
        }
        cx.update(|cx| cx.global_mut::<TrayStatus>().running = false);
    })
    .detach();
}

/// The `[[tray_graphs]] source` a Performance card key maps to (leader decision, wox.2). Overview,
/// fan and battery cards have none.
pub fn tray_source_of_card(key: &DeviceKey) -> Option<String> {
    match key.kind() {
        DeviceKind::Cpu => Some("cpu".to_owned()),
        DeviceKind::Memory => Some("memory".to_owned()),
        DeviceKind::Disk | DeviceKind::AggregateDisk => Some("disk".to_owned()),
        DeviceKind::Net => Some("network".to_owned()),
        DeviceKind::Gpu => Some(format!("gpu:{}", key.0)),
        DeviceKind::AggregateGpu => Some("gpu".to_owned()),
        DeviceKind::Overview | DeviceKind::Fan | DeviceKind::Battery => None,
    }
}

fn toast(cx: &mut App, kind: ToastKind, key: &str) {
    let text = tr(cx, key);
    push_main_toast(cx, kind, text, None, None);
}

fn add_graph(cx: &mut App, source: SharedString, column: SharedString) {
    let bridge = TrayBridge::global(cx);
    let added = bridge.update(cx, |b, cx| b.add(source, column, cx));
    toast(
        cx,
        if added {
            ToastKind::Success
        } else {
            ToastKind::Warning
        },
        if added {
            "tray-toast-added"
        } else {
            "tray-toast-exists"
        },
    );
}

fn remove_matching(cx: &mut App, source: &str, column: &str) {
    let bridge = TrayBridge::global(cx);
    let id = bridge.read(cx).find(source, column);
    remove_by_id(cx, id);
}

fn remove_by_id(cx: &mut App, id: Option<TrayGraphId>) {
    let Some(id) = id else {
        toast(cx, ToastKind::Error, "tray-toast-remove-failed");
        return;
    };
    TrayBridge::global(cx).update(cx, |b, cx| b.remove(id, cx));
    toast(cx, ToastKind::Success, "tray-toast-removed");
}

/// The main window while it is open; `None` when it was removed to the tray.
pub fn main_window_target(cx: &App) -> Option<WindowHandle<Root>> {
    cx.try_global::<ShellState>()
        .and_then(|s| s.main_window)
        .filter(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

/// Sends a window action to the main window, or to the app when it is gone.
fn dispatch_shell(cx: &mut App, action: Box<dyn Action>) {
    if let Some(handle) = main_window_target(cx) {
        let a = action.boxed_clone();
        if handle
            .update(cx, |_, window, cx| window.dispatch_action(a, cx))
            .is_ok()
        {
            return;
        }
    }
    cx.dispatch_action(&*action);
}

fn handle_event(cx: &mut App, event: TrayEvent) {
    match event {
        TrayEvent::WatcherOnline => cx.global_mut::<TrayStatus>().available = true,
        TrayEvent::WatcherOffline => cx.global_mut::<TrayStatus>().available = false,
        TrayEvent::RegisterFailed(id) => {
            TrayBridge::global(cx).update(cx, |b, cx| b.remove(id, cx));
            toast(cx, ToastKind::Error, "tray-toast-add-failed");
        }
        TrayEvent::ToggleMainWindow => dispatch_shell(cx, Box::new(ToggleMainWindow)),
        TrayEvent::ShowMainWindow => dispatch_shell(cx, Box::new(ShowMainWindow)),
        TrayEvent::ShowMiniWindow => dispatch_shell(cx, Box::new(ToggleMiniWindow)),
        TrayEvent::Exit => dispatch_shell(cx, Box::new(Quit)),
        TrayEvent::SetColor { id, color } => {
            TrayBridge::global(cx).update(cx, |b, cx| b.set_color(id, &color, cx));
        }
        TrayEvent::SetRange { id, secs } => {
            TrayBridge::global(cx).update(cx, |b, cx| b.set_range(id, secs, cx));
        }
        TrayEvent::Remove { id } => {
            let known = TrayBridge::global(cx)
                .read(cx)
                .graphs
                .iter()
                .any(|g| g.id == id);
            remove_by_id(cx, known.then_some(id));
        }
        TrayEvent::TogglePageSource { source } => {
            let found = TrayBridge::global(cx).read(cx).find_source(source);
            match found {
                Some(id) => remove_by_id(cx, Some(id)),
                None => add_graph(cx, source.into(), "cpu".into()),
            }
        }
    }
}

impl TrayBridge {
    fn new(
        end: TrayUiEnd,
        client: Arc<dyn EngineClient>,
        entries: &[TrayGraphEntry],
        cx: &mut Context<Self>,
    ) -> Self {
        let graphs: Vec<TrayGraph> = entries
            .iter()
            .zip(1u32..)
            .map(|(e, n)| TrayGraph {
                id: TrayGraphId(n),
                source: e.source.clone().into(),
                column: e.column.clone().into(),
                color: e.color.clone().into(),
                range_s: u32::from(e.range_s),
                scale: ScaleState::default(),
            })
            .collect();
        let next_id = graphs.len() as u32 + 1;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TRAY_TICK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            end,
            client,
            graphs,
            next_id,
            main_window_hidden: false,
            active: false,
            demand_sent: None,
        }
    }

    /// The app's bridge, set by [`install`].
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTrayBridge>().0.clone()
    }

    /// Drops one graph (E31-A2 calls it for a `Remove` event). The next tick applies it.
    pub fn remove_graph(&mut self, id: TrayGraphId) {
        self.graphs.retain(|g| g.id != id);
    }

    /// The id of the graph with this source and column.
    fn find(&self, source: &str, column: &str) -> Option<TrayGraphId> {
        self.graphs
            .iter()
            .find(|g| &*g.source == source && &*g.column == column)
            .map(|g| g.id)
    }

    /// The id of the first graph with this source.
    fn find_source(&self, source: &str) -> Option<TrayGraphId> {
        self.graphs
            .iter()
            .find(|g| &*g.source == source)
            .map(|g| g.id)
    }

    /// Writes `[[tray_graphs]]` in graph order with all four keys. The persist callback is the
    /// 500 ms debounce (ADR-011).
    fn save(&self, cx: &mut App) {
        let entries: Vec<TrayGraphEntry> = self
            .graphs
            .iter()
            .map(|g| TrayGraphEntry {
                source: g.source.to_string(),
                column: g.column.to_string(),
                color: g.color.to_string(),
                range_s: u16::try_from(g.range_s).unwrap_or(60),
            })
            .collect();
        if cx.has_global::<TlSettings>() {
            TlSettings::update(cx, |d| d.state.tray_graphs = entries);
        }
    }

    /// Appends a graph unless the same source and column exist. True when it was added.
    fn add(&mut self, source: SharedString, column: SharedString, cx: &mut App) -> bool {
        if self.find(&source, &column).is_some() {
            return false;
        }
        let id = self.next_id();
        self.next_id += 1;
        self.graphs.push(TrayGraph {
            id,
            source,
            column,
            color: GraphColor::Accent.as_str().into(),
            range_s: 60,
            scale: ScaleState::default(),
        });
        self.save(cx);
        true
    }

    /// Removes a graph and writes `[[tray_graphs]]`.
    fn remove(&mut self, id: TrayGraphId, cx: &mut App) {
        self.remove_graph(id);
        self.save(cx);
    }

    fn set_color(&mut self, id: TrayGraphId, color: &str, cx: &mut App) {
        let Some(color) = GraphColor::from_str(color) else {
            return;
        };
        if let Some(g) = self.graphs.iter_mut().find(|g| g.id == id) {
            g.color = color.as_str().into();
            self.save(cx);
        }
    }

    fn set_range(&mut self, id: TrayGraphId, secs: u32, cx: &mut App) {
        if !matches!(secs, 30 | 60 | 300 | 600) {
            return;
        }
        if let Some(g) = self.graphs.iter_mut().find(|g| g.id == id) {
            g.range_s = secs;
            self.save(cx);
        }
    }

    /// The graphs in `[[tray_graphs]]` order.
    pub fn graphs(&self) -> &[TrayGraph] {
        &self.graphs
    }

    /// The id the next added graph gets.
    pub fn next_id(&self) -> TrayGraphId {
        TrayGraphId(self.next_id)
    }

    /// Called by the close decision (E31-009) when the main window is removed to the tray (true)
    /// and when it is reopened (false). The next tick applies it.
    pub fn set_main_window_hidden(&mut self, hidden: bool, _cx: &mut Context<Self>) {
        self.main_window_hidden = hidden;
    }

    /// The tray is in use: it has a graph, or the main icon shows.
    fn in_use(&self, always_show: bool) -> bool {
        !self.graphs.is_empty() || self.show_main_icon(always_show)
    }

    /// UI-SHL-113.
    fn show_main_icon(&self, always_show: bool) -> bool {
        self.graphs.is_empty() && (self.main_window_hidden || always_show)
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let (always_show, unit, auto_scale) = if cx.has_global::<TlSettings>() {
            let s = TlSettings::settings(cx);
            (
                s.tray.always_show,
                s.units.network,
                s.units.network_auto_scale,
            )
        } else {
            (false, NetworkUnit::default(), false)
        };
        if !self.in_use(always_show) {
            if self.active {
                self.active = false;
                self.set_demand(None, cx);
                self.send(empty_model());
            }
            return;
        }
        self.active = true;
        let show_main_icon = self.show_main_icon(always_show);
        let snap = self.client.latest();
        let gpu_order = gpu_order(cx);
        let theme = current(cx).clone();
        let mut demand = TrayDemand {
            icons_shown: true,
            ..TrayDemand::default()
        };
        let mut frames = Vec::with_capacity(self.graphs.len());
        let mut process_graphs: Vec<(TrayGraphId, Arc<str>)> = Vec::new();
        let mut page_sources = [false; 7];
        for graph in &mut self.graphs {
            if let Some(i) = TRAY_PAGE_SOURCES
                .iter()
                .position(|(s, _)| *s == &*graph.source)
            {
                page_sources[i] = true;
            }
            let ctx = Ctx {
                snap: &snap,
                gpu_order: &gpu_order,
                theme: &theme,
                unit,
                auto_scale,
            };
            let frame = build_frame(graph, &ctx, &mut demand);
            if is_process_source(&graph.source) {
                process_graphs.push((graph.id, Arc::from(&*frame.tooltip_title)));
            }
            frames.push(frame);
        }
        let main_description = if show_main_icon {
            demand.cpu = true;
            demand.memory = true;
            demand.disks = true;
            demand.nets = true;
            demand.gpus = true;
            main_description(&snap, &gpu_order, unit, auto_scale)
        } else {
            String::new()
        };
        self.set_demand(Some(demand), cx);
        self.send(TrayModel {
            show_main_icon,
            main_tooltip_title: Arc::from(MAIN_TITLE),
            main_tooltip_description: Arc::from(main_description),
            graphs: Arc::from(frames),
            menu: TrayMenuState {
                page_sources,
                process_graphs: Arc::from(process_graphs),
            },
        });
    }

    /// A full channel drops this model; the next tick replaces it.
    fn send(&self, model: TrayModel) {
        let _ = self.end.models.try_send(model);
    }

    /// Gives the aggregator this tray part, or removes it for `None`. Only a change is sent.
    fn set_demand(&mut self, part: Option<TrayDemand>, cx: &mut Context<Self>) {
        if self.demand_sent == part {
            return;
        }
        self.demand_sent = part;
        let Some(agg) = cx.try_global::<GlobalDemand>().map(|g| g.0.clone()) else {
            return;
        };
        let id = cx.entity_id();
        agg.update(cx, |agg: &mut DemandAggregator, cx| match part {
            Some(tray) => agg.set(
                id,
                None,
                Demand {
                    tray,
                    ..Demand::default()
                },
                cx,
            ),
            None => agg.remove(id, cx),
        });
    }
}

fn empty_model() -> TrayModel {
    TrayModel {
        show_main_icon: false,
        main_tooltip_title: Arc::from(MAIN_TITLE),
        main_tooltip_description: Arc::from(""),
        graphs: Arc::from([]),
        menu: TrayMenuState::default(),
    }
}

fn is_process_source(source: &str) -> bool {
    ["process:", "name:", "app:"]
        .iter()
        .any(|p| source.starts_with(p))
}

/// What a tick reads besides the graph.
struct Ctx<'a> {
    snap: &'a Snapshot,
    gpu_order: &'a [DeviceKey],
    theme: &'a crate::theme::TlTheme,
    unit: NetworkUnit,
    auto_scale: bool,
}

/// The GPU keys of `performance.device_order`, for a source with no GPU key (R-085).
fn gpu_order(cx: &App) -> Vec<DeviceKey> {
    if !cx.has_global::<TlSettings>() {
        return Vec::new();
    }
    TlSettings::settings(cx)
        .performance
        .device_order
        .iter()
        .map(|k| DeviceKey(Arc::from(k.as_str())))
        .filter(|k| k.kind() == DeviceKind::Gpu)
        .collect()
}

/// The series metric a page source draws.
fn metric_of(source: &MinigraphSource) -> Option<MetricId> {
    Some(match source {
        MinigraphSource::Cpu => MetricId::CPU_TOTAL,
        MinigraphSource::Memory => MetricId::MEM_USED,
        MinigraphSource::Disk => MetricId::DISK_ACTIVE,
        MinigraphSource::Network => MetricId::NET_RX,
        MinigraphSource::Gpu(_) => MetricId::GPU_UTIL,
        MinigraphSource::Vram(_) => MetricId::GPU_MEM,
        MinigraphSource::GpuTemp(_) => MetricId::GPU_TEMP,
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            return None;
        }
    })
}

/// The collector that feeds a page source is in the Failed state.
fn collector_failed(snap: &Snapshot, source: &MinigraphSource) -> bool {
    let failed = |id: SourceId| snap.health.sources[id.index()].state == SourceState::Failed;
    match source {
        MinigraphSource::Cpu => failed(SourceId::Cpu),
        MinigraphSource::Memory => failed(SourceId::Memory),
        MinigraphSource::Disk => failed(SourceId::Disk),
        MinigraphSource::Network => failed(SourceId::Net),
        MinigraphSource::Gpu(_) | MinigraphSource::Vram(_) | MinigraphSource::GpuTemp(_) => {
            failed(SourceId::GpuNvidia) || failed(SourceId::GpuDrm)
        }
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            false
        }
    }
}

/// UI-PRF-051 title of a page source.
fn page_title(source: &MinigraphSource) -> &'static str {
    match source {
        MinigraphSource::Cpu => "CPU Usage",
        MinigraphSource::Memory => "Memory Usage",
        MinigraphSource::Disk => "Disk Usage",
        MinigraphSource::Network => "Network Usage",
        MinigraphSource::Gpu(_) => "GPU Usage",
        MinigraphSource::Vram(_) => "VRAM Usage",
        MinigraphSource::GpuTemp(_) => "GPU temp",
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => "",
    }
}

fn newest(snap: &Snapshot, id: SeriesId) -> Option<f32> {
    snap.series
        .get(id)?
        .samples
        .iter()
        .rev()
        .find(|s| s.is_valid())
        .map(|s| s.value)
}

fn percent_text(v: Option<f32>) -> String {
    v.map_or_else(
        || WARMING_UP_TEXT.to_owned(),
        |v| {
            let mut s = String::new();
            units::percent(&mut s, v);
            s
        },
    )
}

fn net_text(rate: Option<f32>, unit: NetworkUnit, auto_scale: bool) -> String {
    rate.map_or_else(
        || WARMING_UP_TEXT.to_owned(),
        |v| {
            let unit = match unit {
                NetworkUnit::Bits => units::NetUnit::Bits,
                NetworkUnit::Bytes => units::NetUnit::Bytes,
            };
            let mut s = String::new();
            units::net_rate(&mut s, f64::from(v), unit, auto_scale);
            s
        },
    )
}

/// The newest valid total rate, rx plus tx; `None` while neither has a valid sample.
fn net_total(snap: &Snapshot, dev: DeviceIdx) -> Option<f32> {
    let rx = newest(
        snap,
        SeriesId {
            device: dev,
            metric: MetricId::NET_RX,
        },
    );
    let tx = newest(
        snap,
        SeriesId {
            device: dev,
            metric: MetricId::NET_TX,
        },
    );
    match (rx, tx) {
        (None, None) => None,
        (rx, tx) => Some(rx.unwrap_or(0.0) + tx.unwrap_or(0.0)),
    }
}

fn mem_percent(snap: &Snapshot, dev: DeviceIdx) -> Option<f32> {
    let total = snap.system.memory.total;
    let used = newest(
        snap,
        SeriesId {
            device: dev,
            metric: MetricId::MEM_USED,
        },
    )?;
    (total > 0).then(|| (f64::from(used) / total as f64 * 100.0) as f32)
}

/// The tooltip description of a live page source.
fn page_description(source: &MinigraphSource, dev: DeviceIdx, ctx: &Ctx<'_>) -> String {
    let snap = ctx.snap;
    let at = |metric| {
        newest(
            snap,
            SeriesId {
                device: dev,
                metric,
            },
        )
    };
    match source {
        MinigraphSource::Cpu => percent_text(at(MetricId::CPU_TOTAL)),
        MinigraphSource::Memory => percent_text(mem_percent(snap, dev)),
        MinigraphSource::Disk => percent_text(at(MetricId::DISK_ACTIVE)),
        MinigraphSource::Network => net_text(net_total(snap, dev), ctx.unit, ctx.auto_scale),
        MinigraphSource::Gpu(_) => percent_text(at(MetricId::GPU_UTIL)),
        MinigraphSource::Vram(_) => at(MetricId::GPU_MEM).map_or_else(
            || WARMING_UP_TEXT.to_owned(),
            |v| {
                let mut s = String::new();
                format_value(ValueFormat::Bytes, v, ctx.unit, &mut s);
                s
            },
        ),
        MinigraphSource::GpuTemp(_) => at(MetricId::GPU_TEMP).map_or_else(
            || WARMING_UP_TEXT.to_owned(),
            |v| {
                let mut s = String::new();
                units::celsius(&mut s, v);
                s
            },
        ),
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            WARMING_UP_TEXT.to_owned()
        }
    }
}

/// The value format the scale rule writes its axis text with.
fn scale_format(source: &MinigraphSource, total: u64) -> ValueFormat {
    match source {
        MinigraphSource::Memory => ValueFormat::PercentOf { total },
        MinigraphSource::Network => ValueFormat::RateNetwork,
        MinigraphSource::Vram(_) => ValueFormat::Bytes,
        MinigraphSource::GpuTemp(_) => ValueFormat::Celsius,
        _ => ValueFormat::Percent,
    }
}

/// Builds the frame of one graph and adds what it needs to `demand`.
fn build_frame(graph: &mut TrayGraph, ctx: &Ctx<'_>, demand: &mut TrayDemand) -> TrayGraphFrame {
    let snap = ctx.snap;
    let range_s = graph.range_s;
    let color = GraphColor::from_str(&graph.color).unwrap_or(GraphColor::Accent);
    let mut frame = TrayGraphFrame {
        id: graph.id,
        series: None,
        now_ns: snap.published_ns,
        range_ns: u64::from(range_s) * 1_000_000_000,
        max: 100.0,
        line_rgb: rgb_of(ctx.theme.accent),
        state: TrayGraphState::Live,
        tooltip_title: Arc::from(""),
        tooltip_description: Arc::from(WARMING_UP_TEXT),
        color: Arc::from(&*graph.color),
        range_s,
    };
    let Some(source) = MinigraphSource::parse(&graph.source) else {
        frame.state = TrayGraphState::Failed;
        frame.tooltip_title = Arc::from(&*graph.source);
        return frame;
    };
    let Some(metric) = metric_of(&source) else {
        // Process, name and app graphs are wox.16: warming-up frame, no series.
        let title = match &source {
            MinigraphSource::Process { key } => format!("Process {}", key.pid),
            MinigraphSource::Name(image) => image.to_string(),
            MinigraphSource::App(id) => id.to_string(),
            _ => String::new(),
        };
        frame.tooltip_title = Arc::from(title.as_str());
        return frame;
    };
    match source {
        MinigraphSource::Cpu => demand.cpu = true,
        MinigraphSource::Memory => demand.memory = true,
        MinigraphSource::Disk => demand.disks = true,
        MinigraphSource::Network => demand.nets = true,
        _ => demand.gpus = true,
    }
    frame.line_rgb = rgb_of(resolve_colors(&source, color, ctx.theme).1);
    let title = page_title(&source);
    frame.tooltip_title = Arc::from(title);
    if collector_failed(snap, &source) {
        frame.state = TrayGraphState::Failed;
        return frame;
    }
    let Some(dev) = device_for(&source, &snap.devices, ctx.gpu_order) else {
        // The hardware is not there (no GPU).
        frame.tooltip_description = Arc::from(UNSUPPORTED_TEXT);
        return frame;
    };
    let id = SeriesId {
        device: dev,
        metric,
    };
    let (view, _) = find_tiered(&snap.series, id, frame.range_ns);
    frame.series = view.cloned();
    frame.tooltip_description = Arc::from(page_description(&source, dev, ctx).as_str());

    // The scale ceiling: the rule of the matching card, with this graph's own memory.
    let spec = chart_spec_for(
        &source,
        dev,
        &snap.system,
        GraphColor::Accent,
        range_s.min(u32::from(u16::MAX)) as u16,
        false,
        ctx.theme,
    );
    let (vmin, vmax) = frame.series.as_deref().map_or((0.0, 0.0), |v| {
        visible_range(v, frame.now_ns, frame.range_ns)
    });
    let mut text = String::new();
    let result = scale::compute(
        &spec.y,
        vmin,
        vmax,
        frame.now_ns,
        &mut graph.scale,
        scale_format(&source, snap.system.memory.total),
        ctx.unit,
        &mut text,
    );
    frame.max = match spec.y {
        YScale::Fixed { max, .. } => max,
        _ => result.y_max,
    };
    frame
}

/// Min and max of the valid samples inside the window; (0, 0) when there are none.
fn visible_range(view: &tl_model::series::SeriesView, now_ns: TimeNs, range_ns: u64) -> (f32, f32) {
    let from = now_ns.saturating_sub(range_ns);
    let mut it = view
        .samples
        .iter()
        .filter(|s| s.is_valid() && s.t_ns >= from)
        .map(|s| s.value);
    let Some(first) = it.next() else {
        return (0.0, 0.0);
    };
    it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

fn rgb_of(color: gpui_kit::Hsla) -> [u8; 3] {
    let c = color.to_rgb();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(c.r), byte(c.g), byte(c.b)]
}

/// The main icon's five-line description (PERF-089).
fn main_description(
    snap: &Snapshot,
    gpu_order: &[DeviceKey],
    unit: NetworkUnit,
    auto_scale: bool,
) -> String {
    use tl_model::devices::{AggregateKind, DeviceIdentity};
    let fixed = |identity: DeviceIdentity| snap.devices.find(&identity).map(|e| e.idx);
    let at = |dev: Option<DeviceIdx>, metric| {
        dev.and_then(|device| newest(snap, SeriesId { device, metric }))
    };
    let cpu = fixed(DeviceIdentity::CpuPackage);
    let mem = fixed(DeviceIdentity::Memory);
    let disk = fixed(DeviceIdentity::Aggregate(AggregateKind::Disks));
    let net = fixed(DeviceIdentity::Aggregate(AggregateKind::Nets));
    let gpu = device_for(&MinigraphSource::Gpu(None), &snap.devices, gpu_order);
    let gpu_text = if gpu.is_some() {
        percent_text(at(gpu, MetricId::GPU_UTIL))
    } else {
        UNSUPPORTED_TEXT.to_owned()
    };
    format!(
        "CPU: {}\nRAM: {}\nGPU: {}\nDisk: {}\nNetwork: {}",
        percent_text(at(cpu, MetricId::CPU_TOTAL)),
        percent_text(mem.and_then(|d| mem_percent(snap, d))),
        gpu_text,
        percent_text(at(disk, MetricId::DISK_ACTIVE)),
        net_text(net.and_then(|d| net_total(snap, d)), unit, auto_scale),
    )
}
