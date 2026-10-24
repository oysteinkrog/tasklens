//! Edit Layout mode of the Overview page (UI-PRF-018 "Edit layout mode", D-023).
//!
//! The page menu item "Edit Layout" turns it on. A 40 px toolbar shows at the top of the page
//! (Add Box, Reset Layout, Done), and each box gets a grip before its title and a Hide box
//! button at the right end of its title line. Both sit in the title line's side margins, drawn
//! by the page over the cached box views, so the boxes stay live and never re-render for edit
//! mode. A box moves by dragging its title line: the left or right third of another box puts it
//! beside that box, the middle third swaps the two, and a row gap (or the space above the first
//! row or below the last) makes a new row there. Every edit action saves the layout.
//!
//! The functions on [`OverviewLayout`] here are pure, so the drop rules are tested without a
//! window. [`EditState`] and the `OverviewPage` methods hold the view side.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, v_flex};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, FocusHandle, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, SharedString, Styled as _,
    Window, canvas, div, point, px, size,
};
use smallvec::{SmallVec, smallvec};

use super::OverviewPage;
use super::layout::{BoxKind, BoxSpec, OverviewLayout, PlacedBox, Placement, RowSpec};
use super::parts::EditChrome;
use super::splitter::{Drag, Target, round_weights, set_weights};
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
use crate::text::style::{numeric_style, t_md};
use crate::theme::{current, metrics};

/// The toolbar height, logical px.
pub const TOOLBAR_H: f32 = 40.;

/// The box title line under the 1 px top border, logical px (the frame's `TITLE_ROW`).
const TITLE_H: f32 = 24.;

/// The grip and the Hide box button, logical px.
const HANDLE: f32 = 16.;

/// Half the gap between boxes and rows, logical px: drop lines sit in the middle of a gap.
const HALF_GAP: f32 = 4.;

/// The drop line width.
const DROP_LINE: f32 = 3.;

/// The pointer moves this far before a press on a title line becomes a drag, logical px.
const DRAG_START: f32 = 3.;

/// The "At least one box must stay on the Overview page" toast.
pub const LAST_BOX_TOAST: ToastKey = ToastKey(FIXED_KEY_BASE | 0x0403);

/// Debug selectors and element ids.
pub const TOOLBAR_ID: &str = "perf-overview-edit-toolbar";
pub const ADD_BUTTON_ID: &str = "perf-overview-add-box";
pub const RESET_BUTTON_ID: &str = "perf-overview-reset-layout";
pub const DONE_BUTTON_ID: &str = "perf-overview-edit-done";
pub const DROP_LINE_ID: &str = "perf-overview-drop-line";
pub const DROP_SWAP_ID: &str = "perf-overview-drop-swap";
pub const GHOST_ID: &str = "perf-overview-drag-ghost";
pub const GHOST_VALUE_ID: &str = "perf-overview-drag-ghost-value";

/// The debug selector of `kind`'s title line drag area in edit mode.
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

/// Where a dragged box lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropTarget {
    /// Beside `kind` in its row: before it (its left third) or after it (its right third).
    Beside { kind: BoxKind, before: bool },
    /// Swaps places with `kind` (its middle third).
    Swap(BoxKind),
    /// A new row at this layout row index: a row gap, above the first row or below the last.
    NewRow(usize),
}

/// The layout row and slot of `kind`.
pub fn find(layout: &OverviewLayout, kind: BoxKind) -> Option<(usize, usize)> {
    layout.rows.iter().enumerate().find_map(|(r, row)| {
        row.boxes
            .iter()
            .position(|b| b.kind == kind)
            .map(|i| (r, i))
    })
}

/// Removes the box at row `r`, slot `i`. The row goes when it becomes empty.
fn take(layout: &mut OverviewLayout, r: usize, i: usize) -> BoxSpec {
    let spec = layout.rows[r].boxes.remove(i);
    if layout.rows[r].boxes.is_empty() {
        layout.rows.remove(r);
    }
    spec
}

