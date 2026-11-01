//! The widgets of one container (ADR-019 "Entities and data flow", UI-PRF-061).
//!
//! A [`WidgetContainer`] holds the widgets of one box or page. It places them: a grid layout on
//! its inner snap grid (`grid.rs`), a flow layout with the placer its host gives (the GPU box
//! presets, `overview/gpu_flow.rs`), or without one in list order on the grid. It keeps a map
//! from widget id to its view, a [`MetricWidget`], a [`GroupWidget`] or a [`FixedWidgetView`],
//! and a [`SourceData`] for every metric widget of the layout, from the page's [`SourceStore`].
//! A group widget takes its members' sources from the same store itself. A fixed widget draws
//! the content view its host gives ([`WidgetContainer::set_fixed_content`]).
//!
//! Render contract (UI-PRF-017, UI-PRF-019): samples go to the widgets only. The container
//! listens to its sources for [`AvailabilityChanged`] and to nothing else on a sample, so it
//! never notifies because of one. It renders again on a layout change, a resize, a UI scale
//! change, an edit mode change and an availability change that moves a widget. Its render reads no snapshot, formats
//! nothing and builds no geometry; each widget is a cached child view.
//!
//! A layout change compares by widget id: a widget whose id stays keeps its entity and takes
//! its new settings in place, and only new ids get new entities. A widget that takes no space
//! (a flow layout leaves out a metric the device does not report) has no entity.
//!
//! A hosted container (the GPU box, `set_hosted`) holds the entities of a flow layout but does
//! not draw them: its host draws each widget in the 1.1.0 part the preset's rules give it
//! (`MetricWidget`'s `FlowLook`), and the values of one line side by side, joined by " · ". The
//! host takes the entities with [`WidgetContainer::flow_widget`], those of group widgets
//! with [`WidgetContainer::flow_group`] (the CPU box's core rows and cells), and those of fixed
//! widgets with [`WidgetContainer::flow_fixed`] (Memory by app). Its placer works out each
//! widget's rectangle from what the host shows and the body size, and the host asks for a new
//! placement ([`WidgetContainer::place_again`]) when what it shows changed. In Edit Widgets,
//! and for a grid layout, the container draws the widgets itself.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, EntityId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString, Size,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window, canvas,
    div, point, px, size,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::demand::Demand;
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, GroupKind, WidgetCell, WidgetContent,
    WidgetSetting, grid_cols,
};
use tl_model::layout_boxes::DeviceKind;

use super::fixed_widget::{FixedContent, FixedWidgetView};
use super::grid::{self, InnerGrid, InnerPlacement, PlacedWidget};
use super::group_widget::{GroupFlowLook, GroupWidget};
use super::metric_widget::{FlowLook, MetricWidget};
use super::source::{Availability, AvailabilityChanged, SourceData, SourceStore};
use crate::hub::SnapshotHub;
use crate::selector::TlSelector;
use crate::theme::{UiMetrics, metrics};

/// The debug selector of container `id`'s body ("overview.gpu" gives
/// "perf-widget-container-overview.gpu").
pub fn container_id(id: &str) -> String {
    format!("perf-widget-container-{id}")
}

/// The debug selector and scroll id of container `id`'s scrolling body.
pub fn scroll_id(id: &str) -> String {
    format!("perf-widget-container-{id}-scroll")
}

/// One widget of a flow layout, for the flow placer.
pub struct FlowItem<'a> {
    pub setting: &'a WidgetSetting,
    pub availability: Availability,
}

/// What a flow placer places: the layout, its widgets with their availability, the body size
/// in px, the UI scale, and the row height Edit Widgets holds (logical px), if any.
pub struct FlowInput<'a> {
    pub layout: &'a ContainerLayout,
    pub items: &'a [FlowItem<'a>],
    pub area: Size<Pixels>,
    pub scale: f32,
    pub frozen: Option<f32>,
}

