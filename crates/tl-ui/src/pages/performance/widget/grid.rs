//! The inner snap grid of a widget container (ADR-019 "Inner grid", UI-PRF-061).
//!
//! Pure functions, like `overview/layout.rs` and `overview/edit.rs`, so the rules are tested
//! without a window:
//!
//! - **Cells**: `cols` columns over the body width, a row unit of 24 logical px and a 4 px
//!   gutter both ways. Column width `cw = (W − (cols − 1)·gutter) / cols`.
//! - **Fill** (D-037, D-040): when the widgets end above the bottom, every row grows by the same
//!   amount, `max(unit, (H − (R − 1)·gutter) / R)` with `R` the last used row. A taller layout
//!   keeps the unit and scrolls vertically; the columns then leave the scrollbar's width free.
//!   Columns never scroll sideways.
//! - **Minimum span**: `max(1, ceil(min_w / cw))` columns and `max(1, ceil(min_h / unit))` rows,
//!   from the smallest composition of the widget's forms. It is enforced when a move, resize or
//!   add ends; at draw time a widget keeps its saved span.
//! - **Insert**: the default span (3 × 4 with a chart, else the full width × 1), raised to the
//!   minimum, in the first free place scanning rows top to bottom and columns left to right.
//! - **Move and resize**: the widget lands on whole cells, at least its minimum span and inside
//!   the columns. The others close up into the space it left, widgets in the way move down, and
//!   all close up again around it, as the Overview grid does (UI-PRF-056).
//! - **Shrink**: a smaller box changes no cell. Spans stay and the body scrolls.
//! - **Scale**: the unit, the gutter and every minimum are logical px × `ui_scale`. Spans do not
//!   change with the scale.
//!
//! [`pin`] turns a flow layout into a grid layout when Edit Widgets starts.
//!
//! A device page container (ADR-019 phase 3, "Inner grid of a page") has a 12 px row unit and a
//! 12 px gutter (`sp4`), and its rows never fill the body: a page is a scrolling column, so a
//! short layout leaves space under it. [`InnerGrid::of`] and [`place_in`] take these rules
//! from the container id.

use std::sync::Arc;

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::{SmallVec, smallvec};
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FormSet, WidgetCell, WidgetContent, WidgetSetting, grid_cols,
};
use tl_model::layout_pages::PAGE_CONTAINER_PREFIX;

use super::compose::{smallest, widget_smallest};

/// The row unit, logical px.
pub const ROW_UNIT: f32 = 24.;
/// The gutter between cells in both directions (`sp2`), logical px.
pub const GUTTER: f32 = 4.;
/// The width a scrolling body keeps free for its scrollbar, logical px.
pub const SCROLLBAR_W: f32 = 16.;
/// The span of a new widget that shows a chart.
pub const CHART_SPAN: (u16, u16) = (3, 4);
/// The row unit of a page container, logical px.
pub const PAGE_ROW_UNIT: f32 = 12.;
/// The gutter of a page container (`sp4`, the gap between the 1.3.0 page parts), logical px.
pub const PAGE_GUTTER: f32 = 12.;
/// The span of a new widget with a chart on a page: the 150 px page chart.
pub const PAGE_CHART_SPAN: (u16, u16) = (6, 7);
/// The rows of a new widget without a chart on a page.
pub const PAGE_METRIC_ROWS: u16 = 2;
/// The rows of a new group or fixed widget on a page.
pub const PAGE_GROUP_ROWS: u16 = 16;

/// Whether `container` is a device page container (`page.<key>`).
pub fn is_page(container: &str) -> bool {
    container.starts_with(PAGE_CONTAINER_PREFIX)
}

/// The row unit of `container`, logical px: 12 on a page, 24 in an Overview box.
pub fn grid_unit(container: &str) -> f32 {
    if is_page(container) {
        PAGE_ROW_UNIT
    } else {
        ROW_UNIT
    }
}

/// The gutter of `container`, logical px: 12 on a page, 4 in an Overview box.
pub fn grid_gutter(container: &str) -> f32 {
    if is_page(container) {
        PAGE_GUTTER
    } else {
        GUTTER
    }
}

/// Whether the rows of `container` grow to fill its body (D-037, D-040). A page's rows never
/// do: a page is a scrolling column (UI-PRF-020).
pub fn grid_fills(container: &str) -> bool {
    !is_page(container)
}

/// The cell sizes of a container body, in px (logical px × scale): the column width `cw`, the
/// row unit `unit`, the drawn row height `row` (at least `unit`, D-037) and the `gutter`.
/// Minimums count in `unit`, cells are drawn in `row`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct InnerGrid {
    pub cols: u16,
    pub cw: f32,
    pub unit: f32,
    pub row: f32,
    pub gutter: f32,
    pub scale: f32,
}

impl InnerGrid {
    /// The grid of a body `width` px wide with `cols` columns, rows at the unit, with the
    /// Overview box unit and gutter.
    pub fn new(width: f32, cols: u16, scale: f32) -> Self {
        Self::with_rules(width, cols, ROW_UNIT, GUTTER, scale)
    }

    /// The grid of container `container` on a body `width` px wide: its columns, unit and
    /// gutter.
    pub fn of(container: &str, width: f32, scale: f32) -> Self {
        Self::with_rules(
            width,
            grid_cols(container),
            grid_unit(container),
            grid_gutter(container),
            scale,
        )
    }

    /// The grid of a body `width` px wide with `cols` columns, a row unit of `unit` and a
    /// gutter of `gutter` logical px.
    pub fn with_rules(width: f32, cols: u16, unit: f32, gutter: f32, scale: f32) -> Self {
        let scale = sane_scale(scale);
        let cols = cols.max(1);
        let gutter = gutter * scale;
        let gutters = f32::from(cols - 1) * gutter;
        Self {
            cols,
            cw: ((width - gutters) / f32::from(cols)).max(0.),
            unit: unit * scale,
            row: unit * scale,
            gutter,
            scale,
        }
    }

