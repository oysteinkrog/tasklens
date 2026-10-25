//! Edit Layout mode of the Overview page (UI-PRF-018 "Edit layout mode", UI-PRF-056, D-023,
//! D-030).
//!
//! The page menu item "Edit Layout" turns it on. A 40 px toolbar shows at the top of the page
//! (Add Box, Reset Layout, Done), and each box gets a grip before its title and a Hide box
//! button at the right end of its title line. Both sit in the title line's side margins, drawn
//! by the page over the cached box views, so the boxes stay live and never re-render for edit
//! mode. "Add Box" puts the kind at its kind minimum in the first free place on the grid. Every
//! edit action saves the layout.
//!
//! A box moves by dragging its title line and resizes by dragging the handle at its
//! bottom-right corner, both in whole cells; Alt+arrow keys do the same one cell at a time. The
//! box lands on the target cells, raised to its kind minimum. The other boxes close up into the
//! space it left, boxes in the way move down to make room, and all close up again around it
//! ([`settle`]). So a box dragged down by the height of the boxes under it ends up below them.
//! A ghost outline shows the target cells during a drag, and the layout follows the pointer, so
//! the boxes move out of the way before the button goes up. The layout is saved when the drag
//! ends. Every action that changes the layout during a drag, and leaving the mode, the page or
//! the window's focus, first ends the drag without a change ([`OverviewPage::end_drag`]), so a
//! later move or release cannot bring the old boxes back. The stacked layout below 720 px has
//! no box drags.
//!
//! The functions on [`OverviewLayout`] here are pure, so the rules are tested without a window.
//! [`EditState`] and the `OverviewPage` methods hold the view side.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, v_flex};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, FocusHandle, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels, Point, SharedString,
    Styled as _, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::settings::OVERVIEW_GRID_ROWS_MAX;

use super::OverviewPage;
use super::layout::{BoxKind, GRID_COLS, GridBox, GridMetrics, OverviewLayout, PlacedBox};
use super::parts::EditChrome;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::focus::TlFocusRing;
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{FIXED_KEY_BASE, ToastKey, ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::tr;
use crate::pages::performance::actions::{
    PerfOverviewAddBox, PerfOverviewEditDone, PerfOverviewEditLayout, PerfOverviewHideBox,
    PerfOverviewMoveBox, PerfOverviewResetLayout, PerfOverviewResizeBox,
};
use crate::pages::performance::page::PAGE_PAD;
use crate::theme::{current, metrics};

/// The toolbar height, logical px.
pub const TOOLBAR_H: f32 = 40.;

/// The box title line under the 1 px top border, logical px (the frame's `TITLE_ROW`).
const TITLE_H: f32 = 24.;

/// The grip and the Hide box button, logical px.
const HANDLE: f32 = 16.;

/// The resize handle at a box's bottom-right corner, logical px.
pub const CORNER: f32 = 12.;

/// The corner mark's stroke and the ghost outline, logical px.
const MARK: f32 = 2.;

/// The pointer moves this far before a press on a title line or corner becomes a drag, logical
/// px.
const DRAG_START: f32 = 3.;

/// The "At least one box must stay on the Overview page" toast.
pub const LAST_BOX_TOAST: ToastKey = ToastKey(FIXED_KEY_BASE | 0x0403);

/// Debug selectors and element ids.
pub const TOOLBAR_ID: &str = "perf-overview-edit-toolbar";
pub const ADD_BUTTON_ID: &str = "perf-overview-add-box";
pub const RESET_BUTTON_ID: &str = "perf-overview-reset-layout";
pub const DONE_BUTTON_ID: &str = "perf-overview-edit-done";
pub const GHOST_ID: &str = "perf-overview-drag-ghost";

/// The debug selector of `kind`'s resize handle in edit mode.
pub fn corner_id(kind: BoxKind) -> String {
    format!("perf-overview-corner-{}", kind.key())
}

/// The debug selector of `kind`'s title line handle in edit mode.
pub fn title_handle_id(kind: BoxKind) -> String {
    format!("perf-overview-edit-title-{}", kind.key())
}

/// The debug selector of `kind`'s Hide box button.
pub fn hide_button_id(kind: BoxKind) -> String {
    format!("perf-overview-hide-{}", kind.key())
}

/// The debug selector of `kind`'s grip in Edit Layout mode.
pub fn grip_id(kind: BoxKind) -> String {
    format!("perf-overview-grip-{}", kind.key())
}

/// Removes `kind` from the grid; its cells become free.
pub fn hide(layout: &mut OverviewLayout, kind: BoxKind) -> bool {
    let before = layout.boxes.len();
    layout.boxes.retain(|b| b.kind != kind);
    layout.boxes.len() != before
}

/// The kinds the layout does not place, in [`BoxKind::ALL`] order: the "Add Box" menu.
pub fn hidden_kinds(layout: &OverviewLayout) -> SmallVec<[BoxKind; 6]> {
    BoxKind::ALL
        .into_iter()
        .filter(|k| !layout.has(*k))
        .collect()
}

/// Puts `kind` at its kind minimum on `grid` in the first free place, scanning rows top to
/// bottom and columns left to right (UI-PRF-056).
pub fn add(layout: &mut OverviewLayout, kind: BoxKind, grid: &GridMetrics) -> bool {
    if layout.has(kind) {
        return false;
    }
    let (cols, rows) = grid.min_span(kind);
    let (col, row) = layout.first_free(cols, rows);
    layout.boxes.push(GridBox {
        kind,
        col,
        row,
        cols,
        rows,
    });
    true
}

/// `b` at least its kind minimum on `grid`, inside the 12 columns and the saved row limit.
pub fn fit(b: GridBox, grid: &GridMetrics) -> GridBox {
    let rows_max = OVERVIEW_GRID_ROWS_MAX as u16;
    let (min_cols, min_rows) = grid.min_span(b.kind);
    let cols = b.cols.max(min_cols).min(GRID_COLS);
    let rows = b.rows.max(min_rows).min(rows_max);
    GridBox {
        col: b.col.min(GRID_COLS - cols),
        row: b.row.min(rows_max - rows),
        cols,
        rows,
        ..b
    }
}

/// Puts `fixed` on its cells and moves the other boxes around it (UI-PRF-056). First the other
/// boxes close up into the space `fixed` left. Then, top to bottom, a box that overlaps `fixed`
/// or a box already placed moves down below it, until it overlaps none. Then they close up
/// again around `fixed`. `fixed` stays where it is put. The boxes keep their order in `boxes`
/// and their spans.
pub fn settle(boxes: &mut [GridBox], fixed: GridBox) {
    let Some(at) = boxes.iter().position(|b| b.kind == fixed.kind) else {
        return;
    };
    boxes[at] = fixed;
    close_up(boxes, at, false);
    let mut order: SmallVec<[usize; 8]> = (0..boxes.len()).filter(|&i| i != at).collect();
    order.sort_by_key(|&i| (boxes[i].row, boxes[i].col));
    let mut placed: SmallVec<[usize; 8]> = smallvec![at];
    for &i in &order {
        // Each step lands on the bottom of a box it overlapped, so the row only grows.
        while let Some(bottom) = placed
            .iter()
            .map(|&j| boxes[j])
            .filter(|p| p.overlaps(boxes[i]))
            .map(GridBox::bottom)
            .max()
        {
            boxes[i].row = bottom;
        }
        placed.push(i);
    }
    close_up(boxes, at, true);
}

/// Raises every box but `boxes[held]`, top to bottom, to the bottom of the lowest box above it
/// in its columns, or to row 0. `boxes[held]` stays, and is in the way only when `blocks`.
fn close_up(boxes: &mut [GridBox], held: usize, blocks: bool) {
    let mut order: SmallVec<[usize; 8]> = (0..boxes.len()).filter(|&i| i != held).collect();
    order.sort_by_key(|&i| (boxes[i].row, boxes[i].col));
    for &i in &order {
        let b = boxes[i];
        boxes[i].row = boxes
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i && (j != held || blocks))
            .filter(|(_, p)| p.col < b.right() && b.col < p.right())
            .map(|(_, p)| p.bottom())
            .filter(|&bottom| bottom <= b.row)
            .max()
            .unwrap_or(0);
    }
}