/// Places a flow layout. A widget it leaves out takes no space and gets no entity.
pub type FlowPlace = Rc<dyn Fn(&FlowInput<'_>) -> InnerPlacement>;

/// The view of one placed widget.
#[derive(Clone)]
pub enum WidgetView {
    Metric(Entity<MetricWidget>),
    Group(Entity<GroupWidget>),
    Fixed(Entity<FixedWidgetView>),
}

impl WidgetView {
    fn any(&self) -> AnyView {
        match self {
            WidgetView::Metric(w) => w.clone().into(),
            WidgetView::Group(g) => g.clone().into(),
            WidgetView::Fixed(f) => f.clone().into(),
        }
    }

    /// Whether the view is the kind `content` needs.
    fn fits(&self, content: &WidgetContent) -> bool {
        matches!(
            (self, content),
            (WidgetView::Metric(_), WidgetContent::Metric(_))
                | (WidgetView::Group(_), WidgetContent::Group(_))
                | (WidgetView::Fixed(_), WidgetContent::Fixed(_))
        )
    }
}

/// The availability of a widget with no source of its own: a group or a fixed widget is shown
/// unless its kind is unknown to this build (a group's members come and go inside it).
fn content_availability(content: &WidgetContent) -> Availability {
    match content {
        WidgetContent::Group(g) if matches!(g.members, GroupKind::Other(_)) => {
            Availability::Unknown
        }
        WidgetContent::Fixed(FixedWidget::Other(_)) => Availability::Unknown,
        WidgetContent::Group(_) | WidgetContent::Fixed(_) => Availability::Available,
        WidgetContent::Metric(_) => Availability::Gone,
    }
}

/// The placement of a flow layout without a placer: the available widgets in list order, each
/// at its default span raised to its minimum span, in the first free place on the grid.
pub fn list_flow(input: &FlowInput<'_>) -> InnerPlacement {
    let container = &input.layout.container;
    let cols = grid_cols(container);
    let g = InnerGrid::of(container, f32::from(input.area.width), input.scale);
    let mut cells: SmallVec<[(u32, WidgetCell); 16]> = SmallVec::new();
    for item in input.items {
        if item.availability != Availability::Available {
            continue;
        }
        let (dw, dh) = grid::widget_default_span_in(container, item.setting);
        let (mw, mh) = g.widget_min_span(item.setting);
        let taken: SmallVec<[WidgetCell; 16]> = cells.iter().map(|(_, c)| *c).collect();
        let cell = grid::first_free(&taken, (dw.max(mw), dh.max(mh)), cols, 0);
        cells.push((item.setting.id, cell));
    }
    grid::place_in(container, &cells, input.area, input.scale, input.frozen)
}

/// The placer of a box whose flow layout is one fixed widget (Top processes, Timeline): the
/// fixed widget fills the whole body, as the box drew its content before it was a widget
/// (ADR-019 "Presets"). A widget of any other kind is not placed. In Edit Widgets, when the
/// layout holds no fixed widget the box could draw, the widgets go in list order
/// ([`list_flow`]).
pub fn fixed_flow() -> FlowPlace {
    Rc::new(|input: &FlowInput<'_>| {
        let fixed = input.items.iter().find(|i| {
            matches!(i.setting.content, WidgetContent::Fixed(_))
                && i.availability == Availability::Available
        });
        let Some(fixed) = fixed else {
            if input.frozen.is_some() {
                return list_flow(input);
            }
            return grid::place(
                &[],
                grid_cols(&input.layout.container),
                input.area,
                input.scale,
                input.frozen,
            );
        };
        let mut placement = grid::place(
            &[],
            grid_cols(&input.layout.container),
            input.area,
            input.scale,
            input.frozen,
        );
        placement.content_h = input.area.height;
        placement.widgets = vec![PlacedWidget {
            id: fixed.setting.id,
            bounds: gpui_kit::Bounds::new(point(px(0.), px(0.)), input.area),
        }];
        placement
    })
}

