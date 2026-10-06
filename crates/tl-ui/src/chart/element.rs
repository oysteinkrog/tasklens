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

use std::sync::Arc;

use gpui_kit::{
    App, Bounds, ContentMask, Element, ElementId, Entity, GlobalElementId, Hitbox, Hsla,
    InspectorElementId, IntoElement, LayoutId, Path, Pixels, Point, Style, Window, point, px,
    relative, size,
};
use smallvec::SmallVec;
use tl_model::series::{SampleFlags, SeriesView};
use tl_model::settings::NetworkUnit;
use tl_model::time::now_boot_ns;

use super::geometry::{self, BuildInput, TimeMap};
use super::grid::{self, GridPaint};
use super::labels::{self, LabelPaint};
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

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) -> Option<Hitbox> {
        None
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let env = PaintEnv {
            scale_factor: window.scale_factor(),
            now: now_boot_ns(),
            metrics: *cx.global::<UiMetrics>(),
            theme: crate::theme::current(cx).clone(),
            reduce_motion: cx.reduce_motion(),
            network_unit: if cx.has_global::<TlSettings>() {
                TlSettings::settings(cx).units.network
            } else {
                NetworkUnit::default()
            },
        };
        let view_id = self.view.entity_id();
        let out = self
            .view
            .update(cx, |view, cx| paint_view(view, bounds, &env, window, cx));

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
    }
}

/// Per-paint inputs read from the window and the app before the view is borrowed.
struct PaintEnv {
    scale_factor: f32,
    now: u64,
    metrics: UiMetrics,
    theme: Arc<TlTheme>,
    reduce_motion: bool,
    network_unit: NetworkUnit,
}

struct PaintOut {
    moving: bool,
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
    env: &PaintEnv,
    window: &mut Window,
    cx: &mut App,
) -> PaintOut {
    let sf = env.scale_factor;
    let now = env.now;
    let plot = bounds.inset(px(1.0));
    let pw = (f32::from(plot.size.width) * sf).round().max(0.0) as u32;
    let ph = (f32::from(plot.size.height) * sf).round().max(0.0) as u32;

    let state = view.derived_state(now);
    let value = view.value_at(now);
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
    let sliding = spec.smooth && !env.reduce_motion;
    let stale = newest_ns > 0 && geometry::is_stale(newest_ns, now, period_ns);
    // No data: the grid is placed against the clock, where the first samples will land.
    let right_edge = if newest_ns == 0 {
        now
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
            env.network_unit,
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
                now_boot_ns: now,
                stack: spec.stack,
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
        box_bounds: bounds,
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
        axis_text: (has_data && !axis_text.is_empty()).then_some(axis_text.as_str()),
        metrics: &env.metrics,
        theme: &env.theme,
        network_unit: env.network_unit,
    };

    grid::paint_background(window, bounds, env.theme.graph.chart_bg, radius);

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
        return PaintOut {
            moving: false,
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
            painted(window, &cache.fills, 1.0);
            painted(window, &cache.stale_fills, STALE_ALPHA);
            painted(window, &cache.strokes, 1.0);
            painted(window, &cache.stale_strokes, STALE_ALPHA);
            grid::paint_gaps(window, &gp, &cache.gaps);
        });
    }

    grid::paint_baseline(window, &gp);
    grid::paint_outline(window, &gp);
    labels::paint(window, cx, text, &label_paint, last_value_text);

    let one_px_ns = range_ns / u64::from(pw.max(1));
    let moving = has_data
        && sliding
        && newest_ns
            .saturating_add(period_ns.saturating_mul(2))
            .saturating_add(one_px_ns)
            > now;

    #[cfg(any(test, feature = "test-hooks"))]
    view.last_right_edge_ns.set(right_edge);

    PaintOut { moving, newest_ns }
}

#[cfg(any(test, feature = "test-hooks"))]
impl ChartView {
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
fn translated(path: &Path<Pixels>, offset: Point<Pixels>, scale_factor: f32) -> Path<Pixels> {
    let inv = 1.0 / scale_factor;
    let mut path = path.clone();
    path.bounds.origin = path.bounds.origin * inv + offset;
    path.bounds.size = size(path.bounds.size.width * inv, path.bounds.size.height * inv);
    for vertex in &mut path.vertices {
        vertex.xy_position = vertex.xy_position * inv + offset;
    }
    path
}