/// Puts `target`'s kind on `target`'s cells, at least its kind minimum on `grid`, and settles
/// the others. Returns whether the layout changed; an edit that would push a box past the
/// saved row limit changes nothing.
pub fn set_cells(layout: &mut OverviewLayout, target: GridBox, grid: &GridMetrics) -> bool {
    set_box_cells(&mut layout.boxes, target, grid)
}

/// [`set_cells`] on the boxes alone.
fn set_box_cells(boxes: &mut SmallVec<[GridBox; 8]>, target: GridBox, grid: &GridMetrics) -> bool {
    if boxes.iter().all(|b| b.kind != target.kind) {
        return false;
    }
    let mut settled = boxes.clone();
    settle(&mut settled, fit(target, grid));
    let rows_max = u32::from(OVERVIEW_GRID_ROWS_MAX as u16);
    let fits = settled
        .iter()
        .all(|b| u32::from(b.row) + u32::from(b.rows) <= rows_max);
    if !fits || settled == *boxes {
        return false;
    }
    *boxes = settled;
    true
}

/// `kind` moved `dx` columns and `dy` rows, inside the grid (Alt+arrow keys).
pub fn moved_by(layout: &OverviewLayout, kind: BoxKind, dx: i32, dy: i32) -> Option<GridBox> {
    let b = layout.get(kind)?;
    Some(GridBox {
        col: shift(b.col, dx, 0, GRID_COLS.saturating_sub(b.cols)),
        row: shift(b.row, dy, 0, u16::MAX - b.rows),
        ..b
    })
}

/// `kind` grown or shrunk `dw` columns at its right edge and `dh` rows at its bottom edge
/// (Alt+Shift+arrow keys).
pub fn grown_by(layout: &OverviewLayout, kind: BoxKind, dw: i32, dh: i32) -> Option<GridBox> {
    let b = layout.get(kind)?;
    Some(GridBox {
        cols: shift(b.cols, dw, 1, GRID_COLS.saturating_sub(b.col).max(1)),
        rows: shift(b.rows, dh, 1, u16::MAX - b.row),
        ..b
    })
}

/// `v + d`, kept between `lo` and `hi`.
fn shift(v: u16, d: i32, lo: u16, hi: u16) -> u16 {
    (i32::from(v) + d).clamp(i32::from(lo), i32::from(hi.max(lo))) as u16
}

/// What a box drag changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grab {
    /// The title line: the box moves.
    Move,
    /// The corner handle: the right and bottom edges move.
    Resize,
}

