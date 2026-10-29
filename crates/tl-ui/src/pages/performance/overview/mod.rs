//! The Performance tab's Overview page (UI-PRF-017, D-022): five btop-style boxes (CPU, MEM,
//! GPU, DISK, NET) in the tasklens theme, laid out from data so a later editor can change them.
//!
//! [`layout`] holds the layout model, placement and device accent family. [`parts`] holds the
//! meter, core bars and the shared accent frame with its inset title line. One module per box
//! owns its view. [`OverviewPage`] places the box views and sends one demand part while it is
//! shown.
//!
//! The layout comes from `[performance.overview]` (UI-PRF-056), the default when there is none.
//! [`procs_box`] is the opt-in Top processes box: it is built only while the layout places it,
//! and only then does the page ask for the process sweep (`process_top`). The boxes sit on the
//! snap grid of UI-PRF-056. [`splitter`] moves the shared edge of two boxes by dragging the gap
//! between them; [`edit`] is Edit Layout mode, which moves, resizes, hides and adds boxes. Each
//! box has layout
//! modes (UI-PRF-019): [`box_menu`] is the box menu and the Edit Layout gear, and
//! [`inner_split`] is the one splitter a mode with two parts uses. The gaps and the inner
//! splitters resize only in Edit Layout (D-033).
//!
//! [`range`] is the time range (UI-PRF-055): the switch in the header, the demand for the
//! device metric history, and the min, avg and peak line and time axis start of each box, which
//! the page works out from the snapshot and hands to the boxes.
//!
//! [`timeline`] is the 24-hour timeline box (UI-PRF-057). A click on it can hold the page at a
//! time; while held, the page asks for every range at that time and its header shows "Back to
//! live", and every box reads [`picked`]'s snapshot at that time instead of the live one.
//!
//! Render contract (D-022, crw.34): the page never notifies on a sample. Each box is a cached
//! child view that notifies only when its own text or bars change. The page re-renders on a
//! resize, when a GPU or disk card comes or goes, and (from GPUI's ancestor marking) when a box
//! notifies; its render reads no snapshot and formats nothing.

pub mod box_menu;
pub mod cpu_box;
pub mod disk_box;
pub mod edit;
pub mod fit;
pub mod gpu_box;
pub mod inner_split;
pub mod layout;
pub mod mem_box;
pub mod net_box;
pub mod parts;
pub mod picked;
pub mod procs_box;
pub mod range;
pub mod splitter;
pub mod timeline;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, SharedString, Size,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Task, Window,
    canvas, div, px, size,
};
use smallvec::SmallVec;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{OverviewRange, StatusBarPlacement};
use tl_model::snapshot::ComponentMask;

use self::cpu_box::{CpuBox, PowerSplitChanged};
use self::disk_box::DiskBox;
use self::edit::EditState;
use self::gpu_box::GpuBox;
use self::inner_split::{InnerSplitChanged, hit_selector};
use self::layout::{BoxKind, GridBox, OverviewLayout, Placement, ROW_UNIT_DEFAULT, place_rows};
use self::mem_box::MemBox;
use self::net_box::NetBox;
use self::procs_box::ProcsBox;
use self::range::{RangeText, history_request};
use self::splitter::{Drag, Splitter, Target, set_span, splitters};
use self::timeline::{OverviewPause, Paused, TimelineBox};
use super::actions::{
    PERFORMANCE_OVERVIEW_EDIT_CONTEXT, PerfOverviewEditLayout, PerfOverviewSetTimelineClick,
    PerfSetStatusBar,
};
use super::details::{KeyValueModel, Template};
use super::page::DevicePage;
use super::sidebar::DeviceSidebar;
use super::status_bar::{self, EdgesChanged, StatusBar, state_color};
use super::{DeviceKey, DeviceKind};
use crate::chart::{NetFormat, ValueFormat, format_value};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::{TlIcon, icon};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::tr;
use crate::settings_global::TlSettings;
use crate::theme::{current, metrics};

/// The Overview page's key.
pub const OVERVIEW_KEY: &str = "overview";

/// The prefix of every box's debug selector (`perf-overview-box-cpu`).
pub const BOX_ID_PREFIX: &str = "perf-overview-box-";

/// The scroll container's id when the boxes do not fit.
const SCROLL_ID: &str = "perf-overview-scroll";

const KEY_TITLE: &str = "perf-overview-title";

/// The debug selector of the Overview header row that holds the range switch.
pub const RANGE_HEADER_ID: &str = "perf-overview-range-header";

/// The debug selector of the layout-grid icon before the Overview page title.
pub const TITLE_ICON_ID: &str = "perf-overview-title-icon";

/// How long the pointer rests on a gap before its accent line shows.
pub const SPLITTER_HOVER_DELAY: Duration = Duration::from_millis(150);

/// The accent line in a hovered gap, logical px.
const SPLITTER_LINE: f32 = 2.;

/// The debug selector of the accent line of the hovered or dragged gap.
pub const SPLITTER_LINE_ID: &str = "perf-overview-splitter-line";

/// The height of a box's state edge (UI-PRF-054), logical px × ui_scale.
pub const EDGE_H: f32 = 2.;

/// The debug selector of a box's state edge: `perf-overview-edge-cpu`.
pub fn edge_id(kind: BoxKind) -> String {
    format!("perf-overview-edge-{}", kind.key())
}

/// The "Status bar" submenu items: the value and the `en-US.ftl` key of its word.
const STATUS_BAR_ITEMS: [(StatusBarPlacement, &str); 4] = [
    (
        StatusBarPlacement::Overview,
        "perf-overview-status-bar-overview",
    ),
    (
        StatusBarPlacement::Performance,
        "perf-overview-status-bar-performance",
    ),
    (
        StatusBarPlacement::AllTabs,
        "perf-overview-status-bar-all-tabs",
    ),
    (StatusBarPlacement::Off, "perf-overview-status-bar-off"),
];

