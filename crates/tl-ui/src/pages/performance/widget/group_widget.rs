//! One group widget: one metric of every member of a group (ADR-019 "Group widgets",
//! UI-PRF-061 phase 2).
//!
//! A [`GroupWidget`] is a GPUI entity keyed by its widget id. Its members (`members.rs`) are
//! every core, every shown disk, every counted adapter or every engine of a GPU, in a fixed
//! order. It shows at most `limit` of them, then a "+n more" item for the rest; members past the
//! limit have no source and no view.
//!
//! Each shown member is one item, drawn by a [`MetricWidget`] on the member's [`SourceData`]
//! from the page's [`SourceStore`], so a group and a single widget on the same member share one
//! source, and the member item reuses the compositions, the cached chart and the "notify only
//! when a shown part changed" rule of single widgets. The item keeps the composition of the
//! group's kind ([`MemberLook`]): a Row for Rows, a Cell (or a Tile without a chart) for Cells.
//! `group_fill.rs` places the items inside the widget's rectangle.
//!
//! Render contract: a sample reaches only the member items whose source changed; the group
//! itself does not notify on a sample. It re-renders, inside its own cells, when its member list
//! changes (catalog, visible cards, `show_network_*`, hidden engines), its size, its edit mode or
//! its settings change. A member that goes away drops its item; one that comes back gets a new
//! item in its place in the order. The group keeps its id, entity and cells through all of it.
//!
//! When the group's own device is gone (a GPU) it shows "Not connected"; with no members,
//! "None found"; for a group kind this build does not know, "Needs a newer tasklens".
//!
//! A group of a flow layout that its host draws (the CPU box presets, `overview/cpu_flow.rs`,
//! and the Disk box presets, `overview/disk_flow.rs`) has a [`GroupFlowLook`] instead: it draws
//! the 1.2.0 part (the core rows of D-040 with their clock column, the `core_charts` cells, the
//! disk sections or the `disk_charts` cells) from the model and charts the host keeps, and makes
//! no member items and takes no member sources of its own. The host updates the model on a
//! sample and notifies only this group ([`GroupWidget::flow_changed`]) when a row's text or
//! bucket changed. The disk sections work out their busiest-first expansion and "+n more" in
//! this group's render, so an activity change renders this group only. The Network box's
//! `adapter_charts` cells (`overview/net_flow.rs`) are drawn the same way, from the adapter cells
//! the box keeps.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Size, StyleRefinement, Styled as _, Subscription, Window, div, px, size,
};
use tl_model::snapshot::ComponentMask;
use tl_ui_settings::layout::{
    DeviceSelector, FormSet, GroupItems, GroupKind, GroupSelector, MetricKey, SourceSelector,
    WidgetContent, WidgetSetting,
};
use tl_ui_settings::layout_boxes::{DeviceKind, METRIC_FREQ, METRIC_LOAD};

use super::compose::{Composition, group_item_composition};
use super::group_fill::{GroupFill, fitted_into, group_fill, rows_fill_min};
use super::members::{Members, members};
use super::meta::MetricMeta;
use super::metric_widget::{MemberLook, MetricWidget, part_id, widget_id};
use super::source::{SourceData, SourceStore};
use crate::chart::ChartView;
use crate::chart::labels::WARMING;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::pages::performance::overview::cpu_box::core_charts_element;
use crate::pages::performance::overview::disk_box::{
    DiskModel, disk_cells_element, disk_rows_element,
};
use crate::pages::performance::overview::layout::Arrange;
use crate::pages::performance::overview::net_box::{NetModelRef, net_cells_element};
use crate::pages::performance::overview::parts::{
    CELL_MIN_W as CORE_CELL_MIN_W, CoreBarsLayout, CoreBarsModel, clock_bucket, clock_text,
    core_board,
};
use crate::pages::performance::overview::picked::{box_mask, box_snapshot};
use crate::pages::performance::overview::timeline::OverviewPause;
use crate::selector::TlSelector;
use crate::settings_global::TlSettings;
use crate::text::style::t_sm;
use crate::theme::{UiMetrics, current, metrics};

