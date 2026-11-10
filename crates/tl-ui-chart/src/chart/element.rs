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
    App, Bounds, ContentMask, Element, ElementId, Entity, EntityId, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseDownEvent,
    MouseMoveEvent, Path, PathVertex, Pixels, Point, Style, Window, point, px, relative, size,
};
use smallvec::SmallVec;
use tl_model::series::{SampleFlags, SeriesView};

use super::geometry::{self, BuildInput, MovedPath, TimeMap};
use super::grid::{self, GridPaint};
use super::hover::{self, HoverPaint};
use super::labels::{self, LabelPaint, LabelRows, NetFormat};
use super::lines;
use super::ramp::{self, RampBand, ValueRamp};
use super::view::ChartView;
use super::{ChartSeries, ChartState, GeometryScalars, PathSet, scale};
use tl_ui_base::anim::{AnimationDriver, Pacing};
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::{TlTheme, UiMetrics};

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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
            snap_dx: AnimationDriver::try_global(window, cx)
                .is_some_and(|d| d.read(cx).pacing() != Pacing::Off),
            now: self.view.read(cx).now_ns(),
            metrics: *cx.global::<UiMetrics>(),
            theme: tl_ui_base::theme::current(cx).clone(),
            reduce_motion: cx.reduce_motion(),
            power_saving: tl_ui_base::demand::power_saving(cx),
            network_unit: NetFormat::of(cx),
            curved: cx.has_global::<TlSettings>()
                && TlSettings::settings(cx).performance.curved_lines,
            value_colors: ValueRamp::enabled(cx),
        };
        let view_id = self.view.entity_id();
        let out = self.view.update(cx, |view, cx| {
            paint_view(view, view_id, bounds, hitbox.as_ref(), &env, window, cx)
        });

        if out.moving {
            AnimationDriver::global(window, cx).update(cx, |d, cx| {
                d.request_paced(view_id, out.step_ns, window, cx)
            });
        }

        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        if let Some(driver) = AnimationDriver::try_global(window, cx) {
            driver.update(cx, |d, _| {
                if let Some(log) = d.paint_log.as_mut() {
                    log.push(tl_ui_base::anim::PaintRecord {
                        view: view_id,
                        newest_ns: out.newest_ns,
                        painted_at_ns: env.now,
                    });
                }
            });
        }

        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        PLOTS.with(|p| {
            if let Some(plots) = p.borrow_mut().as_mut() {
                plots.push((view_id, out.plot.dilate(px(1.0))));
            }
        });

        if let Some(hitbox) = hitbox.clone() {
            register_hover_events(&self.view, hitbox, &out, env.scale_factor, window);
        }
    }
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
type PlotLog = Vec<(gpui_kit::EntityId, Bounds<Pixels>)>;

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
thread_local! {
    /// `None` until the first `take_plots` call, so the app never collects plots.
    static PLOTS: std::cell::RefCell<Option<PlotLog>> =
        const { std::cell::RefCell::new(None) };
}

/// The plots that charts painted on this thread since the last call, each with its 1 px
/// outline, with their chart's entity, in paint order (tasklens-o55.54). The first call turns
/// collection on and returns nothing.
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub fn take_plots() -> PlotLog {
    PLOTS.with(|p| p.borrow_mut().replace(Vec::new()).unwrap_or_default())
}

/// Pointer moves over the plot pin the sample time nearest the pointer; leaving the plot drops
/// the pin. Any mouse button down drops it too. Both notify only when the pin changed, so moves
/// inside one sample do not notify. A linked chart passes its pin to the link, which notifies
/// the other members when the link's pin changed.
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
        let others = entity.update(cx, |v, cx| {
            if v.hover.pinned_t_ns != t {
                v.hover.pinned_t_ns = t;
                cx.notify();
            }
            link_pin(v, cx.entity_id(), t)
        });
        notify_all(others, cx);
    });
    if out.pinned {
        let entity = view.clone();
        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
            if phase.bubble() {
                let others = entity.update(cx, |v, cx| {
                    if v.hover.clear() {
                        cx.notify();
                    }
                    link_pin(v, cx.entity_id(), None)
                });
                notify_all(others, cx);
            }
        });
    }
}