/// Adds the records a page widget on a device of `kind` reads to `demand`.
fn device_demand(demand: &mut Demand, kind: DeviceKind) {
    match kind {
        DeviceKind::Cpu | DeviceKind::CpuLogical => {
            if !demand.cpu.wants_total() {
                demand.cpu = tl_model::demand::CpuDemand::Total;
            }
        }
        DeviceKind::Memory => demand.memory = true,
        DeviceKind::Disk | DeviceKind::Disks => demand.disks = true,
        DeviceKind::Net | DeviceKind::Nets => demand.nets = true,
        DeviceKind::Gpu | DeviceKind::AggregateGpu => demand.gpus = true,
        DeviceKind::Fan | DeviceKind::Battery => demand.sensors = true,
        DeviceKind::Other => {}
    }
}

/// The widgets of one container.
pub struct WidgetContainer {
    id: SharedString,
    hub: Entity<SnapshotHub>,
    store: Rc<RefCell<SourceStore>>,
    layout: ContainerLayout,
    flow: Option<FlowPlace>,
    /// The host draws the widgets of a flow layout outside Edit Widgets.
    hosted: bool,
    /// The content views the host gives its fixed widgets.
    fixed_content: Option<FixedContent>,
    /// The source of every widget of the layout, placed or not, by widget id.
    sources: FxHashMap<u32, Entity<SourceData>>,
    /// One availability subscription per source entity.
    source_subs: FxHashMap<EntityId, Subscription>,
    /// The views of the placed widgets, by widget id.
    widgets: FxHashMap<u32, WidgetView>,
    placement: InnerPlacement,
    /// The body size at the last paint.
    area: Size<Pixels>,
    /// The drawn row height Edit Widgets holds, logical px.
    frozen: Option<f32>,
    /// The height of the editor's body while Edit Widgets runs. The container itself is as tall
    /// as its rows then, so this is the height that decides whether the rows scroll and keep the
    /// scrollbar's width free, as in the applied view.
    view_h: Option<Pixels>,
    /// The scroll position of the body when the rows are taller than it. Edit Widgets starts at
    /// this offset and hands its own back when it ends.
    scroll: ScrollHandle,
    /// The UI scale of the last placement.
    scale: f32,
    _metrics_sub: Subscription,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: std::cell::Cell<u32>,
    /// Placements run, so a test can tell a fixed-work render from one that placed widgets.
    #[cfg(any(test, feature = "test-hooks"))]
    pub relayout_count: std::cell::Cell<u32>,
}

impl WidgetContainer {
    /// The container `id` (such as "overview.gpu") showing `layout`, with its sources from the
    /// page's `store`.
    pub fn new(
        id: impl Into<SharedString>,
        layout: ContainerLayout,
        hub: &Entity<SnapshotHub>,
        store: Rc<RefCell<SourceStore>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            id: id.into(),
            hub: hub.clone(),
            store,
            layout,
            flow: None,
            hosted: false,
            fixed_content: None,
            sources: FxHashMap::default(),
            source_subs: FxHashMap::default(),
            widgets: FxHashMap::default(),
            placement: InnerPlacement::default(),
            area: size(px(0.), px(0.)),
            frozen: None,
            view_h: None,
            scroll: ScrollHandle::new(),
            scale: metrics(cx).ui_scale,
            _metrics_sub: cx.observe_global::<UiMetrics>(Self::on_metrics),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            relayout_count: std::cell::Cell::new(0),
        };
        this.sync_sources(cx);
        this.relayout(cx);
        this
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn layout(&self) -> &ContainerLayout {
        &self.layout
    }

    pub fn placement(&self) -> &InnerPlacement {
        &self.placement
    }

    /// The body size at the last paint.
    pub fn area(&self) -> Size<Pixels> {
        self.area
    }

    /// Whether Edit Widgets holds the rows.
    pub fn is_editing(&self) -> bool {
        self.frozen.is_some()
    }

    /// The entity of metric widget `id`, while it takes space.
    pub fn widget(&self, id: u32) -> Option<&Entity<MetricWidget>> {
        match self.widgets.get(&id)? {
            WidgetView::Metric(w) => Some(w),
            WidgetView::Group(_) | WidgetView::Fixed(_) => None,
        }
    }

