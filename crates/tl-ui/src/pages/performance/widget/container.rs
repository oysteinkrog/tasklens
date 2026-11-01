//! The widgets of one container (ADR-019 "Entities and data flow", UI-PRF-061).
//!
//! A [`WidgetContainer`] holds the widgets of one box or page. It places them: a grid layout on
//! its inner snap grid (`grid.rs`), a flow layout with the placer its host gives (the GPU box
//! presets, `overview/gpu_flow.rs`), or without one in list order on the grid. It keeps a map
//! from widget id to its [`MetricWidget`] entity and a [`SourceData`] for every widget of the
//! layout, from the page's [`SourceStore`].
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
//! host takes the entities with [`WidgetContainer::flow_widget`] and places the widgets again
//! ([`WidgetContainer::place_again`]) when the set it draws changed. In Edit Widgets, and for a
//! grid layout, the container draws the widgets itself.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EntityId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement, Styled as _,
    Subscription, Window, canvas, div, px, size,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::demand::Demand;
use tl_model::layout::{ContainerLayout, DeviceSelector, WidgetCell, WidgetSetting, grid_cols};

use super::grid::{self, InnerGrid, InnerPlacement};
use super::metric_widget::{FlowLook, MetricWidget};
use super::source::{Availability, AvailabilityChanged, SourceData, SourceStore};
use crate::hub::SnapshotHub;
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

/// The placement of a flow layout without a placer: the available widgets in list order, each
/// at its default span raised to its minimum span, in the first free place on the grid.
pub fn list_flow(input: &FlowInput<'_>) -> InnerPlacement {
    let cols = grid_cols(&input.layout.container);
    let g = InnerGrid::new(f32::from(input.area.width), cols, input.scale);
    let mut cells: SmallVec<[(u32, WidgetCell); 16]> = SmallVec::new();
    for item in input.items {
        if item.availability != Availability::Available {
            continue;
        }
        let forms = item.setting.forms;
        let (dw, dh) = grid::default_span(forms, cols);
        let (mw, mh) = g.forms_min_span(forms);
        let taken: SmallVec<[WidgetCell; 16]> = cells.iter().map(|(_, c)| *c).collect();
        let cell = grid::first_free(&taken, (dw.max(mw), dh.max(mh)), cols, 0);
        cells.push((item.setting.id, cell));
    }
    grid::place(&cells, cols, input.area, input.scale, input.frozen)
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
    /// The source of every widget of the layout, placed or not, by widget id.
    sources: FxHashMap<u32, Entity<SourceData>>,
    /// One availability subscription per source entity.
    source_subs: FxHashMap<EntityId, Subscription>,
    /// The entities of the placed widgets, by widget id.
    widgets: FxHashMap<u32, Entity<MetricWidget>>,
    placement: InnerPlacement,
    /// The body size at the last paint.
    area: Size<Pixels>,
    /// The drawn row height Edit Widgets holds, logical px.
    frozen: Option<f32>,
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
            sources: FxHashMap::default(),
            source_subs: FxHashMap::default(),
            widgets: FxHashMap::default(),
            placement: InnerPlacement::default(),
            area: size(px(0.), px(0.)),
            frozen: None,
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

    /// The entity of widget `id`, while it takes space.
    pub fn widget(&self, id: u32) -> Option<&Entity<MetricWidget>> {
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
    /// widget. A container with no placed widget asks for nothing.
    pub fn demand_part(&self) -> Demand {
        Demand {
            gpus: !self.placement.widgets.is_empty(),
            ..Demand::default()
        }
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
    /// look when it has none yet. The host then places the widgets again with the ids it drew
    /// ([`Self::place_again`]), so a widget it no longer draws loses its entity. `None` when the
    /// layout has no widget `id`.
    pub fn flow_widget(
        &mut self,
        id: u32,
        look: FlowLook,
        cx: &mut Context<Self>,
    ) -> Option<Entity<MetricWidget>> {
        if let Some(widget) = self.widgets.get(&id) {
            let widget = widget.clone();
            widget.update(cx, |w, cx| w.set_flow(Some(look), cx));
            return Some(widget);
        }
        let setting = self.layout.widgets.iter().find(|w| w.id == id)?.clone();
        let source = self.sources.get(&id)?.clone();
        let hub = self.hub.clone();
        let widget = cx.new(|cx| MetricWidget::with_flow(setting, source, &hub, Some(look), cx));
        self.widgets.insert(id, widget.clone());
        Some(widget)
    }

    /// The host drew another set of widgets: places them again, which drops the entities of
    /// the widgets it no longer draws. A hosted flow layout draws nothing here, so this does
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

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A new body size: a new placement, and one render when it moved a widget.
    fn resize(&mut self, area: Size<Pixels>, cx: &mut Context<Self>) {
        if area == self.area {
            return;
        }
        self.area = area;
        if self.relayout(cx) {
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
            let source = self.store.borrow_mut().source(&w.source, cx);
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
            grid::place(
                &cells,
                grid_cols(&self.layout.container),
                self.area,
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
                    availability: self
                        .sources
                        .get(&w.id)
                        .map_or(Availability::Gone, |s| s.read(cx).availability()),
                })
                .collect();
            let input = FlowInput {
                layout: &self.layout,
                items: &items,
                area: self.area,
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
            if let Some(w) = self.layout.widgets.iter().find(|w| w.id == p.id)
                && !devices.contains(&&w.source.device)
            {
                devices.push(&w.source.device);
            }
        }
        let prefix = devices.len() > 1;
        let host_draws = self.host_draws();
        for p in &placement.widgets {
            let Some(setting) = self.layout.widgets.iter().find(|w| w.id == p.id).cloned() else {
                continue;
            };
            let Some(source) = self.sources.get(&p.id).cloned() else {
                continue;
            };
            let area = p.bounds.size;
            if let Some(widget) = self.widgets.get(&p.id) {
                widget.update(cx, |w, cx| {
                    w.set_setting(setting, source, cx);
                    w.set_device_prefix(prefix, cx);
                    w.set_size(area, cx);
                });
            } else if host_draws {
                // The host creates the entity with its look when it draws the widget.
                continue;
            } else {
                let hub = self.hub.clone();
                let widget = cx.new(|cx| {
                    let mut w = MetricWidget::new(setting, source, &hub, cx);
                    w.set_device_prefix(prefix, cx);
                    w.set_size(area, cx);
                    w
                });
                self.widgets.insert(p.id, widget);
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
            let widget = self.widgets.get(&p.id)?;
            Some(
                div()
                    .absolute()
                    .left(p.bounds.left())
                    .top(p.bounds.top())
                    .w(p.bounds.size.width)
                    .h(p.bounds.size.height)
                    .child(
                        widget
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
        });
        let id = container_id(&self.id);
        let body = if self.placement.scrolls {
            let scroll = scroll_id(&self.id);
            let selector = scroll.clone();
            div()
                .size_full()
                .debug_selector(move || selector)
                .child(
                    div()
                        .size_full()
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .h(self.placement.content_h)
                                .children(items),
                        )
                        .overflow_y_scrollbar()
                        .id(SharedString::from(scroll)),
                )
                .into_any_element()
        } else {
            div()
                .relative()
                .size_full()
                .children(items)
                .into_any_element()
        };
        div()
            .debug_selector(move || id)
            .relative()
            .size_full()
            .overflow_hidden()
            .child(measure)
            .child(body)
    }
}