/// A box drag in progress: the layout's boxes as they were when the button went down. Each
/// pointer position gives the target cells and the boxes settled around them from that start,
/// so the boxes go back when the pointer does.
#[derive(Clone, Debug)]
pub struct BoxDrag {
    pub kind: BoxKind,
    pub grab: Grab,
    /// Where the button went down, window px.
    start: Point<Pixels>,
    /// The boxes at the start.
    boxes: SmallVec<[GridBox; 8]>,
    /// The pointer moved past [`DRAG_START`].
    moved: bool,
    /// The cells the box lands on, once the pointer moved.
    pub target: Option<GridBox>,
}

impl BoxDrag {
    /// A drag of `kind` with the button down at `start`. `None` when the layout has no such box.
    pub fn new(
        layout: &OverviewLayout,
        kind: BoxKind,
        grab: Grab,
        start: Point<Pixels>,
    ) -> Option<Self> {
        layout.get(kind)?;
        Some(Self {
            kind,
            grab,
            start,
            boxes: layout.boxes.clone(),
            moved: false,
            target: None,
        })
    }

    /// The box at the start.
    fn start_box(&self) -> GridBox {
        self.boxes
            .iter()
            .copied()
            .find(|b| b.kind == self.kind)
            .expect("the drag's box is in its start layout")
    }

    /// The cells under the pointer at `at`: the start cells moved or grown by the whole cells
    /// the pointer moved, rounded to the nearest, then fitted to the kind minimum.
    pub fn cells_at(&self, at: Point<Pixels>, grid: &GridMetrics) -> GridBox {
        let step = |d: Pixels, pitch: f32| {
            if pitch > 0. {
                (f32::from(d) / pitch).round() as i32
            } else {
                0
            }
        };
        let dx = step(at.x - self.start.x, grid.cw + grid.gutter);
        let dy = step(at.y - self.start.y, grid.unit + grid.gutter);
        let f = self.start_box();
        let cells = match self.grab {
            Grab::Move => GridBox {
                col: shift(f.col, dx, 0, GRID_COLS.saturating_sub(f.cols)),
                row: shift(f.row, dy, 0, u16::MAX - f.rows),
                ..f
            },
            Grab::Resize => GridBox {
                cols: shift(f.cols, dx, 1, GRID_COLS.saturating_sub(f.col).max(1)),
                rows: shift(f.rows, dy, 1, u16::MAX - f.row),
                ..f
            },
        };
        fit(cells, grid)
    }

    /// The pointer moved to `at`: a press becomes a drag past [`DRAG_START`] × `scale`. Returns
    /// the layout's boxes for the new target cells when they changed.
    pub fn update(
        &mut self,
        at: Point<Pixels>,
        grid: &GridMetrics,
        scale: f32,
    ) -> Option<SmallVec<[GridBox; 8]>> {
        if !self.moved {
            let d = at - self.start;
            if f32::from(d.x).abs().max(f32::from(d.y).abs()) < DRAG_START * scale {
                return None;
            }
            self.moved = true;
        }
        let target = self.cells_at(at, grid);
        if self.target == Some(target) {
            return None;
        }
        self.target = Some(target);
        // An edit past the row limit keeps the start boxes.
        let mut boxes = self.boxes.clone();
        set_box_cells(&mut boxes, target, grid);
        Some(boxes)
    }

    /// The boxes as they were when the button went down.
    pub fn start_boxes(&self) -> &[GridBox] {
        &self.boxes
    }
}

/// Edit Layout mode state, kept by [`OverviewPage`] while the mode is on.
pub struct EditState {
    /// One focus handle per kind in [`BoxKind::ALL`] order; Tab moves between the placed ones.
    focus: [FocusHandle; 6],
    /// The box drag in progress.
    pub drag: Option<BoxDrag>,
    /// The Add Box button's bounds from the last paint, to open its menu below it.
    add_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl EditState {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            drag: None,
            add_bounds: Rc::default(),
        }
    }

    /// The focus handle of `kind`'s box.
    pub fn focus_of(&self, kind: BoxKind) -> &FocusHandle {
        let i = BoxKind::ALL
            .iter()
            .position(|k| *k == kind)
            .expect("every kind is in ALL");
        &self.focus[i]
    }

    /// The kind whose box has focus.
    fn focused(&self, window: &Window) -> Option<BoxKind> {
        BoxKind::ALL
            .into_iter()
            .zip(&self.focus)
            .find(|(_, f)| f.is_focused(window))
            .map(|(k, _)| k)
    }
}

/// The name of `kind` in the Add Box menu.
fn kind_name(kind: BoxKind, cx: &App) -> SharedString {
    tr(
        cx,
        match kind {
            BoxKind::Cpu => "perf-card-heading-cpu",
            BoxKind::Memory => "perf-card-heading-memory",
            BoxKind::Gpu => "perf-overview-box-gpu",
            BoxKind::Disk => "perf-overview-box-disks",
            BoxKind::Net => "perf-overview-box-network",
            BoxKind::TopProcs => "perf-overview-box-processes",
        },
    )
}

impl OverviewPage {
    /// Whether Edit Layout mode is on.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// `PerfOverviewEditLayout`: turns the mode on and takes focus, so Esc reaches the page.
    pub(super) fn on_edit_layout(
        &mut self,
        _: &PerfOverviewEditLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit.is_none() {
            self.edit = Some(EditState::new(cx));
            self.set_edit_chrome(true, cx);
            self.notify(cx);
        }
        window.focus(&self.focus, cx);
    }

