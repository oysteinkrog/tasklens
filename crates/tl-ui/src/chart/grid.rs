//! Chart background, grid, baseline, dotted gaps and outline (ADR-009 rule 8, UI-PRF-001,
//! PERF-015, PERF-022). Every mark is a quad. Paint order is the element's (E19-007):
//! background, grid, fills, strokes, gaps, baseline, outline.
//!
//! A frame paints 3 to 13 grid quads plus one dotted quad per 3 physical px of gap, and allocates
//! nothing.

use std::ops::Range;

use gpui_kit::{
    BorderStyle, Bounds, Edges, Hsla, PaintQuad, Pixels, Window, fill, point, px, quad, size,
    transparent_black,
};

use super::GridStyle;
use super::geometry::TimeMap;
use crate::theme::TlTheme;

/// No grid on plots lower than this (the tray graph). Sidebar cards are 56 high and keep it.
const MIN_GRID_HEIGHT: f32 = 40.0;

/// Alpha of the baseline, dotted gaps and outline over `outline_color`.
const OUTLINE_ALPHA: f32 = 0.40;

/// Spacing of the dotted gap quads, physical px.
const GAP_DOT_STEP: f32 = 3.0;

/// Where quads go. `Window` paints them; tests can record them instead.
pub trait QuadSink {
    fn quad(&mut self, quad: PaintQuad);
}

impl QuadSink for Window {
    fn quad(&mut self, quad: PaintQuad) {
        self.paint_quad(quad);
    }
}

impl QuadSink for Vec<PaintQuad> {
    fn quad(&mut self, quad: PaintQuad) {
        self.push(quad);
    }
}

/// Inputs shared by the grid, baseline, gap and outline painters.
pub struct GridPaint<'a> {
    /// Plot area, logical px.
    pub plot: Bounds<Pixels>,
    /// Whole chart box, logical px.
    pub box_bounds: Bounds<Pixels>,
    pub map: &'a TimeMap,
    /// Paint-time shift from `TimeMap::dx`, chart-local physical px.
    pub dx: Pixels,
    pub scale_factor: f32,
    pub theme: &'a TlTheme,
    pub style: GridStyle,
    pub outline_color: Hsla,
    pub radius: Pixels,
}

impl GridPaint<'_> {
    /// 1 physical px in logical px.
    fn hairline(&self) -> Pixels {
        px(1.0 / self.scale_factor)
    }

    /// Chart-local physical x to window logical x.
    fn to_logical_x(&self, x_phys: f32) -> Pixels {
        self.plot.left() + px(x_phys / self.scale_factor)
    }

    fn line_color(&self) -> Hsla {
        self.outline_color.alpha(OUTLINE_ALPHA)
    }
}

/// Plot background, `theme.graph.chart_bg` for every chart (ADR-009 theme model).
pub fn paint_background(
    window: &mut impl QuadSink,
    box_bounds: Bounds<Pixels>,
    bg: Hsla,
    radius: Pixels,
) {
    window.quad(fill(box_bounds, bg).corner_radii(radius));
}

/// The `k` values, first and last inclusive, with `k × (range_ns / 10)` inside
/// `[right_edge − range, right_edge]`. Placing lines from absolute time makes them scroll with
/// the data.
pub fn grid_k_range(map: &TimeMap) -> (u64, u64) {
    let step = (map.range_ns / 10).max(1);
    let left = map.right_edge_ns.saturating_sub(map.range_ns);
    (left.div_ceil(step), map.right_edge_ns / step)
}

/// Vertical hairlines every tenth of the range and horizontal hairlines at 25 %, 50 % and 75 % of
/// the plot height, colour `theme.graph.grid`. Nothing when `style.lines` is off or the plot is
/// under 40 logical px high.
pub fn paint_grid(window: &mut impl QuadSink, p: &GridPaint) {
    let plot = p.plot;
    if !p.style.lines || plot.size.height < px(MIN_GRID_HEIGHT) {
        return;
    }
    let hairline = p.hairline();
    let color = p.theme.graph.grid;

    let step = (p.map.range_ns / 10).max(1);
    let dx = f32::from(p.dx);
    let (first, last) = grid_k_range(p.map);
    for k in first..=last {
        let x = p.to_logical_x(p.map.x_local(k * step) + dx);
        if x < plot.left() || x >= plot.right() {
            continue;
        }
        window.quad(fill(
            Bounds::new(point(x, plot.top()), size(hairline, plot.size.height)),
            color,
        ));
    }

    for quarter in [0.25, 0.5, 0.75] {
        let y = plot.top() + plot.size.height * quarter;
        window.quad(fill(
            Bounds::new(point(plot.left(), y), size(plot.size.width, hairline)),
            color,
        ));
    }
}

/// 1 physical px line along the plot bottom, `outline_color` at 0.40. Nothing when
/// `style.baseline` is off.
pub fn paint_baseline(window: &mut impl QuadSink, p: &GridPaint) {
    if !p.style.baseline {
        return;
    }
    let hairline = p.hairline();
    let plot = p.plot;
    window.quad(fill(
        Bounds::new(
            point(plot.left(), plot.bottom() - hairline),
            size(plot.size.width, hairline),
        ),
        p.line_color(),
    ));
}

/// Dotted baseline across each gap (PERF-022): 1 × 1 physical px quads at `a + 3 i` while below
/// `b`, along the plot bottom, `outline_color` at 0.40. Spans are chart-local physical px, shifted
/// by `dx` and clipped to the plot.
pub fn paint_gaps(window: &mut impl QuadSink, p: &GridPaint, gaps: &[Range<f32>]) {
    let hairline = p.hairline();
    let color = p.line_color();
    let y = p.plot.bottom() - hairline;
    let dx = f32::from(p.dx);
    let plot_width = f32::from(p.plot.size.width) * p.scale_factor;
    for gap in gaps {
        let a = gap.start + dx;
        let end = (gap.end + dx).min(plot_width);
        // Skip the dots left of the plot but keep them on the `a + 3 i` grid.
        let mut i = if a < 0.0 {
            (-a / GAP_DOT_STEP).ceil()
        } else {
            0.0
        };
        loop {
            let x = a + GAP_DOT_STEP * i;
            if x >= end {
                break;
            }
            window.quad(fill(
                Bounds::new(point(p.to_logical_x(x), y), size(hairline, hairline)),
                color,
            ));
            i += 1.0;
        }
    }
}

/// Border-only quad around the box, 1 physical px, `outline_color` at 0.40, corner radius
/// `radius` (PERF-015). Nothing when `style.outline` is off.
pub fn paint_outline(window: &mut impl QuadSink, p: &GridPaint) {
    if !p.style.outline {
        return;
    }
    window.quad(quad(
        p.box_bounds,
        p.radius,
        transparent_black(),
        Edges::all(p.hairline()),
        p.line_color(),
        BorderStyle::Solid,
    ));
}
