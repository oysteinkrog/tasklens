//! The Overview page's layout as data (UI-PRF-017, D-022): a grid of rows and columns, each box
//! a kind on one row spanning one or more columns. A later bead edits and saves it; this module
//! only validates and places it.
//!
//! [`place`] turns a layout, the kinds present now and the page area into box bounds. It runs on
//! resize and on presence changes, never per tick, and does not allocate for up to 8 boxes.

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::{SmallVec, smallvec};

use super::super::page::PAGE_PAD;

/// The gap between boxes and rows (`sp4`), logical px.
const GAP: f32 = 8.;

/// Below this page width the boxes stack in one column, logical px.
pub const STACK_W: f32 = 720.;

/// The Overview boxes: the five of the default layout and the opt-in Top processes box
/// (crw.45).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BoxKind {
    Cpu,
    Memory,
    Gpu,
    Disk,
    Net,
    TopProcs,
}

/// One box: its kind, its row, its first column and how many columns it spans.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxSpec {
    pub kind: BoxKind,
    pub row: u8,
    pub col: u8,
    pub col_span: u8,
}

/// One row: its share of the page height and its minimum height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RowSpec {
    pub weight: u16,
    /// Logical px.
    pub min_h: f32,
}

/// The whole layout.
#[derive(Clone, PartialEq, Debug)]
pub struct OverviewLayout {
    pub columns: u8,
    pub rows: SmallVec<[RowSpec; 4]>,
    pub boxes: SmallVec<[BoxSpec; 8]>,
}

/// Why a layout is not valid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutError {
    /// No column, no row or no box.
    Empty,
    /// The box's row does not exist, its span is 0, or it reaches past the last column.
    OutOfGrid(BoxKind),
    /// Two boxes share a cell.
    Overlap(BoxKind, BoxKind),
    /// A kind appears twice.
    Duplicate(BoxKind),
}

impl OverviewLayout {
    /// The btop-style default: CPU (2 columns) and Memory on a 3:2 top row, then GPU, Disk and
    /// Net.
    pub fn btop_default() -> Self {
        let b = |kind, row, col, col_span| BoxSpec {
            kind,
            row,
            col,
            col_span,
        };
        Self {
            columns: 3,
            rows: smallvec![
                RowSpec {
                    weight: 3,
                    min_h: 280.,
                },
                RowSpec {
                    weight: 2,
                    min_h: 220.,
                },
            ],
            boxes: smallvec![
                b(BoxKind::Cpu, 0, 0, 2),
                b(BoxKind::Memory, 0, 2, 1),
                b(BoxKind::Gpu, 1, 0, 1),
                b(BoxKind::Disk, 1, 1, 1),
                b(BoxKind::Net, 1, 2, 1),
            ],
        }
    }

    /// [`Self::btop_default`] with a third row, weight 2 and at least 260 px, that holds the Top
    /// processes box across all columns (crw.45).
    pub fn with_top_procs() -> Self {
        let mut layout = Self::btop_default();
        layout.rows.push(RowSpec {
            weight: 2,
            min_h: 260.,
        });
        layout.boxes.push(BoxSpec {
            kind: BoxKind::TopProcs,
            row: 2,
            col: 0,
            col_span: 3,
        });
        layout
    }

    pub fn has(&self, kind: BoxKind) -> bool {
        self.boxes.iter().any(|b| b.kind == kind)
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        if self.columns == 0 || self.rows.is_empty() || self.boxes.is_empty() {
            return Err(LayoutError::Empty);
        }
        for (i, b) in self.boxes.iter().enumerate() {
            let end = u16::from(b.col) + u16::from(b.col_span);
            if usize::from(b.row) >= self.rows.len()
                || b.col_span == 0
                || end > u16::from(self.columns)
            {
                return Err(LayoutError::OutOfGrid(b.kind));
            }
            for other in &self.boxes[..i] {
                if other.kind == b.kind {
                    return Err(LayoutError::Duplicate(b.kind));
                }
                let other_end = u16::from(other.col) + u16::from(other.col_span);
                if other.row == b.row && u16::from(b.col) < other_end && u16::from(other.col) < end
                {
                    return Err(LayoutError::Overlap(other.kind, b.kind));
                }
            }
        }
        Ok(())
    }
}

/// One placed box.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlacedBox {
    pub kind: BoxKind,
    pub bounds: Bounds<Pixels>,
}

/// Where the present boxes go, in (row, col) order. `content_h` includes the padding; the page
/// scrolls when `scrolls`. `stacked` is the one-column layout below [`STACK_W`].
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Placement {
    pub boxes: SmallVec<[PlacedBox; 8]>,
    pub content_h: Pixels,
    pub scrolls: bool,
    pub stacked: bool,
}

