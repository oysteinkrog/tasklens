//! `HistoryMinimap`: the 24 px map of the whole retained span under the strip (UI-PRC-033,
//! PROC-089).
//!
//! It draws one area path of the active metric's system total over the whole span (the page's
//! kept `HistoryView.overview`) and a viewport box over the strip's `view_range`. The path is
//! built in map-local physical px against the span the overview covered, and kept in
//! [`MinimapCache`] until the overview, size, scale, metric or style changes. Each paint stretches
//! the cached path onto the current span, so a new span or strip window builds nothing.
//!
//! Pointer interaction: drag the box to pan the strip, drag an edge to zoom, click outside the box
//! to centre it there, wheel to zoom like the strip. Listeners are registered in `paint` and
//! update the page through `set_view_range`. A hover change notifies only the cached
//! `MinimapView`.

use std::sync::Arc;

use gpui_kit::{
    App, Bounds, ContentMask, Context, CursorStyle, Element, ElementId, Empty, Entity, EntityId,
    FillOptions, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, PathBuilder,
    PathStyle, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Style, WeakEntity, Window,
    fill, point, px, relative, size,
};
use tl_model::history::{HistoryMetric, HistoryPoint};
use tl_model::time::TimeNs;

use super::HistoryPage;
use super::scrubber::{pan, x_to_t, zoom};
use crate::theme::{GraphPalette, UiMetrics};

const NS_PER_SEC: u64 = 1_000_000_000;

/// Height in logical px at 100% scale (UI-PRC-033).
const MINIMAP_H: f32 = 24.0;
/// A press this close (logical px at 100% scale) to a box edge grabs the edge. Also the width
/// of the painted edge grips.
const GRIP: f32 = 4.0;
/// The shortest window an edge drag leaves.
const MIN_VIEW_NS: u64 = 60 * NS_PER_SEC;
/// A pixel scroll delta counts as one notch per this many px.
const PIXELS_PER_NOTCH: f32 = 20.0;

const AREA_ALPHA: f32 = 0.50;
const BOX_ALPHA: f32 = 0.15;

/// The last overview the engine sent, kept across views that carry none.
#[derive(Clone, Debug)]
pub struct KeptOverview {
    pub metric: HistoryMetric,
    /// The span the points cover.
    pub span: (TimeNs, TimeNs),
    pub points: Arc<[HistoryPoint]>,
}

/// The part of the map under a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Box,
    LeftEdge,
    RightEdge,
    Outside,
}

/// The part under `x`, for a box from `l` to `r` on a map `width` wide (map-local logical px)
/// and a grab distance of `grab`. Inside a box narrower than 4 grab widths the box wins, so a
/// small box can still be dragged; its edges are then grabbed from just outside. An edge with
/// less than `grab` of map outside it cannot be reached that way, so it takes the third of the
/// box next to it instead.
pub fn hit(x: f32, l: f32, r: f32, grab: f32, width: f32) -> Part {
    let (dl, dr) = ((x - l).abs(), (x - r).abs());
    let nearer = if dl <= dr {
        Part::LeftEdge
    } else {
        Part::RightEdge
    };
    if x < l || x > r {
        return if dl.min(dr) <= grab {
            nearer
        } else {
            Part::Outside
        };
    }
    if r - l >= 4.0 * grab {
        return if dl.min(dr) <= grab {
            nearer
        } else {
            Part::Box
        };
    }
    let third = (r - l) / 3.0;
    if l < grab && dl < third {
        Part::LeftEdge
    } else if width - r < grab && dr < third {
        Part::RightEdge
    } else {
        Part::Box
    }
}

/// Zooms `view` by `notches` like the strip's [`zoom`] (1.25 per notch, 60 s up to all of
/// `span`), around a `cursor` that may lie outside the view: the cursor keeps its place relative
/// to the view, so the box grows or shrinks about the pointer. The result stays inside `span`.
pub fn zoom_at(
    view: (TimeNs, TimeNs),
    cursor: TimeNs,
    notches: f32,
    span: (TimeNs, TimeNs),
) -> (TimeNs, TimeNs) {
    let old = view.1.saturating_sub(view.0);
    // Zooming about the left end changes only the width.
    let sized = zoom(view, view.0, notches, span);
    let width = sized.1 - sized.0;
    if old == 0 || width == 0 {
        return sized;
    }
    let frac = (i128::from(cursor) - i128::from(view.0)) as f64 / old as f64;
    let start = i128::from(cursor) - (frac * width as f64).round() as i128;
    let delta = (start - i128::from(sized.0)).clamp(i128::from(i64::MIN), i128::from(i64::MAX));
    pan(sized, delta as i64, span)
}