    /// The left edge of column `col`.
    pub fn x(&self, col: u16) -> f32 {
        f32::from(col) * (self.cw + self.gutter)
    }

    /// The top edge of row `row`.
    pub fn y(&self, row: u16) -> f32 {
        f32::from(row) * (self.row + self.gutter)
    }

    /// The width of `cols` columns: `cols·cw + (cols − 1)·gutter`.
    pub fn w(&self, cols: u16) -> f32 {
        f32::from(cols) * self.cw + f32::from(cols.saturating_sub(1)) * self.gutter
    }

    /// The height of `rows` drawn rows: `rows·row + (rows − 1)·gutter`.
    pub fn h(&self, rows: u16) -> f32 {
        f32::from(rows) * self.row + f32::from(rows.saturating_sub(1)) * self.gutter
    }

    /// The bounds of `cell`, every edge rounded to a whole px.
    pub fn bounds(&self, cell: WidgetCell) -> Bounds<Pixels> {
        snapped(
            self.x(cell.col),
            self.y(cell.row),
            self.w(cell.cols),
            self.h(cell.rows),
        )
    }

    /// The smallest span of a widget whose smallest composition is `min` logical px:
    /// `max(1, ceil(min_w / cw))` columns, at most `cols`, and `max(1, ceil(min_h / unit))` rows.
    pub fn min_span(&self, min: Size<f32>) -> (u16, u16) {
        let span = |min: f32, cell: f32| {
            if cell > 0. {
                (min * self.scale / cell - 1e-3)
                    .ceil()
                    .clamp(1., f32::from(u16::MAX)) as u16
            } else {
                u16::MAX
            }
        };
        (
            span(min.width, self.cw).clamp(1, self.cols),
            span(min.height, self.unit).max(1),
        )
    }

    /// The smallest span of a widget with `forms`, from its smallest composition (UI-PRF-061).
    pub fn forms_min_span(&self, forms: FormSet) -> (u16, u16) {
        self.min_span(smallest(forms))
    }

    /// The smallest span of widget `w`: [`Self::forms_min_span`] for a metric widget, one item
    /// for a group, the fixed widget's minimum (ADR-019 phase 2 "Minimum span").
    pub fn widget_min_span(&self, w: &WidgetSetting) -> (u16, u16) {
        self.min_span(widget_smallest(w))
    }
}

fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    }
}

/// Bounds with every edge rounded to a whole px, so neighbours never overlap or leave a seam.
fn snapped(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    let (left, top) = (x.round(), y.round());
    let (right, bottom) = ((x + w).round(), (y + h).round());
    Bounds::new(
        point(px(left), px(top)),
        size(px(right - left), px(bottom - top)),
    )
}

/// One placed widget: its id and its bounds in the body, px.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlacedWidget {
    pub id: u32,
    pub bounds: Bounds<Pixels>,
}

/// Where the widgets of a container go. `content_h` is the height of all rows; the body
/// scrolls when `scrolls`. `grid` is the cell size used.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct InnerPlacement {
    pub widgets: Vec<PlacedWidget>,
    pub content_h: Pixels,
    pub scrolls: bool,
    pub grid: InnerGrid,
}

impl InnerPlacement {
    /// The bounds of widget `id`.
    pub fn bounds(&self, id: u32) -> Option<Bounds<Pixels>> {
        self.widgets.iter().find(|w| w.id == id).map(|w| w.bounds)
    }
}

/// The bottom row of `cell`.
fn bottom(cell: &WidgetCell) -> u16 {
    cell.row.saturating_add(cell.rows)
}

/// The right column of `cell`.
fn right(cell: &WidgetCell) -> u16 {
    cell.col.saturating_add(cell.cols)
}

/// `cell` inside `cols` columns with spans of at least 1, as drawn. A saved span is kept when
/// it fits; a cell past the last column moves left first, then narrows.
fn inside(cell: WidgetCell, cols: u16) -> WidgetCell {
    let cols_span = cell.cols.clamp(1, cols);
    WidgetCell {
        col: cell.col.min(cols - cols_span),
        row: cell.row,
        cols: cols_span,
        rows: cell.rows.max(1),
    }
}

/// Places `cells` (widget id and cells) on a body of `area` px with `cols` columns at UI scale
/// `scale`. Rows fill the body (D-037, D-040) unless `frozen` holds them at that many logical
/// px (at least the unit), as Edit Widgets does so cells do not move under a drag. A layout
/// taller than the body keeps its rows and scrolls; its columns leave the scrollbar free.
pub fn place(
    cells: &[(u32, WidgetCell)],
    cols: u16,
    area: Size<Pixels>,
    scale: f32,
    frozen: Option<f32>,
) -> InnerPlacement {
    place_with(cells, cols, (ROW_UNIT, GUTTER, true), area, scale, frozen)
}

/// [`place`] with the rules of container `container`: its columns, unit and gutter, and no
/// fill on a page.
pub fn place_in(
    container: &str,
    cells: &[(u32, WidgetCell)],
    area: Size<Pixels>,
    scale: f32,
    frozen: Option<f32>,
) -> InnerPlacement {
    let rules = (
        grid_unit(container),
        grid_gutter(container),
        grid_fills(container),
    );
    place_with(cells, grid_cols(container), rules, area, scale, frozen)
}