/// A gap drag in progress. `changed` is set once a move changed a weight, so a click that
/// moved nothing saves nothing.
struct ActiveDrag {
    drag: Drag,
    changed: bool,
    /// The boxes when the button went down, for [`OverviewPage::end_drag`].
    start: SmallVec<[GridBox; 8]>,
}

/// (key, device, heading) of each live GPU or disk card, in sidebar order.
type CardList = SmallVec<[(DeviceKey, DeviceIdx, SharedString); 8]>;

/// The live cards of `kind` in `sidebar`.
fn cards_of(sidebar: &Entity<DeviceSidebar>, kind: DeviceKind, cx: &App) -> CardList {
    sidebar
        .read(cx)
        .cards()
        .iter()
        .filter(|c| !c.is_removing() && c.row.kind == kind)
        .map(|c| (c.row.key.clone(), c.row.idx, c.lines.heading.clone()))
        .collect()
}

/// The key of the first live card of `kind` in `sidebar`.
fn first_card(sidebar: &Entity<DeviceSidebar>, kind: DeviceKind, cx: &App) -> Option<DeviceKey> {
    sidebar
        .read(cx)
        .cards()
        .iter()
        .find(|c| !c.is_removing() && c.row.kind == kind)
        .map(|c| c.row.key.clone())
}

/// The saved layout (`[performance.overview]`); the default before settings load or when none
/// is saved.
fn saved_layout(cx: &App) -> OverviewLayout {
    let saved = cx
        .has_global::<TlSettings>()
        .then(|| TlSettings::settings(cx).performance.overview.as_ref())
        .flatten();
    OverviewLayout::from_setting(saved)
}

/// `performance.overview_row_unit_px`: the grid row height in logical px.
fn row_unit(cx: &App) -> f32 {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).performance.overview_row_unit_px as f32
    } else {
        ROW_UNIT_DEFAULT
    }
}

