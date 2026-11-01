//! How a group widget lays its items out inside its own rectangle (ADR-019 "Group widgets",
//! "Fill"). Pure functions, like `grid.rs`, so the rules are tested without a window:
//!
//! - **Rows**: each item is a Row composition. The column count is the fewest columns, at most
//!   4, in which every item fits at 18 px with each column at least the Row minimum width of the
//!   forms. Items flow top to bottom, then into the next column (C0 to C{k − 1} in the first).
//!   Every row then takes the same share of the height, at least 18 px (D-040). When the items do
//!   not fit at 18 px in the columns that fit the width, rows stay 18 px and the group scrolls
//!   vertically, keeping the scrollbar's width free. Columns are 8 px apart, as the core rows.
//! - **Cells**: each item is a Cell composition (a Tile when the forms have no chart), laid out
//!   by `parts::arrange_grid`: `auto` picks the column count that gives the largest equal cells,
//!   every cell at least the composition's minimum; `vertical` is one column and `horizontal`
//!   one row. Cells that do not fit at their minimum scroll in the arrangement's direction.
//!   Cells are 8 px apart, as the `core_charts` cells.
//!
//! The "+n more" item is one more item: the last row, or a cell of its own. Every size here is
//! window px; the functions take the UI scale.

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use tl_model::layout::{FormSet, GroupItems};

use super::compose::{Composition, ROW_H, group_item_composition, group_item_min};
use crate::pages::performance::overview::layout::Arrange;
use crate::pages::performance::overview::parts::arrange_grid;

/// The most columns Rows flow into before they scroll (D-029).
pub const MAX_ROW_COLS: usize = 4;
/// The gap between columns of Rows and between Cells, logical px.
pub const ITEM_GAP: f32 = 8.;
/// The width a scrolling group keeps free for its scrollbar, logical px.
pub const SCROLLBAR_W: f32 = 16.;

/// Where the items of a group go, relative to the top left of its scrolling content.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct GroupFill {
    /// One rectangle per item, in item order, every edge a whole px.
    pub items: Vec<Bounds<Pixels>>,
    /// The size of the content: larger than the area in the direction it scrolls.
    pub content: Size<Pixels>,
    /// The content is taller than the area and scrolls vertically.
    pub scrolls: bool,
    /// The content is wider than the area and scrolls sideways (Cells side by side).
    pub scrolls_x: bool,
    pub cols: usize,
    pub rows: usize,
    /// The composition every item draws.
    pub composition: Composition,
}

fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    }
}

/// A rectangle with every edge rounded to a whole px.
fn snapped(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    let (left, top) = (x.round(), y.round());
    let (right, bottom) = ((x + w).round(), (y + h).round());
    Bounds::new(
        point(px(left), px(top)),
        size(px((right - left).max(0.)), px((bottom - top).max(0.))),
    )
}

/// `n` items drawn as `items` with `forms` in `area` at UI scale `scale`; `arrange` is the
/// Cells arrangement (`Auto` for Rows).
pub fn group_fill(
    items: GroupItems,
    arrange: Arrange,
    n: usize,
    area: Size<Pixels>,
    forms: FormSet,
    scale: f32,
) -> GroupFill {
    match items {
        GroupItems::Rows => rows_fill(n, area, forms, scale),
        GroupItems::Cells => cells_fill(n, area, forms, arrange, scale),
    }
}

/// Rows (D-029, D-040): see the module notes.
pub fn rows_fill(n: usize, area: Size<Pixels>, forms: FormSet, scale: f32) -> GroupFill {
    let min_w = group_item_min(GroupItems::Rows, forms).width;
    rows_fill_min(n, area, min_w, scale)
}

