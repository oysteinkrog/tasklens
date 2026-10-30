//! `ChartElement`: paints one `ChartView` (ADR-009 rules 1 to 7, UI-PRF-001 "Paint plan per
//! frame", UI-PRF-005, UI-PRF-007).
//!
//! Geometry is rebuilt only when the `ChartGeometryKey` changes, checked in place by
//! `ChartGeometryKey::sync`. Every other frame clones the cached paths, moves them into place
//! and paints them, so a moving chart costs 2 to 4 path clones and 3 to 13 grid quads per frame
//! and allocates nothing else.
//!
//! Stale tail: while every series is stale (R-051) the right edge keeps moving with the clock,
//! `now − period` when sliding and `now` otherwise, so the flat half-alpha tail from the newest
//! sample to the right edge shows (UI-PRF-007). The geometry is rebuilt whenever that edge moved,
//! because the tail is drawn up to it.
//!
//! Frozen (`ChartView::freeze`): the right edge stays at the newest valid sample, the chart is
//! never stale and never asks for frames, so it stops where its data stopped (PERF-086).

use std::sync::Arc;

use gpui_kit::{
    App, Bounds, ContentMask, Element, ElementId, Entity, GlobalElementId, Hitbox, HitboxBehavior,
    Hsla, InspectorElementId, IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent, Path,
    PathVertex, Pixels, Point, Style, Window, point, px, relative, size,
};
use smallvec::SmallVec;
use tl_model::series::{SampleFlags, SeriesView};
use tl_model::time::now_boot_ns;

use super::geometry::{self, BuildInput, TimeMap};
use super::grid::{self, GridPaint};
use super::hover::{self, HoverPaint};
use super::labels::{self, LabelPaint, LabelRows, NetFormat};
use super::view::ChartView;
use super::{ChartSeries, ChartState, GeometryScalars, PathSet, scale};
use crate::anim::AnimationDriver;
use crate::settings_global::TlSettings;
use crate::theme::{TlTheme, UiMetrics};

/// Period used when no series has data: 1 s.
const DEFAULT_PERIOD_NS: u64 = 1_000_000_000;

/// Alpha factor of the stale sets.
const STALE_ALPHA: f32 = 0.5;

pub struct ChartElement {
    view: Entity<ChartView>,
}

impl ChartElement {
    pub fn new(view: Entity<ChartView>) -> Self {
        Self { view }
    }
}

impl IntoElement for ChartElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ChartElement {
    type RequestLayoutState = ();
    /// `Some` only when `spec.hover` (E19-014).
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    /// The chart fills the bounds its parent gives.
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    /// A hitbox over the plot when `spec.hover`, none otherwise.
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let ui_scale = cx.global::<UiMetrics>().ui_scale;
        let view = self.view.read(cx);
        let hitbox = view.spec.hover.then(|| {
            let (frame, _) = frame_of(view, bounds, ui_scale);
            window.insert_hitbox(frame.inset(px(1.0)), HitboxBehavior::Normal)
        });
        #[cfg(any(test, feature = "test-hooks"))]
        self.view
            .update(cx, |v, _| v.hover.hitbox_inserted = hitbox.is_some());
        hitbox
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let env = PaintEnv {
            scale_factor: window.scale_factor(),
            now: now_boot_ns(),
            metrics: *cx.global::<UiMetrics>(),
            theme: crate::theme::current(cx).clone(),
            reduce_motion: cx.reduce_motion(),
            power_saving: crate::demand::power_saving(cx),
            network_unit: NetFormat::of(cx),
            curved: cx.has_global::<TlSettings>()
                && TlSettings::settings(cx).performance.curved_lines,
        };
        let view_id = self.view.entity_id();
        let out = self.view.update(cx, |view, cx| {
            paint_view(view, bounds, hitbox.as_ref(), &env, window, cx)
        });

        if out.moving {
            AnimationDriver::global(window, cx).update(cx, |d, cx| d.request(view_id, window, cx));
        }

        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(driver) = AnimationDriver::try_global(window, cx) {
            driver.update(cx, |d, _| {
                if let Some(log) = d.paint_log.as_mut() {
                    log.push(crate::anim::PaintRecord {
                        view: view_id,
                        newest_ns: out.newest_ns,
                        painted_at_ns: env.now,
                    });
                }
            });
        }

        #[cfg(any(test, feature = "test-hooks"))]
        PLOTS.with(|p| p.borrow_mut().push((view_id, out.plot)));