    /// The box drag in progress.
    pub fn box_drag(&self) -> Option<&BoxDrag> {
        self.edit.as_ref().and_then(|e| e.drag.as_ref())
    }

    /// `PerfOverviewEditDone` (Done, Esc): turns the mode off. Focus on a box goes to the page.
    /// A drag in progress ends without a change.
    pub(super) fn on_edit_done(
        &mut self,
        _: &PerfOverviewEditDone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit.is_none() {
            return;
        }
        self.end_drag(cx);
        let Some(edit) = self.edit.take() else {
            return;
        };
        if edit.focused(window).is_some() {
            window.focus(&self.focus, cx);
        }
        self.set_edit_chrome(false, cx);
        self.notify(cx);
    }

    /// Ends a box drag or a gap drag in progress without a change: the boxes go back to their
    /// cells at the press, nothing is saved, and a later move or release does nothing. Every
    /// action that changes the layout calls this first, as do leaving the mode, leaving the
    /// page and window focus loss. Returns whether a drag ended.
    pub(super) fn end_drag(&mut self, cx: &mut App) -> bool {
        let box_drag = self.edit.as_mut().and_then(|e| e.drag.take());
        let gap_drag = self.drag.take();
        let start = match (box_drag, gap_drag) {
            (Some(drag), _) => drag.boxes,
            (None, Some(active)) => active.start,
            (None, None) => return false,
        };
        if self.layout.boxes != start {
            self.layout.boxes = start;
            self.relayout(cx);
        }
        cx.notify(self.id);
        true
    }

    /// Tells the box frames whether the edit controls take the right end of the title line, and
    /// redraws every box, which only re-renders for its own changes otherwise.
    fn set_edit_chrome(&self, on: bool, cx: &mut Context<Self>) {
        cx.set_global(EditChrome(on));
        self.cpu.update(cx, |_, cx| cx.notify());
        self.mem.update(cx, |_, cx| cx.notify());
        self.gpu.update(cx, |_, cx| cx.notify());
        self.disk.update(cx, |_, cx| cx.notify());
        self.net.update(cx, |_, cx| cx.notify());
        if let Some(procs) = &self.procs {
            procs.update(cx, |_, cx| cx.notify());
        }
    }

    /// `PerfOverviewResetLayout`: the default layout, saved; the mode stays on.
    pub(super) fn on_reset_layout(
        &mut self,
        _: &PerfOverviewResetLayout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit.is_some() {
            self.end_drag(cx);
            self.commit(OverviewLayout::grid_default(), cx);
        }
    }