/// Places the boxes of `layout` whose kind is `present` in `area`. Logical sizes are × `scale`.
///
/// - Padding `PAGE_PAD` on every side; gap 8 between boxes and rows.
/// - A row's present boxes share the row width by `col_span`, in `col` order; with every box
///   present a box is `span·unit + (span − 1)·gap` wide. A row with no present box is dropped.
/// - Row heights split the height by `weight`. If a row comes out below its `min_h`, every row
///   takes its `min_h` and the page scrolls.
/// - Below `STACK_W` the boxes stack full width, each with its row's `min_h`.
pub fn place(
    layout: &OverviewLayout,
    present: impl Fn(BoxKind) -> bool,
    area: Size<Pixels>,
    scale: f32,
) -> Placement {
    let pad = PAGE_PAD * scale;
    let gap = GAP * scale;
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let inner_w = (w - 2. * pad).max(0.);

    // The present boxes in (row, col) order.
    let mut shown: SmallVec<[BoxSpec; 8]> = layout
        .boxes
        .iter()
        .copied()
        .filter(|b| present(b.kind) && usize::from(b.row) < layout.rows.len())
        .collect();
    shown.sort_by_key(|b| (b.row, b.col));
    let mut placement = Placement::default();
    if shown.is_empty() {
        placement.content_h = px(2. * pad);
        return placement;
    }

    if w < STACK_W * scale {
        placement.stacked = true;
        let mut y = pad;
        for (i, b) in shown.iter().enumerate() {
            if i > 0 {
                y += gap;
            }
            let box_h = layout.rows[usize::from(b.row)].min_h * scale;
            placement.boxes.push(PlacedBox {
                kind: b.kind,
                bounds: Bounds::new(point(px(pad), px(y)), size(px(inner_w), px(box_h))),
            });
            y += box_h;
        }
        let content_h = y + pad;
        placement.content_h = px(content_h);
        placement.scrolls = content_h > h;
        return placement;
    }

    // The rows that keep a box, as (row index, height).
    let mut rows: SmallVec<[(u8, f32); 4]> = SmallVec::new();
    for b in &shown {
        if rows.last().is_none_or(|(r, _)| *r != b.row) {
            rows.push((b.row, 0.));
        }
    }
    let gaps_h = (rows.len() as f32 - 1.) * gap;
    let avail_h = h - 2. * pad - gaps_h;
    let weights: f32 = rows
        .iter()
        .map(|(r, _)| f32::from(layout.rows[usize::from(*r)].weight))
        .sum();
    let mut short = weights <= 0.;
    for (r, row_h) in rows.iter_mut() {
        let spec = layout.rows[usize::from(*r)];
        *row_h = if weights > 0. {
            avail_h * f32::from(spec.weight) / weights
        } else {
            0.
        };
        short |= *row_h < spec.min_h * scale;
    }
    if short {
        for (r, row_h) in rows.iter_mut() {
            *row_h = layout.rows[usize::from(*r)].min_h * scale;
        }
        placement.scrolls = true;
    }

    let mut y = pad;
    for (i, (r, row_h)) in rows.iter().enumerate() {
        if i > 0 {
            y += gap;
        }
        let in_row = shown.iter().filter(|b| b.row == *r);
        let spans: u16 = in_row.clone().map(|b| u16::from(b.col_span)).sum();
        let spans_f = f32::from(spans.max(1));
        let unit = (inner_w - (spans_f - 1.) * gap) / spans_f;
        let mut x = pad;
        for b in in_row {
            let span = f32::from(b.col_span);
            let box_w = span * unit + (span - 1.) * gap;
            placement.boxes.push(PlacedBox {
                kind: b.kind,
                bounds: Bounds::new(point(px(x), px(y)), size(px(box_w), px(*row_h))),
            });
            x += box_w + gap;
        }
        y += row_h;
    }
    placement.content_h = px(y + pad);
    placement
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    /// (x, y, w, h) of `kind`.
    fn rect(p: &Placement, kind: BoxKind) -> (f32, f32, f32, f32) {
        let b = p
            .boxes
            .iter()
            .find(|b| b.kind == kind)
            .expect("placed")
            .bounds;
        (
            f32::from(b.origin.x),
            f32::from(b.origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        )
    }

    #[test]
    fn overview_default_layout_validates() {
        assert_eq!(OverviewLayout::btop_default().validate(), Ok(()));
        assert!(!OverviewLayout::btop_default().has(BoxKind::TopProcs));
    }

    #[test]
    fn overview_top_procs_layout_adds_a_full_row() {
        let layout = OverviewLayout::with_top_procs();
        assert_eq!(layout.validate(), Ok(()));
        assert!(layout.has(BoxKind::TopProcs));
        let p = place(&layout, |_| true, size(px(1200.), px(1000.)), 1.);
        let procs = p.boxes.last().expect("placed");
        assert_eq!(procs.kind, BoxKind::TopProcs);
        let cpu = p.boxes.first().expect("placed");
        let net = p
            .boxes
            .iter()
            .find(|b| b.kind == BoxKind::Net)
            .expect("placed");
        assert_eq!(procs.bounds.left(), cpu.bounds.left());
        assert_eq!(procs.bounds.right(), net.bounds.right());
        assert!(procs.bounds.size.height >= px(260.));
    }

    #[test]
    fn overview_place_default_1200x900() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(1200., 900.),
            1.,
        );
        assert!(!p.stacked && !p.scrolls);
        assert_eq!(rect(&p, BoxKind::Cpu), (16., 16., 776., 516.));
        assert_eq!(rect(&p, BoxKind::Memory), (800., 16., 384., 516.));
        for (kind, x) in [
            (BoxKind::Gpu, 16.),
            (BoxKind::Disk, 408.),
            (BoxKind::Net, 800.),
        ] {
            assert_eq!(rect(&p, kind), (x, 540., 384., 344.), "{kind:?}");
        }
        assert_eq!(p.content_h, px(900.));
        let order: Vec<BoxKind> = p.boxes.iter().map(|b| b.kind).collect();
        assert_eq!(
            order,
            [
                BoxKind::Cpu,
                BoxKind::Memory,
                BoxKind::Gpu,
                BoxKind::Disk,
                BoxKind::Net
            ]
        );
    }

    #[test]
    fn overview_place_without_gpu_widens_row() {
        let p = place(
            &OverviewLayout::btop_default(),
            |k| k != BoxKind::Gpu,
            area(1200., 900.),
            1.,
        );
        assert!(p.boxes.iter().all(|b| b.kind != BoxKind::Gpu));
        assert_eq!(rect(&p, BoxKind::Disk), (16., 540., 580., 344.));
        assert_eq!(rect(&p, BoxKind::Net), (604., 540., 580., 344.));
        // A row with no present box is dropped: the top row takes the whole height.
        let top_only = place(
            &OverviewLayout::btop_default(),
            |k| matches!(k, BoxKind::Cpu | BoxKind::Memory),
            area(1200., 900.),
            1.,
        );
        assert_eq!(rect(&top_only, BoxKind::Cpu), (16., 16., 776., 868.));
    }

    #[test]
    fn overview_place_narrow_stacks() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(700., 900.),
            1.,
        );
        assert!(p.stacked);
        assert!(p.boxes.iter().all(|b| b.bounds.size.width == px(668.)));
        // 280 + 280 + 3 × 220, 4 gaps of 8 and 2 × 16 padding.
        assert_eq!(p.content_h, px(1284.));
        assert!(p.scrolls);
        assert_eq!(rect(&p, BoxKind::Memory), (16., 304., 668., 280.));
        assert_eq!(rect(&p, BoxKind::Net), (16., 1048., 668., 220.));
    }

    #[test]
    fn overview_place_short_uses_min_heights() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(1200., 500.),
            1.,
        );
        assert!(p.scrolls && !p.stacked);
        assert_eq!(rect(&p, BoxKind::Cpu).3, 280.);
        assert_eq!(rect(&p, BoxKind::Disk).3, 220.);
        assert_eq!(p.content_h, px(540.));
    }

    #[test]
    fn overview_place_scales() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(2400., 1800.),
            2.,
        );
        assert_eq!(rect(&p, BoxKind::Cpu), (32., 32., 1552., 1032.));
    }

    #[test]
    fn overview_validate_rejects_overlap_and_out_of_grid() {
        let mut overlap = OverviewLayout::btop_default();
        overlap.boxes[1].col = 1;
        assert_eq!(
            overlap.validate(),
            Err(LayoutError::Overlap(BoxKind::Cpu, BoxKind::Memory))
        );

        let mut out = OverviewLayout::btop_default();
        out.boxes[4].col_span = 2;
        assert_eq!(out.validate(), Err(LayoutError::OutOfGrid(BoxKind::Net)));

        let mut no_row = OverviewLayout::btop_default();
        no_row.boxes[0].row = 5;
        assert_eq!(no_row.validate(), Err(LayoutError::OutOfGrid(BoxKind::Cpu)));

        let mut dup = OverviewLayout::btop_default();
        dup.boxes[4].kind = BoxKind::Gpu;
        assert_eq!(dup.validate(), Err(LayoutError::Duplicate(BoxKind::Gpu)));

        let mut empty = OverviewLayout::btop_default();
        empty.boxes.clear();
        assert_eq!(empty.validate(), Err(LayoutError::Empty));
    }
}