        if let Some(hitbox) = hitbox.clone() {
            register_hover_events(&self.view, hitbox, &out, env.scale_factor, window);
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static PLOTS: std::cell::RefCell<Vec<(gpui_kit::EntityId, Bounds<Pixels>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The plots that charts painted on this thread since the last call, with their chart's
/// entity, in paint order (tasklens-o55.54).
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_plots() -> Vec<(gpui_kit::EntityId, Bounds<Pixels>)> {
    PLOTS.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Pointer moves over the plot pin the sample time nearest the pointer; leaving the plot drops
/// the pin. Any mouse button down drops it too. Both notify only when the pin changed, so moves
/// inside one sample do not notify.
fn register_hover_events(
    view: &Entity<ChartView>,
    hitbox: Hitbox,
    out: &PaintOut,
    sf: f32,
    window: &mut Window,
) {
    let (plot, map, grid) = (out.plot, out.map, out.grid);
    let entity = view.clone();
    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        let t = hitbox.is_hovered(window).then(|| {
            let x = f32::from(e.position.x - plot.left()) * sf;
            hover::snap_time(hover::pointer_time(&map, map.plot_width, x), grid)
        });
        entity.update(cx, |v, cx| {
            if v.hover.pinned_t_ns != t {
                v.hover.pinned_t_ns = t;
                cx.notify();
            }
        });
    });
    if out.pinned {
        let entity = view.clone();
        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
            if phase.bubble() {
                entity.update(cx, |v, cx| {
                    if v.hover.clear() {
                        cx.notify();
                    }
                });
            }
        });
    }
}

/// The chart's frame inside `bounds`, and the label rows outside it: an Overview chart (compact
/// scale) keeps a title row above for a heading (UI-PRF-059); any other chart fills `bounds`.
fn frame_of(
    view: &ChartView,
    bounds: Bounds<Pixels>,
    ui_scale: f32,
) -> (Bounds<Pixels>, Option<LabelRows>) {
    if !view.compact_scale {
        return (bounds, None);
    }
    let rows = LabelRows::of(bounds, view.labels.heading.is_some(), ui_scale);
    (rows.frame(bounds), Some(rows))
}

/// Per-paint inputs read from the window and the app before the view is borrowed.
struct PaintEnv {
    scale_factor: f32,
    now: u64,
    metrics: UiMetrics,
    theme: Arc<TlTheme>,
    reduce_motion: bool,
    /// Save power on battery is in effect: no sliding (wlf.32).
    power_saving: bool,
    network_unit: NetFormat,
    /// `performance.curved_lines`.
    curved: bool,
}

struct PaintOut {
    moving: bool,
    /// The plot of this paint, for the hover listeners.
    plot: Bounds<Pixels>,
    /// Paint-time mapping, for the hover listeners.
    map: TimeMap,
    /// A hover pin survived this paint.
    pinned: bool,
    /// The sample grid the hover pin snaps to.
    grid: hover::SampleGrid,
    #[cfg_attr(
        not(any(test, feature = "test-hooks")),
        allow(dead_code, reason = "read by the bench paint hook only")
    )]
    newest_ns: u64,
}

