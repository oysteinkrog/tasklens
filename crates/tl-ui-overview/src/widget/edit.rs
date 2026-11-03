//! Edit Widgets: editing the widgets inside one box (ADR-019 "Editing", UI-PRF-061).
//!
//! Editing the boxes on the page (Edit Layout) and editing the widgets in one box (Edit Widgets)
//! are separate modes. Edit Widgets is a transaction, [`LayoutEdit`]: it works on a copy of the
//! box's layout, every action puts the previous copy on the undo stack (at most
//! [`UNDO_LIMIT`] steps), and only Apply writes settings. Cancel and Esc put back the layout the
//! box showed before.
//!
//! [`WidgetEditor`] is the entity a box hosts around its [`WidgetContainer`]. Outside Edit
//! Widgets it only draws the container. In Edit Widgets it adds:
//!
//! - the 32 px edit toolbar, always one line: Add Widget, Undo and Reset as icon buttons with
//!   tooltips at the left, Cancel (icon) and Apply at the right;
//! - the edit chrome over each widget: a 1 px divider border (2 px accent when selected), the
//!   grip and the gear at the top left and the resize corner at the bottom right. The widget
//!   keeps its text clear of all of it (`MetricWidget::set_edit_inset`), and the chrome lets
//!   the wheel through, so the body scrolls as in the applied view;
//! - the placement preview of a drag: an accent outline of the target cells, the widget dimmed
//!   in them, and the other widgets where the drop puts them. The preview is the drop's layout,
//!   so no widget overlaps another and the outline lies in the target's text-free margin;
//! - the keys of the `PerfWidgetEdit` context (Tab, Alt+arrows, Alt+Shift+arrows, Delete,
//!   Ctrl+Z, Enter, Esc).
//!
//! Entering Edit Widgets on a flow layout pins it into a grid layout (`grid::pin`). The
//! container keeps the entities of widgets whose id stays, so Cancel creates none again.
//!
//! A device page hosts an editor too (ADR-019 phase 3): its toolbar sits between the page
//! header and the body, its Add Widget lists the page's own device and the page parts it does
//! not hold yet (Details first), and its Reset asks with the page's name. At most one container
//! of a window is in Edit Widgets: [`WidgetEditor::start`] does nothing while another is.
//!
//! Render contract (UI-PRF-017, UI-PRF-019): the editor never listens to samples. It renders on
//! an edit action, a drag step and a container layout change while editing.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, EntityId, EventEmitter, FocusHandle, Global,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, WeakEntity, Window, canvas, div,
    point, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_ui_settings::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, DISK_BOX_CONTAINER, DeviceSelector, FixedWidget, FormSet,
    LAYOUT_VERSION, MEMORY_BOX_CONTAINER, MetricKey, NET_BOX_CONTAINER, PROCESSES_BOX_CONTAINER,
    SourceSelector, TIMELINE_BOX_CONTAINER, WidgetCell, WidgetContent, WidgetOptions,
    WidgetSetting, default_box_layout, overview_kind, reset_to_default,
};
use tl_ui_settings::layout_boxes::DeviceKind;
use tl_ui_settings::layout_pages::default_page_layout;

use super::container::WidgetContainer;
use super::fixed_widget::PAGE_FIXED_NAMES;
use super::grid::{self, Drawn, InnerGrid, SlotDevice};
use super::menu::{self, OPTION_DASHED, OPTION_FILL, SourceGroup};
use super::meta::MetricMeta;
use super::source::Availability;
use crate::overview::picked::box_snapshot;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::tr;
use tl_ui_base::selector::TlSelector;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::{current, metrics};
use tl_ui_kit::components::button::{TlButton, TlButtonKind, TlButtonSize};
use tl_ui_kit::components::icons::TlIcon;
use tl_ui_kit::components::menu::{TlMenu, open_context_menu_at};
use tl_ui_kit::components::tooltip::TlTooltip;
use tl_ui_perf_core::core::actions::{
    PERF_WIDGET_EDIT_CONTEXT, PerfWidgetAdd, PerfWidgetAddFixed, PerfWidgetAddGroup,
    PerfWidgetApply, PerfWidgetCancel, PerfWidgetMove, PerfWidgetRemove, PerfWidgetReset,
    PerfWidgetResize, PerfWidgetSelectNext, PerfWidgetSelectPrev, PerfWidgetSetForm,
    PerfWidgetSetGroup, PerfWidgetSetOption, PerfWidgetSetSource, PerfWidgetUndo, WidgetSource,
};

/// The most undo steps an edit keeps.
pub const UNDO_LIMIT: usize = 100;

/// The fixed widgets Add Widget offers, each in its own box, with their name keys
/// (UI-PRF-061 phase 2).
const FIXED_ADDS: [(FixedWidget, &str); 3] = [
    (FixedWidget::MemoryByApp, "perf-widget-fixed-by-app"),
    (FixedWidget::Processes, "perf-widget-fixed-processes"),
    (FixedWidget::Timeline, "perf-widget-fixed-timeline"),
];
/// The edit toolbar's height, logical px.
pub const TOOLBAR_H: f32 = 32.;
/// The grip and the gear, logical px. The gear sits right of the grip.
pub const HANDLE: f32 = 16.;
/// The resize corner, logical px.
pub const CORNER: f32 = 10.;
/// The selected widget's outline and the preview's outline, px at every scale.
pub const OUTLINE: f32 = 2.;
/// The corner mark's stroke, logical px.
const MARK: f32 = 1.5;
/// The pointer moves this far before a press becomes a drag, logical px.
const DRAG_START: f32 = 3.;
/// The opacity of the preview outline.
const PREVIEW_OPACITY: f32 = 0.8;
/// How much of the background covers a dragged widget in its target cells, so it shows at 0.3.
const DIM: f32 = 0.7;

/// Debug selectors and element ids.
pub const TOOLBAR_ID: &str = "perf-widget-edit-toolbar";
pub const ADD_ID: &str = "perf-widget-edit-add";
pub const UNDO_ID: &str = "perf-widget-edit-undo";
pub const RESET_ID: &str = "perf-widget-edit-reset";
pub const CANCEL_ID: &str = "perf-widget-edit-cancel";
pub const APPLY_ID: &str = "perf-widget-edit-apply";
pub const PREVIEW_ID: &str = "perf-widget-edit-preview";

/// The debug selector of the editor of container `id`.
pub fn editor_id(id: &str) -> String {
    format!("perf-widget-editor-{id}")
}

/// The debug selector of the edit frame of widget `id` in container `container`.
pub fn frame_id(container: &str, id: u32) -> String {
    format!("perf-widget-edit-frame-{container}-{id}")
}

/// The debug selector of the grip of widget `id` in container `container`.
pub fn grip_id(container: &str, id: u32) -> String {
    format!("perf-widget-edit-grip-{container}-{id}")
}

/// The debug selector of the gear of widget `id` in container `container`.
pub fn gear_id(container: &str, id: u32) -> String {
    format!("perf-widget-edit-gear-{container}-{id}")
}

/// The debug selector of the outline of the selected widget `id` in container `container`.
pub fn outline_id(container: &str, id: u32) -> String {
    format!("perf-widget-edit-outline-{container}-{id}")
}

/// The debug selector of the resize corner of widget `id` in container `container`.
pub fn corner_id(container: &str, id: u32) -> String {
    format!("perf-widget-edit-corner-{container}-{id}")
}

/// The built-in default layout of `container`, which no entry stands for: the Overview box's
/// default preset, a device page's default flow layout, else no widgets.
pub fn default_layout(container: &str) -> ContainerLayout {
    if let Some(layout) = overview_kind(container).and_then(default_box_layout) {
        return layout;
    }
    if let Some(layout) = default_page_layout(container) {
        return layout;
    }
    ContainerLayout {
        container: container.to_owned(),
        version: LAYOUT_VERSION,
        preset: None,
        widgets: Vec::new(),
        next_id: 1,
    }
}

/// The saved entry of `container`; `None` when there is none (the built-in default).
pub fn saved_layout(container: &str, cx: &App) -> Option<ContainerLayout> {
    cx.has_global::<TlSettings>()
        .then(|| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(container)
                .cloned()
        })
        .flatten()
}

/// Saves `working` as `container`'s layout: its entry, or the default for `None` (ADR-019
/// "Saving the default").
fn save(container: &str, working: Option<&ContainerLayout>, cx: &mut App) {
    if !cx.has_global::<TlSettings>() {
        return;
    }
    TlSettings::update(cx, |d| {
        let perf = &mut d.settings.performance;
        match working {
            Some(layout) => perf.layouts.set(layout.clone()),
            None => reset_to_default(perf, container),
        }
    });
}