/// The edge every item keeps free in Edit Widgets, px at every scale: the selected outline.
const EDIT_EDGE: f32 = super::edit::OUTLINE;
/// The right edge the items keep free in Edit Widgets, logical px: the resize corner.
const EDIT_RIGHT: f32 = super::edit::CORNER;
/// The width an empty group's lines keep free in Edit Widgets, from the widget's left edge:
/// the grip and the gear, each `edit::HANDLE` wide, logical px.
const EDIT_LEAD: f32 = 2. * super::edit::HANDLE;

const KEY_MORE: &str = "perf-overview-more";
const KEY_NONE: &str = "perf-widget-group-none";
const KEY_NOT_CONNECTED: &str = "perf-widget-not-connected";
const KEY_UNKNOWN: &str = "perf-widget-unknown";

/// The parts of a snapshot the member list reads: the catalog, and for engines the GPU
/// records (the engines a GPU reports) and the inventory (its vendor).
const MEMBERS_MASK: ComponentMask =
    ComponentMask(ComponentMask::DEVICES.0 | ComponentMask::SYSTEM.0 | ComponentMask::INVENTORY.0);

/// The widget id of the item a group's member gets: unique among the group's items and apart
/// from every widget id a container gives, so the items' debug selectors do not meet. `serial`
/// counts the items the group made.
pub fn member_widget_id(group: u32, serial: u32) -> u32 {
    0x8000_0000 | ((group & 0x7fff) << 16) | (serial & 0xffff)
}

/// The debug selector and element id of the scrolling items of group `id` in container
/// `container`.
pub fn scroll_id(container: &str, id: u32) -> String {
    format!("perf-widget-{container}-{id}-scroll")
}

/// The group's state after its member list was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupState {
    /// It shows its members (`count` of them, before the limit).
    Members { count: usize },
    /// No members: "None found".
    NoneFound,
    /// The group's own device is gone: "Not connected".
    Gone,
    /// An unknown group kind: "Needs a newer tasklens".
    Unknown,
}

/// How a group widget of a host-drawn flow layout draws: the CPU box part of 1.2.0 (ADR-019
/// "Presets", UI-PRF-019). The host keeps the core rows' model (smoothed load, clock, names) and
/// one sparkline per logical CPU, and hands them in; the group draws them.
#[derive(Clone)]
pub enum GroupFlowLook {
    /// The core rows (`parts::core_board`): name, sparkline, meter, percent and clock, filling
    /// the height (D-040). `layout` takes the layout the last render used; `grid_size` is the
    /// rows' size at the last paint.
    CoreRows {
        bars: Rc<RefCell<CoreBarsModel>>,
        sparks: Rc<[Entity<ChartView>]>,
        grid_size: Rc<Cell<Size<Pixels>>>,
        layout: Rc<Cell<CoreBarsLayout>>,
    },
    /// The `core_charts` cells: one filled chart per logical CPU, placed as `arrange` says.
    CoreCells {
        bars: Rc<RefCell<CoreBarsModel>>,
        sparks: Rc<[Entity<ChartView>]>,
        grid_size: Rc<Cell<Size<Pixels>>>,
        arrange: Arrange,
    },
    /// The disk sections of the Disk box's `rows` (`disk_box::disk_rows_element`): the label
    /// part takes `share` of the width, and the sections the body does not fit collapse to
    /// their headers, the busiest expanded first. `body` is the body's size at the last paint.
    DiskRows {
        model: Rc<RefCell<DiskModel>>,
        body: Rc<Cell<Size<Pixels>>>,
        share: f32,
    },
    /// The `disk_charts` cells (`disk_box::disk_cells_element`): one read and write chart per
    /// disk, placed as `arrange` says, only whole cells.
    DiskCells {
        model: Rc<RefCell<DiskModel>>,
        body: Rc<Cell<Size<Pixels>>>,
        arrange: Arrange,
    },
    /// The Network box's `adapter_charts` cells (`net_box::net_cells_element`): one combined
    /// download and upload chart per counted adapter, placed as `arrange` says, only whole
    /// cells. `body` is the body's size at the last paint.
    NetCells {
        model: NetModelRef,
        body: Rc<Cell<Size<Pixels>>>,
        arrange: Arrange,
    },
}