/// [`place`] with `rules`: the row unit and the gutter in logical px, and whether the rows fill
/// the body.
fn place_with(
    cells: &[(u32, WidgetCell)],
    cols: u16,
    rules: (f32, f32, bool),
    area: Size<Pixels>,
    scale: f32,
    frozen: Option<f32>,
) -> InnerPlacement {
    let (unit, gutter, fills) = rules;
    let scale = sane_scale(scale);
    let cols = cols.max(1);
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let cells: SmallVec<[(u32, WidgetCell); 16]> =
        cells.iter().map(|&(id, c)| (id, inside(c, cols))).collect();
    let rows = cells.iter().map(|(_, c)| bottom(c)).max().unwrap_or(0);
    let mut grid = InnerGrid::with_rules(w, cols, unit, gutter, scale);
    if let Some(row) = frozen {
        grid.row = (row * scale).max(grid.unit);
    }
    let content =
        |row: f32| f32::from(rows) * row + f32::from(rows.saturating_sub(1)) * grid.gutter;
    let scrolls = rows > 0 && h > 0. && content(grid.row) > h + 0.5;
    if scrolls {
        let row = grid.row;
        grid = InnerGrid::with_rules((w - SCROLLBAR_W * scale).max(0.), cols, unit, gutter, scale);
        grid.row = row;
    } else if fills && frozen.is_none() && rows > 0 && h > 0. {
        let free = h - f32::from(rows - 1) * grid.gutter;
        grid.row = (free / f32::from(rows)).max(grid.unit);
    }
    let widgets: Vec<PlacedWidget> = cells
        .iter()
        .map(|&(id, cell)| PlacedWidget {
            id,
            bounds: grid.bounds(cell),
        })
        .collect();
    let content_h = widgets
        .iter()
        .map(|p| f32::from(p.bounds.bottom()))
        .fold(0., f32::max);
    InnerPlacement {
        widgets,
        content_h: px(content_h),
        scrolls,
        grid,
    }
}

/// The span a new widget with `forms` gets on a grid of `cols` columns: 3 × 4 with a chart,
/// else the full width × 1 (6 × 1 in the GPU box).
pub fn default_span(forms: FormSet, cols: u16) -> (u16, u16) {
    let cols = cols.max(1);
    if forms.contains(FormSet::CHART) {
        (CHART_SPAN.0.min(cols), CHART_SPAN.1)
    } else {
        (cols, 1)
    }
}

/// The rows a new group or fixed widget spans (ADR-019 phase 2).
pub const GROUP_ROWS: u16 = 8;

/// The span a new widget `w` gets on a grid of `cols` columns (ADR-019 phase 2): with a chart
/// half the columns (rounded up) × 4, any other metric widget all columns × 1, a group or fixed
/// widget all columns × 8. On the GPU box's 6 columns a chart is 3 × 4, as [`default_span`].
pub fn widget_default_span(w: &WidgetSetting, cols: u16) -> (u16, u16) {
    let cols = cols.max(1);
    match w.content {
        WidgetContent::Metric(_) if w.forms.contains(FormSet::CHART) => {
            (cols.div_ceil(2), CHART_SPAN.1)
        }
        WidgetContent::Metric(_) => (cols, 1),
        WidgetContent::Group(_) | WidgetContent::Fixed(_) => (cols, GROUP_ROWS),
    }
}

/// The span a new widget `w` gets in container `container`: on a page (ADR-019 phase 3) 6 × 7
/// with a chart, 12 × 2 for any other metric widget and 12 × 16 for a group or fixed widget,
/// each inside the columns; in a box [`widget_default_span`].
pub fn widget_default_span_in(container: &str, w: &WidgetSetting) -> (u16, u16) {
    let cols = grid_cols(container).max(1);
    if !is_page(container) {
        return widget_default_span(w, cols);
    }
    match w.content {
        WidgetContent::Metric(_) if w.forms.contains(FormSet::CHART) => {
            (PAGE_CHART_SPAN.0.min(cols), PAGE_CHART_SPAN.1)
        }
        WidgetContent::Metric(_) => (cols, PAGE_METRIC_ROWS),
        WidgetContent::Group(_) | WidgetContent::Fixed(_) => (cols, PAGE_GROUP_ROWS),
    }
}

/// The first place a widget of `span` fits among `cells` on `cols` columns, scanning rows from
/// `from` down and columns left to right. With no free place it goes under the lowest widget.
pub fn first_free(cells: &[WidgetCell], span: (u16, u16), cols: u16, from: u16) -> WidgetCell {
    let cols = cols.max(1);
    let (w, h) = (span.0.clamp(1, cols), span.1.max(1));
    let lowest = cells.iter().map(bottom).max().unwrap_or(0).max(from);
    for row in from..=lowest {
        for col in 0..=cols - w {
            let probe = WidgetCell {
                col,
                row,
                cols: w,
                rows: h,
            };
            if cells.iter().all(|c| !c.overlaps(&probe)) {
                return probe;
            }
        }
    }
    WidgetCell {
        col: 0,
        row: lowest,
        cols: w,
        rows: h,
    }
}

/// Adds `widget` to the grid layout `layout` (ADR-019 "Insert"): it takes the next id, its
/// default span raised to its minimum span on `grid`, and the first free place. Returns its id.
pub fn insert(layout: &mut ContainerLayout, mut widget: WidgetSetting, grid: &InnerGrid) -> u32 {
    let (dw, dh) = if is_page(&layout.container) {
        widget_default_span_in(&layout.container, &widget)
    } else {
        widget_default_span(&widget, grid.cols)
    };
    let (mw, mh) = grid.widget_min_span(&widget);
    let span = (dw.max(mw).min(grid.cols), dh.max(mh));
    let cells: SmallVec<[WidgetCell; 16]> = layout.widgets.iter().filter_map(|w| w.cell).collect();
    let id = layout.next_id.max(
        layout
            .widgets
            .iter()
            .map(|w| w.id.saturating_add(1))
            .max()
            .unwrap_or(1),
    );
    layout.next_id = id.saturating_add(1);
    widget.id = id;
    widget.cell = Some(first_free(&cells, span, grid.cols, 0));
    layout.widgets.push(widget);
    id
}

/// `cell` at least `min` (columns, rows) and inside `cols` columns.
pub fn fit(cell: WidgetCell, min: (u16, u16), cols: u16) -> WidgetCell {
    let cols = cols.max(1);
    let w = cell.cols.max(min.0).clamp(1, cols);
    let h = cell.rows.max(min.1).max(1);
    WidgetCell {
        col: cell.col.min(cols - w),
        row: cell.row,
        cols: w,
        rows: h,
    }
}