/// The transaction of one Edit Widgets session (ADR-019 "Editing"). `None` stands for no entry
/// (the built-in default).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutEdit {
    pub container: String,
    /// The saved entry when editing started.
    pub before: Option<ContainerLayout>,
    /// The layout being edited.
    pub working: Option<ContainerLayout>,
    /// The earlier working copies, oldest first, at most [`UNDO_LIMIT`].
    pub undo: Vec<Option<ContainerLayout>>,
    pub selected: Option<u32>,
    /// The working copy when editing started (a flow layout pinned to the grid).
    start: Option<ContainerLayout>,
}

impl LayoutEdit {
    /// An edit of `container` whose saved entry is `before`, starting from `working`.
    pub fn new(
        container: impl Into<String>,
        before: Option<ContainerLayout>,
        working: Option<ContainerLayout>,
    ) -> Self {
        Self {
            container: container.into(),
            before,
            start: working.clone(),
            working,
            undo: Vec::new(),
            selected: None,
        }
    }

    /// Whether the working copy differs from where editing started: Apply is enabled.
    pub fn changed(&self) -> bool {
        self.working != self.start
    }

    /// Whether Apply writes settings: the working copy changed and is not the saved entry.
    pub fn saves(&self) -> bool {
        self.changed() && self.working != self.before
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Makes `next` the working copy and puts the old one on the undo stack. Returns false and
    /// keeps everything when `next` is the working copy.
    pub fn push(&mut self, next: Option<ContainerLayout>) -> bool {
        if next == self.working {
            return false;
        }
        let previous = std::mem::replace(&mut self.working, next);
        self.undo.push(previous);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.keep_selection();
        true
    }

    /// The previous working copy comes back. False when there is none.
    pub fn step_back(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.working = previous;
        self.keep_selection();
        true
    }

    /// A selection whose widget left the working copy ends.
    fn keep_selection(&mut self) {
        let kept = self.selected.is_some_and(|id| {
            self.working
                .as_ref()
                .is_some_and(|l| l.widgets.iter().any(|w| w.id == id))
        });
        if !kept {
            self.selected = None;
        }
    }

    /// The widget ids of the working copy in reading order: by row, then column.
    pub fn reading_order(&self) -> SmallVec<[u32; 16]> {
        self.working.as_ref().map(reading_order).unwrap_or_default()
    }

    /// Selects the next widget in reading order (`forward`) or the previous one, wrapping
    /// around. With none selected, the first or the last.
    pub fn select_step(&mut self, forward: bool) -> Option<u32> {
        let order = self.reading_order();
        self.select_step_in(&order, forward)
    }

    /// [`LayoutEdit::select_step`] over the widget ids `order`.
    pub fn select_step_in(&mut self, order: &[u32], forward: bool) -> Option<u32> {
        if order.is_empty() {
            self.selected = None;
            return None;
        }
        let n = order.len();
        let at = self
            .selected
            .and_then(|id| order.iter().position(|o| *o == id));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        self.selected = Some(order[next]);
        self.selected
    }
}

/// The widget ids of `layout` that have cells, in reading order: by row, then column.
pub fn reading_order(layout: &ContainerLayout) -> SmallVec<[u32; 16]> {
    let mut cells: SmallVec<[(u16, u16, u32); 16]> = layout
        .widgets
        .iter()
        .filter_map(|w| w.cell.map(|c| (c.row, c.col, w.id)))
        .collect();
    cells.sort_unstable();
    cells.into_iter().map(|(_, _, id)| id).collect()
}

/// Removes widget `id` from `layout`. Its id is not given out again (`next_id` stays).
pub fn remove_widget(layout: &mut ContainerLayout, id: u32) -> bool {
    let before = layout.widgets.len();
    layout.widgets.retain(|w| w.id != id);
    layout.widgets.len() != before
}

/// Turns `form` of widget `id` on or off. The form set never becomes empty. The widget then
/// spans at least its new minimum on `grid`.
pub fn set_widget_form(
    layout: &mut ContainerLayout,
    id: u32,
    form: FormSet,
    on: bool,
    grid: &InnerGrid,
) -> bool {
    let Some(w) = layout.widgets.iter_mut().find(|w| w.id == id) else {
        return false;
    };
    let mut forms = w.forms;
    forms.set(form, on);
    if forms.is_empty() || forms == w.forms {
        return false;
    }
    w.forms = forms;
    if let Some(cell) = w.cell {
        grid::set_cell(layout, id, cell, grid);
    }
    true
}

/// Sets a chart option of widget `id`: `fill` or `dashed`.
pub fn set_widget_option(layout: &mut ContainerLayout, id: u32, option: &str, on: bool) -> bool {
    let Some(w) = layout.widgets.iter_mut().find(|w| w.id == id) else {
        return false;
    };
    let before = w.options;
    match option {
        OPTION_FILL => w.options.fill = on,
        OPTION_DASHED => w.options.dashed = on,
        _ => return false,
    }
    w.options != before
}

/// Gives widget `id` the source `source` on the device `name`, whose metric offers `offered`.
/// The widget keeps the forms the new source offers, or takes its default forms when none is
/// left, and then spans at least its new minimum on `grid`.
pub fn set_widget_source(
    layout: &mut ContainerLayout,
    id: u32,
    source: SourceSelector,
    name: Option<Arc<str>>,
    offered: FormSet,
    grid: &InnerGrid,
) -> bool {
    let Some(w) = layout.widgets.iter_mut().find(|w| w.id == id) else {
        return false;
    };
    if w.content.metric() == Some(&source) {
        return false;
    }
    let kept = w.forms & offered;
    w.forms = if kept.is_empty() {
        menu::default_forms(offered)
    } else {
        kept
    };
    w.content = WidgetContent::Metric(source);
    w.device_name = name;
    if let Some(cell) = w.cell {
        grid::set_cell(layout, id, cell, grid);
    }
    true
}

/// The cell a drag of `grab` that started on `start` gives after the pointer moved `delta` px,
/// with cells `pitch` px apart (cell plus gutter) on `cols` columns. A move keeps the span and
/// stays inside the columns; a resize keeps the top left and spans at least one cell.
pub fn drag_target(
    grab: Grab,
    start: WidgetCell,
    delta: (f32, f32),
    pitch: (f32, f32),
    cols: u16,
) -> WidgetCell {
    let steps = |d: f32, p: f32| if p > 0. { (d / p).round() as i32 } else { 0 };
    let (dx, dy) = (steps(delta.0, pitch.0), steps(delta.1, pitch.1));
    let shift = |v: u16, d: i32, lo: i32, hi: i32| (i32::from(v) + d).clamp(lo, hi.max(lo)) as u16;
    let cols = i32::from(cols.max(1));
    match grab {
        Grab::Move => WidgetCell {
            col: shift(start.col, dx, 0, cols - i32::from(start.cols)),
            row: shift(start.row, dy, 0, i32::from(u16::MAX - start.rows)),
            ..start
        },
        Grab::Resize => WidgetCell {
            cols: shift(start.cols, dx, 1, cols - i32::from(start.col)),
            rows: shift(start.rows, dy, 1, i32::from(u16::MAX - start.row)),
            ..start
        },
    }
}

/// What a press on a widget's edit chrome grabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    /// The widget or its grip: the widget moves.
    Move,
    /// The resize corner: the right and bottom edges move.
    Resize,
}

/// A drag of one widget in Edit Widgets.
struct WidgetDrag {
    id: u32,
    grab: Grab,
    start: Point<Pixels>,
    start_cell: WidgetCell,
    /// The working copy when the drag started.
    base: ContainerLayout,
    moved: bool,
    /// The layout the drop gives, once the pointer moved.
    preview: Option<ContainerLayout>,
}

impl WidgetDrag {
    /// Where the dragged widget lands.
    fn target(&self) -> Option<WidgetCell> {
        self.preview
            .as_ref()?
            .widgets
            .iter()
            .find(|w| w.id == self.id)?
            .cell
    }
}

/// Edit Widgets started or ended. A host saves nothing itself: Apply has saved already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetEditEvent {
    Started,
    /// `applied`: Apply saved a change.
    Ended {
        applied: bool,
    },
}

/// The editors of every container, so the page can start Edit Widgets on a box and tell
/// whether a box is in it. Keyed by the page's hub, since each host window has its own page.
#[derive(Default)]
struct WidgetEditors(Vec<(EntityId, SharedString, WeakEntity<WidgetEditor>)>);

impl Global for WidgetEditors {}