/// Steps 1 to 8 and 10 of the paint plan. Returns whether the chart wants the next frame.
fn paint_view(
    view: &mut ChartView,
    bounds: Bounds<Pixels>,
    hitbox: Option<&Hitbox>,
    env: &PaintEnv,
    window: &mut Window,
    cx: &mut App,
) -> PaintOut {
    let sf = env.scale_factor;
    // A chart that reads the device metric history runs on CLOCK_REALTIME (UI-PRF-055).
    let history = view.history;
    let now = history.map_or(env.now, |h| h.right_edge_ns(env.now));
    let (frame, rows) = frame_of(view, bounds, env.metrics.ui_scale);
    let plot = frame.inset(px(1.0));
    let pw = (f32::from(plot.size.width) * sf).round().max(0.0) as u32;
    let ph = (f32::from(plot.size.height) * sf).round().max(0.0) as u32;

    let state = view.derived_state(now);
    let value = view.value_at(now);
    // The readout shows only while the pointer is over the plot of an active window.
    if !(hitbox.is_some_and(|h| h.is_hovered(window)) && window.is_window_active()) {
        view.hover.clear();
    }
    let compact_scale = view.compact_scale;
    let frozen = view.frozen;
    let newest_marks = view.newest_marks;
    let ChartView {
        spec,
        labels,
        cache,
        scratch,
        built_right_edge_ns,
        scale_state,
        axis_text,
        series,
        text,
        last_value_text,
        value_text,
        hover: hover_state,
        boots,
        ..
    } = view;

    let mut newest_ns = 0u64;
    let mut period_ns = u64::MAX;
    for s in series.iter().flatten() {
        if let Some(last) = s.samples.last() {
            newest_ns = newest_ns.max(last.t_ns);
        }
        period_ns = period_ns.min(s.period_ns);
    }
    if period_ns == u64::MAX {
        period_ns = DEFAULT_PERIOD_NS;
    }
    let sliding =
        !frozen && history.is_none() && spec.smooth && !env.reduce_motion && !env.power_saving;
    let stale = !frozen
        && history.is_none()
        && newest_ns > 0
        && geometry::is_stale(newest_ns, now, period_ns);
    // No data: the grid is placed against the clock, where the first samples will land. A
    // history chart ends at its request's end, so a stretch with no points before it shows.
    let right_edge = if newest_ns == 0 || history.is_some() {
        now
    } else if frozen {
        newest_valid_ns(series).unwrap_or(newest_ns)
    } else if stale {
        if sliding {
            now.saturating_sub(period_ns)
        } else {
            now
        }
    } else {
        geometry::right_edge_ns(now, newest_ns, period_ns, sliding)
    };
    let range_ns = spec.range_ns;
    let sample_grid = hover::SampleGrid {
        anchor_ns: if newest_ns == 0 {
            right_edge
        } else {
            newest_ns
        },
        period_ns,
    };

    let has_data = matches!(state, ChartState::Live | ChartState::Stale { .. });
    let mut cache = cache.borrow_mut();
    if has_data {
        let (vmin, vmax) = visible_min_max(series, right_edge, range_ns);
        let y = scale::compute(
            &spec.y,
            vmin,
            vmax,
            now,
            scale_state,
            labels.value_fmt,
            env.network_unit.unit,
            axis_text,
        );
        // Compared in place, so an unchanged chart allocates nothing here (tasklens-yrf.19).
        let key_changed = cache.key.sync(
            series.iter().map(|s| s.as_ref().map_or(0, |s| s.version)),
            GeometryScalars {
                physical_width: pw,
                physical_height: ph,
                range_ns,
                scale_bits: spec.scale_bits(),
                y_min_bits: y.y_min.to_bits(),
                y_max_bits: y.y_max.to_bits(),
                style_version: env.metrics.style_version,
                curved: env.curved,
            },
        );
        #[cfg(any(test, feature = "test-hooks"))]
        if key_changed {
            cache.key_rebuilds += 1;
        }
        if key_changed || (stale && right_edge != *built_right_edge_ns) {
            let pairs: SmallVec<[(ChartSeries, Option<&SeriesView>); 2]> = spec
                .series
                .iter()
                .enumerate()
                .map(|(i, s)| (s.clone(), series.get(i).and_then(|v| v.as_deref())))
                .collect();
            let input = BuildInput {
                series: &pairs,
                map: TimeMap {
                    anchor_ns: newest_ns,
                    right_edge_ns: right_edge,
                    range_ns,
                    plot_width: pw as f32,
                },
                plot_height: ph as f32,
                y_min: y.y_min,
                y_max: y.y_max,
                // Frozen and history: the clock stops at the right edge, so no stale tail is
                // drawn.
                now_boot_ns: if frozen || history.is_some() {
                    right_edge
                } else {
                    now
                },
                stack: spec.stack,
                curved: env.curved,
            };
            geometry::build(&input, &mut cache, scratch);
            *built_right_edge_ns = right_edge;
            #[cfg(any(test, feature = "test-hooks"))]
            view_hook_build(&view.build_count);
        }
    }

    // Paint-time mapping: the cached anchor when there is geometry, else the right edge itself.
    let map = TimeMap {
        anchor_ns: if has_data {
            cache.anchor_ns
        } else {
            right_edge
        },
        right_edge_ns: right_edge,
        range_ns,
        plot_width: pw as f32,
    };
    let dx = map.dx(pw as f32);
    let radius = if spec.grid.rounded {
        env.metrics.radius_small
    } else {
        px(0.)
    };
    let gp = GridPaint {
        plot,
        box_bounds: frame,
        map: &map,
        dx: px(dx),
        scale_factor: sf,
        theme: &env.theme,
        style: spec.grid.clone(),
        outline_color: spec.series.first().map_or(Hsla::default(), |s| s.line),
        radius,
    };

    let label_paint = LabelPaint {
        box_bounds: bounds,
        plot,
        labels,
        spec,
        state: &state,
        value,
        value_text: value_text.as_deref(),
        axis_text: (has_data && !axis_text.is_empty()).then_some(axis_text.as_str()),
        compact_scale,
        rows,
        metrics: &env.metrics,
        theme: &env.theme,
        network_unit: env.network_unit,
        clock_right_ns: history.map(|_| right_edge),
    };

    grid::paint_background(window, frame, env.theme.graph.chart_bg, radius);

    #[cfg(any(test, feature = "test-hooks"))]
    view.path_colors.borrow_mut().clear();

    // Failed, Restricted, Unsupported: background, outline and a dotted baseline across the
    // whole plot (UI-PRF-001). No grid, no paths, no frame requests.
    if matches!(
        state,
        ChartState::Failed(_) | ChartState::Restricted | ChartState::Unsupported
    ) {
        let gp = GridPaint { dx: px(0.), ..gp };
        // `paint_gaps` clips to the plot width, so no content mask is needed.
        let whole = [0.0..pw as f32];
        grid::paint_gaps(window, &gp, &whole);
        grid::paint_outline(window, &gp);
        labels::paint(window, cx, text, &label_paint, last_value_text);
        #[cfg(any(test, feature = "test-hooks"))]
        view_hook_quads(&view.last_grid_quads, &view.last_gap_spans, 0, &whole);
        #[cfg(any(test, feature = "test-hooks"))]
        view.last_right_edge_ns.set(right_edge);
        hover_state.clear();
        return PaintOut {
            moving: false,
            plot,
            map,
            pinned: false,
            grid: sample_grid,
            newest_ns,
        };
    }

    grid::paint_grid(window, &gp);
    #[cfg(any(test, feature = "test-hooks"))]
    view_hook_quads(
        &view.last_grid_quads,
        &view.last_gap_spans,
        {
            let mut n = QuadCount(0);
            grid::paint_grid(&mut n, &gp);
            n.0
        },
        if has_data { &cache.gaps[..] } else { &[] },
    );

    if has_data {
        let offset = point(plot.left() + px(dx / sf), plot.top());
        let painted = |window: &mut Window, sets: &[PathSet], alpha: f32| {
            for set in sets {
                let color = Hsla {
                    a: set.color.a * alpha,
                    ..set.color
                };
                window.paint_path(translated(&set.path, offset, sf), color);
                #[cfg(any(test, feature = "test-hooks"))]
                {
                    view.path_paints.set(view.path_paints.get() + 1);
                    view.path_colors.borrow_mut().push(color);
                }
            }
        };
        window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
            let live =
                newest_marks && matches!(state, ChartState::Live) && grid::newest_marks_fit(frame);
            if live {
                grid::paint_newest_glow(window, plot, gp.outline_color, env.reduce_motion);
            }
            painted(window, &cache.fills, 1.0);
            painted(window, &cache.stale_fills, STALE_ALPHA);
            painted(window, &cache.strokes, 1.0);
            painted(window, &cache.stale_strokes, STALE_ALPHA);
            if live {
                for &(p, color) in &cache.newest {
                    let c = newest_point(p, offset, sf);
                    grid::paint_newest_dot(window, c, color, env.reduce_motion);
                }
            }
            grid::paint_gaps(window, &gp, &cache.gaps);
        });
    }
    // A history chart marks each boot inside its range (UI-PRF-055).
    let _boot_marks = if history.is_some() {
        grid::paint_boots(window, &gp, boots)
    } else {
        0
    };
    #[cfg(any(test, feature = "test-hooks"))]
    view.last_boot_marks.set(_boot_marks);

    grid::paint_baseline(window, &gp);
    grid::paint_outline(window, &gp);
    labels::paint(window, cx, text, &label_paint, last_value_text);

    if spec.hover {
        let hp = HoverPaint {
            box_bounds: frame,
            plot,
            flip_right: plot.right(),
            map,
            dx,
            scale_factor: sf,
            spec,
            labels,
            series,
            y_min: cache.y_min,
            y_max: cache.y_max,
            metrics: &env.metrics,
            theme: &env.theme,
            network_unit: env.network_unit,
        };
        hover::paint(window, cx, hover_state, &hp);
    }

    let one_px_ns = range_ns / u64::from(pw.max(1));
    let moving = has_data
        && sliding
        && newest_ns
            .saturating_add(period_ns.saturating_mul(2))
            .saturating_add(one_px_ns)
            > now;

    #[cfg(any(test, feature = "test-hooks"))]
    view.last_right_edge_ns.set(right_edge);

    PaintOut {
        moving,
        plot,
        map,
        pinned: hover_state.pinned_t_ns.is_some(),
        grid: sample_grid,
        newest_ns,
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl ChartView {
    /// Hover pin, shaped lines and the last paint's box (E19-016).
    pub fn hover_state(&self) -> &hover::HoverState {
        &self.hover
    }

    /// Paints that found the geometry key changed and updated it.
    pub fn key_rebuilds(&self) -> u32 {
        self.cache.borrow().key_rebuilds
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn view_hook_build(count: &std::cell::Cell<u32>) {
    count.set(count.get() + 1);
}

/// Records the grid quad count and the dotted spans of this paint.
#[cfg(any(test, feature = "test-hooks"))]
fn view_hook_quads(
    quads: &std::cell::Cell<u32>,
    spans: &std::cell::RefCell<SmallVec<[std::ops::Range<f32>; 4]>>,
    grid_quads: u32,
    gaps: &[std::ops::Range<f32>],
) {
    quads.set(grid_quads);
    let mut spans = spans.borrow_mut();
    spans.clear();
    spans.extend(gaps.iter().cloned());
}

/// Counts quads without painting them (test hook).
#[cfg(any(test, feature = "test-hooks"))]
struct QuadCount(u32);

#[cfg(any(test, feature = "test-hooks"))]
impl grid::QuadSink for QuadCount {
    fn quad(&mut self, _: gpui_kit::PaintQuad) {
        self.0 += 1;
    }
}

/// A newest point from `ChartCache::newest` (chart-local physical px) in window logical px, as
/// `translated` maps the path vertices.
fn newest_point(p: Point<Pixels>, offset: Point<Pixels>, scale_factor: f32) -> Point<Pixels> {
    p * (1.0 / scale_factor) + offset
}

/// Time of the newest valid sample over every series, `None` when there is none.
fn newest_valid_ns(series: &[Option<Arc<SeriesView>>]) -> Option<u64> {
    series
        .iter()
        .flatten()
        .filter_map(|s| s.samples.iter().rev().find(|s| s.is_valid()))
        .map(|s| s.t_ns)
        .max()
}

/// Min and max of the valid samples inside `[right_edge − range, right_edge]` over every series,
/// `(0, 0)` when there are none. Walks the visible window only.
fn visible_min_max(
    series: &[Option<Arc<SeriesView>>],
    right_edge: u64,
    range_ns: u64,
) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for s in series.iter().flatten() {
        let from = geometry::window_start(&s.samples, right_edge, range_ns, 0);
        for sample in &s.samples[from..] {
            if sample.t_ns > right_edge {
                break;
            }
            if sample.flags.contains(SampleFlags::VALID) {
                lo = lo.min(sample.value);
                hi = hi.max(sample.value);
            }
        }
    }
    if lo > hi { (0.0, 0.0) } else { (lo, hi) }
}

/// `path` (chart-local physical px) as logical px moved by `offset`: every vertex and the bounds
/// divided by `scale_factor` once, then shifted, like gpui-component `path_cache.rs::translated`.
/// `Path::translate` is not used: it works in scaled pixels and would apply the scale twice.
/// `paint_path` takes the path by value, so one copy per paint is needed. The vertices are
/// written once in that copy, not cloned and then rewritten (tasklens-wlf.34).
pub(super) fn translated(
    path: &Path<Pixels>,
    offset: Point<Pixels>,
    scale_factor: f32,
) -> Path<Pixels> {
    let inv = 1.0 / scale_factor;
    let mut out = Path::new(path.bounds.origin * inv + offset);
    out.bounds.size = size(path.bounds.size.width * inv, path.bounds.size.height * inv);
    out.vertices = path
        .vertices
        .iter()
        .map(|v| PathVertex {
            xy_position: v.xy_position * inv + offset,
            st_position: v.st_position,
            content_mask: Default::default(),
        })
        .collect();
    out
}
