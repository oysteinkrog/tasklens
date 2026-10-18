//! Resizing the Overview boxes by dragging the gaps (UI-PRF-018 "Resizing in normal use", D-023).
//!
//! The gap between two boxes of a row is a vertical splitter; the gap between two rows is a
//! horizontal one. [`splitters`] finds them in a [`Placement`]. A [`Drag`] holds the two
//! neighbours' weights, placed sizes and minimums when the button went down, and turns the
//! pointer position into new weights for those two only: their sum stays, every other weight
//! stays. There are no splitters in the stacked layout.

use gpui_kit::{Bounds, Pixels, point, size};
use smallvec::SmallVec;
use tl_model::settings::OVERVIEW_WEIGHT_RANGE;

use super::layout::{BoxKind, CPU_SPLIT_RANGE, OverviewLayout, Placement};

/// Which two neighbours a splitter sits between.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// The boxes left and right of a vertical gap, in one row.
    Boxes(BoxKind, BoxKind),
    /// The rows (layout indices) above and below a horizontal gap.
    Rows(usize, usize),
}

impl Target {
    /// True for a vertical splitter, which drags along x.
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Boxes(..))
    }

    /// The debug selector of the gap: `perf-overview-splitter-col-<left kind>` or
    /// `perf-overview-splitter-row-<upper row index>`.
    pub fn selector(self) -> String {
        match self {
            Self::Boxes(a, _) => format!("perf-overview-splitter-col-{}", a.key()),
            Self::Rows(a, _) => format!("perf-overview-splitter-row-{a}"),
        }
    }
}

/// One gap and what it resizes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Splitter {
    pub target: Target,
    pub bounds: Bounds<Pixels>,
}

/// The narrowest column of core rows (UI-PRF-018 "CPU box splitter"), logical px.
pub const CORE_COLUMN_MIN_W: f32 = 188.;

/// `share` of a CPU box body `body_w` wide, clamped to 20%..=80% and so that the core rows keep
/// one [`CORE_COLUMN_MIN_W`] column beside the `gap` (all px, `scale` the UI scale). The 20%
/// floor wins when the body is too narrow for both.
pub fn clamp_cpu_split(share: f32, body_w: f32, gap: f32, scale: f32) -> f32 {
    let (lo, hi) = CPU_SPLIT_RANGE;
    let hi = if body_w > 0. {
        hi.min((body_w - gap - CORE_COLUMN_MIN_W * scale) / body_w)
    } else {
        hi
    };
    share.clamp(lo, hi.max(lo))
}

/// A drag of the CPU box splitter: the share and pointer x when the button went down.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CpuSplitDrag {
    start_x: f32,
    start_share: f32,
    /// True once a move changed the share.
    pub changed: bool,
}

impl CpuSplitDrag {
    pub fn start(pointer_x: f32, share: f32) -> Self {
        Self {
            start_x: pointer_x,
            start_share: share,
            changed: false,
        }
    }

    /// The share with the pointer at `pointer_x`, in a body `body_w` wide.
    pub fn share_at(&self, pointer_x: f32, body_w: f32, gap: f32, scale: f32) -> f32 {
        if body_w <= 0. {
            return self.start_share;
        }
        let share = self.start_share + (pointer_x - self.start_x) / body_w;
        clamp_cpu_split(share, body_w, gap, scale)
    }
}

/// A share rounded to 3 decimals, as the profile stores it.
pub fn round_split(share: f32) -> f32 {
    (share * 1000.).round() / 1000.
}

/// The row of `kind` in `layout`.
fn row_of(layout: &OverviewLayout, kind: BoxKind) -> Option<usize> {
    layout
        .rows
        .iter()
        .position(|r| r.boxes.iter().any(|b| b.kind == kind))
}

/// The splitters of `placement`: one between each two neighbours in a line of boxes, and one
/// between each two rows. A row that wrapped onto extra lines has no splitter between its lines.
pub fn splitters(layout: &OverviewLayout, placement: &Placement) -> SmallVec<[Splitter; 8]> {
    let mut out = SmallVec::new();
    if placement.stacked {
        return out;
    }
    let left = placement
        .boxes
        .iter()
        .map(|b| b.bounds.left())
        .fold(None, |m: Option<Pixels>, x| Some(m.map_or(x, |m| m.min(x))));
    let right = placement
        .boxes
        .iter()
        .map(|b| b.bounds.right())
        .fold(None, |m: Option<Pixels>, x| Some(m.map_or(x, |m| m.max(x))));
    let (Some(left), Some(right)) = (left, right) else {
        return out;
    };
    for pair in placement.boxes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (Some(ra), Some(rb)) = (row_of(layout, a.kind), row_of(layout, b.kind)) else {
            continue;
        };
        if a.bounds.top() == b.bounds.top() {
            if ra == rb && b.bounds.left() > a.bounds.right() {
                out.push(Splitter {
                    target: Target::Boxes(a.kind, b.kind),
                    bounds: Bounds::new(
                        point(a.bounds.right(), a.bounds.top()),
                        size(b.bounds.left() - a.bounds.right(), a.bounds.size.height),
                    ),
                });
            }
        } else if ra != rb && b.bounds.top() > a.bounds.bottom() {
            out.push(Splitter {
                target: Target::Rows(ra, rb),
                bounds: Bounds::new(
                    point(left, a.bounds.bottom()),
                    size(right - left, b.bounds.top() - a.bounds.bottom()),
                ),
            });
        }
    }
    out
}