/// Puts `cells[at]` where it is and moves the others around it (UI-PRF-056, ADR-019). First
/// the others close up into the space it left. Then, top to bottom, a cell that overlaps it or
/// a cell already placed moves down below it. Then they close up again around it. The cells
/// keep their order and spans.
pub fn settle(cells: &mut [WidgetCell], at: usize) {
    if at >= cells.len() {
        return;
    }
    close_up(cells, at, false);
    let mut order: SmallVec<[usize; 16]> = (0..cells.len()).filter(|&i| i != at).collect();
    order.sort_by_key(|&i| (cells[i].row, cells[i].col));
    let mut placed: SmallVec<[usize; 16]> = smallvec![at];
    for &i in &order {
        // Each step lands on the bottom of a cell it overlapped, so the row only grows.
        while let Some(b) = placed
            .iter()
            .map(|&j| cells[j])
            .filter(|p| p.overlaps(&cells[i]))
            .map(|p| bottom(&p))
            .max()
        {
            cells[i].row = b;
        }
        placed.push(i);
    }
    close_up(cells, at, true);
}

/// Raises every cell but `cells[held]`, top to bottom, to the bottom of the lowest cell above
/// it in its columns, or to row 0. `cells[held]` stays, and is in the way only when `blocks`.
fn close_up(cells: &mut [WidgetCell], held: usize, blocks: bool) {
    let mut order: SmallVec<[usize; 16]> = (0..cells.len()).filter(|&i| i != held).collect();
    order.sort_by_key(|&i| (cells[i].row, cells[i].col));
    for &i in &order {
        let c = cells[i];
        cells[i].row = cells
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i && (j != held || blocks))
            .filter(|(_, p)| p.col < right(&c) && c.col < right(p))
            .map(|(_, p)| bottom(p))
            .filter(|&b| b <= c.row)
            .max()
            .unwrap_or(0);
    }
}

/// Puts widget `id` of the grid layout `layout` on `target` (a move or a resize), at least its
/// minimum span on `grid` and inside the columns, and settles the others. Returns whether a
/// cell changed.
pub fn set_cell(
    layout: &mut ContainerLayout,
    id: u32,
    target: WidgetCell,
    grid: &InnerGrid,
) -> bool {
    let Some(at) = layout.widgets.iter().position(|w| w.id == id) else {
        return false;
    };
    let min = grid.widget_min_span(&layout.widgets[at]);
    // Widgets without cells (none in a loaded grid layout) take no part.
    let index: SmallVec<[usize; 16]> = (0..layout.widgets.len())
        .filter(|&i| i == at || layout.widgets[i].cell.is_some())
        .collect();
    let mut cells: SmallVec<[WidgetCell; 16]> = index
        .iter()
        .map(|&i| layout.widgets[i].cell.unwrap_or(target))
        .collect();
    let held = index.iter().position(|&i| i == at).unwrap_or(0);
    cells[held] = fit(target, min, grid.cols);
    settle(&mut cells, held);
    let mut changed = false;
    for (&i, cell) in index.iter().zip(cells) {
        let w = &mut layout.widgets[i];
        if w.cell != Some(cell) {
            w.cell = Some(cell);
            changed = true;
        }
    }
    changed
}

/// Widget `id`'s cell moved `dx` columns and `dy` rows, inside the columns (Alt+arrow keys).
pub fn moved_by(layout: &ContainerLayout, id: u32, dx: i32, dy: i32) -> Option<WidgetCell> {
    let c = layout.widgets.iter().find(|w| w.id == id)?.cell?;
    let cols = tl_model::layout::grid_cols(&layout.container);
    Some(WidgetCell {
        col: shift(c.col, dx, 0, cols.saturating_sub(c.cols)),
        row: shift(c.row, dy, 0, u16::MAX - c.rows),
        ..c
    })
}

/// Widget `id`'s cell grown or shrunk `dw` columns at its right edge and `dh` rows at its
/// bottom edge (Alt+Shift+arrow keys).
pub fn grown_by(layout: &ContainerLayout, id: u32, dw: i32, dh: i32) -> Option<WidgetCell> {
    let c = layout.widgets.iter().find(|w| w.id == id)?.cell?;
    let cols = tl_model::layout::grid_cols(&layout.container);
    Some(WidgetCell {
        cols: shift(c.cols, dw, 1, cols.saturating_sub(c.col).max(1)),
        rows: shift(c.rows, dh, 1, u16::MAX - c.row),
        ..c
    })
}

/// `v + d`, kept between `lo` and `hi`.
fn shift(v: u16, d: i32, lo: u16, hi: u16) -> u16 {
    (i32::from(v) + d).clamp(i32::from(lo), i32::from(hi.max(lo))) as u16
}

/// How a widget of a flow layout was drawn, for [`pin`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Drawn {
    /// At these bounds in the body, px.
    At(Bounds<Pixels>),
    /// Cut by a fit rule ("+n more"): it would show with more room.
    Cut,
    /// Left out: an empty slot or a metric the device does not report.
    Absent,
}

/// The device in a slot of a flow layout: its persistent key and its name.
#[derive(Clone, PartialEq, Debug)]
pub struct SlotDevice {
    pub key: Arc<str>,
    pub name: Arc<str>,
}