/// Passes chart `id`'s pin to its hover link. Returns the members to notify.
fn link_pin(view: &ChartView, id: EntityId, t: Option<u64>) -> SmallVec<[EntityId; 2]> {
    view.hover_link
        .as_ref()
        .map_or_else(SmallVec::new, |link| link.set(id, t))
}

fn notify_all(ids: SmallVec<[EntityId; 2]>, cx: &mut App) {
    for id in ids {
        App::notify(cx, id);
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
    /// Pixel pacing is on: the paint shift is whole physical px (wlf.31).
    snap_dx: bool,
    now: u64,
    metrics: UiMetrics,
    theme: Arc<TlTheme>,
    reduce_motion: bool,
    /// Save power on battery is in effect: no sliding (wlf.32).
    power_saving: bool,
    network_unit: NetFormat,
    /// `performance.curved_lines`.
    curved: bool,
    /// `appearance.color_charts_by_value` (tasklens-j477q).
    value_colors: bool,
}

struct PaintOut {
    moving: bool,
    /// How long the plot takes to move one physical px; the paced frame request's step.
    step_ns: u64,
    /// The plot of this paint, for the hover listeners.
    plot: Bounds<Pixels>,
    /// Paint-time mapping, for the hover listeners.
    map: TimeMap,
    /// A hover pin survived this paint.
    pinned: bool,
    /// The sample grid the hover pin snaps to.
    grid: hover::SampleGrid,
    #[cfg_attr(
        not(any(test, debug_assertions, feature = "test-hooks")),
        allow(dead_code, reason = "read by the bench paint hook only")
    )]
    newest_ns: u64,
}