impl PartialEq for GroupFlowLook {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::CoreRows {
                    bars,
                    sparks,
                    grid_size,
                    layout,
                },
                Self::CoreRows {
                    bars: b,
                    sparks: s,
                    grid_size: g,
                    layout: l,
                },
            ) => {
                Rc::ptr_eq(bars, b)
                    && sparks[..] == s[..]
                    && Rc::ptr_eq(grid_size, g)
                    && Rc::ptr_eq(layout, l)
            }
            (
                Self::CoreCells {
                    bars,
                    sparks,
                    grid_size,
                    arrange,
                },
                Self::CoreCells {
                    bars: b,
                    sparks: s,
                    grid_size: g,
                    arrange: a,
                },
            ) => {
                Rc::ptr_eq(bars, b)
                    && sparks[..] == s[..]
                    && Rc::ptr_eq(grid_size, g)
                    && arrange == a
            }
            (
                Self::DiskRows { model, body, share },
                Self::DiskRows {
                    model: m,
                    body: b,
                    share: s,
                },
            ) => Rc::ptr_eq(model, m) && Rc::ptr_eq(body, b) && share == s,
            (
                Self::DiskCells {
                    model,
                    body,
                    arrange,
                },
                Self::DiskCells {
                    model: m,
                    body: b,
                    arrange: a,
                },
            ) => Rc::ptr_eq(model, m) && Rc::ptr_eq(body, b) && arrange == a,
            (
                Self::NetCells {
                    model,
                    body,
                    arrange,
                },
                Self::NetCells {
                    model: m,
                    body: b,
                    arrange: a,
                },
            ) => model == m && Rc::ptr_eq(body, b) && arrange == a,
            _ => false,
        }
    }
}

/// One shown member.
struct Item {
    selector: SourceSelector,
    widget: Entity<MetricWidget>,
    /// A core row's clock column: the logical CPU's `freq` source and its observation.
    clock: Option<(Entity<SourceData>, Subscription)>,
}

/// One group widget of a container.
pub struct GroupWidget {
    setting: WidgetSetting,
    /// The id of the container the widget is in, for its own and its items' debug selectors.
    container: SharedString,
    hub: Entity<SnapshotHub>,
    store: Rc<RefCell<SourceStore>>,
    /// The widget is in a flow layout, where `hidden_gpu_engines` applies to engines.
    flow: bool,
    state: GroupState,
    /// Every member now, in order; the first `limit` are shown.
    list: Vec<SourceSelector>,
    /// The shown members, in order.
    items: Vec<Item>,
    /// Members past the limit, counted in "+n more".
    hidden: usize,
    /// The items the group made, for their ids.
    serial: u32,
    /// The 1.2.0 part the group draws, while its host draws its flow layout.
    flow_look: Option<GroupFlowLook>,
    /// Edit Widgets is on: the items keep the edit margins free.
    edit_inset: bool,
    /// The clocks come from BogoMIPS: the core rows' clock column carries the mark (PERF-033).
    clock_mark: bool,
    size: Size<Pixels>,
    scale: f32,
    fill: GroupFill,
    more: SharedString,
    reason: SharedString,
    strings_version: u64,
    _subs: [Subscription; 5],
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: std::cell::Cell<u32>,
}

impl GroupWidget {
    /// A group widget for `setting` (whose content is a group) in container `container`, its
    /// members' sources from the page's `store`. `flow` says the widget is in a flow layout.
    pub fn new(
        container: SharedString,
        setting: WidgetSetting,
        hub: &Entity<SnapshotHub>,
        store: Rc<RefCell<SourceStore>>,
        flow: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(container, setting, hub, store, flow, None, cx)
    }

    /// A group of a flow layout that draws `look` (see [`GroupFlowLook`]) from the start, so it
    /// makes no member items.
    pub fn with_flow_look(
        container: SharedString,
        setting: WidgetSetting,
        hub: &Entity<SnapshotHub>,
        store: Rc<RefCell<SourceStore>>,
        look: Option<GroupFlowLook>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(container, setting, hub, store, true, look, cx)
    }