/// The placed height of row `row`: the height of its first placed box.
fn row_height(layout: &OverviewLayout, placement: &Placement, row: usize) -> Option<f32> {
    placement
        .boxes
        .iter()
        .find(|b| row_of(layout, b.kind) == Some(row))
        .map(|b| f32::from(b.bounds.size.height))
}

/// A row's minimum height: the largest minimum of its placed boxes.
fn row_min_h(layout: &OverviewLayout, placement: &Placement, row: usize, scale: f32) -> f32 {
    layout.rows[row]
        .boxes
        .iter()
        .filter(|b| placement.boxes.iter().any(|p| p.kind == b.kind))
        .map(|b| b.kind.min_h() * scale)
        .fold(0., f32::max)
}

/// A drag in progress: the two neighbours as they were when the button went down.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Drag {
    pub target: Target,
    /// The pointer position along the drag axis at the start, px.
    start: f32,
    /// The two neighbours' weights at the start.
    weights: (f32, f32),
    /// Their placed sizes along the axis, px.
    placed: (f32, f32),
    /// Their minimum sizes along the axis, px.
    min: (f32, f32),
}

impl Drag {
    /// Starts a drag of `target` with the pointer at `pointer` (x for a vertical splitter, y
    /// for a horizontal one). `None` when a neighbour is not placed. `scale` is the UI scale.
    pub fn start(
        layout: &OverviewLayout,
        placement: &Placement,
        target: Target,
        pointer: f32,
        scale: f32,
    ) -> Option<Self> {
        let (weights, placed, min) = match target {
            Target::Boxes(a, b) => {
                let row = &layout.rows[row_of(layout, a)?];
                let weight = |k: BoxKind| row.boxes.iter().find(|s| s.kind == k).map(|s| s.width);
                let width = |k: BoxKind| {
                    placement
                        .boxes
                        .iter()
                        .find(|p| p.kind == k)
                        .map(|p| f32::from(p.bounds.size.width))
                };
                (
                    (weight(a)?, weight(b)?),
                    (width(a)?, width(b)?),
                    (a.min_w() * scale, b.min_w() * scale),
                )
            }
            Target::Rows(a, b) => (
                (layout.rows.get(a)?.height, layout.rows.get(b)?.height),
                (
                    row_height(layout, placement, a)?,
                    row_height(layout, placement, b)?,
                ),
                (
                    row_min_h(layout, placement, a, scale),
                    row_min_h(layout, placement, b, scale),
                ),
            ),
        };
        Some(Self {
            target,
            start: pointer,
            weights,
            placed,
            min,
        })
    }

    /// The new weights of the two neighbours with the pointer at `pointer`. Their sum is the
    /// start sum. Each side stays at or above its minimum; when both minimums do not fit, the
    /// start weights come back.
    pub fn weights_at(&self, pointer: f32) -> (f32, f32) {
        let total = self.placed.0 + self.placed.1;
        let lo = self.min.0;
        let hi = total - self.min.1;
        if total <= 0. || lo > hi {
            return self.weights;
        }
        let first = (self.placed.0 + pointer - self.start).clamp(lo, hi);
        self.split(first / total)
    }

    /// Equal sizes for the two neighbours: the start sum split in half.
    pub fn equal_weights(&self) -> (f32, f32) {
        self.split(0.5)
    }

    /// The start sum split at `share` for the first neighbour, kept inside the weight range.
    fn split(&self, share: f32) -> (f32, f32) {
        let sum = self.weights.0 + self.weights.1;
        let (min, max) = OVERVIEW_WEIGHT_RANGE;
        let (min, max) = (min as f32, max as f32);
        if sum < 2. * min {
            return self.weights;
        }
        let first = (sum * share).clamp(min, (sum - min).min(max));
        (first, sum - first)
    }
}

/// Writes `weights` to the two neighbours of `target`, widths for boxes and heights for rows.
pub fn set_weights(layout: &mut OverviewLayout, target: Target, weights: (f32, f32)) {
    match target {
        Target::Boxes(a, b) => {
            for row in &mut layout.rows {
                for s in &mut row.boxes {
                    if s.kind == a {
                        s.width = weights.0;
                    } else if s.kind == b {
                        s.width = weights.1;
                    }
                }
            }
        }
        Target::Rows(a, b) => {
            if let Some(r) = layout.rows.get_mut(a) {
                r.height = weights.0;
            }
            if let Some(r) = layout.rows.get_mut(b) {
                r.height = weights.1;
            }
        }
    }
}