/// Steps 1 to 8 and 10 of the paint plan. Returns whether the chart wants the next frame.
fn paint_view(
    view: &mut ChartView,
    view_id: EntityId,
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
    // A linked chart draws the cursor another member pinned, in an active window only.
    let mut follow_t_ns = None;
    if !(hitbox.is_some_and(|h| h.is_hovered(window)) && window.is_window_active()) {
        view.hover.clear();
        let others = link_pin(view, view_id, None);
        if !others.is_empty() {
            cx.defer(move |cx| notify_all(others, cx));
        }
        follow_t_ns = view
            .hover_link
            .as_ref()
            .filter(|_| window.is_window_active())
            .and_then(|link| link.follow(view_id));
    }
    let compact_scale = view.compact_scale;
    // Power draw charts (tasklens-1vwxr) and one-series charts on a fixed scale (tasklens-j477q)
    // fill and draw series 0 with a value ramp, while the setting is on.
    let value_ramp = ValueRamp::of_chart(
        view.labels.value_fmt,
        &view.spec,
        &env.theme,
        env.value_colors,
    );
    let ramp_target = view.spec.series.first().map(|s| (s.fill, s.line));
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
        line_set,
        lines: line_state,
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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
            line_state.dirty = true;
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            view_hook_build(&view.build_count);
        }
        // A line set's peaks and highlighted line follow the main geometry (tasklens-o55.57).
        if line_set.is_some() && line_state.dirty {
            let ctx = lines::BuildCtx {
                spec,
                series,
                map: TimeMap {
                    anchor_ns: cache.anchor_ns,
                    right_edge_ns: *built_right_edge_ns,
                    range_ns,
                    plot_width: pw as f32,
                },
                plot_height: ph as f32,
                y_min: cache.y_min,
                y_max: cache.y_max,
                now_boot_ns: if frozen || history.is_some() {
                    right_edge
                } else {
                    now
                },
                curved: env.curved,
            };
            lines::rebuild(line_state, &ctx);
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
    // Pixel pacing draws once per physical px, so the shift moves in whole px (wlf.31).
    let dx = map.dx(pw as f32);
    let dx = if env.snap_dx { dx.round() } else { dx };
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    view.last_dx.set(dx);
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

    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    {
        view.path_colors.borrow_mut().clear();
        view.path_ramps.borrow_mut().clear();
    }

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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        view_hook_quads(&view.last_grid_quads, &view.last_gap_spans, 0, &whole);
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        view.last_right_edge_ns.set(right_edge);
        hover_state.clear();
        return PaintOut {
            moving: false,
            step_ns: 0,
            plot,
            map,
            pinned: false,
            grid: sample_grid,
            newest_ns,
        };
    }

    grid::paint_grid(window, &gp);
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
        // The ramp alpha of a set: series 0's fill (at `ramp::FILL_ALPHA`) and its solid line
        // take the ramp; every other set keeps its colour.
        let ramp_alpha = |set: &PathSet, fill: bool| -> Option<f32> {
            let (s_fill, s_line) = ramp_target?;
            if fill && s_fill == Some(set.color) {
                Some(ramp::FILL_ALPHA * set.color.a)
            } else if !fill && !set.dashed && s_line == set.color {
                Some(set.color.a)
            } else {
                None
            }
        };
        let painted = |window: &mut Window, sets: &[PathSet], alpha: f32, fill: bool| {
            for set in sets {
                if let (Some(r), Some(a)) = (value_ramp.as_ref(), ramp_alpha(set, fill)) {
                    let bands = r.bands(plot, sf, a * alpha);
                    paint_ramped(window, moved(&set.path, offset, sf), &bands, |_band| {
                        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
                        {
                            view.path_paints.set(view.path_paints.get() + 1);
                            view.path_ramps.borrow_mut().push(*_band);
                        }
                    });
                    continue;
                }
                let color = Hsla {
                    a: set.color.a * alpha,
                    ..set.color
                };
                window.paint_path(translated(&set.path, offset, sf), color);
                #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
                {
                    view.path_paints.set(view.path_paints.get() + 1);
                    view.path_colors.borrow_mut().push(color);
                }
            }
        };
        // A line set with a highlighted line dims the others (tasklens-o55.57).
        let dim = if line_set.is_some() && line_state.highlight.is_some() {
            lines::DIM
        } else {
            1.0
        };
        window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
            let live =
                newest_marks && matches!(state, ChartState::Live) && grid::newest_marks_fit(frame);
            if live {
                grid::paint_newest_glow(window, plot, gp.outline_color, env.reduce_motion);
            }
            painted(window, &cache.fills, dim, true);
            painted(window, &cache.stale_fills, STALE_ALPHA * dim, true);
            painted(window, &cache.strokes, dim, false);
            painted(window, &cache.stale_strokes, STALE_ALPHA * dim, false);
            if let Some(set) = line_set.as_ref() {
                if let Some(value) = set.reference {
                    lines::paint_reference(
                        window,
                        value,
                        plot,
                        cache.y_min,
                        cache.y_max,
                        sf,
                        &env.metrics,
                        &env.theme,
                    );
                }
                lines::paint_highlight(window, line_state, offset, sf);
                lines::paint_peaks(
                    window,
                    line_state,
                    spec,
                    offset,
                    sf,
                    &env.metrics,
                    env.theme.graph.chart_bg,
                );
            }
            if live {
                for &(p, color) in &cache.newest {
                    // A ramped line's dot takes the colour of its value.
                    let color = match (value_ramp.as_ref(), ramp_target) {
                        (Some(r), Some((_, line))) if line == color && ph > 0 => Hsla {
                            a: color.a,
                            ..r.at(1.0 - f32::from(p.y) / ph as f32)
                        },
                        _ => color,
                    };
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
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    view.last_boot_marks.set(_boot_marks);

    grid::paint_baseline(window, &gp);
    grid::paint_outline(window, &gp);
    labels::paint(window, cx, text, &label_paint, last_value_text);

    if let Some(set) = line_set.as_ref().filter(|_| spec.hover) {
        let hp = lines::HoverInput {
            plot,
            map,
            dx,
            sf,
            spec,
            value_fmt: labels.value_fmt,
            set,
            series,
            y_min: cache.y_min,
            y_max: cache.y_max,
            metrics: &env.metrics,
            theme: &env.theme,
            network_unit: env.network_unit,
            viewport: window.viewport_size(),
        };
        lines::paint_hover(
            window,
            line_state,
            hover_state.pinned_t_ns,
            follow_t_ns,
            &hp,
        );
    } else if spec.hover {
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
        hover::paint_linked(window, cx, hover_state, &hp, follow_t_ns);
    }

    let one_px_ns = range_ns / u64::from(pw.max(1));
    let moving = has_data
        && sliding
        && newest_ns
            .saturating_add(period_ns.saturating_mul(2))
            .saturating_add(one_px_ns)
            > now;

    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    view.last_right_edge_ns.set(right_edge);

    PaintOut {
        moving,
        step_ns: one_px_ns,
        plot,
        map,
        pinned: hover_state.pinned_t_ns.is_some(),
        grid: sample_grid,
        newest_ns,
    }
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
fn view_hook_build(count: &std::cell::Cell<u32>) {
    count.set(count.get() + 1);
}

/// Records the grid quad count and the dotted spans of this paint.
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
struct QuadCount(u32);

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
impl grid::QuadSink for QuadCount {
    fn quad(&mut self, _: gpui_kit::PaintQuad) {
        self.0 += 1;
    }
}

/// Paints `path` once per ramp band it reaches, each time with its bounds set to that band: the
/// bounds clip the path and span the band's gradient (tasklens-1vwxr). Calls `on_paint` after
/// each paint. The vertices are written while they are moved into place: when the path reaches
/// both bands, each vertex goes only to the bands its triangle reaches ([`BandSplit`]), so a
/// ramped path costs one vertex copy, as a flat one does, plus the triangles that cross the
/// split (tasklens-3uosp).
pub(super) fn paint_ramped<'a>(
    window: &mut Window,
    path: MovedPath<'a, impl Iterator<Item = &'a [PathVertex<Pixels>]>>,
    bands: &[RampBand; 2],
    mut on_paint: impl FnMut(&RampBand),
) {
    let [lower, upper] = bands;
    let (low, up) = (lower.reaches(&path.bounds), upper.reaches(&path.bounds));
    let one = |window: &mut Window, band: &RampBand, path: MovedPath<'a, _>| {
        let mut path = path.collect();
        path.bounds = band.bounds;
        window.paint_path(path, band.background());
    };
    match (low, up) {
        (true, true) => {
            let (below, above) = BandSplit::run(path, lower.bounds, upper.bounds);
            window.paint_path(below, lower.background());
            on_paint(lower);
            window.paint_path(above, upper.background());
            on_paint(upper);
        }
        (true, false) => {
            one(window, lower, path);
            on_paint(lower);
        }
        (false, true) => {
            one(window, upper, path);
            on_paint(upper);
        }
        (false, false) => {}
    }
}