/// The window after an edge drag to `t`: the edge that does not move stays at `fixed`, the
/// moving one goes to `t`, kept inside `span` and at least `min(60 s, span)` from `fixed`.
pub fn edge_drag(fixed: TimeNs, left: bool, t: TimeNs, span: (TimeNs, TimeNs)) -> (TimeNs, TimeNs) {
    let min = MIN_VIEW_NS.min(span.1.saturating_sub(span.0));
    if left {
        (
            t.min(fixed.saturating_sub(min)).max(span.0).min(fixed),
            fixed,
        )
    } else {
        (
            fixed,
            t.max(fixed.saturating_add(min)).min(span.1).max(fixed),
        )
    }
}

/// The pointer gesture in progress on the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MinDrag {
    /// The box follows the pointer from where it was grabbed: `anchor_x` (map-local logical px)
    /// and the window at that moment.
    Box {
        anchor_x: f32,
        anchor_view: (TimeNs, TimeNs),
    },
    /// One edge follows the pointer; the other stays at `fixed`.
    Edge { fixed: TimeNs, left: bool },
}

/// Everything that forces a path rebuild when it changes. Not the span and not the strip window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PathKey {
    points: usize,
    len: usize,
    physical_width: u32,
    physical_height: u32,
    scale_factor_bits: u32,
    ui_scale_bits: u32,
    metric: HistoryMetric,
    style_version: u64,
}

/// The map's built path and pointer state, kept on the page between frames.
#[derive(Default)]
pub struct MinimapCache {
    key: Option<PathKey>,
    /// The area, map-local physical px over the kept overview's span, and its colour.
    area: Option<(Path<Pixels>, Hsla)>,
    /// The part under the pointer while hovered and not dragging.
    pub hover: Option<Part>,
    /// The gesture in progress.
    pub drag: Option<MinDrag>,
    /// Path builds over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub build_count: u32,
    /// The map of the last paint, window logical px.
    #[cfg(any(test, feature = "test-hooks"))]
    pub map_bounds: Bounds<Pixels>,
    /// The viewport box of the last paint, window logical px. `None` when none was drawn.
    #[cfg(any(test, feature = "test-hooks"))]
    pub box_bounds: Option<Bounds<Pixels>>,
}

/// The map as its own cached view, so a hover change repaints it without rendering the page.
///
/// The page owns this view, so the view holds the page weakly.
pub struct MinimapView {
    page: WeakEntity<HistoryPage>,
}

impl MinimapView {
    pub fn new(page: WeakEntity<HistoryPage>) -> Self {
        Self { page }
    }
}

impl Render for MinimapView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.page.upgrade() {
            Some(page) => HistoryMinimap { page }.into_any_element(),
            None => Empty.into_any_element(),
        }
    }
}

/// The map element. Reads the page's view, state and kept overview and paints into its
/// `MinimapCache`.
pub struct HistoryMinimap {
    page: Entity<HistoryPage>,
}