/// Bumped each time a container starts or ends Edit Widgets, so views that follow the mode (the
/// sidebar's Select Pages) can observe it.
#[derive(Clone, Copy, Debug, Default)]
pub struct WidgetEditMode(pub u64);

impl Global for WidgetEditMode {}

fn bump_edit_mode(cx: &mut App) {
    let next = cx.try_global::<WidgetEditMode>().map_or(1, |g| g.0 + 1);
    cx.set_global(WidgetEditMode(next));
}

/// The editor of `container` on the page of `hub`.
pub fn editor_of(
    hub: &Entity<SnapshotHub>,
    container: &str,
    cx: &App,
) -> Option<Entity<WidgetEditor>> {
    let editors = cx.try_global::<WidgetEditors>()?;
    editors
        .0
        .iter()
        .filter(|(h, c, _)| *h == hub.entity_id() && &**c == container)
        .find_map(|(_, _, e)| e.upgrade())
}

/// Whether a box of the page of `hub` is in Edit Widgets.
pub fn any_editing(hub: &Entity<SnapshotHub>, cx: &App) -> bool {
    editing_except(hub, None, cx)
}

/// Whether a container of `hub`'s window other than editor `except` is in Edit Widgets. An
/// editor asks this while it is being updated, so it must not read itself.
fn editing_except(hub: &Entity<SnapshotHub>, except: Option<EntityId>, cx: &App) -> bool {
    cx.try_global::<WidgetEditors>().is_some_and(|editors| {
        editors
            .0
            .iter()
            .filter(|(h, _, _)| *h == hub.entity_id())
            .filter_map(|(_, _, e)| e.upgrade())
            .filter(|e| Some(e.entity_id()) != except)
            .any(|e| e.read(cx).is_editing())
    })
}

/// The editor around one container.
pub struct WidgetEditor {
    id: SharedString,
    container: Entity<WidgetContainer>,
    hub: Entity<SnapshotHub>,
    focus: FocusHandle,
    /// The GPUs in slot 0 and slot 1, for pinning a flow layout and for the source lists.
    slots: Vec<SlotDevice>,
    /// A device page's header type word ("Aggregate storage"), for its Reset dialog.
    page_name: Option<SharedString>,
    /// The page parts the page can show now; Add Widget lists the ones it does not hold.
    page_parts: Vec<FixedWidget>,
    edit: Option<LayoutEdit>,
    /// The layout the container showed when editing started; Cancel puts it back.
    shown_before: Option<ContainerLayout>,
    drag: Option<WidgetDrag>,
    /// The body size at the last paint.
    body: Cell<(Pixels, Pixels)>,
    /// The scroll position of the editor's body.
    scroll: ScrollHandle,
    add_bounds: Rc<Cell<Bounds<Pixels>>>,
    _container_sub: gpui_kit::Subscription,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl EventEmitter<WidgetEditEvent> for WidgetEditor {}

impl WidgetEditor {
    /// The editor of `container`, whose page reads snapshots through `hub`.
    pub fn new(
        container: Entity<WidgetContainer>,
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = container.read(cx).id().clone();
        let me = cx.weak_entity();
        let editors = cx.default_global::<WidgetEditors>();
        editors.0.retain(|(_, _, e)| e.upgrade().is_some());
        editors.0.push((hub.entity_id(), id.clone(), me));
        // The chrome follows the container's placement while editing.
        let sub = cx.observe(&container, |this: &mut Self, _, cx| {
            if this.edit.is_some() {
                cx.notify();
            }
        });
        Self {
            id,
            container,
            hub: hub.clone(),
            focus: cx.focus_handle(),
            slots: Vec::new(),
            page_name: None,
            page_parts: Vec::new(),
            edit: None,
            shown_before: None,
            drag: None,
            body: Cell::new((px(0.), px(0.))),
            scroll: ScrollHandle::new(),
            add_bounds: Rc::new(Cell::new(Bounds::default())),
            _container_sub: sub,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
        }
    }

    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The edit in progress.
    pub fn edit(&self) -> Option<&LayoutEdit> {
        self.edit.as_ref()
    }

    /// How far the body is scrolled down in Edit Widgets, logical px.
    pub fn scroll_offset(&self) -> Pixels {
        -self.scroll.offset().y
    }

    /// The cells a drag in progress lands on.
    pub fn drag_cell(&self) -> Option<WidgetCell> {
        self.drag.as_ref().and_then(WidgetDrag::target)
    }

    /// The GPUs in slot 0 and slot 1, as the box shows them.
    pub fn set_slots(&mut self, slots: Vec<SlotDevice>) {
        self.slots = slots;
    }

    /// A device page's header type word, which its Reset dialog names.
    pub fn set_page_name(&mut self, name: SharedString) {
        self.page_name = Some(name);
    }

    /// The page parts the page can show now, in the order Add Widget lists them.
    pub fn set_page_parts(&mut self, parts: Vec<FixedWidget>) {
        self.page_parts = parts;
    }

    /// Whether the editor serves a device page.
    fn is_page(&self) -> bool {
        grid::is_page(&self.id)
    }

    /// The grid the edit actions use: the container's cells now.
    fn edit_grid(&self, cx: &App) -> InnerGrid {
        let c = self.container.read(cx);
        let g = c.placement().grid;
        if g.cols > 0 && g.cw > 0. {
            g
        } else {
            InnerGrid::of(&self.id, f32::from(c.area().width), metrics(cx).ui_scale)
        }
    }

    /// The grid layout the flow layout `layout` becomes, from where the container draws it
    /// now. Rows count in the drawn row height, which Edit Widgets then holds, so the widgets
    /// keep their place.
    fn pinned(&self, layout: &ContainerLayout, cx: &App) -> ContainerLayout {
        let c = self.container.read(cx);
        let placement = c.placement();
        let drawn: Vec<(u32, Drawn)> = layout
            .widgets
            .iter()
            .map(|w| {
                let how = match placement.bounds(w.id) {
                    Some(b) => Drawn::At(b),
                    // A page scrolls, so a widget it does not place takes no space in its flow
                    // (a secondary fallback not shown) and is left out; only a box cuts one.
                    None if grid::grid_fills(&self.id)
                        && c.source(w.id).is_some_and(|s| {
                            s.read(cx).availability() == Availability::Available
                        }) =>
                    {
                        Drawn::Cut
                    }
                    None => Drawn::Absent,
                };
                (w.id, how)
            })
            .collect();
        let unit_grid = self.edit_grid(cx);
        let mut g = unit_grid;
        g.unit = g.unit.max(g.row);
        let mut fit_to = None;
        // A layout that fits the box body pins into at most the rows that fit at the unit in
        // the body under the toolbar, so it fits there too (tasklens-mo5t5). The rows then
        // fill that body (`WidgetContainer::set_view_height`).
        if let Some(body_h) = self.edit_body_h(cx) {
            let bottom = drawn
                .iter()
                .filter_map(|(_, how)| match how {
                    Drawn::At(b) => Some(f32::from(b.bottom())),
                    _ => None,
                })
                .fold(0., f32::max);
            let unit = grid::grid_unit(&self.id) * g.scale;
            let fit = ((body_h + g.gutter) / (unit + g.gutter)).floor().max(1.);
            let rows = ((bottom + g.gutter) / (g.unit + g.gutter)).round();
            let fits_box = bottom <= f32::from(c.area().height) + 0.5;
            if fits_box && rows > fit {
                g.unit = (bottom + g.gutter) / fit - g.gutter;
            }
            if fits_box {
                fit_to = Some(fit as u16);
            }
        }
        let mut pinned = grid::pin(layout, &drawn, &self.slots, &g);
        // Flow rows packed tighter than the grid's row pitch (a 1.2.0 GPU section draws its
        // 18 px text rows 22 px apart) still pin to more rows: the tall widgets give rows down
        // to their minimum span, so the box fits under the toolbar where it can.
        if let Some(rows) = fit_to {
            grid::fit_rows(&mut pinned, rows, &unit_grid);
        }
        pinned
    }

    /// The height of the body under the toolbar in Edit Widgets, px: the box body less the
    /// toolbar. `None` for a device page, whose rows never fill (its toolbar sits outside the
    /// container's body), and before the container has a size.
    fn edit_body_h(&self, cx: &App) -> Option<f32> {
        if !grid::grid_fills(&self.id) {
            return None;
        }
        let h = f32::from(self.container.read(cx).area().height);
        let body = h - TOOLBAR_H * metrics(cx).ui_scale;
        (h > 0. && body > 0.).then_some(body)
    }