/// The rows with a placed box, top to bottom: (layout row, top, bottom) in px. A row that
/// wrapped onto extra lines is one band.
fn row_bands(layout: &OverviewLayout, placement: &Placement) -> SmallVec<[(usize, f32, f32); 4]> {
    let mut out: SmallVec<[(usize, f32, f32); 4]> = SmallVec::new();
    for b in &placement.boxes {
        let Some((r, _)) = find(layout, b.kind) else {
            continue;
        };
        let (top, bottom) = (f32::from(b.bounds.top()), f32::from(b.bounds.bottom()));
        match out.last_mut() {
            Some(last) if last.0 == r => {
                last.1 = last.1.min(top);
                last.2 = last.2.max(bottom);
            }
            _ => out.push((r, top, bottom)),
        }
    }
    out
}

/// The drop target under `at` (page content px) while `dragged` is dragged. `None` over the
/// dragged box, over a gap inside a row, and next to the dragged box's own row when it is
/// alone there (a new row would change nothing).
pub fn drop_target(
    layout: &OverviewLayout,
    placement: &Placement,
    dragged: BoxKind,
    at: Point<Pixels>,
) -> Option<DropTarget> {
    let (x, y) = (f32::from(at.x), f32::from(at.y));
    let over = placement
        .boxes
        .iter()
        .find(|b| b.kind != dragged && b.bounds.contains(&at));
    let target = if let Some(b) = over {
        let third = (x - f32::from(b.bounds.left())) / f32::from(b.bounds.size.width);
        if third < 1. / 3. {
            DropTarget::Beside {
                kind: b.kind,
                before: true,
            }
        } else if third > 2. / 3. {
            DropTarget::Beside {
                kind: b.kind,
                before: false,
            }
        } else {
            DropTarget::Swap(b.kind)
        }
    } else {
        let bands = row_bands(layout, placement);
        let (first, last) = (bands.first()?, bands.last()?);
        if y < first.1 {
            DropTarget::NewRow(first.0)
        } else if y >= last.2 {
            DropTarget::NewRow(layout.rows.len())
        } else {
            bands
                .windows(2)
                .find(|w| y >= w[0].2 && y < w[1].1)
                .map(|w| DropTarget::NewRow(w[1].0))?
        }
    };
    if let DropTarget::NewRow(at) = target {
        let (r, _) = find(layout, dragged)?;
        if layout.rows[r].boxes.len() == 1 && (at == r || at == r + 1) {
            return None;
        }
    }
    Some(target)
}

/// Drops `dragged` on `target`. A box that joins another row takes width 1; a new row has
/// height 1; a row left empty is removed. Returns whether the layout changed.
pub fn apply_drop(layout: &mut OverviewLayout, dragged: BoxKind, target: DropTarget) -> bool {
    let before = layout.clone();
    let Some((r, i)) = find(layout, dragged) else {
        return false;
    };
    match target {
        DropTarget::Swap(other) => {
            let Some((r2, i2)) = find(layout, other) else {
                return false;
            };
            layout.rows[r].boxes[i].kind = other;
            layout.rows[r2].boxes[i2].kind = dragged;
        }
        DropTarget::Beside { kind, before: left } => {
            let Some((r2, _)) = find(layout, kind).filter(|_| kind != dragged) else {
                return false;
            };
            let mut spec = take(layout, r, i);
            if r2 != r {
                spec.width = 1.;
            }
            let Some((r2, i2)) = find(layout, kind) else {
                return false;
            };
            layout.rows[r2]
                .boxes
                .insert(if left { i2 } else { i2 + 1 }, spec);
        }
        DropTarget::NewRow(at) => {
            let row_goes = layout.rows[r].boxes.len() == 1;
            take(layout, r, i);
            let at = if row_goes && r < at { at - 1 } else { at };
            layout.rows.insert(
                at.min(layout.rows.len()),
                RowSpec {
                    height: 1.,
                    boxes: smallvec![BoxSpec {
                        kind: dragged,
                        width: 1.
                    }],
                },
            );
        }
    }
    *layout != before
}