    /// The entity of group widget `id`, while it takes space.
    pub fn group(&self, id: u32) -> Option<&Entity<GroupWidget>> {
        match self.widgets.get(&id)? {
            WidgetView::Group(g) => Some(g),
            WidgetView::Metric(_) | WidgetView::Fixed(_) => None,
        }
    }

    /// The entity of fixed widget `id`, while it takes space.
    pub fn fixed(&self, id: u32) -> Option<&Entity<FixedWidgetView>> {
        match self.widgets.get(&id)? {
            WidgetView::Fixed(f) => Some(f),
            WidgetView::Metric(_) | WidgetView::Group(_) => None,
        }
    }

    /// The view of widget `id`, while it takes space.
    pub fn view(&self, id: u32) -> Option<&WidgetView> {
        self.widgets.get(&id)
    }

    /// The ids of the placed widgets, in placement order.
    pub fn placed_ids(&self) -> SmallVec<[u32; 16]> {
        self.placement.widgets.iter().map(|p| p.id).collect()
    }

    /// The source of widget `id`, placed or not.
    pub fn source(&self, id: u32) -> Option<&Entity<SourceData>> {
        self.sources.get(&id)
    }

    /// The part of the page's demand this container adds: the GPU records while it places a
    /// widget, the records of the members of the groups it places (fast CPU samples for
    /// cores, as the core rows ask), the process sweep while it places Memory by app or the
    /// Processes table, and the timeline while it places the Timeline. A
    /// container with no placed widget asks for nothing.
    ///
    /// A device page container asks for the records of the devices its widgets read instead of
    /// the GPU records alone (ADR-019 phase 3 "Render contract and demand").
    pub fn demand_part(&self) -> Demand {
        let page = grid::is_page(&self.layout.container);
        let mut demand = Demand {
            gpus: !page && !self.placement.widgets.is_empty(),
            ..Demand::default()
        };
        for p in &self.placement.widgets {
            let content = self
                .layout
                .widgets
                .iter()
                .find(|w| w.id == p.id)
                .map(|w| &w.content);
            if page && let Some(device) = content.and_then(WidgetContent::device) {
                device_demand(&mut demand, DeviceKind::of(device));
            }
            match content {
                Some(WidgetContent::Fixed(FixedWidget::MemoryByApp | FixedWidget::Processes)) => {
                    demand.process_top = true;
                }
                Some(WidgetContent::Fixed(FixedWidget::Timeline)) => demand.device_timeline = true,
                _ => {}
            }
            let Some(WidgetContent::Group(g)) = content else {
                continue;
            };
            match g.members {
                GroupKind::Cores => demand.cpu_fast = true,
                GroupKind::Disks => demand.disks = true,
                GroupKind::Adapters => demand.nets = true,
                GroupKind::Engines | GroupKind::Other(_) => {}
            }
        }
        demand
    }

    /// A new layout. Widgets whose id stays keep their entities; nothing renders when the
    /// layout is the same.
    pub fn set_layout(&mut self, layout: ContainerLayout, cx: &mut Context<Self>) {
        if layout == self.layout {
            return;
        }
        self.layout = layout;
        self.sync_sources(cx);
        self.relayout(cx);
        self.notify(cx);
    }

    /// The placer of flow layouts; `None` places them in list order.
    pub fn set_flow_placer(&mut self, flow: Option<FlowPlace>, cx: &mut Context<Self>) {
        self.flow = flow;
        if self.layout.preset.is_some() && self.relayout(cx) {
            self.notify(cx);
        }
    }

    /// The content views of the fixed widgets: the host keeps them (Memory by app in the Memory
    /// box) and the fixed widgets draw them, in a flow or a grid layout alike.
    pub fn set_fixed_content(&mut self, content: Option<FixedContent>, cx: &mut Context<Self>) {
        self.fixed_content = content;
        if self.relayout(cx) {
            self.notify(cx);
        }
    }

    /// The content view the host gives fixed kind `kind`.
    fn content_of(&self, kind: &WidgetContent, cx: &mut Context<Self>) -> Option<AnyView> {
        let WidgetContent::Fixed(kind) = kind else {
            return None;
        };
        let content = self.fixed_content.clone()?;
        content(kind, cx)
    }

