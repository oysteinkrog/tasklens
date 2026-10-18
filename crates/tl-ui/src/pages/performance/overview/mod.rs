//! The Performance tab's Overview page (UI-PRF-017, D-022): five btop-style boxes (CPU, MEM,
//! GPU, DISK, NET) in the tasklens theme, laid out from data so a later editor can change them.
//!
//! [`layout`] holds the layout model, placement and device accent family. [`parts`] holds the
//! meter, core bars and the shared accent frame with its inset title line. One module per box
//! owns its view. [`OverviewPage`] places the box views and sends one demand part while it is
//! shown.
//!
//! The layout comes from `[performance.overview]` (UI-PRF-018), the default when there is none.
//! [`procs_box`] is the opt-in Top processes box: it is built only while the layout places it,
//! and only then does the page ask for the process sweep (`process_top`). [`splitter`] resizes
//! boxes and rows by dragging the gaps; [`edit`] is Edit Layout mode, which moves, hides and
//! adds boxes.
//!
//! Render contract (D-022, crw.34): the page never notifies on a sample. Each box is a cached
//! child view that notifies only when its own text or bars change. The page re-renders on a
//! resize, when a GPU or disk card comes or goes, and (from GPUI's ancestor marking) when a box
//! notifies; its render reads no snapshot and formats nothing.

pub mod cpu_box;
pub mod disk_box;
pub mod edit;
pub mod gpu_box;
pub mod layout;
pub mod mem_box;
pub mod net_box;
pub mod parts;
pub mod procs_box;
pub mod splitter;

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
use tl_model::ids::DeviceIdx;

use self::cpu_box::CpuBox;
use self::disk_box::DiskBox;
use self::edit::EditState;
use self::gpu_box::GpuBox;
use self::layout::{BoxKind, OverviewLayout, Placement, place};
use self::mem_box::MemBox;
use self::net_box::NetBox;
use self::procs_box::ProcsBox;
use self::splitter::{Drag, Splitter, Target, round_weights, set_weights, splitters};
use super::actions::{PERFORMANCE_OVERVIEW_EDIT_CONTEXT, PerfOverviewEditLayout};
use super::details::KeyValueModel;
use super::page::DevicePage;
use super::sidebar::DeviceSidebar;
use super::{DeviceKey, DeviceKind};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
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

/// How long the pointer rests on a gap before its accent line shows.
pub const SPLITTER_HOVER_DELAY: Duration = Duration::from_millis(150);

/// The accent line in a hovered gap, logical px.
const SPLITTER_LINE: f32 = 2.;

/// The debug selector of the accent line of the hovered or dragged gap.
pub const SPLITTER_LINE_ID: &str = "perf-overview-splitter-line";

/// A gap drag in progress. `changed` is set once a move changed a weight, so a click that
/// moved nothing saves nothing.
struct ActiveDrag {
    drag: Drag,
    changed: bool,
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

/// The Overview page.
pub struct OverviewPage {
    key: DeviceKey,
    layout: OverviewLayout,
    placement: Placement,
    area: Size<Pixels>,
    cpu: Entity<CpuBox>,
    mem: Entity<MemBox>,
    gpu: Entity<GpuBox>,
    disk: Entity<DiskBox>,
    net: Entity<NetBox>,
    /// The Top processes box, built only while the layout places it.
    procs: Option<Entity<ProcsBox>>,
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
    _subs: SmallVec<[Subscription; 4]>,
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
        let mem = cx.new(|cx| MemBox::new(&hub, mem_idx, cx));
        let gpu = cx.new(|cx| GpuBox::new(&hub, &gpu_cards, cx));
        let disk = cx.new(|cx| DiskBox::new(&hub, &disk_cards, cx));
        let net_target = first_card(sidebar, DeviceKind::Net, cx);
        let net = cx.new(|cx| {
            let mut b = NetBox::new(&hub, cx);
            b.set_target(net_target, cx);
            b
        });