impl IntoElement for HistoryMinimap {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HistoryMinimap {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    /// Full width, 24 px high.
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let scale = cx.global::<UiMetrics>().ui_scale;
        let style = Style {
            size: size(relative(1.).into(), px(MINIMAP_H * scale).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Hitbox {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let metrics = *cx.global::<UiMetrics>();
        let geo = Geo {
            map: bounds,
            grab: GRIP * metrics.ui_scale,
            hitbox: hitbox.clone(),
            view: self
                .page
                .read(cx)
                .minimap_view
                .as_ref()
                .map(Entity::entity_id),
        };
        register_events(&self.page, &geo, window);
        let theme = crate::theme::current(cx).clone();
        let sf = window.scale_factor();
        self.page.update(cx, |page, _| {
            let span = page.view.as_ref().and_then(|v| v.span);
            let input = Input {
                span,
                view_range: page.state.view_range,
                scrub: page.state.scrub,
                metric: page.state.metric,
                overview: page.overview.as_ref(),
            };
            paint_map(
                &mut page.minimap,
                &input,
                (&theme, &metrics, sf),
                bounds,
                hitbox,
                window,
            );
        });
    }
}

/// What the page hands the map.
struct Input<'a> {
    span: Option<(TimeNs, TimeNs)>,
    view_range: (TimeNs, TimeNs),
    scrub: Option<TimeNs>,
    metric: HistoryMetric,
    overview: Option<&'a KeptOverview>,
}

fn paint_map(
    cache: &mut MinimapCache,
    input: &Input,
    (theme, metrics, sf): (&crate::theme::TlTheme, &UiMetrics, f32),
    bounds: Bounds<Pixels>,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    let hairline = px(1.0 / sf);
    let pw = (f32::from(bounds.size.width) * sf).round().max(0.0) as u32;
    let ph = (f32::from(bounds.size.height) * sf).round().max(0.0) as u32;
    #[cfg(any(test, feature = "test-hooks"))]
    {
        cache.map_bounds = bounds;
        cache.box_bounds = None;
    }

    let overview = input.overview.filter(|o| o.metric == input.metric);
    if let Some(o) = overview {
        let key = PathKey {
            points: Arc::as_ptr(&o.points).cast::<HistoryPoint>() as usize,
            len: o.points.len(),
            physical_width: pw,
            physical_height: ph,
            scale_factor_bits: sf.to_bits(),
            ui_scale_bits: metrics.ui_scale.to_bits(),
            metric: o.metric,
            style_version: metrics.style_version,
        };
        if cache.key != Some(key) {
            cache.key = Some(key);
            let color = metric_fill(&theme.graph, o.metric).alpha(AREA_ALPHA);
            cache.area = build_area(o, pw as f32, ph as f32).map(|p| (p, color));
            #[cfg(any(test, feature = "test-hooks"))]
            {
                cache.build_count += 1;
            }
        }
    }

    window.paint_quad(fill(bounds, theme.list_bg));

    let span = input.span.filter(|s| s.1 > s.0);
    if let Some((s0, s1)) = span {
        let width = f32::from(bounds.size.width);
        let span_w = (s1 - s0) as f64;
        let to_x = |t: TimeNs| {
            bounds.left() + px(((t as f64 - s0 as f64) / span_w * f64::from(width)) as f32)
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let (Some(o), Some((path, color))) = (overview, &cache.area) {
                let (o0, o1) = o.span;
                let sx = ((o1 - o0) as f64 / span_w) as f32;
                let dx = ((o0 as f64 - s0 as f64) / span_w * f64::from(pw)) as f32;
                window.paint_path(placed(path, bounds.origin, sx, dx, 1.0 / sf), *color);
            }
            let (v0, v1) = input.view_range;
            if v1 > v0 {
                let (l, r) = (to_x(v0), to_x(v1));
                let b = Bounds::new(point(l, bounds.top()), size(r - l, bounds.size.height));
                paint_box(cache, theme.accent, b, hairline, metrics.ui_scale, window);
                #[cfg(any(test, feature = "test-hooks"))]
                {
                    cache.box_bounds = Some(b);
                }
            }
            if let Some(t) = input.scrub {
                window.paint_quad(fill(
                    Bounds::new(
                        point(to_x(t), bounds.top()),
                        size(hairline, bounds.size.height),
                    ),
                    theme.accent,
                ));
            }
        });
    }

    // Top border.
    window.paint_quad(fill(
        Bounds::new(bounds.origin, size(bounds.size.width, hairline)),
        theme.divider,
    ));

