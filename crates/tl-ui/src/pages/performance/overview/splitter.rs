//! Resizing the Overview boxes by dragging the gaps (UI-PRF-056, D-030).
//!
//! The gap between two boxes that share a whole edge is a splitter: vertical between boxes side
//! by side with the same rows, horizontal between boxes one above the other with the same
//! columns. [`splitters`] finds them in a [`Placement`]. A [`Drag`] holds the first box's span,
//! the two boxes' total and their kind minimums when the button went down, and turns the
//! pointer position into a new span in whole cells: the shared edge moves, the outer edges and
//! every other box stay. There are no splitters in the stacked layout.

use gpui_kit::{Bounds, Pixels, point, size};
use smallvec::SmallVec;

use super::layout::{BoxKind, OverviewLayout, Placement};

/// Which two boxes a splitter sits between.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// The boxes left and right of a vertical gap.
    Cols(BoxKind, BoxKind),
    /// The boxes above and below a horizontal gap.
    Rows(BoxKind, BoxKind),
}

impl Target {
    /// True for a vertical splitter, which drags along x.
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Cols(..))
    }

    /// The debug selector of the gap: `perf-overview-splitter-col-<left kind>` or
    /// `perf-overview-splitter-row-<upper kind>`.
    pub fn selector(self) -> String {
        match self {
            Self::Cols(a, _) => format!("perf-overview-splitter-col-{}", a.key()),
            Self::Rows(a, _) => format!("perf-overview-splitter-row-{}", a.key()),
        }
    }

    const fn kinds(self) -> (BoxKind, BoxKind) {
        match self {
            Self::Cols(a, b) | Self::Rows(a, b) => (a, b),
        }
    }
}

/// One gap and what it resizes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Splitter {
    pub target: Target,
    pub bounds: Bounds<Pixels>,
}

/// The splitters of `placement`: one between each two placed boxes that share a whole edge.
pub fn splitters(layout: &OverviewLayout, placement: &Placement) -> SmallVec<[Splitter; 8]> {
    let mut out = SmallVec::new();
    if placement.stacked {
        return out;
    }
    for pa in &placement.boxes {
        for pb in &placement.boxes {
            let (Some(a), Some(b)) = (layout.get(pa.kind), layout.get(pb.kind)) else {
                continue;
            };
            let (ra, rb) = (pa.bounds, pb.bounds);
            if a.right() == b.col && a.row == b.row && a.rows == b.rows {
                out.push(Splitter {
                    target: Target::Cols(a.kind, b.kind),
                    bounds: Bounds::new(
                        point(ra.right(), ra.top()),
                        size(rb.left() - ra.right(), ra.size.height),
                    ),
                });
            } else if a.bottom() == b.row && a.col == b.col && a.cols == b.cols {
                out.push(Splitter {
                    target: Target::Rows(a.kind, b.kind),
                    bounds: Bounds::new(
                        point(ra.left(), ra.bottom()),
                        size(ra.size.width, rb.top() - ra.bottom()),
                    ),
                });
            }
        }
    }
    out
}

/// A drag in progress: the two boxes as they were when the button went down.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Drag {
    pub target: Target,
    /// The pointer position along the drag axis at the start, px.
    start: f32,
    /// The first box's span along the axis at the start, cells.
    first: u16,
    /// Both boxes' spans together, cells.
    total: u16,
    /// The first box's smallest and largest span: each side keeps its kind minimum, or its
    /// start span when that is smaller.
    lo: u16,
    hi: u16,
    /// One cell and its gutter along the axis, px.
    pitch: f32,
}

impl Drag {
    /// Starts a drag of `target` with the pointer at `pointer` (x for a vertical splitter, y
    /// for a horizontal one). `None` when a box is not in the layout.
    pub fn start(
        layout: &OverviewLayout,
        placement: &Placement,
        target: Target,
        pointer: f32,
    ) -> Option<Self> {
        let (ka, kb) = target.kinds();
        let (a, b) = (layout.get(ka)?, layout.get(kb)?);
        let g = placement.grid;
        let (min_a, min_b) = (g.min_span(ka), g.min_span(kb));
        let (first, second, min_a, min_b, pitch) = if target.is_vertical() {
            (a.cols, b.cols, min_a.0, min_b.0, g.cw + g.gutter)
        } else {
            (a.rows, b.rows, min_a.1, min_b.1, g.unit + g.gutter)
        };
        let total = first + second;
        let lo = min_a.min(first);
        let hi = total - min_b.min(second);
        Some(Self {
            target,
            start: pointer,
            first,
            total,
            lo,
            hi: hi.max(lo),
            pitch,
        })
    }

    /// The first box's span with the pointer at `pointer`: the start span plus the whole
    /// cells the pointer moved, kept between both kind minimums.
    pub fn span_at(&self, pointer: f32) -> u16 {
        if self.pitch <= 0. {
            return self.first;
        }
        let cells = ((pointer - self.start) / self.pitch).round();
        let span = (f32::from(self.first) + cells).clamp(f32::from(self.lo), f32::from(self.hi));
        span as u16
    }