/// Removes `kind` from its row; the row goes when it becomes empty.
pub fn hide(layout: &mut OverviewLayout, kind: BoxKind) -> bool {
    let Some((r, i)) = find(layout, kind) else {
        return false;
    };
    take(layout, r, i);
    true
}

/// The kinds the layout does not place, in [`BoxKind::ALL`] order: the "Add Box" menu.
pub fn hidden_kinds(layout: &OverviewLayout) -> SmallVec<[BoxKind; 6]> {
    BoxKind::ALL
        .into_iter()
        .filter(|k| !layout.has(*k))
        .collect()
}

/// Puts `kind` at the end of the last row with width 1, or as a new row of height 2 when the
/// last row already holds 3 boxes.
pub fn add(layout: &mut OverviewLayout, kind: BoxKind) -> bool {
    if layout.has(kind) {
        return false;
    }
    let spec = BoxSpec { kind, width: 1. };
    match layout.rows.last_mut() {
        Some(row) if row.boxes.len() < 3 => row.boxes.push(spec),
        _ => layout.rows.push(RowSpec {
            height: 2.,
            boxes: smallvec![spec],
        }),
    }
    true
}

/// The keyboard moves. `dx` −1 or 1 moves `kind` one slot left or right in its row. `dy` −1 or
/// 1 moves it to the end of the row above or below (width 1), or into a new row of height 1
/// past either end. Returns whether the layout changed.
pub fn move_box(layout: &mut OverviewLayout, kind: BoxKind, dx: i8, dy: i8) -> bool {
    let Some((r, i)) = find(layout, kind) else {
        return false;
    };
    if dx != 0 {
        let row = &mut layout.rows[r];
        let Some(j) = i
            .checked_add_signed(isize::from(dx.signum()))
            .filter(|j| *j < row.boxes.len())
        else {
            return false;
        };
        row.boxes.swap(i, j);
        return true;
    }
    if dy == 0 {
        return false;
    }
    let alone = layout.rows[r].boxes.len() == 1;
    let up = dy < 0;
    let next = if up {
        r.checked_sub(1)
    } else {
        Some(r + 1).filter(|t| *t < layout.rows.len())
    };
    match next {
        Some(t) => {
            let mut spec = take(layout, r, i);
            spec.width = 1.;
            let t = if alone && t > r { t - 1 } else { t };
            layout.rows[t].boxes.push(spec);
        }
        None if alone => return false,
        None => {
            let mut spec = take(layout, r, i);
            spec.width = 1.;
            let at = if up { 0 } else { layout.rows.len() };
            layout.rows.insert(
                at,
                RowSpec {
                    height: 1.,
                    boxes: smallvec![spec],
                },
            );
        }
    }
    true
}

/// The splitter a size key moves for `kind`, and the pointer step along it that grows `kind`.
/// Width: against the right neighbour (the left one for the last box), by 0.1 of the row's
/// placed width. Height: against the row below (above for the last row), by 0.1 of the placed
/// rows' height. `None` without a neighbour on that axis.
pub fn size_step(
    layout: &OverviewLayout,
    placement: &Placement,
    kind: BoxKind,
    width: bool,
) -> Option<(Target, f32)> {
    let (r, _) = find(layout, kind)?;
    if width {
        let row: SmallVec<[(BoxKind, f32); 4]> = placement
            .boxes
            .iter()
            .filter(|b| find(layout, b.kind).map(|f| f.0) == Some(r))
            .map(|b| (b.kind, f32::from(b.bounds.size.width)))
            .collect();
        let pos = row.iter().position(|b| b.0 == kind)?;
        let step = 0.1 * row.iter().map(|b| b.1).sum::<f32>();
        match row.get(pos + 1) {
            Some(next) => Some((Target::Boxes(kind, next.0), step)),
            None => {
                let prev = row.get(pos.checked_sub(1)?)?;
                Some((Target::Boxes(prev.0, kind), -step))
            }
        }
    } else {
        let bands = row_bands(layout, placement);
        let pos = bands.iter().position(|b| b.0 == r)?;
        let step = 0.1 * bands.iter().map(|b| b.2 - b.1).sum::<f32>();
        match bands.get(pos + 1) {
            Some(next) => Some((Target::Rows(r, next.0), step)),
            None => {
                let prev = bands.get(pos.checked_sub(1)?)?;
                Some((Target::Rows(prev.0, r), -step))
            }
        }
    }
}