/// Splits a triangle list at the row `split` (window logical px, y down) while its vertices
/// are moved into place: the triangles that reach above `split` go to `above`, the ones that
/// reach below it to `below`. A triangle that crosses `split` goes to both. Each band's bounds
/// clip the rest of the triangle, so the painted pixels are the same as with the whole path in
/// each band. GPUI sends every path vertex with its own colour and bounds (104 bytes), so this
/// halves the upload of a ramped path (tasklens-3uosp).
///
/// A fill column that reaches from above `split` down to a flat bottom below it
/// ([`full_column`]) crosses `split` with both triangles. The upper band gets the column; the
/// lower band gets only its part below `split`, a rectangle, and adjacent columns share one
/// rectangle. A trailing partial triangle goes to `below`.
///
/// Both outputs are reserved once at the full vertex count, so no buffer grows and is copied
/// while it fills. The split takes whole triangles ([`MovedPath::for_each_triangle`]) and holds
/// back one, so a fill column is seen whole without a per-vertex buffer (tasklens-i5sgo).
struct BandSplit {
    split: Pixels,
    below: Vec<PathVertex<Pixels>>,
    above: Vec<PathVertex<Pixels>>,
    /// The last triangle, not yet placed: with the next one it may be a fill column.
    pending: Option<[PathVertex<Pixels>; 3]>,
    run: Option<Run>,
}