/// Every weight rounded to 3 decimals, as the profile stores it. A weight never rounds to 0.
pub fn round_weights(layout: &mut OverviewLayout) {
    let round = |w: f32| ((w * 1000.).round() / 1000.).max(0.001);
    for row in &mut layout.rows {
        row.height = round(row.height);
        for s in &mut row.boxes {
            s.width = round(s.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::performance::overview::layout::place;
    use gpui_kit::px;

    fn placed() -> (OverviewLayout, Placement) {
        let layout = OverviewLayout::btop_default();
        let placement = place(&layout, |_| true, size(px(1200.), px(900.)), 1.);
        (layout, placement)
    }

    #[test]
    fn splitters_of_the_default() {
        let (layout, placement) = placed();
        let s = splitters(&layout, &placement);
        let targets: Vec<Target> = s.iter().map(|s| s.target).collect();
        assert_eq!(
            targets,
            vec![
                Target::Boxes(BoxKind::Cpu, BoxKind::Memory),
                Target::Rows(0, 1),
                Target::Boxes(BoxKind::Gpu, BoxKind::Disk),
                Target::Boxes(BoxKind::Disk, BoxKind::Net),
            ]
        );
        // The CPU/Memory gap is 8 px wide between 789 and 797.
        assert_eq!(f32::from(s[0].bounds.size.width), 8.);
        // The row gap spans the boxes' width and is 8 px high.
        assert_eq!(f32::from(s[1].bounds.size.height), 8.);
        assert_eq!(f32::from(s[1].bounds.size.width), 1168.);
    }

    #[test]
    fn no_splitters_when_stacked() {
        let layout = OverviewLayout::btop_default();
        let placement = place(&layout, |_| true, size(px(600.), px(900.)), 1.);
        assert!(splitters(&layout, &placement).is_empty());
    }

    #[test]
    fn drag_keeps_the_sum_and_clamps_at_minimums() {
        let (layout, placement) = placed();
        let target = Target::Boxes(BoxKind::Cpu, BoxKind::Memory);
        let drag = Drag::start(&layout, &placement, target, 793., 1.).expect("placed");
        let (a, b) = drag.weights_at(793. - 100.);
        assert!((a + b - 3.).abs() < 1e-4);
        assert!(a < 2. && b > 1.);
        // Far left: CPU stops at its 280 px minimum (773 px placed, 387 px for Memory).
        let (a, b) = drag.weights_at(-5000.);
        assert!((a / (a + b) - 280. / 1160.).abs() < 1e-4);
        // Far right: Memory stops at 200 px.
        let (a, b) = drag.weights_at(5000.);
        assert!((b / (a + b) - 200. / 1160.).abs() < 1e-4);
    }

    #[test]
    fn equal_weights_halve_the_sum() {
        let (layout, placement) = placed();
        let drag = Drag::start(&layout, &placement, Target::Rows(0, 1), 540., 1.).expect("placed");
        assert_eq!(drag.equal_weights(), (2.5, 2.5));
    }

    #[test]
    fn cpu_split_clamps_to_range_and_one_core_column() {
        // A 1000 px body: 20%..=80%, and 1000 - 8 - 188 = 804 leaves 80.4%.
        assert_eq!(clamp_cpu_split(0.1, 1000., 8., 1.), 0.2);
        assert_eq!(clamp_cpu_split(0.9, 1000., 8., 1.), 0.8);
        assert_eq!(clamp_cpu_split(0.5, 1000., 8., 1.), 0.5);
        // A 600 px body keeps 188 px for the rows: (600 - 8 - 188) / 600.
        let hi = clamp_cpu_split(0.8, 600., 8., 1.);
        assert!((hi - 404. / 600.).abs() < 1e-6, "{hi}");
        // A body too narrow for both takes the floor.
        assert_eq!(clamp_cpu_split(0.5, 300., 8., 1.), 0.2);
        // Scale widens the column.
        assert!(clamp_cpu_split(0.8, 600., 8., 2.) < hi);
    }

    #[test]
    fn cpu_split_drag_follows_the_pointer() {
        let drag = CpuSplitDrag::start(400., 0.4);
        assert!((drag.share_at(500., 1000., 8., 1.) - 0.5).abs() < 1e-6);
        assert_eq!(drag.share_at(-5000., 1000., 8., 1.), 0.2);
        assert_eq!(drag.share_at(5000., 1000., 8., 1.), 0.8);
        assert_eq!(round_split(0.123_456), 0.123);
    }

    #[test]
    fn round_weights_to_three_decimals() {
        let mut layout = OverviewLayout::btop_default();
        layout.rows[0].height = 2.123_456;
        layout.rows[0].boxes[0].width = 0.000_1;
        round_weights(&mut layout);
        assert_eq!(layout.rows[0].height, 2.123);
        assert_eq!(layout.rows[0].boxes[0].width, 0.001);
    }
}