    match cache.drag {
        Some(MinDrag::Box { .. }) => window.set_window_cursor_style(CursorStyle::ClosedHand),
        Some(MinDrag::Edge { .. }) => window.set_window_cursor_style(CursorStyle::ResizeLeftRight),
        None => match cache.hover {
            Some(Part::Box) => window.set_cursor_style(CursorStyle::OpenHand, hitbox),
            Some(Part::LeftEdge | Part::RightEdge) => {
                window.set_cursor_style(CursorStyle::ResizeLeftRight, hitbox);
            }
            _ => {}
        },
    }
}

/// The viewport box: `accent` 15% fill, a 1 px `accent` border (2 px while hovered or dragged)
/// and 4 px grips centred on both edges.
fn paint_box(
    cache: &MinimapCache,
    accent: Hsla,
    b: Bounds<Pixels>,
    hairline: Pixels,
    scale: f32,
    window: &mut Window,
) {
    window.paint_quad(fill(b, accent.alpha(BOX_ALPHA)));
    let active = cache.drag.is_some()
        || matches!(
            cache.hover,
            Some(Part::Box | Part::LeftEdge | Part::RightEdge)
        );
    let w = if active { hairline * 2.0 } else { hairline };
    let (bw, bh) = (b.size.width, b.size.height);
    for edge in [
        Bounds::new(b.origin, size(bw, w)),
        Bounds::new(point(b.left(), b.bottom() - w), size(bw, w)),
        Bounds::new(b.origin, size(w, bh)),
        Bounds::new(point(b.right() - w, b.top()), size(w, bh)),
    ] {
        window.paint_quad(fill(edge, accent));
    }
    let grip = size(px(GRIP * scale), bh / 2.0);
    for x in [b.left(), b.right()] {
        window.paint_quad(fill(
            Bounds::new(point(x - grip.width / 2.0, b.top() + bh / 4.0), grip),
            accent,
        ));
    }
}

/// Fill colour of `metric` (UI-PRC-035).
fn metric_fill(g: &GraphPalette, metric: HistoryMetric) -> Hsla {
    match metric {
        HistoryMetric::Cpu => g.cpu_fill,
        HistoryMetric::Memory => g.mem_fill,
        HistoryMetric::Disk => g.disk_fill,
        HistoryMetric::Network => g.net_fill,
        HistoryMetric::Gpu => g.gpu_fill,
    }
}

/// One fill from the baseline per run of points with a finite `max`, in map-local physical px
/// over `o.span`. NaN points are gaps; runs under 2 points are skipped. `None` when nothing
/// is drawn.
fn build_area(o: &KeptOverview, pw: f32, ph: f32) -> Option<Path<Pixels>> {
    let (o0, o1) = o.span;
    if o1 <= o0 || pw <= 0.0 || ph <= 0.0 {
        return None;
    }
    let pts = &o.points;
    let y_max = if matches!(o.metric, HistoryMetric::Cpu | HistoryMetric::Gpu) {
        100.0
    } else {
        let max = pts
            .iter()
            .map(|p| p.max)
            .filter(|v| v.is_finite())
            .fold(0.0f32, f32::max);
        if max > 0.0 { max } else { 1.0 }
    };
    let x = |t: TimeNs| ((t as f64 - o0 as f64) / (o1 - o0) as f64 * f64::from(pw)) as f32;
    let y = |v: f32| ph - (v / y_max).clamp(0.0, 1.0) * ph;
    let mut b = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
    let mut any = false;
    let mut start = 0;
    while start < pts.len() {
        if !pts[start].max.is_finite() {
            start += 1;
            continue;
        }
        let end = pts[start..]
            .iter()
            .position(|p| !p.max.is_finite())
            .map_or(pts.len(), |n| start + n);
        if end - start >= 2 {
            b.move_to(point(px(x(pts[start].t_ns)), px(ph)));
            for p in &pts[start..end] {
                b.line_to(point(px(x(p.t_ns)), px(y(p.max))));
            }
            b.line_to(point(px(x(pts[end - 1].t_ns)), px(ph)));
            b.close();
            any = true;
        }
        start = end;
    }
    if !any {
        return None;
    }
    b.build().ok().filter(|p| !p.vertices.is_empty())
}

/// `path` (map-local physical px) as window logical px at `origin`: x scaled by `sx` and shifted
/// by `dx` physical px, then both axes scaled by `inv` and shifted.
fn placed(path: &Path<Pixels>, origin: Point<Pixels>, sx: f32, dx: f32, inv: f32) -> Path<Pixels> {
    let map = |p: Point<Pixels>| point(p.x * sx + px(dx), p.y) * inv + origin;
    let mut path = path.clone();
    let b = path.bounds;
    let (a, z) = (
        map(b.origin),
        map(b.origin + point(b.size.width, b.size.height)),
    );
    path.bounds = Bounds::new(a, size(z.x - a.x, z.y - a.y));
    for vertex in &mut path.vertices {
        vertex.xy_position = map(vertex.xy_position);
    }
    path
}

/// What the pointer listeners need, captured at paint time.
#[derive(Clone)]
struct Geo {
    map: Bounds<Pixels>,
    /// Edge grab distance, logical px.
    grab: f32,
    hitbox: Hitbox,
    /// The map view; hover changes notify it instead of the page.
    view: Option<EntityId>,
}

impl Geo {
    /// Pointer x relative to the map's left edge, logical px.
    fn x(&self, p: Point<Pixels>) -> f32 {
        f32::from(p.x - self.map.left())
    }

    fn width(&self) -> f32 {
        f32::from(self.map.size.width)
    }