/// The 3 px drop line of `target` in page content px. A swap has none: its target box gets
/// a border instead.
pub fn drop_line(
    layout: &OverviewLayout,
    placement: &Placement,
    target: DropTarget,
    scale: f32,
) -> Option<Bounds<Pixels>> {
    let half = HALF_GAP * scale;
    match target {
        DropTarget::Swap(_) => None,
        DropTarget::Beside { kind, before } => {
            let b = placement.boxes.iter().find(|b| b.kind == kind)?.bounds;
            let x = if before {
                f32::from(b.left()) - half
            } else {
                f32::from(b.right()) + half
            };
            Some(Bounds::new(
                point(px(x - DROP_LINE / 2.), b.top()),
                size(px(DROP_LINE), b.size.height),
            ))
        }
        DropTarget::NewRow(at) => {
            let bands = row_bands(layout, placement);
            let prev = bands.iter().rev().find(|b| b.0 < at);
            let next = bands.iter().find(|b| b.0 >= at);
            let y = match (prev, next) {
                (Some(p), Some(n)) => (p.2 + n.1) / 2.,
                (None, Some(n)) => n.1 - half,
                (Some(p), None) => p.2 + half,
                (None, None) => return None,
            };
            let left = placement.boxes.iter().map(|b| f32::from(b.bounds.left()));
            let right = placement.boxes.iter().map(|b| f32::from(b.bounds.right()));
            let (left, right) = (left.fold(f32::MAX, f32::min), right.fold(0., f32::max));
            Some(Bounds::new(
                point(px(left), px(y - DROP_LINE / 2.)),
                size(px(right - left), px(DROP_LINE)),
            ))
        }
    }
}

/// A box drag in progress.
#[derive(Clone, Copy, Debug)]
pub struct BoxDrag {
    pub kind: BoxKind,
    /// Where the button went down, window px.
    start: Point<Pixels>,
    /// The pointer's offset from the box origin when the button went down.
    grab: Point<Pixels>,
    /// The pointer in page content px.
    pointer: Point<Pixels>,
    /// The pointer moved past [`DRAG_START`].
    moved: bool,
    pub target: Option<DropTarget>,
}

/// Edit Layout mode state, kept by [`OverviewPage`] while the mode is on.
pub struct EditState {
    /// One focus handle per kind in [`BoxKind::ALL`] order; Tab moves between the placed ones.
    focus: [FocusHandle; 6],
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

/// The name of `kind` in the Add Box menu and on the drag copy.
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
    /// The title and key value on `kind`'s title line, as the box last rendered them. Disks,
    /// Network and Top processes show their name and no value.
    fn title_and_value(&self, kind: BoxKind, cx: &App) -> (SharedString, SharedString) {
        let name = || (kind_name(kind, cx), SharedString::default());
        match kind {
            BoxKind::Cpu => {
                let (title, value, _) = self.cpu.read(cx).text();
                (title.clone(), value.clone())
            }
            BoxKind::Memory => {
                let (title, value) = self.mem.read(cx).text();
                (title.clone(), value.clone())
            }
            BoxKind::Gpu => self
                .gpu
                .read(cx)
                .sections()
                .first()
                .map(|s| (s.heading.clone(), s.value.clone()))
                .unwrap_or_else(name),
            BoxKind::Disk | BoxKind::Net | BoxKind::TopProcs => name(),
        }
    }

    /// Whether Edit Layout mode is on.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The box drag in progress.
    pub fn box_drag(&self) -> Option<BoxDrag> {
        self.edit.as_ref().and_then(|e| e.drag)
    }