    /// Starts Edit Widgets: a flow layout is pinned to the grid, the rows hold their height and
    /// the editor takes the keys.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            window.focus(&self.focus, cx);
            return;
        }
        // At most one container of the window is in Edit Widgets.
        if editing_except(&self.hub, Some(cx.entity_id()), cx) {
            return;
        }
        window.focus(&self.focus, cx);
        let shown = self.container.read(cx).layout().clone();
        let working = if shown.preset.is_some() {
            self.pinned(&shown, cx)
        } else {
            shown.clone()
        };
        let before = saved_layout(&self.id, cx);
        // The body keeps the offset the applied view had.
        let offset = self.container.read(cx).scroll_offset();
        self.scroll.set_offset(point(px(0.), -offset));
        let body_h = self.edit_body_h(cx);
        self.container.update(cx, |c, cx| {
            c.set_editing(true, cx);
            c.set_layout(working.clone(), cx);
            // The rows fill the body under the toolbar from the first frame; the body's
            // measured height follows after it.
            if let Some(h) = body_h {
                c.set_view_height(px(h), cx);
            }
        });
        self.shown_before = Some(shown);
        self.edit = Some(LayoutEdit::new(self.id.to_string(), before, Some(working)));
        self.drag = None;
        bump_edit_mode(cx);
        cx.emit(WidgetEditEvent::Started);
        cx.notify();
    }

    /// Apply: saves the working copy when it changed and ends Edit Widgets. Returns whether it
    /// saved.
    pub fn apply(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(edit) = self.edit.take() else {
            return false;
        };
        let saves = edit.saves();
        let shown = if saves {
            save(&edit.container, edit.working.as_ref(), cx);
            edit.working
                .clone()
                .unwrap_or_else(|| default_layout(&edit.container))
        } else if edit.changed() {
            edit.working
                .clone()
                .unwrap_or_else(|| default_layout(&edit.container))
        } else {
            self.shown_before
                .take()
                .unwrap_or_else(|| default_layout(&edit.container))
        };
        self.finish(shown, saves, cx);
        saves
    }

    /// Cancel and Esc: the box shows what it showed before, and nothing is saved.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        let shown = self
            .shown_before
            .take()
            .unwrap_or_else(|| default_layout(&edit.container));
        self.finish(shown, false, cx);
    }

    fn finish(&mut self, shown: ContainerLayout, applied: bool, cx: &mut Context<Self>) {
        self.drag = None;
        self.shown_before = None;
        // The applied view goes on at the offset the edit ended at.
        let offset = -self.scroll.offset().y;
        self.container.update(cx, |c, cx| {
            c.set_layout(shown, cx);
            c.set_editing(false, cx);
            c.set_scroll_offset(offset);
        });
        bump_edit_mode(cx);
        cx.emit(WidgetEditEvent::Ended { applied });
        cx.notify();
    }

    /// Shows the working copy. For `None` (after Reset) it shows the default layout pinned to
    /// the grid, as entering Edit Widgets does, so its widgets can be selected, dragged and
    /// configured. The working copy stays `None`, so Apply saves the default.
    fn show_working(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else {
            return;
        };
        let Some(layout) = edit.working.clone() else {
            let default = default_layout(&edit.container);
            self.container
                .update(cx, |c, cx| c.set_layout(default.clone(), cx));
            if default.preset.is_some() {
                // The container placed the flow layout just now, so it can be pinned.
                let pinned = self.pinned(&default, cx);
                self.container.update(cx, |c, cx| c.set_layout(pinned, cx));
            }
            return;
        };
        self.container.update(cx, |c, cx| c.set_layout(layout, cx));
    }

    /// The layout edit actions work on: the working copy, or for `None` (after Reset) the
    /// pinned default the container shows.
    pub fn working_layout(&self, cx: &App) -> Option<ContainerLayout> {
        let edit = self.edit.as_ref()?;
        if let Some(layout) = &edit.working {
            return Some(layout.clone());
        }
        let shown = self.container.read(cx).layout();
        Some(if shown.preset.is_some() {
            self.pinned(shown, cx)
        } else {
            shown.clone()
        })
    }

    /// Runs `f` on a copy of the working layout (the default pinned to the grid after Reset)
    /// and makes the result the working copy when `f` changed it.
    fn edit_with(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ContainerLayout, &InnerGrid) -> bool,
    ) -> bool {
        let Some(mut next) = self.working_layout(cx) else {
            return false;
        };
        let grid = self.edit_grid(cx);
        if !f(&mut next, &grid) {
            return false;
        }
        let pushed = self.edit.as_mut().is_some_and(|e| e.push(Some(next)));
        if pushed {
            self.show_working(cx);
            cx.notify();
        }
        pushed
    }

    /// Undo: the previous working copy.
    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.edit.as_mut().is_some_and(LayoutEdit::step_back) {
            return false;
        }
        self.drag = None;
        self.show_working(cx);
        cx.notify();
        true
    }

    /// Reset asks first ([`menu::confirm_reset`]).
    pub fn request_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_none() {
            return;
        }
        let this = cx.entity().downgrade();
        let on_ok = move |_: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |e, cx| e.reset_confirmed(cx));
        };
        match (&self.page_name, self.is_page()) {
            (Some(name), true) => menu::confirm_reset_page(name.clone(), false, window, cx, on_ok),
            _ => menu::confirm_reset(&self.id, window, cx, on_ok),
        }
    }

    /// The confirmed Reset: the working copy becomes the default. Undo brings it back.
    pub fn reset_confirmed(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.edit.as_mut().is_some_and(|e| e.push(None)) {
            return false;
        }
        self.drag = None;
        self.show_working(cx);
        cx.notify();
        true
    }

    /// Tab and Shift+Tab.
    pub fn select_step(&mut self, forward: bool, cx: &mut Context<Self>) -> Option<u32> {
        let order = self
            .working_layout(cx)
            .map(|l| reading_order(&l))
            .unwrap_or_default();
        let id = self.edit.as_mut()?.select_step_in(&order, forward);
        cx.notify();
        id
    }

    /// Selects widget `id`.
    pub fn select(&mut self, id: u32, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.edit
            && edit.selected != Some(id)
        {
            edit.selected = Some(id);
            cx.notify();
        }
    }

    fn selected(&self) -> Option<u32> {
        self.edit.as_ref()?.selected
    }

    /// Alt+arrow keys: the selected widget moves one cell.
    pub fn move_selected(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.selected() else {
            return false;
        };
        let moved = self.edit_with(cx, |l, g| {
            grid::moved_by(l, id, dx, dy).is_some_and(|t| grid::set_cell(l, id, t, g))
        });
        if moved {
            self.show_widget(id, cx);
        }
        moved
    }

    /// Alt+Shift+arrow keys: the selected widget grows or shrinks one cell at its right or
    /// bottom edge.
    pub fn resize_selected(&mut self, dw: i32, dh: i32, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.selected() else {
            return false;
        };
        let grown = self.edit_with(cx, |l, g| {
            grid::grown_by(l, id, dw, dh).is_some_and(|t| grid::set_cell(l, id, t, g))
        });
        if grown {
            self.show_widget(id, cx);
        }
        grown
    }

    /// Scrolls the body the least that shows widget `id` fully (ADR-019 "Insert"); a widget
    /// taller than the body shows its top. The offset is clamped to the content at paint.
    fn show_widget(&mut self, id: u32, cx: &mut Context<Self>) {
        let (top, bottom, content) = {
            let c = self.container.read(cx);
            let Some(b) = c.placement().bounds(id) else {
                return;
            };
            (
                f32::from(b.top()),
                f32::from(b.bottom()),
                f32::from(c.placement().content_h),
            )
        };
        let view = f32::from(self.body.get().1);
        if view <= 0. || content <= view {
            return;
        }
        let shown = -f32::from(self.scroll.offset().y);
        let target = if top < shown || bottom - top > view {
            top
        } else if bottom > shown + view {
            bottom - view
        } else {
            return;
        };
        let target = target.clamp(0., content - view);
        self.scroll.set_offset(point(px(0.), px(-target)));
        cx.notify();
    }

    /// Delete and "Remove Widget": the selected widget goes.
    pub fn remove_selected(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.selected() else {
            return false;
        };
        self.edit_with(cx, |l, _| remove_widget(l, id))
    }

    /// The sources the box can show, now: the CPU's metrics and logical CPUs in the CPU box,
    /// the memory metrics in the Memory box, the shown disks and the disk total in the Disk box,
    /// the counted adapters and their total in the Network box, the CPU's in the Top processes
    /// box, the four bands' devices (CPU, memory, disks, adapters) in the Timeline box, else the
    /// GPUs the box shows.
    pub fn source_groups(&self, cx: &App) -> Vec<SourceGroup> {
        let snap = box_snapshot(&self.hub, cx);
        if self.is_page() {
            return menu::page_source_groups(&self.id, &snap, cx);
        }
        if *self.id == *PROCESSES_BOX_CONTAINER {
            return menu::cpu_source_groups(&snap, cx);
        }
        if *self.id == *TIMELINE_BOX_CONTAINER {
            let mut groups = menu::cpu_source_groups(&snap, cx);
            groups.extend(menu::mem_source_groups(&snap, cx));
            groups.extend(menu::disk_source_groups(&snap, cx));
            groups.extend(menu::net_source_groups(&snap, cx));
            return groups;
        }
        if *self.id == *CPU_BOX_CONTAINER {
            return menu::cpu_source_groups(&snap, cx);
        }
        if *self.id == *MEMORY_BOX_CONTAINER {
            return menu::mem_source_groups(&snap, cx);
        }
        if *self.id == *DISK_BOX_CONTAINER {
            return menu::disk_source_groups(&snap, cx);
        }
        if *self.id == *NET_BOX_CONTAINER {
            return menu::net_source_groups(&snap, cx);
        }
        menu::source_groups(&self.slots, &snap, cx)
    }

    /// The fixed widgets Add Widget offers: on a device page the parts the page can show and
    /// does not hold yet, Details first (ADR-019 phase 3 "Source lists"); in a box
    /// [`Self::fixed_add`].
    pub fn fixed_adds(&self, cx: &App) -> Vec<menu::FixedAdd> {
        if !self.is_page() {
            return self.fixed_add(cx).into_iter().collect();
        }
        let Some(layout) = self.working_layout(cx) else {
            return Vec::new();
        };
        PAGE_FIXED_NAMES
            .iter()
            .filter(|(kind, _)| kind.allowed_in(&self.id) && self.page_parts.contains(kind))
            .filter(|(kind, _)| {
                !layout
                    .widgets
                    .iter()
                    .any(|w| matches!(&w.content, WidgetContent::Fixed(f) if f == kind))
            })
            .map(|(kind, key)| menu::FixedAdd {
                name: Arc::from(kind.name()),
                label: tr(cx, key),
            })
            .collect()
    }

    /// The fixed widget Add Widget offers: the one the box allows (Memory by app in the Memory
    /// box, the table in Top processes, the bands in Timeline), while the working layout has
    /// none.
    pub fn fixed_add(&self, cx: &App) -> Option<menu::FixedAdd> {
        let (kind, key) = FIXED_ADDS
            .iter()
            .find(|(kind, _)| kind.container() == Some(&*self.id))?;
        let layout = self.working_layout(cx)?;
        let has = layout
            .widgets
            .iter()
            .any(|w| matches!(&w.content, WidgetContent::Fixed(f) if f == kind));
        (!has).then(|| menu::FixedAdd {
            name: Arc::from(kind.name()),
            label: tr(cx, key),
        })
    }

    /// Add Widget's fixed item: the fixed widget `name` (`memory_by_app`), placed and selected
    /// as [`Self::add`] places a widget. Nothing when the box does not allow it or already has
    /// it. Returns its id.
    pub fn add_fixed(&mut self, name: &str, cx: &mut Context<Self>) -> Option<u32> {
        self.edit.as_ref()?;
        let kind = FixedWidget::from_name(name);
        if !self.fixed_adds(cx).iter().any(|a| *a.name == *kind.name()) {
            return None;
        }
        let widget = WidgetSetting {
            id: 0,
            content: WidgetContent::Fixed(kind),
            forms: FormSet::empty(),
            cell: None,
            options: WidgetOptions::default(),
            device_name: None,
        };
        let added = Rc::new(Cell::new(None));
        let out = Rc::clone(&added);
        self.edit_with(cx, move |l, g| {
            out.set(Some(grid::insert(l, widget, g)));
            true
        });
        let id = added.get()?;
        self.select(id, cx);
        self.show_widget(id, cx);
        Some(id)
    }

    /// The selector, device name and offered forms of `source`.
    fn resolve(
        &self,
        source: &WidgetSource,
        cx: &App,
    ) -> (SourceSelector, Option<Arc<str>>, FormSet) {
        let metric = MetricKey::new(&source.metric);
        let groups = self.source_groups(cx);
        let group = groups.iter().find(|g| *g.key == *source.device);
        let offered = groups
            .iter()
            .find_map(|g| g.item_on(&source.device, &metric))
            .map(|i| i.offered)
            .or_else(|| {
                MetricMeta::of(DeviceKind::of_key(&source.device), &metric).map(|m| m.forms())
            })
            .unwrap_or(FormSet::VALUE | FormSet::LABEL);
        // A logical CPU is named by its own catalog entry, not by the CPU's.
        let snap = box_snapshot(&self.hub, cx);
        let own = snap
            .devices
            .entries
            .iter()
            .find(|e| *e.persistent_key == *source.device)
            .map(|e| Arc::clone(&e.display_name));
        let name = own
            .or_else(|| group.map(|g| Arc::from(g.name.as_ref())))
            .or_else(|| {
                self.slots
                    .iter()
                    .find(|s| *s.key == *source.device)
                    .map(|s| Arc::clone(&s.name))
            });
        let selector = SourceSelector {
            device: DeviceSelector::Key(Arc::clone(&source.device)),
            metric,
        };
        (selector, name, offered)
    }

    /// Add Widget: a widget on `source` with its default forms, placed as ADR-019 "Insert"
    /// says, and selected. Returns its id.
    pub fn add(&mut self, source: &WidgetSource, cx: &mut Context<Self>) -> Option<u32> {
        self.edit.as_ref()?;
        let (selector, name, offered) = self.resolve(source, cx);
        let widget = WidgetSetting {
            id: 0,
            content: WidgetContent::Metric(selector),
            forms: menu::default_forms(offered),
            cell: None,
            options: WidgetOptions::default(),
            device_name: name,
        };
        let added = Rc::new(Cell::new(None));
        let out = Rc::clone(&added);
        self.edit_with(cx, move |l, g| {
            out.set(Some(grid::insert(l, widget, g)));
            true
        });
        let id = added.get()?;
        self.select(id, cx);
        self.show_widget(id, cx);
        Some(id)
    }

    /// Add Widget's groups: a group widget of the members `group` of `device`, each on
    /// `metric`, with the defaults of a new group, placed and selected as [`Self::add`] places
    /// a widget. Returns its id.
    pub fn add_group(
        &mut self,
        device: &str,
        group: &str,
        metric: &str,
        cx: &mut Context<Self>,
    ) -> Option<u32> {
        self.edit.as_ref()?;
        let members = tl_ui_settings::layout::GroupKind::from_name(group);
        let name = self
            .slots
            .iter()
            .find(|s| *s.key == *device)
            .map(|s| Arc::clone(&s.name));
        let widget = menu::new_group(
            DeviceSelector::Key(Arc::from(device)),
            members,
            MetricKey::new(metric),
            name,
        );
        let added = Rc::new(Cell::new(None));
        let out = Rc::clone(&added);
        self.edit_with(cx, move |l, g| {
            out.set(Some(grid::insert(l, widget, g)));
            true
        });
        let id = added.get()?;
        self.select(id, cx);
        self.show_widget(id, cx);
        Some(id)
    }

    /// The group items of the widget menu: group option `option` of widget `id` is `value`.
    pub fn set_group(
        &mut self,
        id: u32,
        option: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        self.edit_with(cx, |l, g| menu::set_group_option(l, id, option, value, g))
    }

    /// "Source >": widget `id` shows `source`.
    pub fn set_source(&mut self, id: u32, source: &WidgetSource, cx: &mut Context<Self>) -> bool {
        let (selector, name, offered) = self.resolve(source, cx);
        self.edit_with(cx, |l, g| {
            set_widget_source(l, id, selector, name, offered, g)
        })
    }

    /// "Show >": turns form `form` ("chart", "bar", "value", "stats" or "label") of widget
    /// `id` on or off. The last form stays on.
    pub fn set_form(&mut self, id: u32, form: &str, on: bool, cx: &mut Context<Self>) -> bool {
        let form = FormSet::from_names([form]);
        if form.is_empty() {
            return false;
        }
        self.edit_with(cx, |l, g| set_widget_form(l, id, form, on, g))
    }

    /// "Chart >": turns chart option `option` ("fill" or "dashed") of widget `id` on or off.
    pub fn set_option(&mut self, id: u32, option: &str, on: bool, cx: &mut Context<Self>) -> bool {
        self.edit_with(cx, |l, _| set_widget_option(l, id, option, on))
    }

    /// The forms widget `id`'s metric offers: from its source, else from its metadata.
    fn offered_forms(&self, w: &WidgetSetting, cx: &App) -> FormSet {
        let from_source = self
            .container
            .read(cx)
            .source(w.id)
            .map_or(FormSet::empty(), |s| s.read(cx).offered_forms());
        if !from_source.is_empty() {
            return from_source;
        }
        if let WidgetContent::Group(g) = &w.content {
            let offered = menu::group_offered(&g.members, &g.metric);
            return if offered.is_empty() { w.forms } else { offered };
        }
        let meta = w
            .content
            .device()
            .zip(w.content.metric_key())
            .and_then(|(device, key)| MetricMeta::of(DeviceKind::of(device), key));
        meta.map_or(w.forms, |m| m.forms())
    }

    /// Opens widget `id`'s menu at `position` and selects the widget.
    pub fn open_widget_menu(
        &mut self,
        id: u32,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(setting) = self
            .working_layout(cx)
            .and_then(|l| l.widgets.into_iter().find(|w| w.id == id))
        else {
            return;
        };
        self.select(id, cx);
        let offered = self.offered_forms(&setting, cx);
        let groups: Rc<[SourceGroup]> = self.source_groups(cx).into();
        let menu = TlMenu::build(window, cx, self.focus.clone(), move |menu, window, cx| {
            menu::widget_menu(menu, &setting, offered, &groups, window, cx)
        });
        open_context_menu_at(position, window, cx, menu);
    }

    /// Opens the Add Widget list under the button, or at `position`.
    fn open_add_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = self.add_bounds.get();
        let at = if b.size.width > px(0.) {
            b.bottom_left()
        } else {
            position
        };
        let groups: Rc<[SourceGroup]> = self.source_groups(cx).into();
        let fixed: Rc<[menu::FixedAdd]> = self.fixed_adds(cx).into();
        let menu = TlMenu::build(window, cx, self.focus.clone(), move |menu, window, cx| {
            menu::add_items(menu, &groups, &fixed, window, cx)
        });
        open_context_menu_at(at, window, cx, menu);
    }

    /// A press on widget `id`'s frame, grip or corner: the widget is selected and a drag may
    /// start. A double-click on the corner sets the widget to its minimum span.
    fn chrome_down(
        &mut self,
        id: u32,
        grab: Grab,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        self.select(id, cx);
        self.drag = None;
        let Some(base) = self.working_layout(cx) else {
            return;
        };
        let Some(start_cell) = base
            .widgets
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.cell)
        else {
            return;
        };
        if grab == Grab::Resize && ev.click_count >= 2 {
            let smallest = WidgetCell {
                cols: 1,
                rows: 1,
                ..start_cell
            };
            self.edit_with(cx, |l, g| grid::set_cell(l, id, smallest, g));
            return;
        }
        self.drag = Some(WidgetDrag {
            id,
            grab,
            start: ev.position,
            start_cell,
            base,
            moved: false,
            preview: None,
        });
        cx.notify();
    }

    /// The layout a drag shows: the drop's layout, the dragged widget in its target cells. At
    /// its old place it would lie under the preview outline and under the widgets that close up
    /// into the freed space, and their decorations would cover its text.
    fn drag_shown(drag: &WidgetDrag) -> ContainerLayout {
        drag.preview.clone().unwrap_or_else(|| drag.base.clone())
    }

    /// The pointer moved while a widget is held: the preview follows the cells under it.
    fn drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let grid = self.edit_grid(cx);
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let delta = position - drag.start;
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if !drag.moved && dx.abs().max(dy.abs()) < DRAG_START * scale {
            return;
        }
        drag.moved = true;
        let target = drag_target(
            drag.grab,
            drag.start_cell,
            (dx, dy),
            (grid.cw + grid.gutter, grid.row + grid.gutter),
            grid.cols,
        );
        let mut preview = drag.base.clone();
        grid::set_cell(&mut preview, drag.id, target, &grid);
        if drag.preview.as_ref() == Some(&preview) {
            return;
        }
        drag.preview = Some(preview);
        let shown = Self::drag_shown(drag);
        self.container.update(cx, |c, cx| c.set_layout(shown, cx));
        cx.notify();
    }

    /// The button went up: the drop commits the preview. `None` is a move without the button,
    /// whose release was not seen.
    fn drag_up(&mut self, position: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        if let Some(at) = position {
            self.drag_move(at, cx);
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        match drag.preview.filter(|p| *p != drag.base) {
            Some(preview) => {
                if let Some(edit) = &mut self.edit {
                    edit.push(Some(preview));
                }
                self.show_working(cx);
                self.show_widget(drag.id, cx);
            }
            None => self.show_working(cx),
        }
        cx.notify();
    }

    fn on_apply(&mut self, _: &PerfWidgetApply, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx);
    }

    fn on_cancel(&mut self, _: &PerfWidgetCancel, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel(cx);
    }

    fn on_undo(&mut self, _: &PerfWidgetUndo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn on_reset(&mut self, _: &PerfWidgetReset, window: &mut Window, cx: &mut Context<Self>) {
        self.request_reset(window, cx);
    }

    fn on_select_next(&mut self, _: &PerfWidgetSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.select_step(true, cx);
    }

    fn on_select_prev(&mut self, _: &PerfWidgetSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.select_step(false, cx);
    }

    fn on_move(&mut self, a: &PerfWidgetMove, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selected(i32::from(a.dx), i32::from(a.dy), cx);
    }

    fn on_resize(&mut self, a: &PerfWidgetResize, _: &mut Window, cx: &mut Context<Self>) {
        self.resize_selected(i32::from(a.dw), i32::from(a.dh), cx);
    }

    fn on_remove(&mut self, _: &PerfWidgetRemove, _: &mut Window, cx: &mut Context<Self>) {
        self.remove_selected(cx);
    }

    fn on_add(&mut self, a: &PerfWidgetAdd, _: &mut Window, cx: &mut Context<Self>) {
        self.add(&a.source, cx);
    }

    fn on_set_source(&mut self, a: &PerfWidgetSetSource, _: &mut Window, cx: &mut Context<Self>) {
        self.set_source(a.id, &a.source, cx);
    }

    fn on_set_form(&mut self, a: &PerfWidgetSetForm, _: &mut Window, cx: &mut Context<Self>) {
        self.set_form(a.id, &a.form, a.on, cx);
    }

    fn on_add_group(&mut self, a: &PerfWidgetAddGroup, _: &mut Window, cx: &mut Context<Self>) {
        self.add_group(&a.device, &a.group, &a.metric, cx);
    }

    fn on_add_fixed(&mut self, a: &PerfWidgetAddFixed, _: &mut Window, cx: &mut Context<Self>) {
        self.add_fixed(&a.fixed, cx);
    }

    fn on_set_group(&mut self, a: &PerfWidgetSetGroup, _: &mut Window, cx: &mut Context<Self>) {
        self.set_group(a.id, &a.option, &a.value, cx);
    }

    fn on_set_option(&mut self, a: &PerfWidgetSetOption, _: &mut Window, cx: &mut Context<Self>) {
        self.set_option(a.id, &a.option, a.on, cx);
    }

    /// The edit toolbar: Add Widget, Undo and Reset, then Cancel and Apply at the right.
    fn toolbar(&self, edit: &LayoutEdit, cx: &mut Context<Self>) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let this = cx.entity().downgrade();
        let button = |id: &'static str, kind: TlButtonKind| {
            TlButton::new(SharedString::from(id), kind).size(TlButtonSize::Sm)
        };
        // An icon button: the label is its tooltip.
        let icon_button = |id: &'static str, icon: TlIcon, label: SharedString| {
            button(id, TlButtonKind::Ghost)
                .icon(icon)
                .icon_size(m.icon.sm)
                .tooltip(label)
        };
        let add = {
            let this = this.clone();
            icon_button(ADD_ID, TlIcon::Plus, tr(cx, "perf-widget-add")).on_click(
                move |ev, window, cx| {
                    let at = ev.position();
                    let _ = this.update(cx, |e, cx| e.open_add_menu(at, window, cx));
                },
            )
        };
        let add_measure = {
            let bounds = Rc::clone(&self.add_bounds);
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .inset_0()
        };
        let undo = {
            let this = this.clone();
            icon_button(UNDO_ID, TlIcon::Undo2, tr(cx, "perf-widget-undo"))
                .disabled(!edit.can_undo())
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| e.undo(cx));
                })
        };
        let reset = {
            let this = this.clone();
            icon_button(RESET_ID, TlIcon::RotateCcw, tr(cx, "perf-widget-reset")).on_click(
                move |_, window, cx| {
                    let _ = this.update(cx, |e, cx| e.request_reset(window, cx));
                },
            )
        };
        let cancel = {
            let this = this.clone();
            icon_button(CANCEL_ID, TlIcon::X, tr(cx, "perf-widget-cancel")).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| e.cancel(cx));
                },
            )
        };
        let apply = button(APPLY_ID, TlButtonKind::Primary)
            .label(tr(cx, "perf-widget-apply"))
            .disabled(!edit.changed())
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |e, cx| {
                    e.apply(cx);
                });
            });
        let gap = m.sp[2];
        let left = div()
            .flex()
            .items_center()
            .gap(gap)
            .child(div().relative().child(add_measure).child(add))
            .child(undo)
            .child(reset);
        let right = div()
            .flex()
            .items_center()
            .gap(gap)
            .child(cancel)
            .child(apply);
        div()
            .tl_selector(|| TOOLBAR_ID.to_owned())
            .flex_none()
            .w_full()
            .h(px(TOOLBAR_H * s))
            .px(gap)
            .bg(theme.toolbar)
            .flex()
            .items_center()
            .justify_between()
            .child(left)
            .child(right)
            .into_any_element()
    }

    /// The edit chrome over each placed widget, and the preview of a drag.
    fn chrome(&self, edit: &LayoutEdit, cx: &mut Context<Self>) -> SmallVec<[AnyElement; 16]> {
        let theme = current(cx).clone();
        let s = metrics(cx).ui_scale;
        let mut out = SmallVec::new();
        let container = self.container.read(cx);
        let placement = container.placement().clone();
        let cid = container.id().clone();
        let this = cx.entity().downgrade();
        let dragged = self
            .drag
            .as_ref()
            .filter(|d| d.preview.is_some())
            .map(|d| d.id);
        for p in &placement.widgets {
            let id = p.id;
            let b = p.bounds;
            let selected = edit.selected == Some(id);
            let grip_sel = grip_id(&cid, id);
            let gear_sel = gear_id(&cid, id);
            let corner_sel = corner_id(&cid, id);
            let frame_sel = frame_id(&cid, id);
            let outline_cid = cid.clone();
            let grip = div()
                .id(SharedString::from(grip_sel.clone()))
                .tl_selector(move || grip_sel)
                .absolute()
                // The frame's 1 px border puts the padding box inside the widget.
                .left(px(-1.))
                .top(px(-1.))
                .size(px(HANDLE * s))
                .cursor_grab()
                .tooltip(TlTooltip::text(tr(cx, "perf-widget-grip")).build())
                .child(
                    Icon::new(IconName::GripVertical)
                        .size(px(HANDLE * s))
                        .text_color(theme.text_secondary),
                );
            let gear = {
                let this = this.clone();
                let button =
                    TlButton::new(SharedString::from(gear_sel.clone()), TlButtonKind::Ghost)
                        .size(TlButtonSize::Xs)
                        .icon(TlIcon::Settings2)
                        .icon_size(px(HANDLE * s))
                        .tooltip(tr(cx, "perf-widget-settings"))
                        .on_click(move |ev, window, cx| {
                            let at = ev.position();
                            let _ = this.update(cx, |e, cx| e.open_widget_menu(id, at, window, cx));
                        });
                div()
                    .tl_selector(move || gear_sel)
                    .absolute()
                    .top(px(-1.))
                    // Right of the grip, clear of the chart's scale label in the top right.
                    .left(px(HANDLE * s - 1.))
                    // As big as the grip: the button is 20 px at scale 1, so its edge ring is
                    // clipped, and the gear covers no more than the first line.
                    .size(px(HANDLE * s))
                    .overflow_hidden()
                    // A press on the gear is not the start of a drag.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .absolute()
                            .left(px(-2. * s))
                            .top(px(-2. * s))
                            .child(button),
                    )
            };
            let corner = div()
                .id(SharedString::from(corner_sel.clone()))
                .tl_selector(move || corner_sel)
                .absolute()
                // In the widget's corner, as the grip is in its top left.
                .right(px(-1.))
                .bottom(px(-1.))
                .size(px(CORNER * s))
                .cursor_nwse_resize()
                .tooltip(TlTooltip::text(tr(cx, "perf-widget-resize")).build())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.chrome_down(id, Grab::Resize, ev, window, cx);
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(MARK * s))
                        .bottom(px(MARK * s))
                        .size(px(CORNER / 2. * s))
                        .border_r(px(MARK * s))
                        .border_b(px(MARK * s))
                        .border_color(theme.text_secondary),
                );
            let frame = div()
                .id(SharedString::from(frame_sel.clone()))
                .tl_selector(move || frame_sel)
                .absolute()
                .left(b.left())
                .top(b.top())
                .w(b.size.width)
                .h(b.size.height)
                .border_1()
                .border_color(theme.divider)
                // The widget content stays live but takes no pointer input. The wheel still
                // scrolls the body.
                .block_mouse_except_scroll()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.chrome_down(id, Grab::Move, ev, window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_widget_menu(id, ev.position, window, cx);
                    }),
                )
                // A quick click can go up before the next frame puts the window's release
                // handler in place.
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                        this.drag_up(Some(ev.position), cx);
                    }),
                )
                .when(dragged == Some(id), |d| {
                    d.child(div().absolute().inset_0().bg(theme.window_bg.opacity(DIM)))
                })
                .when(selected, |d| {
                    // Over the frame's border: the outline takes the widget's outer 2 px,
                    // which the widget keeps free of text.
                    d.child(
                        div()
                            .tl_selector(move || outline_id(&outline_cid, id))
                            .absolute()
                            .inset(px(-1.))
                            .border(px(OUTLINE))
                            .border_color(theme.accent),
                    )
                })
                .child(grip)
                .child(gear)
                .child(corner);
            out.push(frame.into_any_element());
        }
        if let Some(target) = self.drag_cell() {
            let b = placement.grid.bounds(target);
            out.push(
                div()
                    .tl_selector(|| PREVIEW_ID.to_owned())
                    .absolute()
                    .left(b.left())
                    .top(b.top())
                    .w(b.size.width)
                    .h(b.size.height)
                    .border(px(OUTLINE))
                    .border_color(theme.accent)
                    .opacity(PREVIEW_OPACITY)
                    .into_any_element(),
            );
        }
        out
    }

    fn with_actions<E: gpui_kit::InteractiveElement>(el: E, cx: &mut Context<Self>) -> E {
        el.on_action(cx.listener(Self::on_apply))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_reset))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_move))
            .on_action(cx.listener(Self::on_resize))
            .on_action(cx.listener(Self::on_remove))
            .on_action(cx.listener(Self::on_add))
            .on_action(cx.listener(Self::on_set_source))
            .on_action(cx.listener(Self::on_set_form))
            .on_action(cx.listener(Self::on_set_option))
            .on_action(cx.listener(Self::on_add_group))
            .on_action(cx.listener(Self::on_add_fixed))
            .on_action(cx.listener(Self::on_set_group))
    }
}