    fn build(
        container: SharedString,
        setting: WidgetSetting,
        hub: &Entity<SnapshotHub>,
        store: Rc<RefCell<SourceStore>>,
        flow: bool,
        flow_look: Option<GroupFlowLook>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            setting,
            container,
            hub: hub.clone(),
            store,
            flow,
            state: GroupState::NoneFound,
            list: Vec::new(),
            items: Vec::new(),
            hidden: 0,
            serial: 0,
            flow_look,
            edit_inset: false,
            // From the current snapshot, so a widget made after the first clock sample (Edit
            // Widgets, Apply) shows the BogoMIPS mark and its column width from the start.
            clock_mark: box_snapshot(hub, cx).system.cpu.freq_is_bogomips,
            size: size(px(0.), px(0.)),
            scale: metrics(cx).ui_scale,
            fill: GroupFill::default(),
            more: SharedString::default(),
            reason: SharedString::default(),
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::on_members_input),
                cx.observe_global::<TlSettings>(Self::on_members_input),
                cx.observe_global::<UiMetrics>(Self::on_metrics),
                cx.observe_global::<Strings>(Self::on_strings),
            ],
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
        };
        this.refresh_members(cx);
        this.sync_items(cx);
        this.format_texts(cx);
        this.relayout(cx);
        this
    }

    /// The widget id, unique in its container.
    pub fn id(&self) -> u32 {
        self.setting.id
    }

    pub fn setting(&self) -> &WidgetSetting {
        &self.setting
    }

    pub fn state(&self) -> GroupState {
        self.state
    }

    /// The group's selector.
    pub fn group(&self) -> Option<&GroupSelector> {
        match &self.setting.content {
            WidgetContent::Group(g) => Some(g),
            _ => None,
        }
    }

    /// The shown members' items, in order.
    pub fn items(&self) -> impl Iterator<Item = (&SourceSelector, &Entity<MetricWidget>)> {
        self.items.iter().map(|i| (&i.selector, &i.widget))
    }

    /// Members past the limit, counted in "+n more".
    pub fn hidden(&self) -> usize {
        self.hidden
    }

    /// Where the items go at the last layout.
    pub fn fill(&self) -> &GroupFill {
        &self.fill
    }

    /// The "+n more" text, empty without one.
    pub fn more_text(&self) -> &SharedString {
        &self.more
    }

    /// The centred line of an empty, gone or unknown group.
    pub fn reason_text(&self) -> &SharedString {
        &self.reason
    }

    /// New settings for the same widget id (a layout change keeps the entity). The items whose
    /// member stays keep their entities.
    pub fn set_setting(&mut self, setting: WidgetSetting, cx: &mut Context<Self>) {
        if self.setting == setting {
            return;
        }
        self.setting = setting;
        self.refresh_members(cx);
        self.sync_items(cx);
        self.format_texts(cx);
        self.relayout(cx);
        self.notify(cx);
    }

    /// The 1.2.0 part the group draws for its host, if any.
    pub fn flow_look(&self) -> Option<&GroupFlowLook> {
        self.flow_look.as_ref()
    }

    /// The host draws the group's flow layout and gives the part it draws, or `None` when the
    /// group draws its own items again. Items are made or dropped to match, and the group
    /// renders again only when the look changed.
    pub fn set_flow_look(&mut self, look: Option<GroupFlowLook>, cx: &mut Context<Self>) {
        if self.flow_look == look {
            return;
        }
        let had = self.flow_look.is_some();
        self.flow_look = look;
        if had != self.flow_look.is_some() {
            self.sync_items(cx);
            self.format_texts(cx);
            self.relayout(cx);
        }
        self.notify(cx);
    }

    /// The host changed what its look draws (a row's text or bucket): one render of this group.
    pub fn flow_changed(&mut self, cx: &mut Context<Self>) {
        if self.flow_look.is_some() {
            self.notify(cx);
        }
    }

    /// Whether the widget is in a flow layout.
    pub fn set_flow(&mut self, flow: bool, cx: &mut Context<Self>) {
        if self.flow == flow {
            return;
        }
        self.flow = flow;
        if self.refresh_members(cx) {
            self.after_members(cx);
        }
    }

    /// Edit Widgets on or off. While on, the items lie inside the outline's 2 px and left of
    /// the resize corner, and the first item's first line starts right of the grip and gear.
    /// The column count and the item kind stay those of the applied view.
    pub fn set_edit_inset(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.edit_inset == on {
            return;
        }
        self.edit_inset = on;
        self.relayout(cx);
        self.notify(cx);
    }

    /// The size the container places the widget at.
    pub fn set_size(&mut self, area: Size<Pixels>, cx: &mut Context<Self>) {
        if self.size == area {
            return;
        }
        self.size = area;
        if self.relayout(cx) {
            self.notify(cx);
        }
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A snapshot that changes the catalog (or, for engines, the GPU records) may change the
    /// members. Any other sample does nothing here: it reaches the member items only.
    fn on_snapshot(
        &mut self,
        _: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let mask = box_mask(ev.mask, cx);
        let engines = self
            .group()
            .is_some_and(|g| g.members == tl_ui_settings::layout::GroupKind::Engines);
        let wanted = if engines {
            MEMBERS_MASK
        } else {
            ComponentMask::DEVICES
        };
        if mask.intersects(wanted) && self.refresh_members(cx) {
            self.after_members(cx);
        }
    }

    /// The pause, or a settings change (visible cards, `device_order`, `show_network_*`, hidden
    /// engines): the members may change.
    fn on_members_input(&mut self, cx: &mut Context<Self>) {
        if self.refresh_members(cx) {
            self.after_members(cx);
        }
    }

    /// A new UI scale: the items' minimums change, and in Edit Widgets an empty group's lines
    /// move to stay clear of the grip, the gear and the resize corner.
    fn on_metrics(&mut self, cx: &mut Context<Self>) {
        if metrics(cx).ui_scale == self.scale {
            return;
        }
        let empty_in_edit = self.edit_inset && !matches!(self.state, GroupState::Members { .. });
        if self.relayout(cx) || empty_in_edit {
            self.notify(cx);
        }
    }

    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.format_texts(cx);
        self.notify(cx);
    }

    /// The member list changed: new items, new texts, a new fit, and one render of this group.
    fn after_members(&mut self, cx: &mut Context<Self>) {
        self.sync_items(cx);
        self.format_texts(cx);
        self.relayout(cx);
        self.notify(cx);
    }

    /// Reads the member list. Returns whether the list, the state or the count past the limit
    /// changed.
    fn refresh_members(&mut self, cx: &App) -> bool {
        let (state, list) = match &self.setting.content {
            WidgetContent::Group(g) => {
                let snap = box_snapshot(&self.hub, cx);
                let perf = cx
                    .try_global::<TlSettings>()
                    .map(|_| &TlSettings::settings(cx).performance);
                let slot_key = match &g.device {
                    DeviceSelector::Slot(n) => self.store.borrow().slot(*n),
                    DeviceSelector::Key(_) => None,
                };
                match members(g, &snap, perf, slot_key.as_deref(), self.flow) {
                    Members::List(l) if l.is_empty() => (GroupState::NoneFound, l),
                    Members::List(l) => (GroupState::Members { count: l.len() }, l),
                    Members::Gone => (GroupState::Gone, Vec::new()),
                    Members::Unknown => (GroupState::Unknown, Vec::new()),
                }
            }
            _ => (GroupState::Unknown, Vec::new()),
        };
        if state == self.state && list == self.list {
            return false;
        }
        self.state = state;
        self.list = list;
        true
    }

    /// The members the group shows: the first `limit` of the list. None while the host draws
    /// the group's look: the host's model holds the values then.
    fn shown(&self) -> &[SourceSelector] {
        if self.flow_look.is_some() {
            return &[];
        }
        let limit = self
            .group()
            .and_then(|g| g.options.limit)
            .map_or(usize::MAX, usize::from);
        &self.list[..self.list.len().min(limit)]
    }

    /// The forms the items draw: the group's forms the member metric offers, else the group's.
    fn item_forms(&self) -> FormSet {
        let offered = self
            .shown()
            .first()
            .and_then(|s| MetricMeta::of(DeviceKind::of(&s.device), &s.metric))
            .map_or(FormSet::all(), |m| m.forms());
        let forms = self.setting.forms & offered;
        if forms.is_empty() {
            self.setting.forms
        } else {
            forms
        }
    }

    fn items_kind(&self) -> GroupItems {
        self.group().map_or(GroupItems::Rows, |g| g.options.items)
    }

    /// Whether the items are the CPU box's core rows: a cores group of `load` drawn as Rows. They
    /// look as the box's core rows in every layout, with the clock column (tasklens-6swdr.12).
    fn core_rows(&self) -> bool {
        self.group().is_some_and(|g| {
            g.members == GroupKind::Cores
                && g.metric.as_str() == METRIC_LOAD
                && g.options.items == GroupItems::Rows
        })
    }

    /// The clock source of a core row's member: its logical CPU's `freq`.
    fn clock_selector(selector: &SourceSelector) -> SourceSelector {
        SourceSelector {
            device: selector.device.clone(),
            metric: MetricKey::new(METRIC_FREQ),
        }
    }

    /// Takes or drops the clock source of `item` as the group's items are core rows or not, and
    /// gives its widget the clock text.
    fn sync_clock(&mut self, at: usize, cx: &mut Context<Self>) {
        let core = self.core_rows();
        let item = &mut self.items[at];
        if !core {
            item.clock = None;
            return;
        }
        if item.clock.is_none() {
            let selector = Self::clock_selector(&item.selector);
            let source = self.store.borrow_mut().source(&selector, cx);
            let sub = cx.observe(&source, Self::on_clock);
            item.clock = Some((source, sub));
        }
        let clock = self.clock_text(at, cx);
        self.items[at]
            .widget
            .update(cx, |w, cx| w.set_clock(clock, cx));
    }

    /// The clock column's text of item `at`: "3.9 GHz", "800 MHz", or "…" with no valid sample,
    /// as the core rows.
    fn clock_text(&self, at: usize, cx: &App) -> SharedString {
        let mhz = self.items[at]
            .clock
            .as_ref()
            .and_then(|(source, _)| source.read(cx).value());
        match mhz {
            Some(v) => clock_text(clock_bucket(v), self.clock_mark),
            None => SharedString::new_static(WARMING),
        }
    }

    /// A core's clock changed: only that core's item renders again, when its text changed.
    fn on_clock(&mut self, source: Entity<SourceData>, cx: &mut Context<Self>) {
        let mark = box_snapshot(&self.hub, cx).system.cpu.freq_is_bogomips;
        if mark != self.clock_mark {
            // Every clock text and the column's width change.
            self.clock_mark = mark;
            for at in 0..self.items.len() {
                self.sync_clock(at, cx);
            }
            if self.relayout(cx) {
                self.notify(cx);
            }
            return;
        }
        let at = self
            .items
            .iter()
            .position(|i| i.clock.as_ref().is_some_and(|(s, _)| *s == source));
        if let Some(at) = at {
            let clock = self.clock_text(at, cx);
            self.items[at]
                .widget
                .update(cx, |w, cx| w.set_clock(clock, cx));
        }
    }

    fn arrange(&self) -> Arrange {
        self.group()
            .and_then(|g| g.options.arrange.as_deref())
            .and_then(Arrange::from_id)
            .unwrap_or(Arrange::Auto)
    }

    /// The setting of the item that shows `selector`.
    fn item_setting(&self, id: u32, selector: &SourceSelector) -> WidgetSetting {
        WidgetSetting {
            id,
            content: WidgetContent::Metric(selector.clone()),
            forms: self.setting.forms,
            cell: None,
            options: self.setting.options,
            device_name: None,
        }
    }

    /// Brings the items in line with the shown members: kept members keep their entities and
    /// take the group's forms and options, new ones get entities, gone ones drop theirs.
    fn sync_items(&mut self, cx: &mut Context<Self>) {
        let shown: Vec<SourceSelector> = self.shown().to_vec();
        self.hidden = if self.flow_look.is_some() {
            0
        } else {
            self.list.len() - shown.len()
        };
        let mut old = std::mem::take(&mut self.items);
        let mut items = Vec::with_capacity(shown.len());
        for selector in shown {
            let item = match old.iter().position(|i| i.selector == selector) {
                Some(at) => {
                    let item = old.swap_remove(at);
                    let id = item.widget.read(cx).id();
                    let setting = self.item_setting(id, &selector);
                    let source = item.widget.read(cx).source().clone();
                    item.widget
                        .update(cx, |w, cx| w.set_setting(setting, source, cx));
                    item
                }
                None => {
                    self.serial = self.serial.wrapping_add(1);
                    let id = member_widget_id(self.setting.id, self.serial);
                    let setting = self.item_setting(id, &selector);
                    let source = self.store.borrow_mut().source(&selector, cx);
                    let (hub, container) = (self.hub.clone(), self.container.clone());
                    let widget =
                        cx.new(|cx| MetricWidget::new(container, setting, source, &hub, cx));
                    Item {
                        selector,
                        widget,
                        clock: None,
                    }
                }
            };
            items.push(item);
        }
        self.items = items;
        for at in 0..self.items.len() {
            self.sync_clock(at, cx);
        }
    }

    /// The "+n more" text and the centred line.
    fn format_texts(&mut self, cx: &App) {
        self.more = if self.hidden > 0 {
            tr_args(
                cx,
                KEY_MORE,
                &[("n", FluentValue::from(self.hidden as i64))],
            )
        } else {
            SharedString::default()
        };
        self.reason = match self.state {
            GroupState::Members { .. } => SharedString::default(),
            GroupState::NoneFound => tr(cx, KEY_NONE),
            GroupState::Gone => tr(cx, KEY_NOT_CONNECTED),
            GroupState::Unknown => tr(cx, KEY_UNKNOWN),
        };
    }

    /// The rectangle the items lie in, from the top left of the widget: in Edit Widgets inside
    /// the outline's 2 px and left of the resize corner.
    fn inner(&self) -> Bounds<Pixels> {
        let (w, h) = (f32::from(self.size.width), f32::from(self.size.height));
        if !self.edit_inset {
            return Bounds::new(gpui_kit::point(px(0.), px(0.)), self.size);
        }
        let right = EDIT_RIGHT * self.scale;
        Bounds::new(
            gpui_kit::point(px(EDIT_EDGE), px(EDIT_EDGE)),
            size(
                px((w - EDIT_EDGE - right).max(0.)),
                px((h - 2. * EDIT_EDGE).max(0.)),
            ),
        )
    }

    /// Fits the items into the widget and gives each its size and look. Returns whether the
    /// fit changed.
    fn relayout(&mut self, cx: &mut Context<Self>) -> bool {
        let scale = metrics(cx).ui_scale;
        self.scale = scale;
        let forms = self.item_forms();
        let kind = self.items_kind();
        let n = self.items.len() + usize::from(self.hidden > 0);
        let core = self.core_rows();
        // Edit Widgets keeps the applied view's columns and item kind inside its margins.
        let applied = if core {
            rows_fill_min(n, self.size, CORE_CELL_MIN_W, scale)
        } else {
            group_fill(kind, self.arrange(), n, self.size, forms, scale)
        };
        let fill = if self.edit_inset {
            fitted_into(&applied, self.size, self.inner().size)
        } else {
            applied
        };
        let composition = group_item_composition(kind, forms);
        let edit = self.edit_inset;
        for (i, item) in self.items.iter().enumerate() {
            let area = fill.items.get(i).map_or(size(px(0.), px(0.)), |b| b.size);
            let look = MemberLook {
                composition: Some(composition),
                lead: edit && i == 0,
                core,
                mark: core && self.clock_mark,
            };
            item.widget.update(cx, |w, cx| {
                w.set_member(look, cx);
                w.set_size(area, cx);
            });
        }
        let changed = fill != self.fill;
        self.fill = fill;
        changed
    }
}

