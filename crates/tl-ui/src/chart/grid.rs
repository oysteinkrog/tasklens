//! Chart background, grid, baseline, dotted gaps and outline (ADR-009 rule 8, UI-PRF-001,
//! PERF-015, PERF-022). Every mark is a quad. Paint order is the element's (E19-007):
//! background, grid, fills, strokes, gaps, baseline, outline.
//!
//! A frame paints 3 to 13 grid quads plus one dotted quad per 3 physical px of gap, and allocates
//! nothing.

use std::ops::Range;

use gpui_kit::{
    BorderStyle, Bounds, Edges, Hsla, PaintQuad, Pixels, Point, Window, fill, linear_color_stop,
    linear_gradient, point, px, quad, size, transparent_black,
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

/// Share of the plot width, from the right edge, that the newest-point glow covers (UI-PRF-058).
pub const GLOW_FRACTION: f32 = 0.10;

/// Alpha of the glow at its right edge.
pub const GLOW_ALPHA: f32 = 0.06;

/// Diameter of the newest-point dot, logical px.
pub const DOT_SIZE: f32 = 4.0;

/// Width of the halo around the newest-point dot, logical px.
pub const HALO_WIDTH: f32 = 2.0;

/// Alpha of the halo.
pub const HALO_ALPHA: f32 = 0.30;

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

/// Vertical hairlines every tenth of the range (or at `style.vertical_at`) and horizontal hairlines at 25 %, 50 % and 75 % of
/// the plot height, colour `theme.graph.grid`. Nothing when `style.lines` is off or the plot is
/// under 40 logical px high.
pub fn paint_grid(window: &mut impl QuadSink, p: &GridPaint) {
    let plot = p.plot;
    if !p.style.lines || plot.size.height < px(MIN_GRID_HEIGHT) {
        return;
    }
    let hairline = p.hairline();
    let color = p.theme.graph.grid;

    match &p.style.vertical_at {
        Some(at) => {
            let width = plot.size.width;
            for frac in at.iter().copied().filter(|f| (0.0..1.0).contains(f)) {
                let x = plot.left() + width * frac;
                window.quad(fill(
                    Bounds::new(point(x, plot.top()), size(hairline, plot.size.height)),
                    color,
                ));
            }
        }
        None => {
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
        }
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

/// The glow behind the newest 10% of the plot: `color` at 6% at the right edge, fading to clear
/// toward the left (UI-PRF-058). Returns whether it was drawn: nothing is drawn under reduced
/// motion. A horizontal gradient stands in for a radial one, which GPUI quads cannot draw.
pub fn paint_newest_glow(
    window: &mut impl QuadSink,
    plot: Bounds<Pixels>,
    color: Hsla,
    reduce_motion: bool,
) -> bool {
    if reduce_motion || plot.size.width <= px(0.) {
        return false;
    }
    let w = plot.size.width * GLOW_FRACTION;
    let bounds = Bounds::new(
        point(plot.right() - w, plot.top()),
        size(w, plot.size.height),
    );
    let gradient = linear_gradient(
        90.0,
        linear_color_stop(color.alpha(0.0), 0.0),
        linear_color_stop(color.alpha(GLOW_ALPHA), 1.0),
    );
    window.quad(fill(bounds, gradient));
    true
}

/// The newest data point: a 4 px dot in the line colour and, unless motion is reduced, a 2 px
/// halo at 30% around it. `center` is in window logical px.
pub fn paint_newest_dot(
    window: &mut impl QuadSink,
    center: Point<Pixels>,
    color: Hsla,
    reduce_motion: bool,
) {
    let disc = |d: f32, c: Hsla| {
        let b = Bounds::new(
            point(center.x - px(d / 2.0), center.y - px(d / 2.0)),
            size(px(d), px(d)),
        );
        fill(b, c).corner_radii(px(d / 2.0))
    };
    if !reduce_motion {
        window.quad(disc(DOT_SIZE + 2.0 * HALO_WIDTH, color.alpha(HALO_ALPHA)));
    }
    window.quad(disc(DOT_SIZE, color));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot() -> Bounds<Pixels> {
        Bounds::new(point(px(10.), px(20.)), size(px(200.), px(80.)))
    }

    #[test]
    fn polish_reduced_motion_drops_glow() {
        let accent = Hsla {
            h: 0.5,
            s: 0.6,
            l: 0.5,
            a: 1.0,
        };
        let mut quads: Vec<PaintQuad> = Vec::new();
        assert!(!paint_newest_glow(&mut quads, plot(), accent, true));
        assert!(quads.is_empty());

        assert!(paint_newest_glow(&mut quads, plot(), accent, false));
        assert_eq!(quads.len(), 1);
        // The newest 10% of the width, at the right edge of the plot.
        assert_eq!(quads[0].bounds.size.width, px(20.));
        assert_eq!(quads[0].bounds.right(), plot().right());
        assert_eq!(quads[0].bounds.size.height, px(80.));
    }

    #[test]
    fn polish_newest_dot_and_halo() {
        let line = Hsla {
            h: 0.1,
            s: 0.8,
            l: 0.5,
            a: 1.0,
        };
        let center = point(px(100.), px(50.));
        let mut quads: Vec<PaintQuad> = Vec::new();
        paint_newest_dot(&mut quads, center, line, false);
        // Halo first (8 px), then the 4 px dot on top.
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[0].bounds.size.width, px(8.));
        assert_eq!(quads[1].bounds.size.width, px(4.));
        assert_eq!(quads[1].bounds.center(), center);

        // Reduced motion drops the halo and keeps the dot.
        let mut quads: Vec<PaintQuad> = Vec::new();
        paint_newest_dot(&mut quads, center, line, true);
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].bounds.size.width, px(4.));
    }
}