    /// Whether the host draws the widgets of a flow layout (outside Edit Widgets). It then
    /// takes their entities with [`Self::flow_widget`].
    pub fn set_hosted(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.hosted != on {
            self.hosted = on;
            self.notify(cx);
        }
    }

    /// Whether the host draws the widgets now.
    pub fn host_draws(&self) -> bool {
        self.hosted && self.layout.preset.is_some() && self.frozen.is_none()
    }

    /// The entity of widget `id` of the flow layout the host draws as `look`, created with that
    /// look when it has none yet. A widget the placer does not place loses its entity at the
    /// next placement. `None` when the layout has no widget `id`.
    pub fn flow_widget(
        &mut self,
        id: u32,
        look: FlowLook,
        cx: &mut Context<Self>,
    ) -> Option<Entity<MetricWidget>> {
        if let Some(view) = self.widgets.get(&id) {
            let WidgetView::Metric(widget) = view else {
                return None;
            };
            let widget = widget.clone();
            widget.update(cx, |w, cx| w.set_flow(Some(look), cx));
            return Some(widget);
        }
        let setting = self.layout.widgets.iter().find(|w| w.id == id)?.clone();
        let source = self.sources.get(&id)?.clone();
        let (hub, container) = (self.hub.clone(), self.id.clone());
        let widget =
            cx.new(|cx| MetricWidget::with_flow(container, setting, source, &hub, Some(look), cx));
        self.widgets.insert(id, WidgetView::Metric(widget.clone()));
        Some(widget)
    }

    /// The entity of group widget `id` of the flow layout the host draws as `look` (the 1.2.0
    /// part, `GroupFlowLook`), created with that look when it has none yet, so it makes no
    /// member items of its own. `None` when the layout has no group widget `id`.
    pub fn flow_group(
        &mut self,
        id: u32,
        look: GroupFlowLook,
        cx: &mut Context<Self>,
    ) -> Option<Entity<GroupWidget>> {
        if let Some(view) = self.widgets.get(&id) {
            let WidgetView::Group(group) = view else {
                return None;
            };
            let group = group.clone();
            group.update(cx, |g, cx| g.set_flow_look(Some(look), cx));
            return Some(group);
        }
        let setting = self.layout.widgets.iter().find(|w| w.id == id)?.clone();
        if !matches!(setting.content, WidgetContent::Group(_)) {
            return None;
        }
        let (hub, store, container) = (self.hub.clone(), Rc::clone(&self.store), self.id.clone());
        let group = cx
            .new(|cx| GroupWidget::with_flow_look(container, setting, &hub, store, Some(look), cx));
        self.widgets.insert(id, WidgetView::Group(group.clone()));
        Some(group)
    }

    /// The entity of fixed widget `id` of the flow layout the host draws, created when it has
    /// none yet. `None` when the layout has no fixed widget `id`.
    pub fn flow_fixed(
        &mut self,
        id: u32,
        cx: &mut Context<Self>,
    ) -> Option<Entity<FixedWidgetView>> {
        if let Some(view) = self.widgets.get(&id) {
            let WidgetView::Fixed(fixed) = view else {
                return None;
            };
            return Some(fixed.clone());
        }
        let setting = self.layout.widgets.iter().find(|w| w.id == id)?.clone();
        if !matches!(setting.content, WidgetContent::Fixed(_)) {
            return None;
        }
        let content = self.content_of(&setting.content, cx);
        let container = self.id.clone();
        let fixed = cx.new(|_| FixedWidgetView::new(container, setting, content));
        self.widgets.insert(id, WidgetView::Fixed(fixed.clone()));
        Some(fixed)
    }

    /// What the host shows changed: places the widgets again, which drops the entities of the
    /// widgets the placer no longer places. A hosted flow layout draws nothing here, so this does
    /// not notify then. Returns whether the placement changed.
    pub fn place_again(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.relayout(cx);
        if changed && !self.host_draws() {
            self.notify(cx);
        }
        changed
    }