/// [`rows_fill`] with columns at least `min_w` logical px wide. A cores group's core rows take
/// the core cell's minimum (`parts::CELL_MIN_W`), so they get the columns of the CPU box's core
/// rows in every layout (tasklens-6swdr.12).
pub fn rows_fill_min(n: usize, area: Size<Pixels>, min_w: f32, scale: f32) -> GroupFill {
    let s = sane_scale(scale);
    let composition = Composition::Row;
    if n == 0 {
        return GroupFill {
            composition,
            ..GroupFill::default()
        };
    }
    let (w, h) = (
        f32::from(area.width).max(0.),
        f32::from(area.height).max(0.),
    );
    let gap = ITEM_GAP * s;
    let row_h = ROW_H * s;
    let min_w = min_w * s;
    let max_cols =
        |w: f32| (((w + gap) / (min_w + gap) + 1e-3).floor() as usize).clamp(1, MAX_ROW_COLS);
    let fits = |cols: usize| n.div_ceil(cols) as f32 * row_h <= h + 1e-3;
    let (w, cols, scrolls) = match (1..=max_cols(w)).find(|&c| fits(c)) {
        Some(cols) => (w, cols, false),
        None => {
            let w = (w - SCROLLBAR_W * s).max(0.);
            (w, max_cols(w), true)
        }
    };
    let rows = n.div_ceil(cols);
    // The fewest columns for those rows, so no column is left empty.
    let cols = n.div_ceil(rows);
    let pitch = if scrolls {
        row_h
    } else {
        row_h.max(h / rows as f32)
    };
    let cell_w = ((w - gap * (cols - 1) as f32) / cols as f32).max(0.);
    let items = (0..n)
        .map(|i| {
            let (col, row) = (i / rows, i % rows);
            snapped(
                col as f32 * (cell_w + gap),
                row as f32 * pitch,
                cell_w,
                pitch,
            )
        })
        .collect();
    GroupFill {
        items,
        content: size(px(w), px(rows as f32 * pitch)),
        scrolls,
        scrolls_x: false,
        cols,
        rows,
        composition,
    }
}

/// Cells (UI-PRF-019 "Arrange", D-026, D-027): see the module notes.
pub fn cells_fill(
    n: usize,
    area: Size<Pixels>,
    forms: FormSet,
    arrange: Arrange,
    scale: f32,
) -> GroupFill {
    let s = sane_scale(scale);
    let composition = group_item_composition(GroupItems::Cells, forms);
    if n == 0 {
        return GroupFill {
            composition,
            ..GroupFill::default()
        };
    }
    let min = group_item_min(GroupItems::Cells, forms);
    let min = size(px(min.width * s), px(min.height * s));
    let gap = ITEM_GAP * s;
    let fit = arrange_grid(arrange, n, area, min, px(gap), px(SCROLLBAR_W * s));
    let (cw, ch) = (f32::from(fit.cell.width), f32::from(fit.cell.height));
    let cols = fit.cols.max(1);
    let items = (0..n)
        .map(|i| {
            let (col, row) = (i % cols, i / cols);
            snapped(col as f32 * (cw + gap), row as f32 * (ch + gap), cw, ch)
        })
        .collect();
    GroupFill {
        items,
        content: size(fit.content_w(px(gap)), fit.content_h(px(gap))),
        scrolls: fit.scrolls,
        scrolls_x: fit.scrolls_x,
        cols: fit.cols,
        rows: fit.rows,
        composition,
    }
}