impl Render for GroupWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let id = self.setting.id;
        let container = self.container.clone();
        let root_id = widget_id(&container, id);
        let root = div()
            .tl_selector(move || root_id)
            .relative()
            .size_full()
            .overflow_hidden();
        if let Some(look) = &self.flow_look {
            let view = cx.entity_id();
            let part = match look {
                GroupFlowLook::CoreRows {
                    bars,
                    sparks,
                    grid_size,
                    layout,
                } => {
                    let (rows, grid) = core_board(bars, sparks, grid_size, &m, view);
                    layout.set(grid);
                    rows
                }
                GroupFlowLook::CoreCells {
                    bars,
                    sparks,
                    grid_size,
                    arrange,
                } => core_charts_element(bars, sparks, grid_size, *arrange, &m, &theme, view),
                GroupFlowLook::DiskRows { model, body, share } => {
                    disk_rows_element(model, body, *share, &m, &theme, view)
                }
                GroupFlowLook::DiskCells {
                    model,
                    body,
                    arrange,
                } => disk_cells_element(model, body, *arrange, &m, &theme, view),
                GroupFlowLook::NetCells {
                    model,
                    body,
                    arrange,
                } => net_cells_element(model, body, *arrange, &m, &theme, view),
            };
            return root.child(part);
        }
        if !matches!(self.state, GroupState::Members { .. }) {
            let mut name = String::new();
            if self.state == GroupState::Gone
                && let Some(d) = &self.setting.device_name
            {
                name.push_str(d);
            }
            let line = |part: &str, text: SharedString| {
                let mut el = div();
                *el.text_style() = t_sm(&m);
                let p = part_id(&container, id, part);
                el.tl_selector(move || p)
                    .max_w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(theme.text_secondary)
                    .child(text)
            };
            // In Edit Widgets the lines lie inside the outline and left of the resize corner,
            // and start right of the grip and the gear, so no edit control covers them.
            let mut body = div().size_full();
            if self.edit_inset {
                body = body
                    .pt(px(EDIT_EDGE))
                    .pb(px(EDIT_EDGE))
                    .pl(px(EDIT_LEAD * m.ui_scale))
                    .pr(px(EDIT_RIGHT * m.ui_scale));
            }
            body = body.flex().flex_col().items_center().justify_center();
            if !name.is_empty() {
                body = body.child(line("name", name.into()));
            }
            return root.child(body.child(line("reason", self.reason.clone())));
        }
        let inner = self.inner();
        let cells = self.fill.composition != Composition::Row;
        let items = self.items.iter().zip(&self.fill.items).map(|(item, b)| {
            div()
                .absolute()
                .left(b.left())
                .top(b.top())
                .w(b.size.width)
                .h(b.size.height)
                .child(
                    item.widget
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
                .into_any_element()
        });
        let more = (self.hidden > 0)
            .then(|| self.fill.items.get(self.items.len()).copied())
            .flatten()
            .map(|b| {
                let mut el = div();
                *el.text_style() = t_sm(&m);
                let p = part_id(&container, id, "more");
                let el = el
                    .tl_selector(move || p)
                    .absolute()
                    .left(b.left())
                    .top(b.top())
                    .w(b.size.width)
                    .h(b.size.height)
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(theme.text_secondary)
                    .child(self.more.clone());
                if cells {
                    el.justify_center().into_any_element()
                } else {
                    el.into_any_element()
                }
            });
        let content = div()
            .relative()
            .flex_none()
            .w(self.fill.content.width)
            .h(self.fill.content.height)
            .children(items)
            .children(more);
        let scroll = scroll_id(&container, id);
        let selector = scroll.clone();
        let body = if self.fill.scrolls_x {
            div()
                .tl_selector(move || selector)
                .size_full()
                .child(content)
                .overflow_x_scrollbar()
                .id(SharedString::from(scroll))
                .into_any_element()
        } else if self.fill.scrolls {
            div()
                .tl_selector(move || selector)
                .size_full()
                .child(content.w_full())
                .overflow_y_scrollbar()
                .id(SharedString::from(scroll))
                .into_any_element()
        } else {
            content.into_any_element()
        };
        root.child(
            div()
                .absolute()
                .left(inner.left())
                .top(inner.top())
                .w(inner.size.width)
                .h(inner.size.height)
                .child(body),
        )
    }
}