/// The Overview page.
pub struct OverviewPage {
    key: DeviceKey,
    layout: OverviewLayout,
    placement: Placement,
    area: Size<Pixels>,
    /// `performance.overview_row_unit_px`, logical px.
    row_unit: f32,
    cpu: Entity<CpuBox>,
    mem: Entity<MemBox>,
    gpu: Entity<GpuBox>,
    disk: Entity<DiskBox>,
    net: Entity<NetBox>,
    /// The Top processes box, built only while the layout places it.
    procs: Option<Entity<ProcsBox>>,
    /// The 24-hour timeline box (UI-PRF-057).
    timeline: Entity<TimelineBox>,
    /// The time the page is held at, and CLOCK_REALTIME when it was set, so the request end
    /// stays the same until the pause or the range changes (UI-PRF-057).
    paused: Option<(Paused, i64)>,
    sidebar: Entity<DeviceSidebar>,
    gpu_keys: SmallVec<[DeviceKey; 4]>,
    disk_keys: SmallVec<[DeviceKey; 8]>,
    /// The GPU and disk cards the boxes were last given.
    gpu_cards: CardList,
    disk_cards: CardList,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    active: bool,
    header: (SharedString, SharedString),
    details: KeyValueModel,
    focus: FocusHandle,
    /// The gap being dragged. The layout is saved when the drag ends, never during it.
    drag: Option<ActiveDrag>,
    /// The gap under the pointer, and whether its accent line shows yet.
    hover: Option<Target>,
    hover_shown: bool,
    hover_task: Option<Task<()>>,
    /// Edit Layout mode (UI-PRF-018), while it is on.
    edit: Option<EditState>,
    /// The box area's origin in window px from the last paint, for the box drag.
    content_origin: Rc<Cell<Point<Pixels>>>,
    /// The machine status, for each box's state edge (UI-PRF-054).
    status: Entity<StatusBar>,
    /// `performance.overview_range` (UI-PRF-055).
    range: OverviewRange,
    /// `performance.status_bar` is `overview`: the bar holds the range switch, so the page
    /// draws no header row.
    bar_on_overview: bool,
    /// `history.device_metrics`: with it off the range switches disable 24 h and 7 d.
    history_kept: bool,
    /// The words of the range text: "min { $min }  avg { $avg }  peak { $peak }", "since
    /// { $time }" and the time axis start of each range.
    figures_words: Template,
    since_words: Template,
    ago_words: [SharedString; 5],
    _subs: SmallVec<[Subscription; 4]>,
    /// Window focus loss ends a drag, for the window of the last render.
    _activation: Option<Subscription>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl OverviewPage {
    pub fn new(
        key: DeviceKey,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        sidebar: &Entity<DeviceSidebar>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snap = std::sync::Arc::clone(hub.read(cx).current());
        let find = |identity: tl_model::devices::DeviceIdentity| {
            snap.devices
                .find(&identity)
                .map_or(DeviceIdx(u16::MAX), |e| e.idx)
        };
        let cpu_idx = find(tl_model::devices::DeviceIdentity::CpuPackage);
        let mem_idx = find(tl_model::devices::DeviceIdentity::Memory);
        let gpu_cards = cards_of(sidebar, DeviceKind::Gpu, cx);
        let disk_cards = cards_of(sidebar, DeviceKind::Disk, cx);
        let cpu = cx.new(|cx| CpuBox::new(&hub, cpu_idx, cx));
        let layout = saved_layout(cx);
        let cpu_mode = layout.config(BoxKind::Cpu).mode.id;
        cpu.update(cx, |b, cx| b.set_mode(cpu_mode, cx));
        if let Some(split) = layout.split(BoxKind::Cpu) {
            cpu.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = layout.arrange(BoxKind::Cpu) {
            cpu.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        let power = layout.config(BoxKind::Cpu);
        cpu.update(cx, |b, cx| b.set_power(power.power, power.power_split, cx));
        let mem = cx.new(|cx| MemBox::new(&hub, mem_idx, cx));
        let mem_mode = layout.config(BoxKind::Memory).mode.id;
        mem.update(cx, |b, cx| b.set_mode(mem_mode, cx));
        if let Some(split) = layout.split(BoxKind::Memory) {
            mem.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = layout.arrange(BoxKind::Memory) {
            mem.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        let gpu = cx.new(|cx| GpuBox::new(&hub, &gpu_cards, cx));
        let gpu_mode = layout.config(BoxKind::Gpu).mode.id;
        gpu.update(cx, |b, cx| b.set_mode(gpu_mode, cx));
        if let Some(split) = layout.split(BoxKind::Gpu) {
            gpu.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = layout.arrange(BoxKind::Gpu) {
            gpu.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        gpu.update(cx, |b, cx| b.set_hidden(layout.hidden_gpu(), cx));
        let disk = cx.new(|cx| DiskBox::new(&hub, &disk_cards, cx));
        let disk_mode = layout.config(BoxKind::Disk).mode.id;
        disk.update(cx, |b, cx| b.set_mode(disk_mode, cx));
        if let Some(split) = layout.split(BoxKind::Disk) {
            disk.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = layout.arrange(BoxKind::Disk) {
            disk.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        let net_target = first_card(sidebar, DeviceKind::Net, cx);
        let net = cx.new(|cx| {
            let mut b = NetBox::new(&hub, cx);
            b.set_target(net_target, cx);
            b
        });
        let net_mode = layout.config(BoxKind::Net).mode.id;
        net.update(cx, |b, cx| b.set_mode(net_mode, cx));
        if let Some(split) = layout.split(BoxKind::Net) {
            net.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = layout.arrange(BoxKind::Net) {
            net.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }

        let mut subs = SmallVec::new();
        subs.push(cx.observe(sidebar, |this, _, cx| this.sync_cards(cx)));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_layout));
        subs.push(cx.subscribe(&cpu, Self::on_cpu_split));
        subs.push(cx.subscribe(&cpu, Self::on_cpu_power_split));
        subs.push(cx.subscribe(&mem, Self::on_mem_split));
        subs.push(cx.subscribe(&gpu, Self::on_gpu_split));
        subs.push(cx.subscribe(&disk, Self::on_disk_split));
        subs.push(cx.subscribe(&net, Self::on_net_split));
        let status = status_bar::status_bar(&hub, &demand, cx);
        subs.push(cx.subscribe(&status, |this, _, _: &EdgesChanged, cx| this.notify(cx)));
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        let procs = layout
            .has(BoxKind::TopProcs)
            .then(|| cx.new(|cx| ProcsBox::new(&hub, cx)));
        let timeline = cx.new(|cx| TimelineBox::new(&hub, cx));
        subs.push(cx.observe_global::<OverviewPause>(Self::on_pause));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            layout,
            placement: Placement::default(),
            row_unit: row_unit(cx),
            area: size(px(0.), px(0.)),
            cpu,
            mem,
            gpu,
            disk,
            net,
            procs,
            timeline,
            paused: timeline::paused(cx).map(|p| (p, timeline::now_realtime_ns())),
            sidebar: sidebar.clone(),
            gpu_keys: gpu_cards.iter().map(|c| c.0.clone()).collect(),
            disk_keys: disk_cards.iter().map(|c| c.0.clone()).collect(),
            gpu_cards,
            disk_cards,
            hub,
            demand,
            id,
            window: None,
            active: false,
            header: (tr(cx, KEY_TITLE), SharedString::default()),
            details: KeyValueModel::new(&[]),
            focus: cx.focus_handle(),
            drag: None,
            hover: None,
            hover_shown: false,
            hover_task: None,
            edit: None,
            content_origin: Rc::default(),
            status,
            range: range::current_range(cx),
            bar_on_overview: status_bar::placement(cx) == StatusBarPlacement::Overview,
            history_kept: range::history_kept(cx),
            figures_words: Template::new(cx, "perf-overview-figures", &["min", "avg", "peak"]),
            since_words: Template::new(cx, "perf-overview-since", &["time"]),
            ago_words: range::ago_words(cx),
            _subs: subs,
            _activation: None,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        // The boxes start with their range text, so the first sample notifies only its box.
        this.sync_range(cx);
        this
    }

    /// Everything the boxes draw, CPU per core at 10 Hz, the process sweep while the Top
    /// processes box is placed or the Memory box shows "By app" (its groups are summed on each
    /// sweep, UI-PRF-060), the device metric history for 1 h, 24 h and 7 d and for every range
    /// while held at a time, and the timeline while its box is placed.
    pub fn demand_part(&self) -> Demand {
        let (paused, at) = self.paused.unzip();
        Demand {
            process_top: self.procs.is_some() || mem_apps(&self.layout),
            device_history_range: history_request(self.range, paused, at.unwrap_or(0)),
            device_timeline: self.layout.has(BoxKind::Timeline),
            any_surface_visible: true,
            cpu: CpuDemand::PerCore,
            cpu_fast: true,
            memory: true,
            disks: true,
            nets: true,
            gpus: true,
            ..Demand::default()
        }
    }

    pub fn placement(&self) -> &Placement {
        &self.placement
    }

    pub fn layout(&self) -> &OverviewLayout {
        &self.layout
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The range every box draws (UI-PRF-055).
    pub fn range(&self) -> OverviewRange {
        self.range
    }

    pub fn cpu_box(&self) -> &Entity<CpuBox> {
        &self.cpu
    }

    pub fn mem_box(&self) -> &Entity<MemBox> {
        &self.mem
    }

    pub fn gpu_box(&self) -> &Entity<GpuBox> {
        &self.gpu
    }

    pub fn disk_box(&self) -> &Entity<DiskBox> {
        &self.disk
    }

    pub fn net_box(&self) -> &Entity<NetBox> {
        &self.net
    }

    /// The Top processes box while the layout places it.
    pub fn procs_box(&self) -> Option<&Entity<ProcsBox>> {
        self.procs.as_ref()
    }

    /// The 24-hour timeline box.
    pub fn timeline_box(&self) -> &Entity<TimelineBox> {
        &self.timeline
    }

    /// The time the page is held at, `None` while live (UI-PRF-057).
    pub fn paused(&self) -> Option<Paused> {
        self.paused.map(|(p, _)| p)
    }

    /// The hub the boxes read.
    pub fn hub(&self) -> &Entity<SnapshotHub> {
        &self.hub
    }

    /// The colour of `kind`'s state edge, `None` for a Calm box (UI-PRF-054).
    pub fn edge_color(&self, kind: BoxKind, cx: &App) -> Option<gpui_kit::Hsla> {
        state_color(self.status.read(cx).box_state(kind), current(cx))
    }

    fn present(&self, kind: BoxKind) -> bool {
        match kind {
            BoxKind::Gpu => !self.gpu_keys.is_empty(),
            BoxKind::Disk => !self.disk_keys.is_empty(),
            BoxKind::TopProcs => self.procs.is_some(),
            _ => true,
        }
    }

    fn relayout(&mut self, cx: &App) {
        let s = metrics(cx).ui_scale;
        let (gpu, disk, procs) = (
            !self.gpu_keys.is_empty(),
            !self.disk_keys.is_empty(),
            self.procs.is_some(),
        );
        self.placement = place_rows(
            &self.layout,
            |k| match k {
                BoxKind::Gpu => gpu,
                BoxKind::Disk => disk,
                BoxKind::TopProcs => procs,
                _ => true,
            },
            self.area,
            s,
            self.row_unit,
            self.edit.as_ref().map(|e| e.row),
        );
    }

    /// A new page size: new placement and one render. The same size does nothing.
    fn resize(&mut self, area: Size<Pixels>, cx: &mut Context<Self>) {
        if area == self.area {
            return;
        }
        self.area = area;
        self.relayout(cx);
        self.notify(cx);
    }

    /// The sidebar changed: the boxes take new GPU and disk lists when they differ; the page
    /// lays out again only when a box came or went.
    fn sync_cards(&mut self, cx: &mut Context<Self>) {
        let gpus = cards_of(&self.sidebar, DeviceKind::Gpu, cx);
        let disks = cards_of(&self.sidebar, DeviceKind::Disk, cx);
        let before = (self.present(BoxKind::Gpu), self.present(BoxKind::Disk));
        if gpus != self.gpu_cards {
            self.gpu_keys = gpus.iter().map(|c| c.0.clone()).collect();
            self.gpu.update(cx, |b, cx| b.set_gpus(&gpus, cx));
            self.gpu_cards = gpus;
        }
        if disks != self.disk_cards {
            self.disk_keys = disks.iter().map(|c| c.0.clone()).collect();
            self.disk.update(cx, |b, cx| b.set_disks(&disks, cx));
            self.disk_cards = disks;
        }
        let net = first_card(&self.sidebar, DeviceKind::Net, cx);
        self.net.update(cx, |b, cx| b.set_target(net, cx));
        if (self.present(BoxKind::Gpu), self.present(BoxKind::Disk)) != before {
            self.relayout(cx);
            self.notify(cx);
        }
    }

    /// `[performance.overview]` changed: a new placement and one render, and the Top processes
    /// box with its demand comes or goes. A change of `history.device_metrics` redraws the range
    /// switches. Any other settings change does nothing here.
    fn sync_layout(&mut self, cx: &mut Context<Self>) {
        let range = range::current_range(cx);
        if range != self.range {
            // The boxes re-spec their charts from the same settings change.
            self.range = range;
            if let Some((_, at)) = &mut self.paused {
                *at = timeline::now_realtime_ns();
            }
            self.send_demand(cx);
            self.sync_range(cx);
            self.notify(cx);
        }
        let bar_on_overview = status_bar::placement(cx) == StatusBarPlacement::Overview;
        if bar_on_overview != self.bar_on_overview {
            self.bar_on_overview = bar_on_overview;
            self.notify(cx);
        }
        let history_kept = range::history_kept(cx);
        if history_kept != self.history_kept {
            // Both range switches draw 24 h and 7 d enabled or disabled.
            self.history_kept = history_kept;
            self.status.update(cx, |_, cx| cx.notify());
            self.notify(cx);
        }
        if self.drag.is_some() || self.box_drag().is_some() {
            return;
        }
        let unit = row_unit(cx);
        let layout = saved_layout(cx);
        if layout != self.layout {
            self.row_unit = unit;
            self.set_layout(layout, cx);
        } else if unit != self.row_unit {
            self.row_unit = unit;
            self.relayout(cx);
            self.notify(cx);
        }
    }

    /// The timeline held the page at a time or made it live: a new history request, the
    /// header's "Back to live" and the range switches' disabled ranges follow.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let paused = timeline::paused(cx);
        if paused == self.paused() {
            return;
        }
        self.paused = paused.map(|p| (p, timeline::now_realtime_ns()));
        self.send_demand(cx);
        self.sync_range(cx);
        self.status.update(cx, |_, cx| cx.notify());
        self.notify(cx);
    }

    /// `PerfOverviewSetTimelineClick`: the timeline menu's "Click" items (UI-PRF-057). An unknown
    /// value, and `history` while recording is off, do nothing.
    fn on_set_timeline_click(
        &mut self,
        a: &PerfOverviewSetTimelineClick,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(click) = timeline::click_from_key(&a.click) else {
            return;
        };
        if !timeline::click_choices(cx).contains(&click) {
            return;
        }
        TlSettings::update(cx, |d| d.settings.performance.timeline_click = click);
    }

    /// A new snapshot: the boxes' range text follows the series and the device history. The
    /// page itself does not notify.
    fn on_snapshot(
        &mut self,
        _: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let wanted = ComponentMask::SERIES
            | ComponentMask::DEVICE_HISTORY
            | ComponentMask::DEVICES
            | ComponentMask::SYSTEM;
        if self.active && ev.mask.intersects(wanted) {
            self.sync_range(cx);
        }
    }

    /// Works out each device box's min, avg and peak line and the time axis start for the range,
    /// and hands them to the boxes, which notify only when theirs changed (UI-PRF-055). The main
    /// metric is CPU total, memory in use, the first GPU's utilization, the active time of the
    /// busiest disk and the total throughput of the engine's aggregate of the counted interfaces,
    /// the set the Network box charts.
    fn sync_range(&mut self, cx: &mut Context<Self>) {
        let snap = std::sync::Arc::clone(self.hub.read(cx).current());
        let range = self.range;
        let now = timeline::now_realtime_ns();
        let axis_start = range::axis_start(
            range,
            &snap.device_history,
            now,
            &self.ago_words,
            &self.since_words,
        );
        let id = |device, metric| SeriesId { device, metric };
        let idx = |identity| snap.devices.find(&identity).map(|e| e.idx);
        // Held at a time, the figures wait for the history's answer and never read live series.
        let waiting = self.paused.is_some()
            && !snap
                .device_history
                .request
                .is_some_and(|r| r.end_realtime_ns.is_some());
        let figures = |ids: &[SeriesId]| {
            if waiting {
                None
            } else {
                range::figures(&snap, ids, range)
            }
        };
        let words = &self.figures_words;
        let text = |figures| RangeText {
            figures,
            axis_start: axis_start.clone(),
        };
        let pct = |f| range::figures_text(words, f, range::whole_percent);
        let cpu =
            idx(DeviceIdentity::CpuPackage).and_then(|d| figures(&[id(d, MetricId::CPU_TOTAL)]));
        let total = snap.system.memory.total.max(1) as f32;
        let mem = idx(DeviceIdentity::Memory)
            .and_then(|d| figures(&[id(d, MetricId::MEM_USED)]))
            .map(|f| range::Figures {
                min: f.min / total * 100.,
                avg: f.avg / total * 100.,
                peak: f.peak / total * 100.,
            });
        let gpu = self
            .gpu_cards
            .first()
            .and_then(|c| figures(&[id(c.1, MetricId::GPU_UTIL)]));
        let disks: SmallVec<[SeriesId; 8]> = self
            .disk_cards
            .iter()
            .map(|c| id(c.1, MetricId::DISK_ACTIVE))
            .collect();
        let disk = if waiting {
            None
        } else {
            range::busiest(&snap, &disks, range)
        };
        let net = net_box::aggregate(&snap.devices);
        let net = figures(&[id(net, MetricId::NET_RX), id(net, MetricId::NET_TX)]);
        let unit = NetFormat::of(cx);
        let net = range::figures_text(words, net, |v, out| {
            format_value(ValueFormat::RateNetwork, v, unit, out);
        });
        let (cpu, mem, gpu, disk) = (pct(cpu), pct(mem), pct(gpu), pct(disk));
        self.cpu.update(cx, |b, cx| b.set_range_text(text(cpu), cx));
        self.mem.update(cx, |b, cx| b.set_range_text(text(mem), cx));
        self.gpu.update(cx, |b, cx| b.set_range_text(text(gpu), cx));
        self.disk
            .update(cx, |b, cx| b.set_range_text(text(disk), cx));
        self.net.update(cx, |b, cx| b.set_range_text(text(net), cx));
    }

    /// The CPU box ended a split drag or a double-click: the share goes into the layout and the
    /// profile.
    fn on_cpu_split(&mut self, _: Entity<CpuBox>, ev: &InnerSplitChanged, cx: &mut Context<Self>) {
        self.layout.set_split(BoxKind::Cpu, ev.0);
        self.save_layout(cx);
    }

    /// The CPU box ended a drag or a double-click on the gap above its power draw strip: the
    /// strip's share goes into the layout and the profile.
    fn on_cpu_power_split(
        &mut self,
        _: Entity<CpuBox>,
        ev: &PowerSplitChanged,
        cx: &mut Context<Self>,
    ) {
        let share = ((1. - ev.0) * 1000.).round() / 1000.;
        self.layout.set_power_split(BoxKind::Cpu, share);
        self.save_layout(cx);
    }

    /// The Memory box ended a split drag or a double-click: the share goes into the layout and
    /// the profile.
    fn on_mem_split(&mut self, _: Entity<MemBox>, ev: &InnerSplitChanged, cx: &mut Context<Self>) {
        self.layout.set_split(BoxKind::Memory, ev.0);
        self.save_layout(cx);
    }

    /// The GPU box ended a split drag or a double-click: the share goes into the layout and the
    /// profile.
    fn on_gpu_split(&mut self, _: Entity<GpuBox>, ev: &InnerSplitChanged, cx: &mut Context<Self>) {
        self.layout.set_split(BoxKind::Gpu, ev.0);
        self.save_layout(cx);
    }

    /// The Disk box ended a split drag or a double-click: the share goes into the layout and the
    /// profile.
    fn on_disk_split(
        &mut self,
        _: Entity<DiskBox>,
        ev: &InnerSplitChanged,
        cx: &mut Context<Self>,
    ) {
        self.layout.set_split(BoxKind::Disk, ev.0);
        self.save_layout(cx);
    }

    /// The Network box ended a split drag or a double-click: the share goes into the layout and
    /// the profile.
    fn on_net_split(&mut self, _: Entity<NetBox>, ev: &InnerSplitChanged, cx: &mut Context<Self>) {
        self.layout.set_split(BoxKind::Net, ev.0);
        self.save_layout(cx);
    }

    /// Takes `layout`: the Top processes box with its demand comes or goes, then a new
    /// placement and one render.
    fn set_layout(&mut self, layout: OverviewLayout, cx: &mut Context<Self>) {
        let procs = layout.has(BoxKind::TopProcs);
        let timeline_moved = layout.has(BoxKind::Timeline) != self.layout.has(BoxKind::Timeline);
        let mem_apps_moved = mem_apps(&layout) != mem_apps(&self.layout);
        let split = layout.split(BoxKind::Cpu);
        let cpu_arrange = layout.arrange(BoxKind::Cpu);
        let cpu_mode = layout.config(BoxKind::Cpu).mode.id;
        let cpu_power = layout.config(BoxKind::Cpu);
        let mem_split = layout.split(BoxKind::Memory);
        let mem_arrange = layout.arrange(BoxKind::Memory);
        let mem_mode = layout.config(BoxKind::Memory).mode.id;
        let gpu_split = layout.split(BoxKind::Gpu);
        let gpu_arrange = layout.arrange(BoxKind::Gpu);
        let gpu_mode = layout.config(BoxKind::Gpu).mode.id;
        let gpu_hidden = layout.hidden_gpu().to_vec();
        let disk_split = layout.split(BoxKind::Disk);
        let disk_mode = layout.config(BoxKind::Disk).mode.id;
        let disk_arrange = layout.arrange(BoxKind::Disk);
        let net_split = layout.split(BoxKind::Net);
        let net_arrange = layout.arrange(BoxKind::Net);
        let net_mode = layout.config(BoxKind::Net).mode.id;
        self.layout = layout;
        self.cpu.update(cx, |b, cx| b.set_mode(cpu_mode, cx));
        if let Some(split) = split {
            self.cpu.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = cpu_arrange {
            self.cpu.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        self.cpu.update(cx, |b, cx| {
            b.set_power(cpu_power.power, cpu_power.power_split, cx);
        });
        self.mem.update(cx, |b, cx| b.set_mode(mem_mode, cx));
        if let Some(split) = mem_split {
            self.mem.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = mem_arrange {
            self.mem.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        self.gpu.update(cx, |b, cx| b.set_mode(gpu_mode, cx));
        if let Some(split) = gpu_split {
            self.gpu.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = gpu_arrange {
            self.gpu.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        self.gpu.update(cx, |b, cx| b.set_hidden(&gpu_hidden, cx));
        self.disk.update(cx, |b, cx| b.set_mode(disk_mode, cx));
        if let Some(split) = disk_split {
            self.disk.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = disk_arrange {
            self.disk.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        self.net.update(cx, |b, cx| b.set_mode(net_mode, cx));
        if let Some(split) = net_split {
            self.net.update(cx, |b, cx| b.set_split(split, cx));
        }
        if let Some(arrange) = net_arrange {
            self.net.update(cx, |b, cx| b.set_arrange(arrange, cx));
        }
        if procs != self.procs.is_some() {
            self.procs = procs.then(|| {
                let hub = self.hub.clone();
                cx.new(|cx| ProcsBox::new(&hub, cx))
            });
            self.send_demand(cx);
        } else if timeline_moved || mem_apps_moved {
            self.send_demand(cx);
        }
        self.relayout(cx);
        self.notify(cx);
    }

    /// The pointer position along `target`'s drag axis.
    fn axis(target: Target, position: gpui_kit::Point<Pixels>) -> f32 {
        f32::from(if target.is_vertical() {
            position.x
        } else {
            position.y
        })
    }

    /// Button down on a gap: a double-click splits the two spans as evenly as whole cells allow
    /// and saves; a single press starts a drag.
    fn splitter_down(&mut self, target: Target, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(drag) = Drag::start(
            &self.layout,
            &self.placement,
            target,
            Self::axis(target, ev.position),
        ) else {
            return;
        };
        cx.stop_propagation();
        if ev.click_count >= 2 {
            self.drag = None;
            if set_span(&mut self.layout, target, drag.even_span()) {
                self.apply_layout(cx);
                self.save_layout(cx);
            } else {
                self.notify(cx);
            }
        } else {
            self.drag = Some(ActiveDrag {
                drag,
                changed: false,
                start: self.layout.boxes.clone(),
            });
            self.notify(cx);
        }
    }

    /// The pointer moved while a gap is held: the shared edge snaps to the nearest whole cell
    /// and the page places again when it moved.
    fn splitter_move(&mut self, position: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(active) = &mut self.drag else {
            return;
        };
        let (drag, target) = (active.drag, active.drag.target);
        let span = drag.span_at(Self::axis(target, position));
        if set_span(&mut self.layout, target, span) {
            active.changed = true;
            self.apply_layout(cx);
        }
    }

    /// The button went up: the drag ends and a changed layout is saved to the profile.
    fn splitter_up(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.drag.take() else {
            return;
        };
        if active.changed {
            self.save_layout(cx);
        }
        self.notify(cx);
    }

    /// Places the page again after a span change.
    fn apply_layout(&mut self, cx: &mut Context<Self>) {
        self.relayout(cx);
        self.notify(cx);
    }

    /// Writes the layout to `[performance.overview]`.
    fn save_layout(&self, cx: &mut Context<Self>) {
        if !cx.has_global::<TlSettings>() {
            return;
        }
        let setting = self.layout.to_setting();
        TlSettings::update(cx, |d| d.settings.performance.overview = Some(setting));
    }

    /// The pointer entered or left a gap. The accent line shows after
    /// [`SPLITTER_HOVER_DELAY`] and goes at once on leaving.
    fn splitter_hover(&mut self, target: Target, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hover == Some(target) {
                return;
            }
            self.hover = Some(target);
            self.hover_shown = false;
            self.hover_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SPLITTER_HOVER_DELAY).await;
                let _ = this.update(cx, |this, cx| {
                    this.hover_task = None;
                    if this.hover == Some(target) {
                        this.hover_shown = true;
                        this.notify(cx);
                    }
                });
            }));
        } else if self.hover == Some(target) {
            self.hover = None;
            self.hover_shown = false;
            self.hover_task = None;
        } else {
            return;
        }
        self.notify(cx);
    }

    /// One gap. In Edit Layout (`live`) it holds the hit area with its cursor, its handlers and
    /// its accent line; outside it the gap only marks its place (D-033).
    fn gap_element(&self, s: Splitter, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let target = s.target;
        let vertical = target.is_vertical();
        let gap = div()
            .debug_selector(move || target.selector())
            .absolute()
            .left(s.bounds.left())
            .top(s.bounds.top())
            .w(s.bounds.size.width)
            .h(s.bounds.size.height);
        if !live {
            return gap.into_any_element();
        }
        let dragging = self.drag.as_ref().map(|a| a.drag.target);
        let lit = dragging == Some(target)
            || (dragging.is_none() && self.hover == Some(target) && self.hover_shown);
        let hit = div()
            .id(SharedString::from(target.selector()))
            .debug_selector(move || hit_selector(&target.selector()))
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.splitter_down(target, ev, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.splitter_hover(target, *hovered, cx);
            }));
        let hit = if vertical {
            hit.cursor_ew_resize()
        } else {
            hit.cursor_ns_resize()
        };
        if !lit {
            return gap.child(hit).into_any_element();
        }
        let line = div()
            .debug_selector(|| SPLITTER_LINE_ID.into())
            .absolute()
            .bg(current(cx).accent);
        let line = if vertical {
            let x = (f32::from(s.bounds.size.width) - SPLITTER_LINE) / 2.;
            line.left(px(x)).top_0().bottom_0().w(px(SPLITTER_LINE))
        } else {
            let y = (f32::from(s.bounds.size.height) - SPLITTER_LINE) / 2.;
            line.top(px(y)).left_0().right_0().h(px(SPLITTER_LINE))
        };
        gap.child(hit.child(line)).into_any_element()
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part());
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    /// The Overview header: the page title, and the time range switch at its right end
    /// (UI-PRF-055). While the status bar is on the Overview the bar carries the switch, so the
    /// page shows one switch, not two; the title stays.
    fn range_header(&self, cx: &App) -> AnyElement {
        let m = metrics(cx);
        let theme = current(cx);
        // The page title at the left, with the Lucide layout-grid icon in `text_secondary`.
        let title = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[3])
            .child(
                div()
                    .debug_selector(|| TITLE_ICON_ID.into())
                    .flex_none()
                    .child(icon(TlIcon::LayoutGrid, m.icon.lg, theme.text_secondary)),
            )
            .child(
                div()
                    .whitespace_nowrap()
                    .text_color(theme.text_primary)
                    .child(self.header.0.clone()),
            );
        let switch = (!self.bar_on_overview).then(|| range::range_switch(range::HEADER_SWITCH, cx));
        // UI-PRF-057: while held at a time, "Back to live" ends the pause.
        let live = self.paused.is_some().then(|| {
            div()
                .debug_selector(|| timeline::BACK_TO_LIVE_ID.into())
                .child(
                    TlButton::new(timeline::BACK_TO_LIVE_ID, TlButtonKind::Primary)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "perf-overview-back-to-live"))
                        .on_click(|_, _, cx| timeline::set_paused(None, cx)),
                )
        });
        let right = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[4])
            .children(live)
            .children(switch);
        div()
            .debug_selector(|| RANGE_HEADER_ID.into())
            .flex()
            .flex_row()
            .flex_none()
            .justify_between()
            .items_center()
            .w_full()
            .px(m.sp[5])
            .pt(m.sp[4])
            .child(title)
            .child(right)
            .into_any_element()
    }

    /// The box view of `kind`, cached so it re-renders only for its own changes.
    fn box_element(&self, kind: BoxKind) -> AnyElement {
        let cached = StyleRefinement::default().size_full();
        match kind {
            BoxKind::Cpu => self.cpu.clone().cached(cached).into_any_element(),
            BoxKind::Memory => self.mem.clone().cached(cached).into_any_element(),
            BoxKind::Gpu => self.gpu.clone().cached(cached).into_any_element(),
            BoxKind::Disk => self.disk.clone().cached(cached).into_any_element(),
            BoxKind::Net => self.net.clone().cached(cached).into_any_element(),
            BoxKind::TopProcs => match &self.procs {
                Some(procs) => procs.clone().cached(cached).into_any_element(),
                None => div().into_any_element(),
            },
            BoxKind::Timeline => self.timeline.clone().cached(cached).into_any_element(),
        }
    }
}

impl DevicePage for OverviewPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        div().into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let placement = status_bar::placement(cx);
        let words: SmallVec<[SharedString; 4]> =
            STATUS_BAR_ITEMS.iter().map(|(_, k)| tr(cx, k)).collect();
        menu.item(
            tr(cx, "perf-overview-edit-layout"),
            Box::new(PerfOverviewEditLayout),
        )
        .submenu(
            tr(cx, "perf-overview-status-bar"),
            window,
            cx,
            move |mut menu, _, _| {
                for ((value, _), word) in STATUS_BAR_ITEMS.iter().zip(&words) {
                    menu = menu.radio_item(
                        word.clone(),
                        *value == placement,
                        Box::new(PerfSetStatusBar { placement: *value }),
                    );
                }
                menu
            },
        )
    }

    /// "Overview", then each box's copy text. The boxes' text is read through the app, so this
    /// writes the title only; [`OverviewPage::copy_all`] writes the whole page.
    fn copy_text(&self, out: &mut String) {
        out.push_str(&self.header.0);
        out.push('\n');
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        SmallVec::new()
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else {
            self.end_drag(cx);
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }
}

impl OverviewPage {
    /// "Overview" and every box's copy text, in layout order.
    pub fn copy_all(&self, out: &mut String, cx: &App) {
        self.copy_text(out);
        for p in &self.placement.boxes {
            match p.kind {
                BoxKind::Cpu => self.cpu.read(cx).copy_text(out),
                BoxKind::Memory => self.mem.read(cx).copy_text(out),
                BoxKind::Gpu => self.gpu.read(cx).copy_text(out),
                BoxKind::Disk => self.disk.read(cx).copy_text(out),
                BoxKind::Net => self.net.read(cx).copy_text(out),
                BoxKind::TopProcs => {
                    if let Some(procs) = &self.procs {
                        procs.read(cx).copy_text(out);
                    }
                }
                BoxKind::Timeline => self.timeline.read(cx).copy_text(out),
            }
        }
    }
}

impl Focusable for OverviewPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl OverviewPage {
    /// The page's action handlers: Edit Layout mode and its keys (UI-PRF-018).
    fn with_actions<E: gpui_kit::InteractiveElement>(el: E, cx: &mut Context<Self>) -> E {
        el.on_action(cx.listener(Self::on_edit_layout))
            .on_action(cx.listener(Self::on_edit_done))
            .on_action(cx.listener(Self::on_reset_layout))
            .on_action(cx.listener(Self::on_add_box))
            .on_action(cx.listener(Self::on_hide_box))
            .on_action(cx.listener(Self::on_move_box))
            .on_action(cx.listener(Self::on_resize_box))
            .on_action(cx.listener(Self::on_set_box_mode))
            .on_action(cx.listener(Self::on_set_box_arrange))
            .on_action(cx.listener(Self::on_set_box_power))
            .on_action(cx.listener(Self::on_set_box_engine))
            .on_action(cx.listener(Self::on_reset_box))
            .on_action(cx.listener(Self::on_set_timeline_click))
    }
}

impl Render for OverviewPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
            self._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.end_drag(cx);
                }
            }));
        }
        let this = cx.entity().downgrade();
        let this_move = this.clone();
        let dragging = self.drag.is_some();
        let box_dragging = self.box_drag().is_some();
        let measure = canvas(
            move |bounds, _, cx| {
                // After the frame: a notify during the frame is lost, so the page would keep
                // its empty first render until something else notified it.
                let (this, size) = (this.clone(), bounds.size);
                cx.defer(move |cx| {
                    let _ = this.update(cx, |v, cx| v.resize(size, cx));
                });
            },
            move |_, _, window, _| {
                if !dragging && !box_dragging {
                    return;
                }
                // The held gap, title line or corner follows the pointer wherever it goes.
                let moved = this_move.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let _ = moved.update(cx, |p, cx| {
                        match (e.pressed_button == Some(MouseButton::Left), box_dragging) {
                            (true, false) => p.splitter_move(e.position, cx),
                            (false, false) => p.splitter_up(cx),
                            (true, true) => p.box_drag_move(e.position, cx),
                            (false, true) => p.box_drag_up(None, cx),
                        }
                    });
                });
                let released = this_move.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase.bubble() {
                        let _ = released.update(cx, |p, cx| {
                            if box_dragging {
                                p.box_drag_up(Some(e.position), cx);
                            } else {
                                p.splitter_up(cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        // A box drag moves the boxes under the pointer, so the gaps are not splitters then.
        let live = self.is_editing();
        let gaps: SmallVec<[AnyElement; 8]> = if box_dragging {
            SmallVec::new()
        } else {
            splitters(&self.layout, &self.placement)
                .into_iter()
                .map(|s| self.gap_element(s, live, cx))
                .collect()
        };
        let boxes: SmallVec<[AnyElement; 8]> = self
            .placement
            .boxes
            .iter()
            .map(|p| {
                let kind = p.kind;
                let el = div()
                    .absolute()
                    .left(p.bounds.left())
                    .top(p.bounds.top())
                    .w(p.bounds.size.width)
                    .h(p.bounds.size.height)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.box_right_down(kind, ev, cx);
                        }),
                    )
                    .child(self.box_element(p.kind));
                // The state edge over the box's top border (UI-PRF-054).
                let edge = self.edge_color(kind, cx).map(|color| {
                    let id = edge_id(kind);
                    let m = metrics(cx);
                    div()
                        .debug_selector(move || id.clone())
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(EDGE_H * m.ui_scale))
                        .rounded_t(m.radius_card)
                        .bg(color)
                });
                let el = el.children(edge);
                self.edit_box(p, el, window, cx).into_any_element()
            })
            .collect();
        let overlays = self.edit_overlays(window, cx);
        let origin = Rc::clone(&self.content_origin);
        let content = div()
            .relative()
            .w_full()
            .h(self.placement.content_h)
            .child(
                canvas(move |b, _, _| origin.set(b.origin), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .children(boxes)
            .children(gaps)
            .children(overlays);
        let header = self.range_header(cx);
        let toolbar = self.edit_toolbar(cx);
        let body = div()
            .id(SCROLL_ID)
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(measure);
        let body = if self.placement.scrolls {
            body.overflow_y_scroll().child(content)
        } else {
            body.child(content)
        };
        let root = div().flex().flex_col().size_full().track_focus(&self.focus);
        let root = if toolbar.is_some() {
            root.key_context(PERFORMANCE_OVERVIEW_EDIT_CONTEXT)
        } else {
            root
        };
        let root = root.child(header).children(toolbar).child(body);
        Self::with_actions(root, cx).into_any_element()
    }
}

/// Whether the Memory box is placed and shows "By app" (UI-PRF-060).
fn mem_apps(layout: &OverviewLayout) -> bool {
    layout.has(BoxKind::Memory) && layout.config(BoxKind::Memory).mode.id == mem_box::APPS_MODE
}