    pub(super) fn box_dragging(&self) -> bool {
        self.box_drag().is_some()
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

    /// `PerfOverviewEditDone` (Done, Esc): turns the mode off. Focus on a box goes to the page.
    pub(super) fn on_edit_done(
        &mut self,
        _: &PerfOverviewEditDone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        if edit.focused(window).is_some() {
            window.focus(&self.focus, cx);
        }
        self.set_edit_chrome(false, cx);
        self.notify(cx);
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
            self.commit(OverviewLayout::btop_default(), cx);
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
        let mut layout = self.layout.clone();
        if add(&mut layout, kind) {
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

    /// `PerfOverviewMoveBox` (Alt+arrows): moves the focused box, which keeps focus.
    pub(super) fn on_move_box(
        &mut self,
        a: &PerfOverviewMoveBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = self.edit.as_ref().and_then(|e| e.focused(window)) else {
            return;
        };
        let mut layout = self.layout.clone();
        if move_box(&mut layout, kind, a.dx, a.dy) {
            self.commit(layout, cx);
        }
    }

    /// `PerfOverviewResizeBox` (Alt+Shift+arrows): grows or shrinks the focused box's width or
    /// row height by one step against its neighbour, within the kind minimums.
    pub(super) fn on_resize_box(
        &mut self,
        a: &PerfOverviewResizeBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = self.edit.as_ref().and_then(|e| e.focused(window)) else {
            return;
        };
        let (width, sign) = if a.dw != 0 {
            (true, f32::from(a.dw.signum()))
        } else {
            (false, f32::from(a.dh.signum()))
        };
        if sign == 0. {
            return;
        }
        let Some((target, step)) = size_step(&self.layout, &self.placement, kind, width) else {
            return;
        };
        let scale = metrics(cx).ui_scale;
        let Some(drag) = Drag::start(&self.layout, &self.placement, target, 0., scale) else {
            return;
        };
        let mut layout = self.layout.clone();
        set_weights(&mut layout, target, drag.weights_at(sign * step));
        round_weights(&mut layout);
        self.commit(layout, cx);
    }

    /// Hides `kind`, or refuses with a toast when it is the only box shown.
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
        let mut layout = self.layout.clone();
        if !hide(&mut layout, kind) {
            return;
        }
        if self
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

    /// Button down on a title line: a drag of `kind` may start.
    fn box_drag_down(
        &mut self,
        kind: BoxKind,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(placed) = self.placement.boxes.iter().find(|b| b.kind == kind) else {
            return;
        };
        let origin = self.content_origin.get();
        let pointer = ev.position - origin;
        let Some(edit) = &mut self.edit else {
            return;
        };
        cx.stop_propagation();
        window.focus(edit.focus_of(kind), cx);
        edit.drag = Some(BoxDrag {
            kind,
            start: ev.position,
            grab: pointer - placed.bounds.origin,
            pointer,
            moved: false,
            target: None,
        });
        self.notify(cx);
    }

    /// The pointer moved while a title line is held: the copy follows it and the drop target
    /// under it shows.
    pub(super) fn box_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let origin = self.content_origin.get();
        let Some(drag) = self.edit.as_mut().and_then(|e| e.drag.as_mut()) else {
            return;
        };
        let d = position - drag.start;
        if !drag.moved && f32::from(d.x).abs().max(f32::from(d.y).abs()) < DRAG_START * scale {
            return;
        }
        drag.moved = true;
        drag.pointer = position - origin;
        drag.target = drop_target(&self.layout, &self.placement, drag.kind, drag.pointer);
        self.notify(cx);
    }

    /// The button went up: the box lands on the drop target, and the layout is saved.
    pub(super) fn box_drag_up(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.edit.as_mut().and_then(|e| e.drag.take()) else {
            return;
        };
        let mut layout = self.layout.clone();
        match drag.target.filter(|_| drag.moved) {
            Some(target) if apply_drop(&mut layout, drag.kind, target) => self.commit(layout, cx),
            _ => self.notify(cx),
        }
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

    /// `el`, the placed box `p`, in edit mode: a focus stop with a keyboard focus ring, at
    /// opacity 0.3 while dragged, with a 2 px accent border while it is the swap target.
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
        if let Some(drag) = edit.drag.filter(|d| d.moved) {
            if drag.kind == p.kind {
                el = el.opacity(0.3);
            }
            if drag.target == Some(DropTarget::Swap(p.kind)) {
                el = el.child(
                    div()
                        .debug_selector(|| DROP_SWAP_ID.into())
                        .absolute()
                        .inset_0()
                        .border_2()
                        .border_color(theme.accent)
                        .rounded(m.radius_card),
                );
            }
        }
        if TlFocusRing::show(handle.is_focused(window), window) {
            el = TlFocusRing::ring(el, m.radius_card, theme);
        }
        el
    }

    /// The grip and Hide box button over each box's title line, the drop line and the dragged
    /// copy, all in page content px.
    pub(super) fn edit_overlays(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SmallVec<[AnyElement; 8]> {
        let mut out = SmallVec::new();
        let Some(edit) = &self.edit else {
            return out;
        };
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let tip_text = tr(cx, "perf-overview-hide-box");
        for p in &self.placement.boxes {
            let kind = p.kind;
            let b = p.bounds;
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
                    // No `occlude()`: it takes the Performance tab out of the hit test, so the
                    // tab's right-press capture never opens the box menu. The left press stops
                    // at the handle (drag, gear, Hide box), so the box below never sees it.
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            this.box_drag_down(kind, ev, window, cx);
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
        let Some(drag) = edit.drag.filter(|d| d.moved) else {
            return out;
        };
        if let Some(line) = drag
            .target
            .and_then(|t| drop_line(&self.layout, &self.placement, t, s))
        {
            out.push(
                div()
                    .debug_selector(|| DROP_LINE_ID.into())
                    .absolute()
                    .left(line.left())
                    .top(line.top())
                    .w(line.size.width)
                    .h(line.size.height)
                    .bg(theme.accent)
                    .into_any_element(),
            );
        }
        if let Some(p) = self.placement.boxes.iter().find(|b| b.kind == drag.kind) {
            // The frame and title line only, without charts or rows, so the drag never renders
            // the box twice per frame.
            let at = drag.pointer - drag.grab;
            let accent = theme.graph.device_accent(drag.kind.accent());
            let (title, value) = self.title_and_value(drag.kind, cx);
            let mut tab = div()
                .h_full()
                .flex()
                .items_center()
                .flex_none()
                .px(m.sp[4])
                .bg(theme.list_bg)
                .border_l_1()
                .border_r_1()
                .border_b_1()
                .border_color(accent)
                .rounded_bl(m.radius_small)
                .rounded_br(m.radius_small)
                .whitespace_nowrap()
                .text_color(theme.text_primary)
                .child(title);
            *tab.text_style() = t_md(&m);
            let mut value_el = div()
                .debug_selector(|| GHOST_VALUE_ID.into())
                .flex_none()
                .whitespace_nowrap()
                .text_color(theme.text_primary)
                .child(value);
            *value_el.text_style() = numeric_style(&m, m.text.md);
            out.push(
                div()
                    .debug_selector(|| GHOST_ID.into())
                    .absolute()
                    .left(at.x)
                    .top(at.y)
                    .w(p.bounds.size.width)
                    .h(p.bounds.size.height)
                    .opacity(0.8)
                    .bg(theme.list_bg)
                    .border_1()
                    .border_color(accent)
                    .rounded(m.radius_card)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(m.sp[4])
                            .h(px(TITLE_H * s))
                            .mx(m.sp[5])
                            .child(tab)
                            .child(div().flex_1())
                            .child(value_el),
                    )
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::performance::overview::layout::place;

    fn kinds(layout: &OverviewLayout) -> Vec<Vec<BoxKind>> {
        layout
            .rows
            .iter()
            .map(|r| r.boxes.iter().map(|b| b.kind).collect())
            .collect()
    }

    fn placed(layout: &OverviewLayout) -> Placement {
        place(layout, |_| true, size(px(1200.), px(900.)), 1.)
    }

    fn center(p: &Placement, kind: BoxKind) -> Point<Pixels> {
        p.boxes
            .iter()
            .find(|b| b.kind == kind)
            .map(|b| b.bounds.center())
            .expect("placed")
    }

    use BoxKind::{Cpu, Disk, Gpu, Memory, Net, TopProcs};

    #[test]
    fn drop_thirds_and_row_gaps() {
        let layout = OverviewLayout::btop_default();
        let p = placed(&layout);
        let disk = p
            .boxes
            .iter()
            .find(|b| b.kind == Disk)
            .expect("disk")
            .bounds;
        let y = disk.center().y;
        let at = |x: f32| point(disk.left() + px(x), y);
        assert_eq!(
            drop_target(&layout, &p, Cpu, at(10.)),
            Some(DropTarget::Beside {
                kind: Disk,
                before: true
            })
        );
        assert_eq!(
            drop_target(&layout, &p, Cpu, center(&p, Disk)),
            Some(DropTarget::Swap(Disk))
        );
        let right = f32::from(disk.size.width) - 10.;
        assert_eq!(
            drop_target(&layout, &p, Cpu, at(right)),
            Some(DropTarget::Beside {
                kind: Disk,
                before: false
            })
        );
        // The row gap at y 532..540, the top padding and the bottom padding.
        assert_eq!(
            drop_target(&layout, &p, Cpu, point(px(600.), px(536.))),
            Some(DropTarget::NewRow(1))
        );
        assert_eq!(
            drop_target(&layout, &p, Disk, point(px(600.), px(8.))),
            Some(DropTarget::NewRow(0))
        );
        assert_eq!(
            drop_target(&layout, &p, Disk, point(px(600.), px(892.))),
            Some(DropTarget::NewRow(2))
        );
        // Over the dragged box itself, or the gap inside a row: nothing.
        assert_eq!(drop_target(&layout, &p, Cpu, center(&p, Cpu)), None);
        assert_eq!(
            drop_target(&layout, &p, Cpu, point(px(793.), px(200.))),
            None
        );
    }

    #[test]
    fn middle_third_swaps_at_every_width() {
        let layout = OverviewLayout::btop_default();
        for w in (720..=1600).step_by(40) {
            let p = place(&layout, |_| true, size(px(w as f32), px(900.)), 1.);
            for b in p.boxes.iter().filter(|b| b.kind != Cpu) {
                let (left, width) = (f32::from(b.bounds.left()), f32::from(b.bounds.size.width));
                let y = b.bounds.center().y;
                // Just inside each edge of the middle third, and at its center.
                for f in [0.34, 0.5, 0.66] {
                    assert_eq!(
                        drop_target(&layout, &p, Cpu, point(px(left + width * f), y)),
                        Some(DropTarget::Swap(b.kind)),
                        "window {w}, {:?}, at {f}",
                        b.kind
                    );
                }
            }
        }
    }

    #[test]
    fn lone_box_next_to_its_own_row_is_no_target() {
        let mut layout = OverviewLayout::btop_default();
        assert!(apply_drop(&mut layout, Memory, DropTarget::NewRow(1)));
        let p = placed(&layout);
        let mem = p
            .boxes
            .iter()
            .find(|b| b.kind == Memory)
            .expect("mem")
            .bounds;
        // The gaps above and below Memory's own row.
        let above = point(px(600.), mem.top() - px(4.));
        let below = point(px(600.), mem.bottom() + px(4.));
        assert_eq!(drop_target(&layout, &p, Memory, above), None);
        assert_eq!(drop_target(&layout, &p, Memory, below), None);
        assert_eq!(
            drop_target(&layout, &p, Cpu, below),
            Some(DropTarget::NewRow(2))
        );
    }

    #[test]
    fn apply_swap_beside_and_new_row() {
        let mut layout = OverviewLayout::btop_default();
        assert!(apply_drop(&mut layout, Cpu, DropTarget::Swap(Disk)));
        assert_eq!(kinds(&layout), [vec![Disk, Memory], vec![Gpu, Cpu, Net]]);
        // The slots keep their widths.
        assert_eq!(layout.rows[0].boxes[0].width, 2.);

        let mut layout = OverviewLayout::btop_default();
        assert!(apply_drop(
            &mut layout,
            Memory,
            DropTarget::Beside {
                kind: Gpu,
                before: false
            }
        ));
        assert_eq!(kinds(&layout), [vec![Cpu], vec![Gpu, Memory, Disk, Net]]);
        assert_eq!(layout.rows[1].boxes[1].width, 1.);

        // Inside one row the box keeps its width.
        let mut layout = OverviewLayout::btop_default();
        assert!(apply_drop(
            &mut layout,
            Cpu,
            DropTarget::Beside {
                kind: Memory,
                before: false
            }
        ));
        assert_eq!(kinds(&layout)[0], [Memory, Cpu]);
        assert_eq!(layout.rows[0].boxes[1].width, 2.);

        // A row left empty goes, and the new row index follows.
        let mut layout = OverviewLayout::btop_default();
        assert!(apply_drop(&mut layout, Memory, DropTarget::NewRow(2)));
        assert!(apply_drop(&mut layout, Cpu, DropTarget::NewRow(3)));
        assert_eq!(
            kinds(&layout),
            [vec![Gpu, Disk, Net], vec![Memory], vec![Cpu]]
        );
        assert_eq!(layout.rows[1].height, 1.);
        assert_eq!(layout.validate(), Ok(()));
    }

    #[test]
    fn add_and_hide() {
        let mut layout = OverviewLayout::btop_default();
        assert_eq!(hidden_kinds(&layout).as_slice(), [TopProcs]);
        assert!(add(&mut layout, TopProcs));
        assert_eq!(kinds(&layout)[2], [TopProcs]);
        assert_eq!(layout.rows[2].height, 2.);
        assert!(!add(&mut layout, TopProcs));
        assert!(hide(&mut layout, Net));
        assert!(hide(&mut layout, TopProcs));
        assert_eq!(layout.rows.len(), 2);
        assert!(add(&mut layout, Net));
        assert_eq!(kinds(&layout)[1], [Gpu, Disk, Net]);
        assert_eq!(layout.rows[1].boxes[2].width, 1.);
    }

    #[test]
    fn keyboard_moves() {
        let mut layout = OverviewLayout::btop_default();
        assert!(move_box(&mut layout, Disk, -1, 0));
        assert_eq!(kinds(&layout)[1], [Disk, Gpu, Net]);
        assert!(!move_box(&mut layout, Disk, -1, 0));
        assert!(move_box(&mut layout, Disk, 0, -1));
        assert_eq!(kinds(&layout), [vec![Cpu, Memory, Disk], vec![Gpu, Net]]);
        assert!(move_box(&mut layout, Cpu, 0, -1));
        assert_eq!(kinds(&layout)[0], [Cpu]);
        // Alone in the top row, Alt+Up changes nothing.
        assert!(!move_box(&mut layout, Cpu, 0, -1));
        assert!(move_box(&mut layout, Cpu, 0, 1));
        assert_eq!(kinds(&layout), [vec![Memory, Disk, Cpu], vec![Gpu, Net]]);
        assert!(move_box(&mut layout, Net, 0, 1));
        assert_eq!(kinds(&layout)[2], [Net]);
        assert_eq!(layout.validate(), Ok(()));
    }

    #[test]
    fn size_step_against_the_neighbour() {
        let layout = OverviewLayout::btop_default();
        let p = placed(&layout);
        // Row 0 is 773 + 387 px wide.
        let (t, step) = size_step(&layout, &p, Cpu, true).expect("cpu");
        assert_eq!(t, Target::Boxes(Cpu, Memory));
        assert!((step - 116.).abs() < 0.01);
        let (t, step) = size_step(&layout, &p, Net, true).expect("net");
        assert_eq!(t, Target::Boxes(Disk, Net));
        assert!(step < 0.);
        let (t, _) = size_step(&layout, &p, Gpu, false).expect("gpu row");
        assert_eq!(t, Target::Rows(0, 1));
    }
}