/// The grid layout that a flow layout `layout` becomes when Edit Widgets starts (ADR-019
/// "Editing"). `drawn` says how each widget was drawn (a widget not listed is absent), `slots`
/// the devices in slot 0 and slot 1, and `grid` the body's grid at the unit row.
///
/// - A drawn widget gets the cells nearest its rectangle: rounded to whole cells, spans of at
///   least 1; then, in reading order, a widget that overlaps one placed before moves down.
/// - A slot selector becomes the device's persistent key, and `device_name` is set.
/// - Cut widgets go under the lowest widget at their minimum span, in list order.
/// - Absent widgets and widgets of an empty slot are left out.
///
/// The result has no preset; `next_id` and the widgets' ids, forms and options stay.
pub fn pin(
    layout: &ContainerLayout,
    drawn: &[(u32, Drawn)],
    slots: &[SlotDevice],
    grid: &InnerGrid,
) -> ContainerLayout {
    let cols = grid.cols.max(1);
    let pitch_x = grid.cw + grid.gutter;
    let pitch_y = grid.unit + grid.gutter;
    let cells_of = |b: Bounds<Pixels>| {
        let round = |v: f32, pitch: f32| {
            if pitch > 0. {
                (v / pitch).round().clamp(0., f32::from(u16::MAX)) as u16
            } else {
                0
            }
        };
        let col = round(f32::from(b.origin.x), pitch_x).min(cols - 1);
        let w = round(f32::from(b.size.width) + grid.gutter, pitch_x).clamp(1, cols - col);
        let row = round(f32::from(b.origin.y), pitch_y);
        let h = round(f32::from(b.size.height) + grid.gutter, pitch_y).max(1);
        WidgetCell {
            col,
            row,
            cols: w,
            rows: h,
        }
    };
    let how = |id: u32| {
        drawn
            .iter()
            .find(|(d, _)| *d == id)
            .map_or(Drawn::Absent, |(_, how)| *how)
    };
    let mut kept: Vec<WidgetSetting> = Vec::with_capacity(layout.widgets.len());
    let mut cut: SmallVec<[usize; 8]> = SmallVec::new();
    for w in &layout.widgets {
        let how = how(w.id);
        if how == Drawn::Absent {
            continue;
        }
        let mut w = w.clone();
        if let Some(device) = w.content.device_mut()
            && let DeviceSelector::Slot(n) = *device
        {
            let Some(dev) = slots.get(usize::from(n)) else {
                continue;
            };
            *device = DeviceSelector::Key(Arc::clone(&dev.key));
            w.device_name = Some(Arc::clone(&dev.name));
        }
        match how {
            Drawn::At(b) => w.cell = Some(cells_of(b)),
            _ => {
                w.cell = None;
                cut.push(kept.len());
            }
        }
        kept.push(w);
    }
    // Overlaps move down, in reading order.
    let mut order: SmallVec<[usize; 16]> = (0..kept.len()).filter(|i| !cut.contains(i)).collect();
    order.sort_by_key(|&i| kept[i].cell.map(|c| (c.row, c.col)));
    let mut placed: SmallVec<[WidgetCell; 16]> = SmallVec::new();
    for &i in &order {
        let Some(mut c) = kept[i].cell else {
            continue;
        };
        while let Some(b) = placed.iter().filter(|p| p.overlaps(&c)).map(bottom).max() {
            c.row = b;
        }
        kept[i].cell = Some(c);
        placed.push(c);
    }
    let lowest = placed.iter().map(bottom).max().unwrap_or(0);
    for &i in &cut {
        let span = grid.widget_min_span(&kept[i]);
        let c = first_free(&placed, span, cols, lowest);
        kept[i].cell = Some(c);
        placed.push(c);
    }
    ContainerLayout {
        container: layout.container.clone(),
        version: layout.version,
        preset: None,
        widgets: kept,
        next_id: layout.next_id,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tl_model::layout::{
        GPU_BOX_CONTAINER, MetricKey, PresetSetting, SourceSelector, WidgetContent, WidgetOptions,
    };

    use super::*;
    use crate::pages::performance::widget::compose::row_min;

    /// The GPU box's columns (`grid_cols(GPU_BOX_CONTAINER)`).
    const COLS: u16 = 6;

    fn cell(col: u16, row: u16, cols: u16, rows: u16) -> WidgetCell {
        WidgetCell {
            col,
            row,
            cols,
            rows,
        }
    }

    fn widget(id: u32, forms: FormSet, cell: Option<WidgetCell>) -> WidgetSetting {
        WidgetSetting {
            id,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Key(Arc::from("gpu:a")),
                metric: MetricKey::new("engine:gpu"),
            }),
            forms,
            cell,
            options: WidgetOptions::default(),
            device_name: None,
        }
    }

    fn grid_layout(cells: &[(u32, FormSet, WidgetCell)]) -> ContainerLayout {
        ContainerLayout {
            container: GPU_BOX_CONTAINER.to_owned(),
            version: tl_model::layout::LAYOUT_VERSION_GPU_ONLY,
            preset: None,
            widgets: cells
                .iter()
                .map(|&(id, forms, c)| widget(id, forms, Some(c)))
                .collect(),
            next_id: cells.iter().map(|c| c.0 + 1).max().unwrap_or(1),
        }
    }

    fn cells_of(layout: &ContainerLayout) -> Vec<(u32, WidgetCell)> {
        layout
            .widgets
            .iter()
            .map(|w| (w.id, w.cell.expect("cell")))
            .collect()
    }

    fn rect(b: Bounds<Pixels>) -> (f32, f32, f32, f32) {
        (
            f32::from(b.origin.x),
            f32::from(b.origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        )
    }

    fn no_overlap(cells: &[WidgetCell]) -> bool {
        cells.iter().enumerate().all(|(i, a)| {
            cells[i + 1..].iter().all(|b| !a.overlaps(b)) && right(a) <= COLS && a.cols >= 1
        })
    }

    const CHART: FormSet = FormSet::CHART.union(FormSet::VALUE).union(FormSet::LABEL);
    const ROW: FormSet = FormSet::BAR.union(FormSet::VALUE).union(FormSet::LABEL);

    #[test]
    fn inner_grid_cells_at_scale_1() {
        // 6 columns over 304 px: cw = (304 − 20) / 6 = 47.33; rows of 24 with 4 px gutters.
        let g = InnerGrid::new(304., COLS, 1.);
        assert!((g.cw - 47.333).abs() < 0.01);
        assert_eq!((g.unit, g.row, g.gutter), (24., 24., 4.));
        assert_eq!(rect(g.bounds(cell(0, 0, 3, 4))), (0., 0., 150., 108.));
        assert_eq!(rect(g.bounds(cell(3, 0, 3, 4))), (154., 0., 150., 108.));
        assert_eq!(rect(g.bounds(cell(0, 4, 6, 1))), (0., 112., 304., 24.));
    }

    #[test]
    fn inner_grid_fill_stretches_rows() {
        // Two rows of charts and a row: R = 5. The body is 300 high: (300 − 16) / 5 = 56.8.
        let cells = [
            (1, cell(0, 0, 3, 4)),
            (2, cell(3, 0, 3, 4)),
            (3, cell(0, 4, 6, 1)),
        ];
        let p = place(&cells, COLS, size(px(304.), px(300.)), 1., None);
        assert!(!p.scrolls);
        assert!((p.grid.row - 56.8).abs() < 0.01);
        let b = p.bounds(3).expect("row");
        assert_eq!(f32::from(b.bottom()), 300.);
        assert_eq!(p.content_h, px(300.));
        // Edit Widgets holds the rows at the unit: the same cells, no fill.
        let held = place(&cells, COLS, size(px(304.), px(300.)), 1., Some(ROW_UNIT));
        assert_eq!(held.grid.row, 24.);
        assert_eq!(rect(held.bounds(3).expect("row")), (0., 112., 304., 24.));
    }

    #[test]
    fn inner_grid_taller_layout_scrolls_and_keeps_spans() {
        // Shrink: a 220 px body (200 × 220 box minus title) under a 9-row layout. The cells
        // keep their spans, rows stay at the unit and the columns leave 16 px for the bar.
        let cells = [
            (1, cell(0, 0, 3, 4)),
            (2, cell(3, 0, 3, 4)),
            (3, cell(0, 4, 6, 4)),
            (4, cell(0, 8, 6, 1)),
        ];
        let p = place(&cells, COLS, size(px(176.), px(180.)), 1., None);
        assert!(p.scrolls);
        assert_eq!(p.grid.row, 24.);
        // 9 rows: 9 × 24 + 8 × 4 = 248.
        assert_eq!(p.content_h, px(248.));
        let last = p.bounds(4).expect("row");
        assert_eq!(f32::from(last.right()), 160.);
        assert_eq!(f32::from(last.size.height), 24.);
    }

    #[test]
    fn inner_grid_scales_unit_gutter_and_minimum() {
        let cells = [(1, cell(0, 0, 3, 4)), (2, cell(0, 4, 6, 1))];
        for scale in [1.0_f32, 1.5, 2.0] {
            let w = 400. * scale;
            let g = InnerGrid::new(w, COLS, scale);
            assert_eq!(g.unit, ROW_UNIT * scale, "{scale}");
            assert_eq!(g.gutter, GUTTER * scale, "{scale}");
            // The body's logical size is the same at every scale, so spans and minimum spans
            // are too.
            let min = g.forms_min_span(ROW);
            assert_eq!(min, InnerGrid::new(400., COLS, 1.).forms_min_span(ROW));
            // Rows at the unit: the 5 rows are 5·24 + 4·4 = 136 logical px.
            let p = place(&cells, COLS, size(px(w), px(136. * scale)), scale, None);
            assert!(!p.scrolls, "{scale}");
            assert!((p.grid.row - 24. * scale).abs() < 0.01, "{scale}");
            let b = p.bounds(2).expect("row");
            assert!(
                (f32::from(b.bottom()) - 136. * scale).abs() <= 1.,
                "{scale}: {b:?}"
            );
            assert!((f32::from(b.size.width) - w).abs() <= 1., "{scale}");
            // One logical px less scrolls.
            let short = place(&cells, COLS, size(px(w), px(134. * scale)), scale, None);
            assert!(short.scrolls, "{scale}");
        }
    }

    #[test]
    fn inner_grid_min_span() {
        // cw = 47.33 at 304 px: a Row with a name (120 px) needs 3 columns, a value alone
        // (48 px) 2, and both one row of 24.
        let g = InnerGrid::new(304., COLS, 1.);
        assert_eq!(g.forms_min_span(ROW), (3, 1));
        assert_eq!(g.forms_min_span(FormSet::VALUE), (2, 1));
        assert_eq!(g.min_span(row_min(ROW)), (3, 1));
        // A narrow body asks for more columns, never more than all.
        let narrow = InnerGrid::new(176., COLS, 1.);
        assert_eq!(narrow.forms_min_span(ROW), (5, 1));
        let tiny = InnerGrid::new(40., COLS, 1.);
        assert_eq!(tiny.forms_min_span(ROW).0, COLS);
        // A 50 px minimum height takes 3 rows of 24.
        assert_eq!(g.min_span(size(10., 50.)), (1, 3));
    }

    #[test]
    fn inner_grid_insert_first_free_then_below() {
        let g = InnerGrid::new(304., COLS, 1.);
        let mut l = grid_layout(&[(1, CHART, cell(0, 0, 3, 4))]);
        let id = insert(&mut l, widget(0, CHART, None), &g);
        assert_eq!(id, 2);
        assert_eq!(l.next_id, 3);
        assert_eq!(l.widgets[1].cell, Some(cell(3, 0, 3, 4)));
        // A row widget takes the full width under the charts.
        let id = insert(&mut l, widget(0, ROW, None), &g);
        assert_eq!(id, 3);
        assert_eq!(l.widgets[2].cell, Some(cell(0, 4, 6, 1)));
        // A gap at the top left is found first.
        let mut gap = grid_layout(&[(5, CHART, cell(3, 0, 3, 4))]);
        insert(&mut gap, widget(0, CHART, None), &g);
        assert_eq!(gap.widgets[1].cell, Some(cell(0, 0, 3, 4)));
        assert_eq!(gap.widgets[1].id, 6, "next id after the highest");
        // A narrow box raises the chart span to its minimum width.
        let narrow = InnerGrid::new(176., COLS, 1.);
        let mut n = grid_layout(&[]);
        insert(&mut n, widget(0, CHART, None), &narrow);
        assert_eq!(n.widgets[0].cell, Some(cell(0, 0, 5, 4)));
    }

    #[test]
    fn inner_grid_move_pushes_down_and_closes_up() {
        let g = InnerGrid::new(304., COLS, 1.);
        let mut l = grid_layout(&[
            (1, CHART, cell(0, 0, 3, 4)),
            (2, CHART, cell(3, 0, 3, 4)),
            (3, ROW, cell(0, 4, 6, 1)),
        ]);
        // The row moves to the top: both charts move down under it.
        assert!(set_cell(&mut l, 3, cell(0, 0, 6, 1), &g));
        assert_eq!(
            cells_of(&l),
            vec![
                (1, cell(0, 1, 3, 4)),
                (2, cell(3, 1, 3, 4)),
                (3, cell(0, 0, 6, 1))
            ]
        );
        // The same cells again change nothing.
        assert!(!set_cell(&mut l, 3, cell(0, 0, 6, 1), &g));
        // The row moves into the middle of the charts: they move down under it.
        assert!(set_cell(&mut l, 3, cell(0, 2, 6, 1), &g));
        assert_eq!(
            cells_of(&l),
            vec![
                (1, cell(0, 3, 3, 4)),
                (2, cell(3, 3, 3, 4)),
                (3, cell(0, 2, 6, 1))
            ]
        );
        // The row moves down past the charts: they close up into the space it left, and the
        // row stays where it was put, as on the Overview grid.
        assert!(set_cell(&mut l, 3, cell(0, 9, 6, 1), &g));
        assert_eq!(
            cells_of(&l),
            vec![
                (1, cell(0, 0, 3, 4)),
                (2, cell(3, 0, 3, 4)),
                (3, cell(0, 9, 6, 1))
            ]
        );
    }

    #[test]
    fn inner_grid_resize_clamps_and_pushes_down() {
        let g = InnerGrid::new(304., COLS, 1.);
        let mut l = grid_layout(&[(1, CHART, cell(0, 0, 3, 4)), (2, ROW, cell(0, 4, 6, 1))]);
        // Wider than the grid: clamped at the last column.
        assert!(set_cell(&mut l, 1, cell(0, 0, 9, 6), &g));
        assert_eq!(cells_of(&l)[0], (1, cell(0, 0, 6, 6)));
        assert_eq!(cells_of(&l)[1], (2, cell(0, 6, 6, 1)), "moved down");
        // Below the minimum span: raised to it (a Row with a name needs 3 columns here).
        assert!(set_cell(&mut l, 2, cell(0, 6, 1, 1), &g));
        assert_eq!(cells_of(&l)[1], (2, cell(0, 6, 3, 1)));
        // Keyboard steps stay inside the grid.
        assert_eq!(moved_by(&l, 2, 9, 0), Some(cell(3, 6, 3, 1)));
        assert_eq!(moved_by(&l, 2, -9, -9), Some(cell(0, 0, 3, 1)));
        assert_eq!(grown_by(&l, 2, 9, 1), Some(cell(0, 6, 6, 2)));
        assert_eq!(grown_by(&l, 2, -9, -9), Some(cell(0, 6, 1, 1)));
        assert_eq!(moved_by(&l, 99, 1, 0), None);
    }

    #[test]
    fn inner_grid_pin_rounds_drawn_rectangles() {
        let g = InnerGrid::new(304., COLS, 1.);
        let slot = |id, metric: &str, forms| WidgetSetting {
            id,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Slot(0),
                metric: MetricKey::new(metric),
            }),
            forms,
            cell: None,
            options: WidgetOptions::default(),
            device_name: None,
        };
        let mut second = slot(5, "vram", ROW);
        *second.content.device_mut().expect("a metric") = DeviceSelector::Slot(1);
        let flow = ContainerLayout {
            container: GPU_BOX_CONTAINER.to_owned(),
            version: tl_model::layout::LAYOUT_VERSION_GPU_ONLY,
            preset: Some(PresetSetting {
                id: "engine_charts".to_owned(),
                ..PresetSetting::default()
            }),
            widgets: vec![
                slot(1, "engine:gpu", CHART),
                slot(2, "engine:copy", CHART),
                slot(3, "engine:enc", CHART),
                slot(4, "vram", ROW),
                second,
                slot(6, "engine:ofa", CHART),
            ],
            next_id: 7,
        };
        let at = |x: f32, y: f32, w: f32, h: f32| {
            Drawn::At(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
        };
        let drawn = [
            // Two cells side by side, a bit off the grid.
            (1, at(0., 0., 148., 110.)),
            (2, at(152., 2., 152., 110.)),
            // Cut by "+n more".
            (3, Drawn::Cut),
            // The VRAM row: 80 / 28 rounds to row 3, which the charts hold, so it moves down.
            (4, at(0., 80., 304., 20.)),
            (5, at(0., 140., 304., 20.)),
            // Not reported.
            (6, Drawn::Absent),
        ];
        let slots = [SlotDevice {
            key: Arc::from("gpu:a"),
            name: Arc::from("Card A"),
        }];
        let pinned = pin(&flow, &drawn, &slots, &g);
        assert_eq!(pinned.preset, None);
        assert_eq!(pinned.next_id, 7);
        // Widget 5 is in slot 1, which has no device; widget 6 is absent.
        let ids: Vec<u32> = pinned.widgets.iter().map(|w| w.id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        assert_eq!(
            cells_of(&pinned),
            vec![
                (1, cell(0, 0, 3, 4)),
                (2, cell(3, 0, 3, 4)),
                // The cut chart under the lowest widget at its minimum span.
                (3, cell(0, 5, 3, 1)),
                (4, cell(0, 4, 6, 1)),
            ]
        );
        for w in &pinned.widgets {
            assert_eq!(
                w.content.device(),
                Some(&DeviceSelector::Key(Arc::from("gpu:a")))
            );
            assert_eq!(w.device_name.as_deref(), Some("Card A"));
        }
        let cells: Vec<WidgetCell> = pinned.widgets.iter().filter_map(|w| w.cell).collect();
        assert!(no_overlap(&cells));
    }

    #[derive(Clone, Debug)]
    enum Action {
        Insert(bool),
        Move(usize, u16, u16),
        Resize(usize, u16, u16),
    }

    fn action() -> impl Strategy<Value = Action> {
        prop_oneof![
            any::<bool>().prop_map(Action::Insert),
            (0usize..12, 0u16..8, 0u16..20).prop_map(|(i, c, r)| Action::Move(i, c, r)),
            (0usize..12, 1u16..8, 1u16..8).prop_map(|(i, w, h)| Action::Resize(i, w, h)),
        ]
    }

    proptest! {
        /// No two widgets overlap after any sequence of inserts, moves and resizes, at any
        /// body width and scale, and every widget stays inside the columns at its minimum.
        #[test]
        fn inner_grid_never_overlaps(
            actions in proptest::collection::vec(action(), 1..40),
            width in 120f32..900.,
            scale in prop_oneof![Just(1.0f32), Just(1.5), Just(2.0)],
        ) {
            let g = InnerGrid::new(width * scale, COLS, scale);
            let mut l = grid_layout(&[]);
            for a in actions {
                match a {
                    Action::Insert(chart) => {
                        let forms = if chart { CHART } else { ROW };
                        insert(&mut l, widget(0, forms, None), &g);
                    }
                    _ if l.widgets.is_empty() => {}
                    Action::Move(i, col, row) => {
                        let w = &l.widgets[i % l.widgets.len()];
                        let c = w.cell.expect("cell");
                        let id = w.id;
                        set_cell(&mut l, id, WidgetCell { col, row, ..c }, &g);
                    }
                    Action::Resize(i, cols, rows) => {
                        let w = &l.widgets[i % l.widgets.len()];
                        let c = w.cell.expect("cell");
                        let id = w.id;
                        set_cell(&mut l, id, WidgetCell { cols, rows, ..c }, &g);
                    }
                }
                let cells: Vec<WidgetCell> = l.widgets.iter().filter_map(|w| w.cell).collect();
                prop_assert_eq!(cells.len(), l.widgets.len());
                prop_assert!(no_overlap(&cells), "{:?}", cells);
            }
            // Drawn: no two rectangles overlap either.
            let p = place(&cells_of(&l), COLS, size(px(width * scale), px(200. * scale)), scale, None);
            for (i, a) in p.widgets.iter().enumerate() {
                for b in &p.widgets[i + 1..] {
                    prop_assert!(!a.bounds.intersects(&b.bounds), "{:?} {:?}", a, b);
                }
            }
        }
    }

    const PAGE: &str = "page.aggregate:disk";

    #[test]
    fn page_grid_has_its_unit_gutter_and_columns() {
        for scale in [1.0, 1.5, 2.0] {
            let g = InnerGrid::of(PAGE, 1000. * scale, scale);
            assert_eq!(g.cols, 12);
            assert_eq!(g.unit, PAGE_ROW_UNIT * scale);
            assert_eq!(g.gutter, PAGE_GUTTER * scale);
            assert!((g.cw - (1000. - 11. * PAGE_GUTTER) * scale / 12.).abs() < 1e-3);
        }
        // The Overview boxes keep theirs.
        let b = InnerGrid::of(GPU_BOX_CONTAINER, 304., 1.);
        assert_eq!((b.cols, b.unit, b.gutter), (COLS, ROW_UNIT, GUTTER));
    }

    #[test]
    fn page_rows_never_fill() {
        // A short page leaves space under it; a box fills.
        let cells = [(1, cell(0, 0, 12, 7)), (2, cell(0, 7, 12, 7))];
        let p = place_in(PAGE, &cells, size(px(1000.), px(800.)), 1., None);
        assert_eq!(p.grid.row, PAGE_ROW_UNIT);
        // 14 rows of 12 px and 13 gutters of 12 px.
        assert_eq!(p.content_h, px(14. * 12. + 13. * 12.));
        assert!(!p.scrolls);
        // 7 rows hold the 150 px page chart and a 12 px gap above the next.
        let b = p.bounds(1).expect("placed");
        assert_eq!(b.size.height, px(7. * 12. + 6. * 12.));
        let boxed = place(&cells, 12, size(px(1000.), px(800.)), 1., None);
        assert!(boxed.grid.row > ROW_UNIT);
        // Taller than the body: it scrolls.
        let tall = place_in(PAGE, &cells, size(px(1000.), px(200.)), 1., None);
        assert!(tall.scrolls);
    }

    #[test]
    fn page_default_spans() {
        let chart = widget(0, FormSet::CHART | FormSet::VALUE, None);
        assert_eq!(widget_default_span_in(PAGE, &chart), PAGE_CHART_SPAN);
        let value = widget(0, FormSet::VALUE, None);
        assert_eq!(widget_default_span_in(PAGE, &value), (12, PAGE_METRIC_ROWS));
        let fixed = WidgetSetting {
            content: WidgetContent::Fixed(tl_model::layout::FixedWidget::Details),
            ..widget(0, FormSet::empty(), None)
        };
        assert_eq!(widget_default_span_in(PAGE, &fixed), (12, PAGE_GROUP_ROWS));
        // A box keeps its rule.
        assert_eq!(widget_default_span_in(GPU_BOX_CONTAINER, &chart), (3, 4));
    }
}