    /// Edit Widgets on or off. While on, the rows keep the height they had when it started, so
    /// cells do not move under a drag (D-037).
    pub fn set_editing(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.frozen.is_some() {
            return;
        }
        self.view_h = None;
        self.frozen = on.then(|| {
            let g = self.placement.grid;
            if g.scale > 0. && g.row > 0. {
                g.row / g.scale
            } else {
                grid::ROW_UNIT
            }
        });
        self.relayout(cx);
        self.notify(cx);
    }

    /// The height of the editor's body, which decides the scroll while editing (see `view_h`).
    /// The editor's body is the box body less the edit toolbar, so a new height fills the rows
    /// to it once more by the Fill rule (D-037) and holds that height: a layout that fit the
    /// box body fits under the toolbar too, and no widget loses its bottom text to the scroll
    /// (tasklens-mo5t5). The body keeps its height during a drag, so cells do not move then.
    pub fn set_view_height(&mut self, h: Pixels, cx: &mut Context<Self>) {
        if self.view_h == Some(h) || self.frozen.is_none() {
            return;
        }
        self.view_h = Some(h);
        if let Some(row) = self.edit_row(h) {
            self.frozen = Some(row);
        }
        if self.relayout(cx) {
            self.notify(cx);
        }
    }

    /// The row height, logical px, that fills an editor body `h` px tall with the layout's
    /// rows, at least the unit. `None` for a container whose rows never fill (a device page)
    /// and for a layout with no placed rows.
    fn edit_row(&self, h: Pixels) -> Option<f32> {
        let container: &str = &self.layout.container;
        if !grid::grid_fills(container) || self.layout.preset.is_some() {
            return None;
        }
        let rows = self
            .layout
            .widgets
            .iter()
            .filter_map(|w| w.cell)
            .map(|c| c.row.saturating_add(c.rows))
            .max()
            .filter(|&r| r > 0)?;
        let scale = if self.scale > 0. { self.scale } else { 1. };
        let gutter = grid::grid_gutter(container) * scale;
        let free = f32::from(h) - f32::from(rows - 1) * gutter;
        let row = (free / f32::from(rows)) / scale;
        Some(row.max(grid::grid_unit(container)))
    }

    /// How far the body is scrolled down, logical px: 0 when the rows fit the body.
    pub fn scroll_offset(&self) -> Pixels {
        if self.placement.scrolls {
            -self.scroll.offset().y
        } else {
            px(0.)
        }
    }

    /// Scrolls the body `y` px down. The paint clamps it to the rows.
    pub fn set_scroll_offset(&mut self, y: Pixels) {
        let at: Point<Pixels> = point(px(0.), -y.max(px(0.)));
        self.scroll.set_offset(at);
    }