    /// `PerfOverviewAddBox`: an item of the Add Box menu.
    pub(super) fn on_add_box(
        &mut self,
        a: &PerfOverviewAddBox,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind).filter(|_| self.edit.is_some()) else {
            return;
        };
        self.end_drag(cx);
        let mut layout = self.layout.clone();
        if add(&mut layout, kind, &self.placement.grid) {
            self.commit(layout, cx);
        }
    }

    /// `PerfOverviewMoveBox` (Alt+arrow keys): moves the focused box one cell; boxes in the way
    /// move down. The box keeps focus.
    pub(super) fn on_move_box(
        &mut self,
        a: &PerfOverviewMoveBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .key_target(window)
            .and_then(|k| moved_by(&self.layout, k, a.dx.into(), a.dy.into()));
        self.key_edit(target, cx);
    }

    /// `PerfOverviewResizeBox` (Alt+Shift+arrow keys): grows or shrinks the focused box one
    /// cell at its right or bottom edge, not below its kind minimum.
    pub(super) fn on_resize_box(
        &mut self,
        a: &PerfOverviewResizeBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .key_target(window)
            .and_then(|k| grown_by(&self.layout, k, a.dw.into(), a.dh.into()));
        self.key_edit(target, cx);
    }

    /// The focused box for a move or resize key: none in the stacked layout or during a drag.
    fn key_target(&mut self, window: &Window) -> Option<BoxKind> {
        let stacked = self.placement.stacked;
        let edit = self.edit.as_mut()?;
        if stacked || edit.drag.as_ref().is_some_and(|d| d.target.is_some()) {
            return None;
        }
        // A press that has not moved is a click, not a drag.
        edit.drag = None;
        edit.focused(window)
    }

    /// Puts a box on `target`'s cells and saves.
    fn key_edit(&mut self, target: Option<GridBox>, cx: &mut Context<Self>) {
        let Some(target) = target else {
            return;
        };
        let mut layout = self.layout.clone();
        if set_cells(&mut layout, target, &self.placement.grid) {
            self.commit(layout, cx);
        }
    }

    /// `PerfOverviewHideBox` (Delete): hides the focused box.
    pub(super) fn on_hide_box(
        &mut self,
        _: &PerfOverviewHideBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(kind) = self.edit.as_ref().and_then(|e| e.focused(window)) {
            self.hide_box(kind, window, cx);
        }
    }

    /// Hides `kind`, or refuses with a toast when it is the only box shown. A drag in progress
    /// ends without a change first, so a later move cannot bring the box back.
    fn hide_box(&mut self, kind: BoxKind, window: &mut Window, cx: &mut Context<Self>) {
        if !self.placement.boxes.iter().any(|b| b.kind != kind) {
            let text = tr(cx, "perf-overview-toast-last-box");
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Info, text, Some(LAST_BOX_TOAST), None, cx)
                });
            }
            return;
        }
        self.end_drag(cx);
        let mut layout = self.layout.clone();
        if hide(&mut layout, kind)
            && self
                .edit
                .as_ref()
                .is_some_and(|e| e.focus_of(kind).is_focused(window))
        {
            window.focus(&self.focus, cx);
        }
        self.commit(layout, cx);
    }

    /// Takes an edited layout and saves it.
    fn commit(&mut self, layout: OverviewLayout, cx: &mut Context<Self>) {
        if layout == self.layout {
            return;
        }
        self.set_layout(layout, cx);
        self.save_layout(cx);
    }

    /// Button down on a title line or a corner handle: `kind`'s box takes focus and a drag may
    /// start. A double-click on a corner sets the box to its kind minimum and saves. The press
    /// stops here, so the box below never sees it.
    fn handle_down(
        &mut self,
        kind: BoxKind,
        grab: Grab,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = &mut self.edit else {
            return;
        };
        cx.stop_propagation();
        window.focus(edit.focus_of(kind), cx);
        edit.drag = None;
        if grab == Grab::Resize && ev.click_count >= 2 {
            let grid = self.placement.grid;
            let mut layout = self.layout.clone();
            let smallest = layout.get(kind).map(|b| GridBox {
                cols: 1,
                rows: 1,
                ..b
            });
            if smallest.is_some_and(|b| set_cells(&mut layout, b, &grid)) {
                self.commit(layout, cx);
                return;
            }
        } else if !self.placement.stacked {
            edit.drag = BoxDrag::new(&self.layout, kind, grab, ev.position);
        }
        self.notify(cx);
    }

    /// The pointer moved while a title line or corner is held: the layout follows the target
    /// cells under it, and the ghost shows them.
    pub(super) fn box_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let grid = self.placement.grid;
        let Some(drag) = self.edit.as_mut().and_then(|e| e.drag.as_mut()) else {
            return;
        };
        let Some(boxes) = drag.update(position, &grid, scale) else {
            return;
        };
        if boxes != self.layout.boxes {
            self.layout.boxes = boxes;
            self.relayout(cx);
        }
        self.notify(cx);
    }

    /// The button went up at `position`: the drag ends where the pointer is, and a changed
    /// layout is saved. The last move event can be older than the release (a fast drop), so
    /// the target is found again here. `None` is a move without the button, whose release was
    /// not seen: the drag ends where the last move left it.
    pub(super) fn box_drag_up(&mut self, position: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let grid = self.placement.grid;
        let Some(mut drag) = self.edit.as_mut().and_then(|e| e.drag.take()) else {
            return;
        };
        if let Some(boxes) = position.and_then(|at| drag.update(at, &grid, scale)) {
            self.layout.boxes = boxes;
            self.relayout(cx);
        }
        if self.layout.boxes.as_slice() != drag.start_boxes() {
            self.save_layout(cx);
        }
        self.notify(cx);
    }

    /// The 40 px toolbar: Add Box, Reset Layout and, at the right, Done.
    pub(super) fn edit_toolbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.edit.as_ref()?;
        let m = *metrics(cx);
        let page_focus = self.focus.clone();
        let hidden: SmallVec<[(BoxKind, SharedString); 6]> = hidden_kinds(&self.layout)
            .into_iter()
            .map(|k| (k, kind_name(k, cx)))
            .collect();
        let empty = hidden.is_empty();
        let bounds = Rc::clone(&edit.add_bounds);
        let menu_focus = page_focus.clone();
        // A dropdown trigger: its items dispatch `PerfOverviewAddBox` on the page.
        let add_button = TlButton::new(ADD_BUTTON_ID, TlButtonKind::Ghost)
            .size(TlButtonSize::Sm)
            .trailing_icon(TlIcon::ChevronDown)
            .icon_size(m.icon.sm)
            .label(tr(cx, "perf-overview-add-box"))
            .disabled(empty)
            .on_click(move |ev, window, cx| {
                let b = bounds.get();
                let at = if b.size.width > px(0.) {
                    b.bottom_left()
                } else {
                    ev.position()
                };
                let hidden = hidden.clone();
                let menu = TlMenu::build(window, cx, menu_focus.clone(), move |mut menu, _, _| {
                    for (kind, name) in &hidden {
                        menu = menu.item(
                            name.clone(),
                            Box::new(PerfOverviewAddBox {
                                kind: kind.key().into(),
                            }),
                        );
                    }
                    menu
                });
                open_context_menu_at(at, window, cx, menu);
            });
        let measure = {
            let bounds = Rc::clone(&edit.add_bounds);
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .inset_0()
        };
        Some(
            div()
                .debug_selector(|| TOOLBAR_ID.into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(TOOLBAR_H * m.ui_scale))
                .px(px(PAGE_PAD * m.ui_scale))
                // The buttons dispatch on the focused handle: keep it on the page.
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if !page_focus.contains_focused(window, cx) {
                        window.focus(&page_focus, cx);
                    }
                })
                .child(v_flex().relative().child(add_button).child(measure))
                .child(
                    TlButton::new(RESET_BUTTON_ID, TlButtonKind::Ghost)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "perf-overview-reset-layout"))
                        .action(Box::new(PerfOverviewResetLayout)),
                )
                .child(div().flex_1())
                .child(
                    TlButton::new(DONE_BUTTON_ID, TlButtonKind::Primary)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "perf-overview-edit-done"))
                        .action(Box::new(PerfOverviewEditDone)),
                )
                .into_any_element(),
        )
    }

    /// `el`, the placed box `p`, in edit mode: a focus stop with a keyboard focus ring.
    pub(super) fn edit_box(&self, p: &PlacedBox, el: Div, window: &Window, cx: &App) -> Div {
        let Some(edit) = &self.edit else {
            return el;
        };
        let theme = current(cx);
        let m = metrics(cx);
        let handle = edit.focus_of(p.kind).clone();
        let focus = handle.clone();
        let mut el =
            el.track_focus(&handle)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.focus(&focus, cx);
                });
        if TlFocusRing::show(handle.is_focused(window), window) {
            el = TlFocusRing::ring(el, m.radius_card, theme);
        }
        el
    }

    /// The grip, gear and Hide box button over each box's title line, the resize handle at
    /// each box's bottom-right corner, and the ghost outline of a drag's target cells, in page
    /// content px. The stacked layout has no resize handles.
    pub(super) fn edit_overlays(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SmallVec<[AnyElement; 16]> {
        let mut out = SmallVec::new();
        let Some(edit) = &self.edit else {
            return out;
        };
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let tip_text = tr(cx, "perf-overview-hide-box");
        let movable = !self.placement.stacked;
        for p in &self.placement.boxes {
            let kind = p.kind;
            let b = p.bounds;
            if movable {
                let corner = px(CORNER * s);
                out.push(
                    div()
                        .id(SharedString::from(corner_id(kind)))
                        .debug_selector(move || corner_id(kind))
                        .absolute()
                        .left(b.right() - corner)
                        .top(b.bottom() - corner)
                        .size(corner)
                        .cursor_nwse_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                this.handle_down(kind, Grab::Resize, ev, window, cx);
                            }),
                        )
                        // A quick click can go up before the next frame puts the window's
                        // release handler in place.
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                                this.box_drag_up(Some(ev.position), cx);
                            }),
                        )
                        .child(
                            div()
                                .absolute()
                                .right(px(MARK * s))
                                .bottom(px(MARK * s))
                                .size(px((CORNER / 2.) * s))
                                .border_r(px(MARK * s))
                                .border_b(px(MARK * s))
                                .border_color(theme.text_secondary),
                        )
                        .into_any_element(),
                );
            }
            let grip = div()
                .debug_selector(move || grip_id(kind))
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(HANDLE * s))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::GripVertical)
                        .size(px(HANDLE * s))
                        .text_color(theme.text_secondary),
                );
            let hide_id = hide_button_id(kind);
            let tip = HoverTooltip::state(SharedString::from(hide_id.clone()), window, cx);
            let hide_button = div()
                .id(SharedString::from(hide_id.clone()))
                .debug_selector(move || hide_id.clone())
                .absolute()
                .right_0()
                .top_0()
                .bottom_0()
                .w(px(HANDLE * s))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.hide_box(kind, window, cx);
                    }),
                )
                .child(
                    Icon::new(IconName::X)
                        .size(px(HANDLE * s))
                        .text_color(theme.text_secondary),
                );
            let hide_button = HoverTooltip::attach(
                &tip,
                hide_button,
                DELAY_CONTROL,
                TlTooltip::text(tip_text.clone()).build(),
                window,
                cx,
            );
            out.push(
                div()
                    .id(SharedString::from(title_handle_id(kind)))
                    .debug_selector(move || title_handle_id(kind))
                    .absolute()
                    .left(b.left())
                    .top(b.top())
                    .w(b.size.width)
                    .h(px(1. + TITLE_H * s))
                    .when(movable, |el| el.cursor_grab())
                    // No `occlude()`: it takes the Performance tab out of the hit test, so the
                    // tab's right-press capture never opens the box menu. The left press stops
                    // at the handle (title, gear, Hide box), so the box below never sees it.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            this.handle_down(kind, Grab::Move, ev, window, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                            this.box_drag_up(Some(ev.position), cx);
                        }),
                    )
                    // The handle occludes the box below, so the box menu is named here.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.title_handle_right_down(kind, cx);
                        }),
                    )
                    .child(grip)
                    .child(self.box_gear(kind, cx))
                    .child(hide_button)
                    .into_any_element(),
            );
        }
        if let Some(target) = edit.drag.as_ref().and_then(|d| d.target) {
            let g = self.placement.grid.bounds(target);
            out.push(
                div()
                    .debug_selector(|| GHOST_ID.into())
                    .absolute()
                    .left(g.left())
                    .top(g.top())
                    .w(g.size.width)
                    .h(g.size.height)
                    .border(px(MARK * s))
                    .border_color(theme.accent)
                    .rounded(m.radius_card)
                    .bg(theme.accent.alpha(0.08))
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::point;
    use proptest::prelude::*;

    use super::*;
    use crate::pages::performance::overview::layout::ROW_UNIT_DEFAULT;

    use BoxKind::{Cpu, Disk, Gpu, Memory, Net, TopProcs};

    /// (col, row, cols, rows) of `kind`.
    fn cells(layout: &OverviewLayout, kind: BoxKind) -> (u16, u16, u16, u16) {
        let b = layout.get(kind).expect("placed");
        (b.col, b.row, b.cols, b.rows)
    }

    /// `kind`'s box in `layout` with its top left cell at (`col`, `row`).
    fn at(layout: &OverviewLayout, kind: BoxKind, col: u16, row: u16) -> GridBox {
        GridBox {
            col,
            row,
            ..layout.get(kind).expect("placed")
        }
    }

    #[test]
    fn overview_grid_move_pushes_down() {
        let grid = GridMetrics::new(1200., ROW_UNIT_DEFAULT, 1.);
        // Net onto CPU's top left: CPU moves down below it, and GPU and Disk below CPU.
        // Memory is not in the way and stays.
        let mut l = OverviewLayout::grid_default();
        let target = at(&l, Net, 0, 0);
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Net), (0, 0, 4, 6));
        assert_eq!(cells(&l, Cpu), (0, 6, 8, 7));
        assert_eq!(cells(&l, Memory), (8, 0, 4, 7));
        assert_eq!(cells(&l, Gpu), (0, 13, 4, 6));
        assert_eq!(cells(&l, Disk), (4, 13, 4, 6));
        assert_eq!(l.validate(), Ok(()));
        let kinds: Vec<BoxKind> = l.boxes.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [Cpu, Memory, Gpu, Disk, Net],
            "the box order is kept"
        );

        // CPU down by the 6 rows of GPU and Disk: they close up into the space CPU left, and
        // CPU goes under them.
        let mut l = OverviewLayout::grid_default();
        let target = at(&l, Cpu, 0, 6);
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Gpu), (0, 0, 4, 6));
        assert_eq!(cells(&l, Disk), (4, 0, 4, 6));
        assert_eq!(cells(&l, Cpu), (0, 6, 8, 7));
        assert_eq!(cells(&l, Net), (8, 7, 4, 6));
        // Memory under Net: Net closes up to row 0.
        let target = at(&l, Memory, 8, 6);
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Net), (8, 0, 4, 6));
        assert_eq!(cells(&l, Memory), (8, 6, 4, 7));
        assert_eq!(l.validate(), Ok(()));

        // A box keeps the cells it is put on, even with free rows above it.
        let mut l = OverviewLayout::grid_default();
        let target = at(&l, Net, 8, 9);
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Net), (8, 9, 4, 6));
        // Its own cells change nothing.
        let mut l = OverviewLayout::grid_default();
        let target = at(&l, Cpu, 0, 0);
        assert!(!set_cells(&mut l, target, &grid));
        assert_eq!(l, OverviewLayout::grid_default());

        // A move ends at the kind minimum: CPU saved at 2 columns comes back at 4. A box is
        // kept inside the 12 columns.
        let mut l = OverviewLayout::grid_default();
        l.boxes = smallvec![GridBox {
            kind: Cpu,
            col: 0,
            row: 0,
            cols: 2,
            rows: 7,
        }];
        let target = at(&l, Cpu, 11, 0);
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Cpu), (8, 0, 4, 7));

        // The pointer moves a box by whole cells: a column and its gutter are 97.83 px, a row
        // and its gutter 46 px.
        let l = OverviewLayout::grid_default();
        let start = point(px(900.), px(400.));
        let drag = BoxDrag::new(&l, Net, Grab::Move, start).expect("placed");
        let cell = |x: f32, y: f32| {
            let b = drag.cells_at(point(start.x + px(x), start.y + px(y)), &grid);
            (b.col, b.row)
        };
        assert_eq!(cell(-48., 22.), (8, 7));
        assert_eq!(cell(-50., 24.), (7, 8));
        assert_eq!(cell(-5000., -5000.), (0, 0));
        assert_eq!(cell(5000., 0.), (8, 7));
    }

    #[test]
    fn overview_grid_resize_snaps() {
        let grid = GridMetrics::new(1200., ROW_UNIT_DEFAULT, 1.);
        let l = OverviewLayout::grid_default();
        let start = point(px(400.), px(600.));
        let to = |x: f32, y: f32| point(start.x + px(x), start.y + px(y));
        let mut drag = BoxDrag::new(&l, Gpu, Grab::Resize, start).expect("placed");
        // 2 px is not a drag yet.
        assert_eq!(drag.update(to(2., 2.), &grid, 1.), None);
        assert_eq!(drag.target, None);
        // Under half a cell keeps the span.
        assert_eq!(drag.update(to(40., 20.), &grid, 1.), Some(l.boxes.clone()));
        assert_eq!(drag.target, l.get(Gpu));
        assert_eq!(drag.update(to(41., 21.), &grid, 1.), None, "same target");
        // 1.5 columns and 1.1 rows: 2 more columns and 1 more row. Disk is in the way and
        // moves down below GPU.
        let boxes = drag.update(to(147., 50.), &grid, 1.).expect("new target");
        let mut grown = l.clone();
        grown.boxes = boxes;
        assert_eq!(cells(&grown, Gpu), (0, 7, 6, 7));
        assert_eq!(cells(&grown, Disk), (4, 14, 4, 6));
        assert_eq!(cells(&grown, Net), (8, 7, 4, 6));
        assert_eq!(grown.validate(), Ok(()));
        // Shrinking stops at the kind minimum, 3 × 6 at 1200 px, and growing at the grid's
        // right edge.
        drag.update(to(-5000., -5000.), &grid, 1.);
        assert_eq!(drag.target.map(|b| (b.cols, b.rows)), Some((3, 6)));
        drag.update(to(5000., 0.), &grid, 1.);
        assert_eq!(drag.target.map(|b| (b.cols, b.rows)), Some((12, 6)));

        // The keys grow and shrink one cell at the right or bottom edge, not below the kind
        // minimum.
        let mut l = OverviewLayout::grid_default();
        let target = grown_by(&l, Net, 0, 1).expect("placed");
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Net), (8, 7, 4, 7));
        let target = grown_by(&l, Net, 1, 0).expect("placed");
        assert!(
            !set_cells(&mut l, target, &grid),
            "the right edge of the grid"
        );
        let target = grown_by(&l, Memory, -1, 0).expect("placed");
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Memory), (8, 0, 3, 7));
        let target = grown_by(&l, Memory, -1, 0).expect("placed");
        assert!(!set_cells(&mut l, target, &grid), "the kind minimum");
        let target = moved_by(&l, Memory, 1, 0).expect("placed");
        assert!(set_cells(&mut l, target, &grid));
        assert_eq!(cells(&l, Memory), (9, 0, 3, 7));

        // The kind minimum and the saved row limit bound the span.
        let tall = GridBox {
            kind: Cpu,
            col: 0,
            row: 999,
            cols: 1,
            rows: 1,
        };
        let fitted = fit(tall, &grid);
        assert_eq!((fitted.row, fitted.cols, fitted.rows), (993, 4, 7));
    }

    proptest! {
        #[test]
        fn overview_grid_no_overlap_after_any_drag(
            width in 720f32..3000.,
            unit in 24f32..=80.,
            procs in any::<bool>(),
            drags in proptest::collection::vec(
                (0usize..6, any::<bool>(), any::<bool>(), -3000f32..3000., -3000f32..3000.),
                1..12,
            ),
        ) {
            let grid = GridMetrics::new(width, unit, 1.);
            let mut layout = OverviewLayout::grid_default();
            if procs {
                add(&mut layout, TopProcs, &grid);
            }
            for (k, resize, key, dx, dy) in drags {
                let kind = layout.boxes[k % layout.boxes.len()].kind;
                let start = layout.boxes.clone();
                let grab = if resize { Grab::Resize } else { Grab::Move };
                let target = if key {
                    // One key press: a step of one cell.
                    let (sx, sy) = (dx.signum() as i32, dy.signum() as i32);
                    let target = if resize {
                        grown_by(&layout, kind, sx, sy)
                    } else {
                        moved_by(&layout, kind, sx, sy)
                    }
                    .expect("placed");
                    set_cells(&mut layout, target, &grid);
                    fit(target, &grid)
                } else {
                    let from = point(px(500.), px(500.));
                    let mut drag = BoxDrag::new(&layout, kind, grab, from).expect("placed");
                    if let Some(boxes) = drag.update(from + point(px(dx), px(dy)), &grid, 1.) {
                        layout.boxes = boxes;
                    }
                    drag.target.unwrap_or_else(|| layout.get(kind).expect("placed"))
                };
                prop_assert_eq!(layout.validate(), Ok(()));
                let kinds = |b: &[GridBox]| b.iter().map(|b| b.kind).collect::<Vec<_>>();
                prop_assert_eq!(kinds(&start), kinds(&layout.boxes));
                if layout.boxes == start {
                    continue;
                }
                // The box is on its target cells, the others keep their columns and spans, and
                // each of them rests on row 0 or on a box above it.
                prop_assert_eq!(layout.get(kind), Some(target));
                for (a, b) in start.iter().zip(&layout.boxes).filter(|(a, _)| a.kind != kind) {
                    prop_assert_eq!((a.col, a.cols, a.rows), (b.col, b.cols, b.rows));
                    let rests = b.row == 0
                        || layout.boxes.iter().any(|p| {
                            p.kind != b.kind
                                && p.bottom() == b.row
                                && p.col < b.right()
                                && b.col < p.right()
                        });
                    prop_assert!(rests, "{b:?} floats in {:?}", layout.boxes);
                }
            }
        }
    }

    #[test]
    fn add_and_hide() {
        let grid = GridMetrics::new(1200., ROW_UNIT_DEFAULT, 1.);
        let mut layout = OverviewLayout::grid_default();
        assert_eq!(hidden_kinds(&layout).as_slice(), [TopProcs]);
        // Top processes goes under the default grid at its kind minimum, 4 × 7 at 1200 px.
        assert!(add(&mut layout, TopProcs, &grid));
        assert_eq!(
            layout.get(TopProcs),
            Some(GridBox {
                kind: TopProcs,
                col: 0,
                row: 13,
                cols: 4,
                rows: 7
            })
        );
        assert!(!add(&mut layout, TopProcs, &grid));
        // Hiding frees the cells; the boxes around keep theirs.
        assert!(hide(&mut layout, Gpu));
        assert!(!hide(&mut layout, Gpu));
        assert_eq!(layout.get(Disk).map(|b| b.col), Some(4));
        // Net comes back at 3 × 6 in the first free place: GPU's old cells.
        assert!(hide(&mut layout, Net));
        assert!(add(&mut layout, Net, &grid));
        assert_eq!(
            layout.get(Net).map(|b| (b.col, b.row, b.cols, b.rows)),
            Some((0, 7, 3, 6))
        );
        assert_eq!(layout.validate(), Ok(()));
        assert!(hide(&mut layout, Memory));
        assert_eq!(hidden_kinds(&layout).as_slice(), [Memory, Gpu]);
    }
}