    /// The box edges in map-local logical px.
    fn box_x(&self, view: (TimeNs, TimeNs), span: (TimeNs, TimeNs)) -> (f32, f32) {
        let w = f64::from(self.width());
        let span_w = (span.1 - span.0) as f64;
        let x = |t: TimeNs| ((t as f64 - span.0 as f64) / span_w * w) as f32;
        (x(view.0), x(view.1))
    }
}

/// The page's strip window and retained span, when both can be used.
fn view_and_span(page: &HistoryPage) -> Option<((TimeNs, TimeNs), (TimeNs, TimeNs))> {
    let span = page
        .view
        .as_ref()
        .and_then(|v| v.span)
        .filter(|s| s.1 > s.0)?;
    let view = page.state.view_range;
    (view.1 > view.0).then_some((view, span))
}

fn on_mouse_down(
    page: &Entity<HistoryPage>,
    geo: &Geo,
    e: &MouseDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    if e.button != MouseButton::Left || !geo.hitbox.is_hovered(window) {
        return;
    }
    let (x, width) = (geo.x(e.position), geo.width());
    page.update(cx, |p, cx| {
        let Some((view, span)) = view_and_span(p) else {
            return;
        };
        let (l, r) = geo.box_x(view, span);
        p.minimap.hover = None;
        let drag = match hit(x, l, r, geo.grab, width) {
            Part::LeftEdge => MinDrag::Edge {
                fixed: view.1,
                left: true,
            },
            Part::RightEdge => MinDrag::Edge {
                fixed: view.0,
                left: false,
            },
            Part::Box => MinDrag::Box {
                anchor_x: x,
                anchor_view: view,
            },
            Part::Outside => {
                // Centre the box at the click; a drag goes on from there.
                let t = x_to_t(x, width, span);
                let mid = view.0 + (view.1 - view.0) / 2;
                let next = pan(view, t as i64 - mid as i64, span);
                p.set_view_range(next, cx);
                MinDrag::Box {
                    anchor_x: x,
                    anchor_view: next,
                }
            }
        };
        p.minimap.drag = Some(drag);
        cx.notify();
    });
}

fn on_mouse_move(
    page: &Entity<HistoryPage>,
    geo: &Geo,
    e: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let hovered = geo.hitbox.is_hovered(window);
    let (x, width) = (geo.x(e.position), geo.width());
    let hover_changed = page.update(cx, |p, cx| {
        let Some(drag) = p.minimap.drag else {
            let part = view_and_span(p).filter(|_| hovered).map(|(view, span)| {
                let (l, r) = geo.box_x(view, span);
                hit(x, l, r, geo.grab, width)
            });
            let changed = p.minimap.hover != part;
            p.minimap.hover = part;
            return Some(changed);
        };
        if e.pressed_button.is_none() {
            // The button was released outside the window.
            p.minimap.drag = None;
            cx.notify();
            return None;
        }
        let (view, span) = view_and_span(p)?;
        let next = match drag {
            MinDrag::Box {
                anchor_x,
                anchor_view,
            } if width > 0.0 => {
                let delta = f64::from((x - anchor_x) / width) * (span.1 - span.0) as f64;
                pan(anchor_view, delta.round() as i64, span)
            }
            MinDrag::Box { .. } => view,
            MinDrag::Edge { fixed, left } => edge_drag(fixed, left, x_to_t(x, width, span), span),
        };
        p.set_view_range(next, cx);
        None
    });
    match (hover_changed, geo.view) {
        // Only the map repaints for a hover change, not the page.
        (Some(true), Some(id)) => cx.notify(id),
        (Some(true), None) => page.update(cx, |_, cx| cx.notify()),
        _ => {}
    }
}

fn on_scroll(
    page: &Entity<HistoryPage>,
    geo: &Geo,
    e: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut App,
) {
    if !geo.hitbox.is_hovered(window) {
        return;
    }
    let notches = match e.delta {
        ScrollDelta::Lines(l) => l.y,
        ScrollDelta::Pixels(p) => f32::from(p.y) / PIXELS_PER_NOTCH,
    };
    if notches == 0.0 {
        return;
    }
    let (x, width) = (geo.x(e.position), geo.width());
    page.update(cx, |p, cx| {
        let Some((view, span)) = view_and_span(p) else {
            return;
        };
        // The time under the pointer keeps its place in the box, inside or outside it.
        let cursor = x_to_t(x, width, span);
        p.set_view_range(zoom_at(view, cursor, notches, span), cx);
    });
    cx.stop_propagation();
}

/// Registers the map's pointer listeners for this frame.
fn register_events(page: &Entity<HistoryPage>, geo: &Geo, window: &mut Window) {
    let (p, g) = (page.clone(), geo.clone());
    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
        if phase.bubble() {
            on_mouse_down(&p, &g, e, window, cx);
        }
    });
    let (p, g) = (page.clone(), geo.clone());
    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
        if phase.bubble() {
            on_mouse_move(&p, &g, e, window, cx);
        }
    });
    let p = page.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase.bubble() {
            p.update(cx, |p, cx| {
                if p.minimap.drag.take().is_some() {
                    cx.notify();
                }
            });
        }
    });
    let (p, g) = (page.clone(), geo.clone());
    window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
        if phase.bubble() {
            on_scroll(&p, &g, e, window, cx);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = NS_PER_SEC;

    #[test]
    fn hit_parts() {
        // A 100 px box from 100 to 200 on a 600 px map, grab 4.
        let hit = |x| hit(x, 100.0, 200.0, 4.0, 600.0);
        assert_eq!(hit(150.0), Part::Box);
        assert_eq!(hit(102.0), Part::LeftEdge);
        assert_eq!(hit(97.0), Part::LeftEdge);
        assert_eq!(hit(203.0), Part::RightEdge);
        assert_eq!(hit(50.0), Part::Outside);
    }

    #[test]
    fn hit_narrow_box() {
        // A 6 px box in the middle: inside is the box, just outside is an edge.
        let mid = |x| hit(x, 100.0, 106.0, 4.0, 600.0);
        assert_eq!(mid(100.5), Part::Box);
        assert_eq!(mid(105.5), Part::Box);
        assert_eq!(mid(98.0), Part::LeftEdge);
        assert_eq!(mid(109.0), Part::RightEdge);
        // At the left end of the map the left edge has no room outside: it takes the left third.
        let left = |x| hit(x, 0.0, 6.0, 4.0, 600.0);
        assert_eq!(left(0.0), Part::LeftEdge);
        assert_eq!(left(1.5), Part::LeftEdge);
        assert_eq!(left(3.0), Part::Box);
        assert_eq!(left(5.5), Part::Box);
        assert_eq!(left(8.0), Part::RightEdge);
        // At the right end the same holds for the right edge.
        let right = |x| hit(x, 594.0, 600.0, 4.0, 600.0);
        assert_eq!(right(600.0), Part::RightEdge);
        assert_eq!(right(598.5), Part::RightEdge);
        assert_eq!(right(597.0), Part::Box);
        assert_eq!(right(594.5), Part::Box);
        assert_eq!(right(592.0), Part::LeftEdge);
    }

    #[test]
    fn zoom_at_keeps_the_pointer_place() {
        let span = (0, 1000 * S);
        let view = (400 * S, 600 * S);
        // Inside the box it matches the strip's zoom.
        assert_eq!(
            zoom_at(view, 450 * S, 1.0, span),
            zoom(view, 450 * S, 1.0, span)
        );
        // Outside: the pointer at 200 s is one box width left of the box. Zooming out by one notch
        // (250 s wide) keeps it one box width left: 200 + 250 = 450.
        assert_eq!(zoom_at(view, 200 * S, -1.0, span), (450 * S, 700 * S));
        // Zooming in (160 s wide) moves the box towards the pointer: 200 + 160 = 360.
        assert_eq!(zoom_at(view, 200 * S, 1.0, span), (360 * S, 520 * S));
        // Pushed past the end of the span: kept inside it at the zoomed width.
        assert_eq!(
            zoom_at((800 * S, 1000 * S), 0, -1.0, span),
            (750 * S, 1000 * S)
        );
    }

    #[test]
    fn edge_drag_keeps_the_fixed_end() {
        let span = (100 * S, 1000 * S);
        assert_eq!(edge_drag(600 * S, true, 450 * S, span), (450 * S, 600 * S));
        // Past the fixed end: stops 60 s short of it.
        assert_eq!(edge_drag(600 * S, true, 590 * S, span), (540 * S, 600 * S));
        assert_eq!(edge_drag(600 * S, true, 0, span), (100 * S, 600 * S));
        assert_eq!(edge_drag(400 * S, false, 410 * S, span), (400 * S, 460 * S));
        assert_eq!(
            edge_drag(400 * S, false, 2000 * S, span),
            (400 * S, 1000 * S)
        );
    }
}