impl BandSplit {
    /// The two band paths of `path`, with their bounds set to `below` and `above`.
    fn run<'a>(
        path: MovedPath<'a, impl Iterator<Item = &'a [PathVertex<Pixels>]>>,
        below: Bounds<Pixels>,
        above: Bounds<Pixels>,
    ) -> (Path<Pixels>, Path<Pixels>) {
        let mut s = Self::new(below.top(), path.len);
        let origin = path.bounds.origin;
        let rest = path.for_each_triangle(|t| s.push(t));
        let (b, a) = s.finish(&rest);
        let band = |vertices, bounds| {
            let mut p = Path::new(origin);
            p.vertices = vertices;
            p.bounds = bounds;
            p
        };
        (band(b, below), band(a, above))
    }

    fn new(split: Pixels, len: usize) -> Self {
        Self {
            split,
            below: Vec::with_capacity(len),
            above: Vec::with_capacity(len),
            pending: None,
            run: None,
        }
    }

    /// Takes the next triangle: with the held one it is a whole fill column, or else the held
    /// one is placed and this one is held.
    #[inline]
    fn push(&mut self, t: [PathVertex<Pixels>; 3]) {
        let Some(held) = self.pending.take() else {
            self.pending = Some(t);
            return;
        };
        let split = self.split;
        if let Some((x0, x1, bottom)) = full_column(&held, &t, split) {
            self.above.extend_from_slice(&held);
            self.above.extend_from_slice(&t);
            let st = held[0].st_position;
            match self.run.as_mut() {
                Some(r) if r.right == x0 && r.bottom == bottom && r.st == st => r.right = x1,
                _ => {
                    flush_run(&mut self.below, self.run.take(), split);
                    self.run = Some(Run {
                        left: x0,
                        right: x1,
                        bottom,
                        st,
                    });
                }
            }
            return;
        }
        self.triangle(&held);
        self.pending = Some(t);
    }

    /// Places triangle `t` in the bands it reaches.
    fn triangle(&mut self, t: &[PathVertex<Pixels>; 3]) {
        flush_run(&mut self.below, self.run.take(), self.split);
        let ys = [t[0].xy_position.y, t[1].xy_position.y, t[2].xy_position.y];
        if ys[0].min(ys[1]).min(ys[2]) < self.split {
            self.above.extend_from_slice(t);
        }
        if ys[0].max(ys[1]).max(ys[2]) > self.split {
            self.below.extend_from_slice(t);
        }
    }

    /// The vertices of the lower and the upper band. `rest`, a last partial triangle, goes to
    /// the lower band.
    fn finish(
        mut self,
        rest: &[PathVertex<Pixels>],
    ) -> (Vec<PathVertex<Pixels>>, Vec<PathVertex<Pixels>>) {
        if let Some(held) = self.pending.take() {
            self.triangle(&held);
        }
        flush_run(&mut self.below, self.run.take(), self.split);
        self.below.extend_from_slice(rest);
        (self.below, self.above)
    }
}

/// A run of adjacent full fill columns: their part below the split is one rectangle.
struct Run {
    left: Pixels,
    right: Pixels,
    bottom: Pixels,
    st: Point<f32>,
}

/// Appends the rectangle of `run` (two triangles) to `below`.
fn flush_run(below: &mut Vec<PathVertex<Pixels>>, run: Option<Run>, split: Pixels) {
    let Some(Run {
        left,
        right,
        bottom,
        st,
    }) = run
    else {
        return;
    };
    let corners = [
        (left, split),
        (right, split),
        (right, bottom),
        (left, split),
        (right, bottom),
        (left, bottom),
    ];
    below.extend(corners.into_iter().map(|(x, y)| PathVertex {
        xy_position: point(x, y),
        st_position: st,
        content_mask: Default::default(),
    }));
}