    /// The two spans as even as whole cells allow, the first box taking the smaller half.
    pub fn even_span(&self) -> u16 {
        (self.total / 2).clamp(self.lo, self.hi)
    }
}

/// Gives the first box of `target` `span` cells along the axis and the second box the rest of
/// their total, moving only the shared edge. Returns whether the layout changed.
pub fn set_span(layout: &mut OverviewLayout, target: Target, span: u16) -> bool {
    let (ka, kb) = target.kinds();
    let (Some(a), Some(b)) = (layout.get(ka), layout.get(kb)) else {
        return false;
    };
    let vertical = target.is_vertical();
    let (start, first, second) = if vertical {
        (a.col, a.cols, b.cols)
    } else {
        (a.row, a.rows, b.rows)
    };
    let total = first + second;
    let span = span.clamp(1, total.saturating_sub(1).max(1));
    if span == first || span >= total {
        return false;
    }
    if let Some(a) = layout.get_mut(ka) {
        if vertical {
            a.cols = span;
        } else {
            a.rows = span;
        }
    }
    if let Some(b) = layout.get_mut(kb) {
        if vertical {
            b.col = start + span;
            b.cols = total - span;
        } else {
            b.row = start + span;
            b.rows = total - span;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::performance::overview::layout::{ROW_UNIT_DEFAULT, place};
    use gpui_kit::px;

    fn placed(layout: &OverviewLayout) -> Placement {
        place(
            layout,
            |_| true,
            size(px(1200.), px(900.)),
            1.,
            ROW_UNIT_DEFAULT,
        )
    }

    #[test]
    fn splitters_of_the_default() {
        let layout = OverviewLayout::grid_default();
        let s = splitters(&layout, &placed(&layout));
        let targets: Vec<Target> = s.iter().map(|s| s.target).collect();
        use BoxKind::*;
        assert_eq!(
            targets,
            vec![
                Target::Cols(Cpu, Memory),
                Target::Rows(Memory, Net),
                Target::Cols(Gpu, Disk),
                Target::Cols(Disk, Net),
            ]
        );
        // The CPU/Memory gap is the 6 px gutter, as high as the boxes.
        assert_eq!(f32::from(s[0].bounds.size.width), 6.);
        assert_eq!(f32::from(s[0].bounds.size.height), 316.);
        // Memory over Net: as wide as the two boxes, 6 px high.
        assert_eq!(f32::from(s[1].bounds.size.height), 6.);
        assert_eq!(f32::from(s[1].bounds.size.width), 385.);
    }

    #[test]
    fn no_splitters_when_stacked_or_absent() {
        let layout = OverviewLayout::grid_default();
        let stacked = place(
            &layout,
            |_| true,
            size(px(600.), px(900.)),
            1.,
            ROW_UNIT_DEFAULT,
        );
        assert!(splitters(&layout, &stacked).is_empty());
        // Without a GPU box there is no GPU/Disk gap.
        let p = place(
            &layout,
            |k| k != BoxKind::Gpu,
            size(px(1200.), px(900.)),
            1.,
            ROW_UNIT_DEFAULT,
        );
        assert!(
            splitters(&layout, &p)
                .iter()
                .all(|s| s.target != Target::Cols(BoxKind::Gpu, BoxKind::Disk))
        );
    }

    #[test]
    fn drag_moves_whole_cells_and_clamps_at_kind_minimums() {
        let layout = OverviewLayout::grid_default();
        let p = placed(&layout);
        let target = Target::Cols(BoxKind::Cpu, BoxKind::Memory);
        let drag = Drag::start(&layout, &p, target, 796.).expect("placed");
        // A column and its gutter are 97.83 px: under half a cell moves nothing.
        assert_eq!(drag.span_at(796. + 40.), 8);
        assert_eq!(drag.span_at(796. - 100.), 7);
        // CPU keeps 4 columns at 1200 px, Memory 3.
        assert_eq!(drag.span_at(-5000.), 4);
        assert_eq!(drag.span_at(5000.), 9);
        assert_eq!(drag.even_span(), 6);

        let mut l = layout.clone();
        assert!(set_span(&mut l, target, 6));
        assert!(!set_span(&mut l, target, 6), "same span");
        let (cpu, mem) = (
            l.get(BoxKind::Cpu).expect("cpu"),
            l.get(BoxKind::Memory).expect("memory"),
        );
        assert_eq!((cpu.col, cpu.cols, mem.col, mem.cols), (0, 6, 6, 6));
        assert_eq!(l.validate(), Ok(()));

        // A row gap: Memory keeps 6 rows, Net 6.
        let rows = Target::Rows(BoxKind::Memory, BoxKind::Net);
        let drag = Drag::start(&layout, &p, rows, 330.).expect("placed");
        assert_eq!(drag.span_at(330. - 1000.), 6);
        assert_eq!(drag.span_at(330. + 1000.), 7);
        let mut l = layout.clone();
        assert!(set_span(&mut l, rows, 6));
        let (mem, net) = (
            l.get(BoxKind::Memory).expect("memory"),
            l.get(BoxKind::Net).expect("net"),
        );
        assert_eq!((mem.rows, net.row, net.rows), (6, 6, 7));
        assert_eq!(l.validate(), Ok(()));
    }
}