impl Render for WidgetEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let id = editor_id(&self.id);
        let container = self
            .container
            .clone()
            .cached(StyleRefinement::default().size_full());
        let root = div()
            .tl_selector(move || id)
            .track_focus(&self.focus)
            .size_full();
        let Some(edit) = self.edit.clone() else {
            return Self::with_actions(root, cx).child(container);
        };
        let toolbar = self.toolbar(&edit, cx);
        let chrome = self.chrome(&edit, cx);
        let (_, body_h) = self.body.get();
        let content_h = self.container.read(cx).placement().content_h;
        let inner_h = body_h.max(content_h);
        let this = cx.entity().downgrade();
        let dragging = self.drag.is_some();
        let measure = canvas(
            {
                let this = this.clone();
                move |bounds, _, cx| {
                    // After the frame: a notify during the frame is lost.
                    let (this, size) = (this.clone(), bounds.size);
                    cx.defer(move |cx| {
                        let _ = this.update(cx, |e, cx| {
                            let body = (size.width, size.height);
                            // The container scrolls by the body's height, not its own.
                            e.container
                                .update(cx, |c, cx| c.set_view_height(size.height, cx));
                            if e.body.get() != body {
                                e.body.set(body);
                                cx.notify();
                            }
                        });
                    });
                }
            },
            move |_, _, window, _| {
                if !dragging {
                    return;
                }
                // The held widget follows the pointer wherever it goes.
                let moved = this.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let _ = moved.update(cx, |ed, cx| {
                        if e.pressed_button == Some(MouseButton::Left) {
                            ed.drag_move(e.position, cx);
                        } else {
                            ed.drag_up(None, cx);
                        }
                    });
                });
                let released = this.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase.bubble() {
                        let _ = released.update(cx, |ed, cx| ed.drag_up(Some(e.position), cx));
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        let scroll = SharedString::from(format!("{}-scroll", editor_id(&self.id)));
        let scroll_selector = scroll.to_string();
        let body = div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(measure)
            .child(
                div()
                    .size_full()
                    .relative()
                    .tl_selector(move || scroll_selector)
                    .child(
                        div()
                            .id(scroll)
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(
                                div().relative().w_full().h(inner_h).child(container).child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .size_full()
                                        .children(chrome),
                                ),
                            ),
                    )
                    .vertical_scrollbar(&self.scroll),
            );
        Self::with_actions(root, cx)
            .key_context(PERF_WIDGET_EDIT_CONTEXT)
            .flex()
            .flex_col()
            .child(toolbar)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use tl_ui_settings::layout::GPU_BOX_CONTAINER;

    use super::*;

    fn widget(id: u32, cell: WidgetCell, forms: FormSet) -> WidgetSetting {
        WidgetSetting {
            id,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Key(Arc::from("gpu:a")),
                metric: MetricKey::new("vram"),
            }),
            forms,
            cell: Some(cell),
            options: WidgetOptions::default(),
            device_name: None,
        }
    }

    fn cell(col: u16, row: u16, cols: u16, rows: u16) -> WidgetCell {
        WidgetCell {
            col,
            row,
            cols,
            rows,
        }
    }

    fn layout(widgets: Vec<WidgetSetting>) -> ContainerLayout {
        ContainerLayout {
            container: GPU_BOX_CONTAINER.to_owned(),
            version: tl_ui_settings::layout::LAYOUT_VERSION_GPU_ONLY,
            preset: None,
            next_id: widgets.iter().map(|w| w.id + 1).max().unwrap_or(1),
            widgets,
        }
    }

    const ROW: FormSet = FormSet::BAR.union(FormSet::VALUE).union(FormSet::LABEL);

    fn two() -> ContainerLayout {
        layout(vec![
            widget(1, cell(0, 0, 6, 1), ROW),
            widget(2, cell(0, 1, 6, 1), ROW),
        ])
    }

    #[test]
    fn undo_keeps_at_most_the_limit() {
        let mut edit = LayoutEdit::new(GPU_BOX_CONTAINER, None, Some(two()));
        assert!(!edit.changed() && !edit.can_undo());
        for n in 0..(UNDO_LIMIT + 20) {
            let mut l = two();
            l.next_id = 10 + n as u32;
            assert!(edit.push(Some(l)));
        }
        assert_eq!(edit.undo.len(), UNDO_LIMIT);
        let mut steps = 0;
        while edit.step_back() {
            steps += 1;
        }
        assert_eq!(steps, UNDO_LIMIT);
        // The oldest steps fell off: the start cannot come back.
        assert!(edit.changed());
    }

    #[test]
    fn push_of_the_same_layout_is_no_step() {
        let mut edit = LayoutEdit::new(GPU_BOX_CONTAINER, Some(two()), Some(two()));
        assert!(!edit.push(Some(two())));
        assert!(!edit.can_undo());
        assert!(edit.push(None));
        assert!(edit.changed() && edit.saves());
        assert!(edit.step_back());
        assert!(!edit.changed() && !edit.saves());
    }

    #[test]
    fn saves_only_a_change_from_the_saved_entry() {
        // A saved grid layout edited back to itself saves nothing.
        let mut edit = LayoutEdit::new(GPU_BOX_CONTAINER, Some(two()), Some(two()));
        let mut moved = two();
        moved.widgets.swap(0, 1);
        assert!(edit.push(Some(moved)));
        assert!(edit.saves());
        // Reset over the default (no entry) is no change to save.
        let mut edit = LayoutEdit::new(GPU_BOX_CONTAINER, None, Some(two()));
        assert!(edit.push(None));
        assert!(edit.changed());
        assert!(!edit.saves());
    }

    #[test]
    fn selection_steps_in_reading_order_and_ends_with_its_widget() {
        let l = layout(vec![
            widget(3, cell(3, 0, 3, 1), ROW),
            widget(1, cell(0, 1, 6, 1), ROW),
            widget(2, cell(0, 0, 3, 1), ROW),
        ]);
        let mut edit = LayoutEdit::new(GPU_BOX_CONTAINER, None, Some(l.clone()));
        assert_eq!(edit.reading_order().as_slice(), [2, 3, 1]);
        assert_eq!(edit.select_step(true), Some(2));
        assert_eq!(edit.select_step(true), Some(3));
        assert_eq!(edit.select_step(true), Some(1));
        assert_eq!(edit.select_step(true), Some(2));
        assert_eq!(edit.select_step(false), Some(1));
        let mut gone = l;
        assert!(remove_widget(&mut gone, 1));
        edit.push(Some(gone));
        assert_eq!(edit.selected, None);
        assert_eq!(edit.select_step(false), Some(3));
    }

    #[test]
    fn forms_never_become_empty() {
        let grid = InnerGrid::new(600., 6, 1.);
        let mut l = layout(vec![widget(1, cell(0, 0, 6, 1), FormSet::VALUE)]);
        assert!(!set_widget_form(&mut l, 1, FormSet::VALUE, false, &grid));
        assert!(set_widget_form(&mut l, 1, FormSet::LABEL, true, &grid));
        assert!(set_widget_form(&mut l, 1, FormSet::VALUE, false, &grid));
        assert_eq!(l.widgets[0].forms, FormSet::LABEL);
        // A chart needs more rows: the widget grows to its minimum.
        assert!(set_widget_form(&mut l, 1, FormSet::CHART, true, &grid));
        let c = l.widgets[0].cell.expect("cell");
        let min = grid.forms_min_span(l.widgets[0].forms);
        assert!(c.cols >= min.0 && c.rows >= min.1, "{c:?} {min:?}");
    }

    #[test]
    fn options_change_only_known_names() {
        let mut l = two();
        assert!(set_widget_option(&mut l, 1, OPTION_DASHED, true));
        assert!(!set_widget_option(&mut l, 1, OPTION_DASHED, true));
        assert!(set_widget_option(&mut l, 1, OPTION_FILL, false));
        assert!(!set_widget_option(&mut l, 1, "glow", true));
        assert!(l.widgets[0].options.dashed && !l.widgets[0].options.fill);
    }

    #[test]
    fn a_new_source_keeps_the_forms_it_offers() {
        let grid = InnerGrid::new(600., 6, 1.);
        let mut l = two();
        let temp = SourceSelector {
            device: DeviceSelector::Key(Arc::from("gpu:b")),
            metric: MetricKey::new("temp"),
        };
        let offered = FormSet::all() - FormSet::BAR;
        assert!(set_widget_source(
            &mut l,
            1,
            temp.clone(),
            Some(Arc::from("Card B")),
            offered,
            &grid
        ));
        assert_eq!(l.widgets[0].forms, FormSet::VALUE | FormSet::LABEL);
        assert_eq!(l.widgets[0].device_name.as_deref(), Some("Card B"));
        assert!(!set_widget_source(&mut l, 1, temp, None, offered, &grid));
        // Nothing left: the source's default forms.
        let mut l = layout(vec![widget(1, cell(0, 0, 6, 1), FormSet::BAR)]);
        let clock = SourceSelector {
            device: DeviceSelector::Key(Arc::from("gpu:a")),
            metric: MetricKey::new("clock_gfx"),
        };
        set_widget_source(&mut l, 1, clock, None, offered, &grid);
        assert_eq!(
            l.widgets[0].forms,
            FormSet::CHART | FormSet::VALUE | FormSet::LABEL
        );
    }

    #[test]
    fn drag_targets_snap_to_whole_cells() {
        let start = cell(1, 2, 2, 2);
        let pitch = (100., 28.);
        assert_eq!(
            drag_target(Grab::Move, start, (149., -20.), pitch, 6),
            cell(2, 1, 2, 2)
        );
        // Kept inside the columns and above row 0.
        assert_eq!(
            drag_target(Grab::Move, start, (900., -900.), pitch, 6),
            cell(4, 0, 2, 2)
        );
        assert_eq!(
            drag_target(Grab::Resize, start, (260., 60.), pitch, 6),
            cell(1, 2, 5, 4)
        );
        assert_eq!(
            drag_target(Grab::Resize, start, (-900., -900.), pitch, 6),
            cell(1, 2, 1, 1)
        );
    }
}