/// `fill`, made for an area `from`, moved into the area `to` with the same columns, rows and
/// item kind (Edit Widgets keeps the applied view's fit inside its edit margins, so nothing
/// jumps). Along a direction that scrolls the items keep their size; along the others they
/// shrink or grow in proportion.
pub fn fitted_into(fill: &GroupFill, from: Size<Pixels>, to: Size<Pixels>) -> GroupFill {
    let ratio = |to: Pixels, from: Pixels| {
        if from > px(0.) {
            f32::from(to) / f32::from(from)
        } else {
            1.
        }
    };
    let sx = if fill.scrolls_x {
        1.
    } else {
        ratio(to.width, from.width)
    };
    let sy = if fill.scrolls {
        1.
    } else {
        ratio(to.height, from.height)
    };
    let items = fill
        .items
        .iter()
        .map(|b| {
            snapped(
                f32::from(b.left()) * sx,
                f32::from(b.top()) * sy,
                f32::from(b.size.width) * sx,
                f32::from(b.size.height) * sy,
            )
        })
        .collect();
    GroupFill {
        items,
        content: size(fill.content.width * sx, fill.content.height * sy),
        ..fill.clone()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::pages::performance::widget::compose::{cell_min, row_min, tile_min};

    fn area(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    fn rows_forms() -> FormSet {
        FormSet::CHART | FormSet::BAR | FormSet::VALUE | FormSet::LABEL
    }

    fn overlaps(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
        a.left() < b.right() && b.left() < a.right() && a.top() < b.bottom() && b.top() < a.bottom()
    }

    #[test]
    fn thirty_two_cores_fill_the_height_as_d040() {
        // A CPU box body of about 750 × 480 px: 32 rows of 18 px do not fit one column
        // (576 px), so they take 2 columns of 16 rows, each 30 px high.
        let f = rows_fill(32, area(750., 480.), rows_forms(), 1.);
        assert!(!f.scrolls);
        assert_eq!((f.cols, f.rows), (2, 16));
        assert_eq!(f.items[0].size.height, px(30.));
        assert_eq!(f.items[15].bottom(), px(480.));
        // C0 to C15 in the first column, then C16 in the second.
        assert_eq!(f.items[16].top(), px(0.));
        assert!(f.items[16].left() > f.items[15].left());
        assert_eq!(f.content.height, px(480.));
    }

    #[test]
    fn rows_scroll_at_18px_when_four_columns_do_not_fit() {
        // 128 items in 4 columns are 32 rows: 576 px, more than 300.
        let f = rows_fill(128, area(800., 300.), rows_forms(), 1.);
        assert!(f.scrolls);
        assert_eq!(f.cols, 4);
        assert_eq!(f.items[0].size.height, px(18.));
        assert_eq!(f.content.height, px(32. * 18.));
        // The scrollbar's width stays free.
        assert!(f.items.iter().all(|b| b.right() <= px(800. - SCROLLBAR_W)));
    }

    #[test]
    fn rows_take_fewer_columns_when_the_width_is_short() {
        // Two Row minimums (120 px with a name) and a gap need 248 px.
        let f = rows_fill(32, area(240., 300.), rows_forms(), 1.);
        assert_eq!(f.cols, 1);
        assert!(f.scrolls);
        let f = rows_fill(32, area(248., 300.), rows_forms(), 1.);
        assert_eq!(f.cols, 2);
        assert_eq!(row_min(rows_forms()).width, 120.);
    }

    #[test]
    fn one_item_fills_the_area() {
        let f = rows_fill(1, area(300., 200.), rows_forms(), 1.);
        assert_eq!(
            f.items,
            vec![Bounds::new(point(px(0.), px(0.)), area(300., 200.))]
        );
        let f = cells_fill(1, area(300., 200.), FormSet::CHART, Arrange::Auto, 1.);
        assert_eq!(f.items[0].size, area(300., 200.));
        assert_eq!(f.composition, Composition::Cell);
    }

    #[test]
    fn cells_take_the_largest_equal_cells() {
        let forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        let f = cells_fill(8, area(400., 400.), forms, Arrange::Auto, 1.);
        assert!(!f.scrolls);
        let first = f.items[0].size;
        assert!(f.items.iter().all(|b| b.size == first));
        let min = cell_min(forms);
        assert!(f32::from(first.width) >= min.width && f32::from(first.height) >= min.height);
        // Without a chart the items are Tiles.
        let f = cells_fill(3, area(400., 400.), FormSet::VALUE, Arrange::Auto, 1.);
        assert_eq!(f.composition, Composition::Tile);
        assert!(f32::from(f.items[0].size.height) >= tile_min(FormSet::VALUE).height);
    }

    #[test]
    fn cells_vertical_and_side_by_side() {
        let forms = FormSet::CHART;
        let f = cells_fill(3, area(300., 300.), forms, Arrange::Vertical, 1.);
        assert_eq!((f.cols, f.rows), (1, 3));
        let f = cells_fill(3, area(600., 100.), forms, Arrange::Horizontal, 1.);
        assert_eq!((f.cols, f.rows), (3, 1));
        // Side by side cells that do not fit scroll sideways.
        let f = cells_fill(10, area(600., 100.), forms, Arrange::Horizontal, 1.);
        assert!(f.scrolls_x);
        assert!(f.content.width > px(600.));
    }

    #[test]
    fn cells_scroll_when_they_do_not_fit() {
        let forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        let f = cells_fill(128, area(400., 300.), forms, Arrange::Auto, 1.);
        assert!(f.scrolls);
        assert!(f.content.height > px(300.));
        assert!(f.items.iter().all(|b| b.right() <= px(400. - SCROLLBAR_W)));
    }

    #[test]
    fn fitted_into_keeps_columns_and_rows() {
        let f = rows_fill(32, area(750., 480.), rows_forms(), 1.);
        let g = fitted_into(&f, area(750., 480.), area(736., 476.));
        assert_eq!(
            (g.cols, g.rows, g.composition),
            (f.cols, f.rows, f.composition)
        );
        assert!(
            g.items
                .iter()
                .all(|b| b.right() <= px(736.) && b.bottom() <= px(476.))
        );
        // Scrolling rows keep their 18 px.
        let f = rows_fill(128, area(800., 300.), rows_forms(), 1.);
        let g = fitted_into(&f, area(800., 300.), area(788., 296.));
        assert_eq!(g.items[0].size.height, px(18.));
        assert_eq!(g.content.height, f.content.height);
    }

    #[test]
    fn no_items_place_nothing() {
        assert!(
            rows_fill(0, area(300., 300.), rows_forms(), 1.)
                .items
                .is_empty()
        );
        assert!(
            cells_fill(0, area(300., 300.), FormSet::CHART, Arrange::Auto, 1.)
                .items
                .is_empty()
        );
    }

    proptest! {
        /// At 1, 8, 32 and 128 items in rows and cells, at 1.0, 1.5 and 2.0 scale: every item is
        /// placed, none overlaps another, rows are at least 18 px × scale, and an area that does
        /// not scroll holds every item.
        #[test]
        fn items_never_overlap(
            n in prop::sample::select(vec![1usize, 8, 32, 128]),
            scale in prop::sample::select(vec![1.0f32, 1.5, 2.0]),
            w in 120f32..1600.,
            h in 18f32..1200.,
            cells in any::<bool>(),
            arrange in prop::sample::select(vec![Arrange::Auto, Arrange::Vertical, Arrange::Horizontal]),
            chart in any::<bool>(),
        ) {
            let forms = if chart { rows_forms() } else { FormSet::VALUE | FormSet::LABEL };
            let items = if cells { GroupItems::Cells } else { GroupItems::Rows };
            let f = group_fill(items, arrange, n, area(w, h), forms, scale);
            prop_assert_eq!(f.items.len(), n);
            for (i, a) in f.items.iter().enumerate() {
                for b in &f.items[i + 1..] {
                    prop_assert!(!overlaps(a, b), "{:?} and {:?}", a, b);
                }
                if !cells {
                    prop_assert!(f32::from(a.size.height) + 0.5 >= ROW_H * scale);
                }
                if !f.scrolls && !f.scrolls_x {
                    prop_assert!(f32::from(a.bottom()) <= h + 0.5);
                    prop_assert!(f32::from(a.right()) <= w + 0.5);
                }
                if f.scrolls {
                    prop_assert!(f32::from(a.right()) <= w - SCROLLBAR_W * scale + 0.5);
                }
            }
        }
    }
}