/// `(left x, right x, bottom y)` when the triangles `a` and `b` are one fill segment as
/// `FillMesh::band` emits it, `(t0, t1, b1), (t0, b1, b0)`, with vertical sides, a flat bottom
/// below `split`, a top that is above `split` at both ends and one s and t for all six vertices.
/// Its part below `split` is then the rectangle from `split` to the bottom.
fn full_column(
    a: &[PathVertex<Pixels>; 3],
    b: &[PathVertex<Pixels>; 3],
    split: Pixels,
) -> Option<(Pixels, Pixels, Pixels)> {
    let (t0, t1, b1) = (a[0].xy_position, a[1].xy_position, a[2].xy_position);
    let b0 = b[2].xy_position;
    let st = a[0].st_position;
    let column = b[0].xy_position == t0
        && b[1].xy_position == b1
        && a.iter().chain(b).all(|v| v.st_position == st)
        && t0.x < t1.x
        && b0.x == t0.x
        && b1.x == t1.x
        && b0.y == b1.y
        && t0.y < split
        && t1.y < split
        && b1.y > split;
    column.then_some((t0.x, t1.x, b1.y))
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
    moved(path, offset, scale_factor).collect()
}

/// [`translated`] before the vertices are written.
pub(super) fn moved<'a>(
    path: &'a Path<Pixels>,
    offset: Point<Pixels>,
    scale_factor: f32,
) -> MovedPath<'a, std::iter::Once<&'a [PathVertex<Pixels>]>> {
    MovedPath::new(
        path.bounds,
        path.vertices.len(),
        offset,
        scale_factor,
        std::iter::once(&path.vertices[..]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(path: &mut Path<Pixels>, ys: [f32; 3]) {
        for (k, y) in ys.into_iter().enumerate() {
            path.vertices.push(PathVertex {
                xy_position: point(px(k as f32), px(y)),
                st_position: point(0.0, 1.0),
                content_mask: Default::default(),
            });
        }
    }

    /// Splits `path` at `at` as `BandSplit` does: returns the upper band and keeps the lower
    /// band in `path`.
    fn split_triangles(path: &mut Path<Pixels>, at: f32) -> Path<Pixels> {
        let src = std::mem::take(&mut path.vertices);
        let mut s = BandSplit::new(px(at), src.len());
        let whole = MovedPath::new(
            path.bounds,
            src.len(),
            point(px(0.), px(0.)),
            1.0,
            std::iter::once(&src[..]),
        );
        let rest = whole.for_each_triangle(|t| s.push(t));
        let (below, above) = s.finish(&rest);
        path.vertices = below;
        let mut out = Path::new(point(px(0.), px(0.)));
        out.vertices = above;
        out
    }

    /// The band paths are the moved vertices, with the band bounds; the split is in window
    /// logical px, after the move (tasklens-3uosp).
    #[test]
    fn band_split_moves_then_splits() {
        let mut path = Path::new(point(px(0.), px(0.)));
        tri(&mut path, [0., 10., 20.]); // logical 20 to 30: above 45
        tri(&mut path, [60., 70., 80.]); // logical 50 to 60: below
        tri(&mut path, [40., 50., 60.]); // logical 40 to 50: crosses
        path.bounds = Bounds::new(point(px(0.), px(0.)), size(px(2.), px(80.)));
        let offset = point(px(100.), px(20.));
        let lower = Bounds::new(point(px(100.), px(45.)), size(px(50.), px(25.)));
        let upper = Bounds::new(point(px(100.), px(20.)), size(px(50.), px(25.)));
        let (below, above) = BandSplit::run(moved(&path, offset, 2.0), lower, upper);
        assert_eq!(below.bounds, lower);
        assert_eq!(above.bounds, upper);
        assert_eq!(rows(&above), vec![[20., 25., 30.], [40., 45., 50.]]);
        assert_eq!(rows(&below), vec![[50., 55., 60.], [40., 45., 50.]]);
        assert_eq!(f32::from(above.vertices[1].xy_position.x), 100.5);
        let whole = translated(&path, offset, 2.0);
        assert_eq!(whole.vertices.len(), 9);
        assert_eq!(whole.bounds.size.height, px(40.));
    }

    fn rows(path: &Path<Pixels>) -> Vec<[f32; 3]> {
        path.vertices
            .chunks(3)
            .map(|t| {
                let y = |i: usize| f32::from(t[i].xy_position.y);
                [y(0), y(1), y(2)]
            })
            .collect()
    }

    /// Above-only triangles go to the upper path, below-only ones stay, a crossing one is in
    /// both, and a triangle that only touches the split is on its own side (tasklens-3uosp).
    #[test]
    fn split_triangles_by_band() {
        let mut path = Path::new(point(px(0.), px(0.)));
        tri(&mut path, [0., 10., 20.]); // above
        tri(&mut path, [60., 70., 80.]); // below
        tri(&mut path, [40., 50., 60.]); // crosses
        tri(&mut path, [30., 50., 50.]); // touches from above
        tri(&mut path, [50., 50., 90.]); // touches from below
        let above = split_triangles(&mut path, 50.);
        assert_eq!(
            rows(&above),
            vec![[0., 10., 20.], [40., 50., 60.], [30., 50., 50.]]
        );
        assert_eq!(
            rows(&path),
            vec![[60., 70., 80.], [40., 50., 60.], [50., 50., 90.]]
        );
    }

    /// No triangle is lost or doubled when none crosses; a trailing partial triangle stays.
    #[test]
    fn split_triangles_keeps_counts() {
        let mut path = Path::new(point(px(0.), px(0.)));
        for i in 0..10 {
            let y = if i % 2 == 0 { 10. } else { 90. };
            tri(&mut path, [y, y + 1., y + 2.]);
        }
        path.vertices.push(path.vertices[0].clone());
        let above = split_triangles(&mut path, 50.);
        assert_eq!(above.vertices.len(), 15);
        assert_eq!(path.vertices.len(), 16);
        assert!(rows(&above).iter().all(|t| t[0] == 10.));
        assert!(
            path.vertices[..15]
                .iter()
                .all(|v| f32::from(v.xy_position.y) >= 90.)
        );
    }

    /// One fill segment as `FillMesh::band` emits it, top `t0` to `t1`, bottom at `b`.
    fn column(path: &mut Path<Pixels>, x0: f32, x1: f32, t0: f32, t1: f32, b: f32) {
        let v = |x: f32, y: f32| PathVertex {
            xy_position: point(px(x), px(y)),
            st_position: point(0.0, 1.0),
            content_mask: Default::default(),
        };
        for (x, y) in [(x0, t0), (x1, t1), (x1, b), (x0, t0), (x1, b), (x0, b)] {
            path.vertices.push(v(x, y));
        }
    }

    /// Adjacent fill columns that cross the split give the lower band one rectangle; a column
    /// whose top crosses the split inside it, and a column below the split, keep their
    /// triangles (tasklens-3uosp).
    #[test]
    fn split_triangles_merges_full_columns() {
        let mut path = Path::new(point(px(0.), px(0.)));
        column(&mut path, 0., 1., 20., 30., 100.);
        column(&mut path, 1., 2., 30., 10., 100.);
        column(&mut path, 2., 3., 10., 40., 100.);
        column(&mut path, 3., 4., 40., 60., 100.); // top crosses the split
        column(&mut path, 4., 5., 60., 70., 100.); // below the split
        column(&mut path, 5., 6., 30., 20., 100.); // a new run
        let above = split_triangles(&mut path, 50.);
        // The four columns that reach above, all six vertices each.
        assert_eq!(above.vertices.len(), 3 * 6 + 6 + 6);
        let tris = rows(&path);
        let xs: Vec<[f32; 3]> = path
            .vertices
            .chunks(3)
            .map(|t| {
                let x = |i: usize| f32::from(t[i].xy_position.x);
                [x(0), x(1), x(2)]
            })
            .collect();
        // The first run's rectangle, the crossing column (2 triangles), the lower column (2),
        // then the second run's rectangle.
        assert_eq!(tris.len(), 2 + 2 + 2 + 2);
        assert_eq!(&tris[0..2], &[[50., 50., 100.], [50., 100., 100.]]);
        assert_eq!(&xs[0..2], &[[0., 3., 3.], [0., 3., 0.]]);
        assert_eq!(&tris[2..4], &[[40., 60., 100.], [40., 100., 100.]]);
        assert_eq!(&tris[4..6], &[[60., 70., 100.], [60., 100., 100.]]);
        assert_eq!(&tris[6..8], &[[50., 50., 100.], [50., 100., 100.]]);
        assert_eq!(&xs[6..8], &[[5., 6., 6.], [5., 6., 5.]]);
    }
}
