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
//! and only then does the page ask for the process sweep (`process_top`).
//!
//! Render contract (D-022, crw.34): the page never notifies on a sample. Each box is a cached
//! child view that notifies only when its own text or bars change. The page re-renders on a
//! resize, when a GPU or disk card comes or goes, and (from GPUI's ancestor marking) when a box
//! notifies; its render reads no snapshot and formats nothing.

pub mod cpu_box;
pub mod disk_box;
pub mod gpu_box;
pub mod layout;
pub mod mem_box;
pub mod net_box;
pub mod parts;
pub mod procs_box;

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Size, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Subscription, Window, canvas, div, px, size,
};
use smallvec::SmallVec;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::ids::DeviceIdx;

use self::cpu_box::CpuBox;
use self::disk_box::DiskBox;
use self::gpu_box::GpuBox;
use self::layout::{BoxKind, OverviewLayout, Placement, place};
use self::mem_box::MemBox;
use self::net_box::NetBox;
use self::procs_box::ProcsBox;
use super::details::KeyValueModel;
use super::page::DevicePage;
use super::sidebar::DeviceSidebar;
use super::{DeviceKey, DeviceKind};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
use crate::i18n::tr;
use crate::settings_global::TlSettings;
use crate::theme::metrics;

/// The Overview page's key.
pub const OVERVIEW_KEY: &str = "overview";

/// The prefix of every box's debug selector (`perf-overview-box-cpu`).
pub const BOX_ID_PREFIX: &str = "perf-overview-box-";

/// The scroll container's id when the boxes do not fit.
const SCROLL_ID: &str = "perf-overview-scroll";

const KEY_TITLE: &str = "perf-overview-title";

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
        if layout == self.layout {
            return;
        }
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

    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
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
        let measure = canvas(
            move |bounds, _, cx| {
                // After the frame: a notify during the frame is lost, so the page would keep
                // its empty first render until something else notified it.
                let (this, size) = (this.clone(), bounds.size);
                cx.defer(move |cx| {
                    let _ = this.update(cx, |v, cx| v.resize(size, cx));
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let content = div()
            .relative()
            .w_full()
            .h(self.placement.content_h)
            .children(self.placement.boxes.iter().map(|p| {
                div()
                    .absolute()
                    .left(p.bounds.left())
                    .top(p.bounds.top())
                    .w(p.bounds.size.width)
                    .h(p.bounds.size.height)
                    .child(self.box_element(p.kind))
            }));
        let root = div()
            .id(SCROLL_ID)
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .child(measure);
        if self.placement.scrolls {
            root.overflow_y_scroll().child(content)
        } else {
            root.child(content)
        }
    }
}