        let mut subs = SmallVec::new();
        subs.push(cx.observe(sidebar, |this, _, cx| this.sync_cards(cx)));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_layout));
        let layout = saved_layout(cx);
        let procs = layout
            .has(BoxKind::TopProcs)
            .then(|| cx.new(|cx| ProcsBox::new(&hub, cx)));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        Self {
            key,
            layout,
            placement: Placement::default(),
            area: size(px(0.), px(0.)),
            cpu,
            mem,
            gpu,
            disk,
            net,
            procs,
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
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        }
    }

    /// Everything the boxes draw, CPU per core at 10 Hz, and the process sweep while the Top
    /// processes box is placed.
    pub fn demand_part(&self) -> Demand {
        Demand {
            process_top: self.procs.is_some(),
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

    /// The hub the boxes read.
    pub fn hub(&self) -> &Entity<SnapshotHub> {
        &self.hub
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
        self.placement = place(
            &self.layout,
            |k| match k {
                BoxKind::Gpu => gpu,
                BoxKind::Disk => disk,
                BoxKind::TopProcs => procs,
                _ => true,
            },
            self.area,
            s,
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
    /// box with its demand comes or goes. Any other settings change does nothing here.
    fn sync_layout(&mut self, cx: &mut Context<Self>) {
        let layout = saved_layout(cx);
        if layout == self.layout || self.drag.is_some() || self.box_dragging() {
            return;
        }
        self.set_layout(layout, cx);
    }

    /// Takes `layout`: the Top processes box with its demand comes or goes, then a new
    /// placement and one render.
    fn set_layout(&mut self, layout: OverviewLayout, cx: &mut Context<Self>) {
        let procs = layout.has(BoxKind::TopProcs);
        self.layout = layout;
        if procs != self.procs.is_some() {
            self.procs = procs.then(|| {
                let hub = self.hub.clone();
                cx.new(|cx| ProcsBox::new(&hub, cx))
            });
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

    /// Button down on a gap: a double-click gives the two neighbours equal sizes and saves; a
    /// single press starts a drag.
    fn splitter_down(&mut self, target: Target, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let Some(drag) = Drag::start(
            &self.layout,
            &self.placement,
            target,
            Self::axis(target, ev.position),
            scale,
        ) else {
            return;
        };
        cx.stop_propagation();
        if ev.click_count >= 2 {
            self.drag = None;
            let weights = drag.equal_weights();
            set_weights(&mut self.layout, target, weights);
            self.apply_layout(cx);
            self.save_layout(cx);
        } else {
            self.drag = Some(ActiveDrag {
                drag,
                changed: false,
            });
            self.notify(cx);
        }
    }

    /// The pointer moved while a gap is held: the two neighbours take new weights and the page
    /// places again.
    fn splitter_move(&mut self, position: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(active) = &mut self.drag else {
            return;
        };
        let (drag, target) = (active.drag, active.drag.target);
        let weights = drag.weights_at(Self::axis(target, position));
        set_weights(&mut self.layout, target, weights);
        active.changed = true;
        self.apply_layout(cx);
    }

    /// The button went up: the drag ends and a changed layout is saved to the profile.
    fn splitter_up(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.drag.take() else {
            return;
        };
        if active.changed {
            round_weights(&mut self.layout);
            self.apply_layout(cx);
            self.save_layout(cx);
        } else {
            self.notify(cx);
        }
    }

    /// Places the page again after a weight change.
    fn apply_layout(&mut self, cx: &mut Context<Self>) {
        self.relayout(cx);
        self.notify(cx);
    }

    /// Writes the layout to `[performance.overview]`.
    fn save_layout(&self, cx: &mut Context<Self>) {
        if !cx.has_global::<TlSettings>() {
            return;
        }
        let mut setting = self.layout.to_setting();
        let round = |w: &mut f64| *w = (*w * 1000.).round() / 1000.;
        for row in &mut setting.rows {
            round(&mut row.height);
            row.boxes.iter_mut().for_each(|b| round(&mut b.width));
        }
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

    /// The hit area of one gap, with its cursor, its handlers and its accent line.
    fn gap_element(&self, s: Splitter, cx: &mut Context<Self>) -> AnyElement {
        let target = s.target;
        let vertical = target.is_vertical();
        let dragging = self.drag.as_ref().map(|a| a.drag.target);
        let lit = dragging == Some(target)
            || (dragging.is_none() && self.hover == Some(target) && self.hover_shown);
        let gap = div()
            .id(SharedString::from(target.selector()))
            .debug_selector(move || target.selector())
            .absolute()
            .left(s.bounds.left())
            .top(s.bounds.top())
            .w(s.bounds.size.width)
            .h(s.bounds.size.height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.splitter_down(target, ev, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.splitter_hover(target, *hovered, cx);
            }));
        let gap = if vertical {
            gap.cursor_ew_resize()
        } else {
            gap.cursor_ns_resize()
        };
        if !lit {
            return gap.into_any_element();
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
        gap.child(line).into_any_element()
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

    fn menu_items(&self, menu: TlMenu, _: &mut Window, cx: &mut App) -> TlMenu {
        menu.item(
            tr(cx, "perf-overview-edit-layout"),
            Box::new(PerfOverviewEditLayout),
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
        }
        let this = cx.entity().downgrade();
        let this_move = this.clone();
        let dragging = self.drag.is_some();
        let box_dragging = self.box_dragging();
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
                // The held gap or box follows the pointer wherever it goes.
                let moved = this_move.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let _ = moved.update(cx, |p, cx| {
                        match (e.pressed_button == Some(MouseButton::Left), box_dragging) {
                            (true, false) => p.splitter_move(e.position, cx),
                            (true, true) => p.box_drag_move(e.position, cx),
                            (false, false) => p.splitter_up(cx),
                            (false, true) => p.box_drag_up(cx),
                        }
                    });
                });
                let released = this_move.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase.bubble() {
                        let _ = released.update(cx, |p, cx| {
                            if box_dragging {
                                p.box_drag_up(cx);
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
        let gaps: SmallVec<[AnyElement; 8]> = if box_dragging {
            SmallVec::new()
        } else {
            splitters(&self.layout, &self.placement)
                .into_iter()
                .map(|s| self.gap_element(s, cx))
                .collect()
        };
        let boxes: SmallVec<[AnyElement; 8]> = self
            .placement
            .boxes
            .iter()
            .map(|p| {
                let el = div()
                    .absolute()
                    .left(p.bounds.left())
                    .top(p.bounds.top())
                    .w(p.bounds.size.width)
                    .h(p.bounds.size.height)
                    .child(self.box_element(p.kind));
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
        let Some(toolbar) = self.edit_toolbar(cx) else {
            let root = div()
                .id(SCROLL_ID)
                .relative()
                .size_full()
                .track_focus(&self.focus)
                .child(measure);
            let root = Self::with_actions(root, cx);
            return if self.placement.scrolls {
                root.overflow_y_scroll().child(content)
            } else {
                root.child(content)
            }
            .into_any_element();
        };
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
        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .track_focus(&self.focus)
            .key_context(PERFORMANCE_OVERVIEW_EDIT_CONTEXT)
            .child(toolbar)
            .child(body);
        Self::with_actions(root, cx).into_any_element()
    }
}