    /// The area the placement runs on: while editing, as tall as the editor's body.
    fn place_area(&self) -> Size<Pixels> {
        match (self.frozen, self.view_h) {
            (Some(_), Some(h)) => size(self.area.width, h),
            _ => self.area,
        }
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A new body size: a new placement, and one render when it moved a widget the container
    /// draws. A hosted flow layout draws nothing here, so it does not render then.
    fn resize(&mut self, area: Size<Pixels>, cx: &mut Context<Self>) {
        if area == self.area {
            return;
        }
        self.area = area;
        if self.relayout(cx) && !self.host_draws() {
            self.notify(cx);
        }
    }

    /// A new UI scale: the unit, the gutter and the minimums change, the spans do not.
    fn on_metrics(&mut self, cx: &mut Context<Self>) {
        if metrics(cx).ui_scale != self.scale && self.relayout(cx) {
            self.notify(cx);
        }
    }

    /// A source's device went or came back, or it started or stopped reporting the metric. A
    /// grid layout keeps its cells, so only a flow layout can move.
    fn on_availability(
        &mut self,
        _: Entity<SourceData>,
        _: &AvailabilityChanged,
        cx: &mut Context<Self>,
    ) {
        if self.relayout(cx) {
            self.notify(cx);
        }
    }

    /// Takes the source of every widget from the store and listens to each for availability.
    fn sync_sources(&mut self, cx: &mut Context<Self>) {
        let mut sources = FxHashMap::default();
        for w in &self.layout.widgets {
            // Group and fixed widgets have no source of their own: a group takes its members'
            // sources itself, and a fixed widget draws its host's content.
            let Some(selector) = w.content.metric() else {
                continue;
            };
            let source = self.store.borrow_mut().source(selector, cx);
            sources.insert(w.id, source);
        }
        let mut old = std::mem::take(&mut self.source_subs);
        for source in sources.values() {
            let key = source.entity_id();
            if self.source_subs.contains_key(&key) {
                continue;
            }
            let sub = old
                .remove(&key)
                .unwrap_or_else(|| cx.subscribe(source, Self::on_availability));
            self.source_subs.insert(key, sub);
        }
        self.sources = sources;
    }

    /// Places the widgets and brings the widget entities in line: new ids get entities, ids
    /// that left drop theirs, and kept ones take their settings and size in place. Returns
    /// whether anything the container draws changed.
    fn relayout(&mut self, cx: &mut Context<Self>) -> bool {
        #[cfg(any(test, feature = "test-hooks"))]
        self.relayout_count.set(self.relayout_count.get() + 1);
        let scale = metrics(cx).ui_scale;
        self.scale = scale;
        let placement = if self.layout.preset.is_none() {
            let cells: SmallVec<[(u32, WidgetCell); 16]> = self
                .layout
                .widgets
                .iter()
                .filter_map(|w| w.cell.map(|c| (w.id, c)))
                .collect();
            grid::place_in(
                &self.layout.container,
                &cells,
                self.place_area(),
                scale,
                self.frozen,
            )
        } else {
            let items: Vec<FlowItem<'_>> = self
                .layout
                .widgets
                .iter()
                .map(|w| FlowItem {
                    setting: w,
                    availability: self.sources.get(&w.id).map_or_else(
                        || content_availability(&w.content),
                        |s| s.read(cx).availability(),
                    ),
                })
                .collect();
            let input = FlowInput {
                layout: &self.layout,
                items: &items,
                area: self.place_area(),
                scale,
                frozen: self.frozen,
            };
            match &self.flow {
                Some(place) => place(&input),
                None => list_flow(&input),
            }
        };
        let mut changed = placement != self.placement;
        let before = self.widgets.len();
        // In Edit Widgets a widget that leaves the layout keeps its entity, so Cancel or Undo
        // creates none again (ADR-019 "Editing"). It goes when editing ends.
        let editing = self.frozen.is_some();
        self.widgets
            .retain(|id, _| editing || placement.widgets.iter().any(|p| p.id == *id));
        changed |= self.widgets.len() != before;
        // The name gets the GPU's name in front when the box shows widgets of two devices.
        let mut devices: SmallVec<[&DeviceSelector; 2]> = SmallVec::new();
        for p in &placement.widgets {
            if let Some(device) = self
                .layout
                .widgets
                .iter()
                .find(|w| w.id == p.id)
                .and_then(|w| w.content.device())
                && !devices.contains(&device)
            {
                devices.push(device);
            }
        }
        let prefix = devices.len() > 1;
        let host_draws = self.host_draws();
        let flow = self.layout.preset.is_some();
        for p in &placement.widgets {
            let Some(setting) = self.layout.widgets.iter().find(|w| w.id == p.id).cloned() else {
                continue;
            };
            let area = p.bounds.size;
            // A widget whose content changed kind gets a new view.
            if self
                .widgets
                .get(&p.id)
                .is_some_and(|v| !v.fits(&setting.content))
            {
                self.widgets.remove(&p.id);
                changed = true;
            }
            if let WidgetContent::Fixed(_) = setting.content {
                let content = self.content_of(&setting.content, cx);
                if let Some(WidgetView::Fixed(fixed)) = self.widgets.get(&p.id) {
                    fixed.update(cx, |f, cx| {
                        f.set_setting(setting, content, cx);
                        f.set_edit_inset(editing, cx);
                    });
                } else if host_draws {
                    // The host creates the entity when it draws the widget.
                    continue;
                } else {
                    let container = self.id.clone();
                    let fixed = cx.new(|cx| {
                        let mut f = FixedWidgetView::new(container, setting, content);
                        f.set_edit_inset(editing, cx);
                        f
                    });
                    self.widgets.insert(p.id, WidgetView::Fixed(fixed));
                    changed = true;
                }
                continue;
            }
            if let WidgetContent::Group(_) = setting.content {
                if let Some(WidgetView::Group(group)) = self.widgets.get(&p.id) {
                    group.update(cx, |g, cx| {
                        // Outside a host-drawn flow layout the group draws its own items.
                        if !host_draws {
                            g.set_flow_look(None, cx);
                        }
                        g.set_flow(flow, cx);
                        g.set_setting(setting, cx);
                        g.set_edit_inset(editing, cx);
                        g.set_size(area, cx);
                    });
                } else if host_draws {
                    // The host creates the entity with its look when it draws the group.
                    continue;
                } else {
                    let (hub, store) = (self.hub.clone(), Rc::clone(&self.store));
                    let container = self.id.clone();
                    let group = cx.new(|cx| {
                        let mut g = GroupWidget::new(container, setting, &hub, store, flow, cx);
                        g.set_edit_inset(editing, cx);
                        g.set_size(area, cx);
                        g
                    });
                    self.widgets.insert(p.id, WidgetView::Group(group));
                    changed = true;
                }
                continue;
            }
            let Some(source) = self.sources.get(&p.id).cloned() else {
                continue;
            };
            if let Some(WidgetView::Metric(widget)) = self.widgets.get(&p.id) {
                widget.update(cx, |w, cx| {
                    w.set_setting(setting, source, cx);
                    w.set_device_prefix(prefix, cx);
                    w.set_edit_inset(editing, cx);
                    w.set_size(area, cx);
                });
            } else if host_draws {
                // The host creates the entity with its look when it draws the widget.
                continue;
            } else {
                let (hub, container) = (self.hub.clone(), self.id.clone());
                let widget = cx.new(|cx| {
                    let mut w = MetricWidget::new(container, setting, source, &hub, cx);
                    w.set_device_prefix(prefix, cx);
                    w.set_edit_inset(editing, cx);
                    w.set_size(area, cx);
                    w
                });
                self.widgets.insert(p.id, WidgetView::Metric(widget));
                changed = true;
            }
        }
        self.placement = placement;
        changed
    }
}

impl Render for WidgetContainer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let this = cx.entity().downgrade();
        // After the frame: a notify during the frame is lost.
        let measure = canvas(
            move |bounds, _, cx| {
                let (this, size) = (this.clone(), bounds.size);
                cx.defer(move |cx| {
                    let _ = this.update(cx, |v, cx| v.resize(size, cx));
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // The host draws a hosted flow layout's widgets.
        let host_draws = self.host_draws();
        let items = self.placement.widgets.iter().filter_map(|p| {
            if host_draws {
                return None;
            }
            let view = self.widgets.get(&p.id)?.any();
            Some(
                div()
                    .absolute()
                    .left(p.bounds.left())
                    .top(p.bounds.top())
                    .w(p.bounds.size.width)
                    .h(p.bounds.size.height)
                    .child(view.cached(StyleRefinement::default().size_full())),
            )
        });
        let id = container_id(&self.id);
        // In Edit Widgets the editor's body scrolls, and the container is as tall as its rows.
        let body = if self.placement.scrolls && self.frozen.is_none() {
            let scroll = scroll_id(&self.id);
            let selector = scroll.clone();
            div()
                .size_full()
                .relative()
                .tl_selector(move || selector)
                .child(
                    div()
                        .id(SharedString::from(scroll))
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .h(self.placement.content_h)
                                .children(items),
                        ),
                )
                .vertical_scrollbar(&self.scroll)
                .into_any_element()
        } else {
            div()
                .relative()
                .size_full()
                .children(items)
                .into_any_element()
        };
        div()
            .tl_selector(move || id)
            .relative()
            .size_full()
            .overflow_hidden()
            .child(measure)
            .child(body)
    }
}
