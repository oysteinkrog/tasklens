//! `HistoryScrubber`: the 120 px timeline strip of History mode (UI-PRC-032, PROC-089).
//!
//! The plot (y 0..100) draws the stacked top-3 plus "other" area of the active metric, the
//! system-total line, one line per selected process, hatched gap bands and the ADR-009 grid. The
//! axis (y 100) carries time labels at the grid lines, the scrub handle and the range bounds.
//!
//! Geometry is built in strip-local physical px and kept in [`ScrubberCache`] until the
//! [`GeometryKey`] changes: view version, size, scale, strip window, metric, words or style.
//! Every other frame clones the cached paths into place, paints the handle, range and hover
//! hairline as quads, and paints the label lines shaped at the last build. A frame where only the
//! hover hairline or the handle moved builds no path and shapes no text.
//!
//! Pointer interaction (E30-022): click or drag scrubs, the wheel zooms around the pointer,
//! Shift+wheel or a middle-button drag pans, Shift+drag selects a range, a double-click returns
//! to live. Listeners are registered in `paint` and update the page. A hover move notifies only
//! the cached `StripView`, and only when the
//! hover x changed. Bound grips are grabbed only on the axis row, so a plot click always scrubs.

use core::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{
    App, Bounds, ContentMask, Context, CursorStyle, Element, ElementId, Empty, Entity, EntityId,
    FillOptions, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path,
    PathBuilder, PathStyle, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    Style, WeakEntity, Window, fill, point, px, relative, size,
};
use smallvec::SmallVec;
use tl_model::history::{HistoryMetric, HistoryView};
use tl_model::time::TimeNs;

use super::actions::{HistoryLive, HistoryScrub, HistorySetRange};
use super::{HistoryPage, HistoryUiState, MAX_SERIES};
use crate::chart::GridStyle;
use crate::chart::geometry::TimeMap;
use crate::chart::grid::{self, GridPaint};
use crate::chart::labels::{CachedLine, LineStyle, paint_line};
use crate::components::focus::TlFocusRing;
use crate::theme::{GraphPalette, TlTheme, UiMetrics};

const NS_PER_SEC: u64 = 1_000_000_000;

/// Heights in logical px at 100% scale (UI-PRC-032).
const STRIP_H: f32 = 120.0;
const PLOT_H: f32 = 100.0;
const LABEL_TOP: f32 = 104.0;
const LABEL_H: f32 = 14.0;
/// Scrub grip: a 10 × 10 diamond centred on the axis line.
const GRIP: f32 = 10.0;
/// The scrub grip while it is dragged.
const GRIP_DRAG: f32 = 12.0;
/// A press this close (logical px at 100% scale) to a range bound grabs the bound.
const BOUND_GRAB: f32 = 4.0;
/// Wheel zoom factor per notch.
const ZOOM_STEP: f64 = 1.25;
/// The shortest zoomed window.
const MIN_VIEW_NS: u64 = 60 * NS_PER_SEC;
/// Shift+wheel pans this share of the view per notch.
const PAN_NOTCH: f64 = 0.10;
/// A pixel scroll delta counts as one notch per this many px.
const PIXELS_PER_NOTCH: f32 = 20.0;
/// Range bound grips: 6 wide, 10 high, centred on the axis line.
const BOUND_W: f32 = 6.0;
const BOUND_H: f32 = 10.0;
/// Single-point marker side.
const MARKER: f32 = 4.0;
/// Distance between the hatch lines of a gap band.
const HATCH_STEP: f32 = 6.0;

const GAP_ALPHA: f32 = 0.30;
const RANGE_ALPHA: f32 = 0.35;
/// Hue step between selected-process series colours (UI-PRC-035).
const SERIES_HUE_STEP: f32 = 45.0 / 360.0;
/// At most this many label slots stay shaped.
const LABEL_POOL: usize = 32;

/// `-M:SS` under an hour, `-H:MM:SS` from an hour on, for an offset before now (R-169 OI-8).
/// Rounds to whole seconds. Appends to `out`.
pub fn time_label(offset_ns: u64, out: &mut String) {
    let secs = offset_ns.saturating_add(NS_PER_SEC / 2) / NS_PER_SEC;
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let _ = if h > 0 {
        write!(out, "-{h}:{m:02}:{s:02}")
    } else {
        write!(out, "-{m}:{s:02}")
    };
}

/// "45s", "2m 05s", "1h 02m" for a duration. Rounds to whole seconds. Appends to `out`.
pub fn span_label(d_ns: u64, out: &mut String) {
    let secs = d_ns.saturating_add(NS_PER_SEC / 2) / NS_PER_SEC;
    let _ = if secs < 60 {
        write!(out, "{secs}s")
    } else if secs < 3600 {
        write!(out, "{}m {:02}s", secs / 60, secs % 60)
    } else {
        write!(out, "{}h {:02}m", secs / 3600, secs % 3600 / 60)
    };
}

/// Indices of the labels to draw, given each label's left edge and width in x order. While any
/// two kept neighbours overlap, every second kept label is dropped (the first stays).
pub fn drop_overlapping(labels: &[(f32, f32)]) -> SmallVec<[usize; 16]> {
    let mut kept: SmallVec<[usize; 16]> = (0..labels.len()).collect();
    let overlaps = |kept: &[usize]| {
        kept.windows(2).any(|w| {
            let (x, width) = labels[w[0]];
            x + width > labels[w[1]].0
        })
    };
    while kept.len() > 1 && overlaps(&kept) {
        let mut i = 0;
        kept.retain(|_| {
            i += 1;
            i % 2 == 1
        });
    }
    kept
}

/// The time at strip-local `x` of a plot `width` wide showing `view`. `x` is clamped to the plot.
pub fn x_to_t(x: f32, width: f32, view: (TimeNs, TimeNs)) -> TimeNs {
    let frac = if width > 0.0 {
        f64::from((x / width).clamp(0.0, 1.0))
    } else {
        0.0
    };
    view.0 + (frac * view.1.saturating_sub(view.0) as f64).round() as TimeNs
}

/// `t` rounded to a whole second and clamped into `span`.
pub fn snap_s(t: TimeNs, span: (TimeNs, TimeNs)) -> TimeNs {
    let s = (t.saturating_add(NS_PER_SEC / 2) / NS_PER_SEC).saturating_mul(NS_PER_SEC);
    s.clamp(span.0, span.1.max(span.0))
}

/// Moves a window of `width` to start at `start`, kept inside `span`.
fn place(start: i128, width: TimeNs, span: (TimeNs, TimeNs)) -> (TimeNs, TimeNs) {
    let last_start = i128::from(span.1) - i128::from(width);
    let start = start.min(last_start).max(i128::from(span.0)) as TimeNs;
    (start, start + width)
}

/// Zooms `view` by `notches` (positive zooms in, 1.25 per notch) around `cursor`, which keeps its
/// place in the view. The span of the result is clamped to 60 s (or all of `span` when shorter)
/// up to all of `span`, and the result stays inside `span`.
pub fn zoom(
    view: (TimeNs, TimeNs),
    cursor: TimeNs,
    notches: f32,
    span: (TimeNs, TimeNs),
) -> (TimeNs, TimeNs) {
    let old = view.1.saturating_sub(view.0);
    let full = span.1.saturating_sub(span.0);
    if old == 0 || full == 0 {
        return view;
    }
    let min = MIN_VIEW_NS.min(full) as f64;
    let width = (old as f64 / ZOOM_STEP.powf(f64::from(notches)))
        .clamp(min, full as f64)
        .round() as TimeNs;
    let frac = ((i128::from(cursor) - i128::from(view.0)) as f64 / old as f64).clamp(0.0, 1.0);
    let start = i128::from(cursor) - (frac * width as f64).round() as i128;
    place(start, width, span)
}

/// Shifts `view` by `delta_ns` (positive is newer), keeping its width and staying inside `span`.
pub fn pan(view: (TimeNs, TimeNs), delta_ns: i64, span: (TimeNs, TimeNs)) -> (TimeNs, TimeNs) {
    let width = view
        .1
        .saturating_sub(view.0)
        .min(span.1.saturating_sub(span.0));
    place(i128::from(view.0) + i128::from(delta_ns), width, span)
}

/// Fill and line colour of `metric` (UI-PRC-035).
fn metric_colors(g: &GraphPalette, metric: HistoryMetric) -> (Hsla, Hsla) {
    match metric {
        HistoryMetric::Cpu => (g.cpu_fill, g.cpu_line),
        HistoryMetric::Memory => (g.mem_fill, g.mem_line),
        HistoryMetric::Disk => (g.disk_fill, g.disk_line),
        HistoryMetric::Network => (g.net_fill, g.net_line),
        HistoryMetric::Gpu => (g.gpu_fill, g.gpu_line),
    }
}

/// Colour of selected-process series `k`: `accent` with its hue turned by `k × 45°`.
pub fn series_color(accent: Hsla, k: usize) -> Hsla {
    Hsla {
        h: (accent.h + k as f32 * SERIES_HUE_STEP).fract(),
        ..accent
    }
}

/// Everything that forces a geometry rebuild when it changes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GeometryKey {
    version: u64,
    query_id: u64,
    physical_width: u32,
    physical_height: u32,
    scale_factor_bits: u32,
    ui_scale_bits: u32,
    view_range: (TimeNs, TimeNs),
    metric: HistoryMetric,
    strings_version: u64,
    style_version: u64,
}

/// One built path and its colour.
struct PathSet {
    color: Hsla,
    path: Path<Pixels>,
}

/// One shaped label line and the text it is looked up by.
#[derive(Default)]
struct LabelSlot {
    key: String,
    line: CachedLine,
    /// Build stamp of the last build that used it.
    used: u64,
}

/// One label to draw: strip-local physical left edge and its slot.
#[derive(Clone, Copy)]
struct PlacedLabel {
    x: f32,
    slot: usize,
}

/// The pointer gesture in progress on the strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    /// The scrub handle follows the pointer.
    Scrub,
    /// Shift+drag from `anchor`.
    Range { anchor: TimeNs },
    /// One range bound follows the pointer; the other stays at `fixed`.
    Bound { fixed: TimeNs },
    /// Middle-button drag; `last_x` is the previous pointer x in logical px.
    Pan { last_x: f32 },
}

/// The strip's built geometry, shaped labels and hover state, kept on the page between frames.
#[derive(Default)]
pub struct ScrubberCache {
    key: Option<GeometryKey>,
    /// Stacked layers from the bottom: top 1, top 1 + 2, top 1 + 2 + 3, all.
    fills: SmallVec<[PathSet; 4]>,
    /// System total, then one per selected process.
    strokes: SmallVec<[PathSet; 9]>,
    hatch: Option<PathSet>,
    /// Gap bands, strip-local physical x.
    gaps: SmallVec<[Range<f32>; 4]>,
    /// Single-point markers, strip-local physical px, with their colour.
    markers: SmallVec<[(Point<f32>, Hsla); 9]>,
    /// The scrub grip, centred on the origin.
    diamond: Option<Path<Pixels>>,
    /// The scrub grip while dragged.
    diamond_drag: Option<Path<Pixels>>,
    labels: SmallVec<[PlacedLabel; 16]>,
    pool: Vec<LabelSlot>,
    stamp: u64,
    text: String,
    /// Pointer x relative to the strip's left edge while hovered. `None` while dragging and
    /// after a wheel event until the pointer moves.
    pub hover_x: Option<Pixels>,
    /// The gesture in progress.
    pub drag: Option<Drag>,
    /// Geometry builds over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub build_count: u32,
    /// Gap bands painted by the last paint, window x in logical px.
    #[cfg(any(test, feature = "test-hooks"))]
    pub painted_gaps: SmallVec<[Range<Pixels>; 4]>,
    /// The plot of the last paint, window logical px.
    #[cfg(any(test, feature = "test-hooks"))]
    pub plot_bounds: Bounds<Pixels>,
    /// True when the last paint drew the focus ring.
    #[cfg(any(test, feature = "test-hooks"))]
    pub focus_ring_painted: bool,
}

impl ScrubberCache {
    /// The texts of the labels the last build placed, in x order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn label_texts(&self) -> Vec<String> {
        self.labels
            .iter()
            .map(|l| self.pool[l.slot].key.clone())
            .collect()
    }

    /// Index of the pool slot shaped from `self.text`, shaping it into a free slot when no slot
    /// holds that text.
    fn label_slot(&mut self, window: &mut Window, style: LineStyle) -> usize {
        let stamp = self.stamp;
        let ix = match self.pool.iter().position(|s| s.key == self.text) {
            Some(i) => i,
            None if self.pool.len() < LABEL_POOL => {
                self.pool.push(LabelSlot::default());
                self.pool.len() - 1
            }
            None => self
                .pool
                .iter()
                .enumerate()
                .filter(|(_, s)| s.used != stamp)
                .min_by_key(|(_, s)| s.used)
                .map_or(0, |(i, _)| i),
        };
        let slot = &mut self.pool[ix];
        if slot.key != self.text {
            slot.key.clear();
            slot.key.push_str(&self.text);
        }
        slot.line.sync(window, &self.text, style);
        slot.used = stamp;
        ix
    }
}

/// The strip as its own cached view, so a hover move repaints it without rendering the page.
///
/// The page owns this view, so the view holds the page weakly. A strong handle would make a
/// cycle and keep the page alive after its window closes.
pub struct StripView {
    page: WeakEntity<HistoryPage>,
}

impl StripView {
    pub fn new(page: WeakEntity<HistoryPage>) -> Self {
        Self { page }
    }
}

impl Render for StripView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.page.upgrade() {
            Some(page) => HistoryScrubber::new(page).into_any_element(),
            None => Empty.into_any_element(),
        }
    }
}

/// The strip element. Reads the page's view and state and paints into its `ScrubberCache`.
pub struct HistoryScrubber {
    page: Entity<HistoryPage>,
}

impl HistoryScrubber {
    pub fn new(page: Entity<HistoryPage>) -> Self {
        Self { page }
    }
}

impl IntoElement for HistoryScrubber {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HistoryScrubber {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    /// Full width, 120 px high.
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let scale = cx.global::<UiMetrics>().ui_scale;
        let style = Style {
            size: size(relative(1.).into(), px(STRIP_H * scale).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    /// One hitbox over the whole strip.
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
        let scale = cx.global::<UiMetrics>().ui_scale;
        let focus = self.page.read(cx).strip_focus.clone();
        let geo = Geo {
            plot: Bounds::new(bounds.origin, size(bounds.size.width, px(PLOT_H * scale))),
            scale,
            hitbox: hitbox.clone(),
            focus,
            strip: self
                .page
                .read(cx)
                .strip_view
                .as_ref()
                .map(Entity::entity_id),
        };
        register_events(&self.page, &geo, window);
        let env = Env {
            sf: window.scale_factor(),
            metrics: *cx.global::<UiMetrics>(),
            theme: crate::theme::current(cx).clone(),
        };
        self.page.update(cx, |page, cx| {
            let HistoryPage {
                state,
                view,
                words,
                strings_version,
                scrubber,
                ..
            } = page;
            let input = Input {
                focused: geo.focus.is_focused(window),
                view: view.as_deref(),
                state,
                now_word: &words.now,
                strings_version: *strings_version,
            };
            paint_strip(scrubber, &input, &env, bounds, window, cx);
        });
    }
}

/// What the pointer listeners need, captured at paint time.
#[derive(Clone)]
struct Geo {
    plot: Bounds<Pixels>,
    scale: f32,
    hitbox: Hitbox,
    focus: FocusHandle,
    /// The strip view; hover changes notify it instead of the page.
    strip: Option<EntityId>,
}

impl Geo {
    /// True when `p` is level with the bound grips on the axis line.
    fn on_grip_row(&self, p: Point<Pixels>) -> bool {
        let dy = f32::from(p.y - self.plot.bottom()).abs();
        dy <= BOUND_H / 2.0 * self.scale
    }

    /// Pointer x relative to the plot's left edge, logical px.
    fn x(&self, p: Point<Pixels>) -> f32 {
        f32::from(p.x - self.plot.left())
    }

    fn width(&self) -> f32 {
        f32::from(self.plot.size.width)
    }
}

/// The page's view window and retained span, when both can be used.
fn view_and_span(page: &HistoryPage) -> Option<((TimeNs, TimeNs), (TimeNs, TimeNs))> {
    let span = page.view.as_ref().and_then(|v| v.span)?;
    let view = page.state.view_range;
    (view.1 > view.0).then_some((view, span))
}

/// Dispatches `HistorySetRange` for two ends in any order when they differ.
fn dispatch_range(a: TimeNs, b: TimeNs, window: &mut Window, cx: &mut App) {
    let (from, to) = (a.min(b), a.max(b));
    if from < to {
        window.dispatch_action(Box::new(HistorySetRange { from, to }), cx);
    }
}

fn on_mouse_down(
    page: &Entity<HistoryPage>,
    geo: &Geo,
    e: &MouseDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    if !geo.hitbox.is_hovered(window) {
        return;
    }
    let width = geo.width();
    let x = geo.x(e.position);
    let on_grip_row = geo.on_grip_row(e.position);
    match e.button {
        MouseButton::Left => {
            window.focus(&geo.focus, cx);
            if e.click_count >= 2 {
                page.update(cx, |p, _| p.scrubber.drag = None);
                window.dispatch_action(Box::new(HistoryLive), cx);
                return;
            }
            page.update(cx, |p, cx| {
                let Some((view, span)) = view_and_span(p) else {
                    return;
                };
                let t = snap_s(x_to_t(x, width, view), span);
                let to_x = |t: TimeNs| {
                    (t as f64 - view.0 as f64) / (view.1 - view.0) as f64 * f64::from(width)
                };
                let grab = BOUND_GRAB * geo.scale;
                let bound = p.state.range.filter(|_| on_grip_row).and_then(|(a, b)| {
                    if (to_x(a) - f64::from(x)).abs() <= f64::from(grab) {
                        Some(b)
                    } else if (to_x(b) - f64::from(x)).abs() <= f64::from(grab) {
                        Some(a)
                    } else {
                        None
                    }
                });
                p.scrubber.hover_x = None;
                if let Some(fixed) = bound {
                    p.scrubber.drag = Some(Drag::Bound { fixed });
                } else if e.modifiers.shift {
                    p.scrubber.drag = Some(Drag::Range { anchor: t });
                } else {
                    p.scrubber.drag = Some(Drag::Scrub);
                    p.set_range(None, cx);
                    window.dispatch_action(Box::new(HistoryScrub { t }), cx);
                }
                cx.notify();
            });
        }
        MouseButton::Middle => page.update(cx, |p, cx| {
            if view_and_span(p).is_some() {
                p.scrubber.hover_x = None;
                p.scrubber.drag = Some(Drag::Pan { last_x: x });
                cx.notify();
            }
        }),
        _ => {}
    }
}

fn on_mouse_move(
    page: &Entity<HistoryPage>,
    geo: &Geo,
    e: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let hovered = geo.hitbox.is_hovered(window);
    let hover = hovered.then(|| e.position.x - geo.plot.left());
    let hover_moved = page.update(cx, |p, _| {
        if p.scrubber.drag.is_some() {
            return None;
        }
        let changed = p.scrubber.hover_x != hover;
        p.scrubber.hover_x = hover;
        Some(changed)
    });
    match (hover_moved, geo.strip) {
        // Only the strip repaints for a hover move, not the page.
        (Some(true), Some(id)) => cx.notify(id),
        (Some(true), None) => page.update(cx, |_, cx| cx.notify()),
        (Some(false), _) => {}
        (None, _) => {}
    }
    if hover_moved.is_some() {
        return;
    }
    page.update(cx, |p, cx| {
        let Some(drag) = p.scrubber.drag else {
            return;
        };
        if e.pressed_button.is_none() {
            // The button was released outside the window.
            p.scrubber.drag = None;
            cx.notify();
            return;
        }
        let Some((view, span)) = view_and_span(p) else {
            return;
        };
        let width = geo.width();
        let x = geo.x(e.position);
        let t = snap_s(x_to_t(x, width, view), span);
        match drag {
            Drag::Scrub => window.dispatch_action(Box::new(HistoryScrub { t }), cx),
            Drag::Range { anchor: fixed } | Drag::Bound { fixed } => {
                dispatch_range(fixed, t, window, cx);
            }
            Drag::Pan { last_x } if width > 0.0 => {
                let delta = -f64::from((x - last_x) / width) * (view.1 - view.0) as f64;
                p.scrubber.drag = Some(Drag::Pan { last_x: x });
                p.set_view_range(pan(view, delta as i64, span), cx);
            }
            Drag::Pan { .. } => {}
        }
        cx.notify();
    });
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
        let next = if e.modifiers.shift {
            let delta = -f64::from(notches) * PAN_NOTCH * (view.1 - view.0) as f64;
            pan(view, delta as i64, span)
        } else {
            zoom(view, x_to_t(x, width, view), notches, span)
        };
        p.scrubber.hover_x = None;
        p.set_view_range(next, cx);
        cx.notify();
    });
    cx.stop_propagation();
}

/// Registers the strip's pointer listeners for this frame.
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
                if p.scrubber.drag.take().is_some() {
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

/// Per-paint values read from the window and the app.
struct Env {
    sf: f32,
    metrics: UiMetrics,
    theme: Arc<TlTheme>,
}

/// What the page hands the strip.
struct Input<'a> {
    /// The strip holds keyboard focus.
    focused: bool,
    view: Option<&'a HistoryView>,
    state: &'a HistoryUiState,
    now_word: &'a SharedString,
    strings_version: u64,
}

impl Input<'_> {
    /// True once the view holds a frame.
    fn has_frames(&self) -> bool {
        self.view.is_some_and(|v| v.span.is_some())
    }

    /// The newest retained second, which time labels count back from.
    fn now(&self) -> TimeNs {
        self.view
            .and_then(|v| v.span)
            .map_or(self.state.view_range.1, |s| s.1)
    }

    fn metric(&self) -> HistoryMetric {
        self.view
            .and_then(|v| v.series.first())
            .map_or(self.state.metric, |s| s.metric)
    }
}

/// Strip-local physical px: time and value to x and y.
struct Mapper {
    map: TimeMap,
    plot_h: f32,
    y_max: f32,
}

impl Mapper {
    fn x(&self, t: TimeNs) -> f32 {
        self.map.x_local(t)
    }

    fn y(&self, v: f32) -> f32 {
        self.plot_h - (v / self.y_max).clamp(0.0, 1.0) * self.plot_h
    }
}

fn paint_strip(
    cache: &mut ScrubberCache,
    input: &Input,
    env: &Env,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let sf = env.sf;
    let scale = env.metrics.ui_scale;
    let theme = &env.theme;
    let plot = Bounds::new(bounds.origin, size(bounds.size.width, px(PLOT_H * scale)));
    let pw = (f32::from(plot.size.width) * sf).round().max(0.0) as u32;
    let ph = (f32::from(plot.size.height) * sf).round().max(0.0) as u32;
    let (v0, v1) = input.state.view_range;
    let map = TimeMap {
        anchor_ns: v0,
        right_edge_ns: v1,
        range_ns: v1.saturating_sub(v0),
        plot_width: pw as f32,
    };
    let has_frames = input.has_frames();
    #[cfg(any(test, feature = "test-hooks"))]
    {
        cache.plot_bounds = plot;
    }

    let key = GeometryKey {
        version: input.view.map_or(0, |v| v.header.version),
        query_id: input.view.map_or(0, |v| v.query_id),
        physical_width: pw,
        physical_height: ph,
        scale_factor_bits: sf.to_bits(),
        ui_scale_bits: scale.to_bits(),
        view_range: (v0, v1),
        metric: input.metric(),
        strings_version: input.strings_version,
        style_version: env.metrics.style_version,
    };
    if cache.key != Some(key) {
        cache.key = Some(key);
        build(cache, input, env, &map, ph as f32, window);
        #[cfg(any(test, feature = "test-hooks"))]
        {
            cache.build_count += 1;
        }
    }

    grid::paint_background(window, plot, theme.list_bg, px(0.));
    let gp = GridPaint {
        plot,
        box_bounds: bounds,
        map: &map,
        dx: px(0.),
        scale_factor: sf,
        theme,
        style: GridStyle {
            lines: true,
            ..GridStyle::default()
        },
        outline_color: theme.divider,
        radius: px(0.),
    };
    grid::paint_grid(window, &gp);

    let hairline = px(1.0 / sf);
    let to_x = |x_phys: f32| plot.left() + px(x_phys / sf);
    let axis_y = plot.bottom();
    let range = input.state.range.filter(|_| has_frames);

    #[cfg(any(test, feature = "test-hooks"))]
    cache.painted_gaps.clear();

    if let Some((a, b)) = range {
        let (xa, xb) = (to_x(map.x_local(a)), to_x(map.x_local(b)));
        let (l, r) = (xa.min(xb).max(plot.left()), xa.max(xb).min(plot.right()));
        if l < r {
            window.paint_quad(fill(
                Bounds::new(point(l, plot.top()), size(r - l, plot.size.height)),
                theme.row_selected.alpha(RANGE_ALPHA),
            ));
        }
    }

    if has_frames {
        let origin = plot.origin;
        let inv = 1.0 / sf;
        window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
            for set in cache.fills.iter().chain(&cache.strokes).chain(&cache.hatch) {
                window.paint_path(placed(&set.path, origin, inv), set.color);
            }
            let side = px(MARKER * scale);
            for &(p, color) in &cache.markers {
                let c = point(to_x(p.x), plot.top() + px(p.y / sf));
                window.paint_quad(fill(
                    Bounds::new(c - point(side / 2., side / 2.), size(side, side)),
                    color,
                ));
            }
        });
        #[cfg(any(test, feature = "test-hooks"))]
        cache.painted_gaps.extend(
            cache
                .gaps
                .iter()
                .map(|g| to_x(g.start).max(plot.left())..to_x(g.end).min(plot.right())),
        );
    }

    // Axis line and labels.
    window.paint_quad(fill(
        Bounds::new(point(plot.left(), axis_y), size(plot.size.width, hairline)),
        theme.divider,
    ));
    let label_y = bounds.top() + px(LABEL_TOP * scale);
    for l in &cache.labels {
        let origin = point(to_x(l.x), label_y);
        paint_line(
            window,
            cx,
            &cache.pool[l.slot].line,
            origin,
            px(LABEL_H * scale),
        );
    }

    if let Some((a, b)) = range {
        let grip = size(px(BOUND_W * scale), px(BOUND_H * scale));
        for t in [a, b] {
            let x = to_x(map.x_local(t));
            if x < plot.left() || x > plot.right() {
                continue;
            }
            window.paint_quad(fill(
                Bounds::new(point(x, plot.top()), size(hairline, plot.size.height)),
                theme.accent,
            ));
            window.paint_quad(fill(
                Bounds::new(point(x - grip.width / 2., axis_y - grip.height / 2.), grip),
                theme.accent,
            ));
        }
    }

    // Handle: at the scrub time, or a plain line at the right edge while live.
    match input.state.scrub.filter(|_| has_frames) {
        Some(t) => {
            let x = to_x(map.x_local(t));
            if x >= plot.left() && x <= plot.right() {
                window.paint_quad(fill(
                    Bounds::new(point(x, plot.top()), size(hairline, plot.size.height)),
                    theme.accent,
                ));
                let grip = if cache.drag == Some(Drag::Scrub) {
                    &cache.diamond_drag
                } else {
                    &cache.diamond
                };
                if let Some(d) = grip {
                    window.paint_path(placed(d, point(x, axis_y), 1.0 / sf), theme.accent);
                }
            }
        }
        None if has_frames => {
            window.paint_quad(fill(
                Bounds::new(
                    point(plot.right() - hairline, plot.top()),
                    size(hairline, plot.size.height),
                ),
                theme.text_secondary,
            ));
        }
        None => {}
    }

    if let Some(hx) = cache.hover_x {
        let x = plot.left() + hx;
        if x >= plot.left() && x < plot.right() {
            window.paint_quad(fill(
                Bounds::new(point(x, plot.top()), size(hairline, plot.size.height)),
                theme.text_secondary,
            ));
        }
    }

    match cache.drag {
        Some(Drag::Scrub | Drag::Bound { .. }) => {
            window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
        }
        Some(Drag::Pan { .. }) => window.set_window_cursor_style(CursorStyle::ClosedHand),
        _ => {}
    }

    // Focus ring: 1 px `accent` inside the strip.
    let ring = TlFocusRing::show(input.focused, window);
    #[cfg(any(test, feature = "test-hooks"))]
    {
        cache.focus_ring_painted = ring;
    }
    if ring {
        let w = px(1.0);
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        for edge in [
            Bounds::new(bounds.origin, size(bw, w)),
            Bounds::new(point(bounds.left(), bounds.bottom() - w), size(bw, w)),
            Bounds::new(bounds.origin, size(w, bh)),
            Bounds::new(point(bounds.right() - w, bounds.top()), size(w, bh)),
        ] {
            window.paint_quad(fill(edge, theme.accent));
        }
    }
}

/// Rebuilds every path, marker, gap band and label of the strip into `cache`.
fn build(
    cache: &mut ScrubberCache,
    input: &Input,
    env: &Env,
    map: &TimeMap,
    plot_h: f32,
    window: &mut Window,
) {
    let sf = env.sf;
    let scale = env.metrics.ui_scale;
    let theme = &env.theme;
    cache.fills.clear();
    cache.strokes.clear();
    cache.hatch = None;
    cache.gaps.clear();
    cache.markers.clear();
    cache.labels.clear();
    cache.diamond = diamond(GRIP * scale * sf / 2.0);
    cache.diamond_drag = diamond(GRIP_DRAG * scale * sf / 2.0);
    let line_w = px(sf);

    if let Some(view) = input.view.filter(|_| input.has_frames()) {
        let metric = input.metric();
        let (fill_color, line_color) = metric_colors(&theme.graph, metric);
        let m = Mapper {
            map: *map,
            plot_h,
            y_max: y_max(view, metric),
        };

        // Stacked top-3 plus other: nested areas from the baseline, so lower layers read darker.
        for layer in 0..4 {
            let value = |i: usize| {
                let b = &view.stack[i];
                match layer {
                    3 => b.top.iter().sum::<f32>() + b.other,
                    _ => b.top[..=layer].iter().sum(),
                }
            };
            let mut b = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
            for_each_run(
                view.stack.len(),
                |i| view.stack[i].t_ns,
                |i| value(i).is_finite(),
                &view.gaps,
                |run| {
                    if run.len() < 2 {
                        return;
                    }
                    let x0 = m.x(view.stack[run.start].t_ns);
                    b.move_to(point(px(x0), px(plot_h)));
                    for i in run.clone() {
                        b.line_to(point(px(m.x(view.stack[i].t_ns)), px(m.y(value(i)))));
                    }
                    b.line_to(point(px(m.x(view.stack[run.end - 1].t_ns)), px(plot_h)));
                    b.close();
                },
            );
            push_set(&mut cache.fills, b, fill_color);
        }

        // The system total, then one line per selected process.
        for (k, s) in view.series.iter().take(MAX_SERIES + 1).enumerate() {
            let color = if k == 0 {
                line_color
            } else {
                series_color(theme.accent, k - 1)
            };
            let pts = &s.points;
            let valid = |i: usize| pts[i].min.is_finite() && pts[i].max.is_finite();
            if pts
                .iter()
                .filter(|p| p.min.is_finite() && p.max.is_finite())
                .count()
                == 1
            {
                if let Some(p) = pts.iter().find(|p| p.min.is_finite() && p.max.is_finite()) {
                    cache.markers.push((point(m.x(p.t_ns), m.y(p.max)), color));
                }
                continue;
            }
            let mut b = PathBuilder::stroke(line_w);
            for_each_run(
                pts.len(),
                |i| pts[i].t_ns,
                valid,
                &view.gaps,
                |run| {
                    if run.len() < 2 {
                        return;
                    }
                    let mut first = true;
                    for p in &pts[run] {
                        let x = px(m.x(p.t_ns));
                        for v in [p.min, p.max]
                            .iter()
                            .take(if p.min == p.max { 1 } else { 2 })
                        {
                            let pt = point(x, px(m.y(*v)));
                            if first {
                                b.move_to(pt);
                                first = false;
                            } else {
                                b.line_to(pt);
                            }
                        }
                    }
                },
            );
            push_set(&mut cache.strokes, b, color);
        }

        // Gap bands: 45° hatch lines in `divider` at 30% alpha.
        let pw = map.plot_width;
        let step = HATCH_STEP * scale * sf;
        let mut b = PathBuilder::stroke(line_w);
        for &(g0, g1) in view.gaps.iter() {
            let (a, z) = (m.x(g0).max(0.0), m.x(g1).min(pw));
            if a >= z {
                continue;
            }
            cache.gaps.push(a..z);
            // Lines x = c − y, clipped to the band.
            let mut c = a;
            while c < z + plot_h {
                let y0 = (c - z).max(0.0);
                let y1 = (c - a).min(plot_h);
                if y0 < y1 {
                    b.move_to(point(px(c - y0), px(y0)));
                    b.line_to(point(px(c - y1), px(y1)));
                }
                c += step;
            }
        }
        if !cache.gaps.is_empty()
            && let Ok(path) = b.build()
        {
            cache.hatch = Some(PathSet {
                color: theme.divider.alpha(GAP_ALPHA),
                path,
            });
        }
    }

    build_labels(cache, input, env, map, window);
}

/// Time labels at the vertical grid lines, counted back from now, and "now" at the right edge
/// when the window reaches the newest second. Overlapping labels are dropped.
fn build_labels(
    cache: &mut ScrubberCache,
    input: &Input,
    env: &Env,
    map: &TimeMap,
    window: &mut Window,
) {
    let (v0, v1) = input.state.view_range;
    if !input.has_frames() || v1 <= v0 {
        return;
    }
    cache.stamp += 1;
    let sf = env.sf;
    let style = LineStyle {
        size: env.metrics.text.xs,
        weight: gpui_kit::FontWeight::NORMAL,
        color: env.theme.text_secondary,
        tnum: true,
    };
    let pw = map.plot_width;
    let now = input.now();
    let step = (map.range_ns / 10).max(1);
    let (first, last) = grid::grid_k_range(map);
    let mut boxes: SmallVec<[(f32, f32); 16]> = SmallVec::new();
    let mut slots: SmallVec<[usize; 16]> = SmallVec::new();
    for k in first..=last {
        let t = k * step;
        let x = map.x_local(t);
        if t >= now || x < 0.0 || x >= pw {
            continue;
        }
        cache.text.clear();
        time_label(now - t, &mut cache.text);
        let slot = cache.label_slot(window, style);
        let w = f32::from(cache.pool[slot].line.width()) * sf;
        boxes.push(((x - w / 2.0).clamp(0.0, (pw - w).max(0.0)), w));
        slots.push(slot);
    }
    let kept = drop_overlapping(&boxes);
    let now_box = (v1 >= now).then(|| {
        cache.text.clear();
        cache.text.push_str(input.now_word);
        let slot = cache.label_slot(window, style);
        let w = f32::from(cache.pool[slot].line.width()) * sf;
        ((pw - w).max(0.0), slot)
    });
    for i in kept {
        let (x, w) = boxes[i];
        if now_box.is_some_and(|(nx, _)| x + w > nx) {
            continue;
        }
        cache.labels.push(PlacedLabel { x, slot: slots[i] });
    }
    if let Some((x, slot)) = now_box {
        cache.labels.push(PlacedLabel { x, slot });
    }
}

/// The y scale top: 100 for CPU and GPU percent, else the largest visible value.
fn y_max(view: &HistoryView, metric: HistoryMetric) -> f32 {
    if matches!(metric, HistoryMetric::Cpu | HistoryMetric::Gpu) {
        return 100.0;
    }
    let stacked = view
        .stack
        .iter()
        .map(|b| b.top.iter().sum::<f32>() + b.other);
    let lines = view
        .series
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p.max));
    let max = stacked
        .chain(lines)
        .filter(|v| v.is_finite())
        .fold(0.0f32, f32::max);
    if max > 0.0 { max } else { 1.0 }
}

/// Calls `f` with each run of consecutive valid items, split at invalid items and at any gap
/// that lies between two neighbours.
fn for_each_run(
    n: usize,
    t: impl Fn(usize) -> TimeNs,
    valid: impl Fn(usize) -> bool,
    gaps: &[(TimeNs, TimeNs)],
    mut f: impl FnMut(Range<usize>),
) {
    let mut start = None;
    for i in 0..n {
        if !valid(i) {
            if let Some(s) = start.take() {
                f(s..i);
            }
            continue;
        }
        if let Some(s) = start
            && i > s
            && gaps.iter().any(|&(g0, g1)| g0 < t(i) && g1 > t(i - 1))
        {
            f(s..i);
            start = None;
        }
        start.get_or_insert(i);
    }
    if let Some(s) = start {
        f(s..n);
    }
}

/// Adds the built path of `b` to `sets` unless it failed or is empty.
fn push_set(sets: &mut impl Extend<PathSet>, b: PathBuilder, color: Hsla) {
    if let Ok(path) = b.build()
        && !path.vertices.is_empty()
    {
        sets.extend([PathSet { color, path }]);
    }
}

/// A diamond with half-diagonal `r` physical px, centred on the origin.
fn diamond(r: f32) -> Option<Path<Pixels>> {
    let mut b = PathBuilder::fill();
    b.move_to(point(px(0.), px(-r)));
    b.line_to(point(px(r), px(0.)));
    b.line_to(point(px(0.), px(r)));
    b.line_to(point(px(-r), px(0.)));
    b.close();
    b.build().ok()
}

/// `path` (strip-local physical px) as window logical px at `origin`: every vertex and the
/// bounds scaled by `inv` once, then shifted.
fn placed(path: &Path<Pixels>, origin: Point<Pixels>, inv: f32) -> Path<Pixels> {
    let mut path = path.clone();
    path.bounds.origin = path.bounds.origin * inv + origin;
    path.bounds.size = size(path.bounds.size.width * inv, path.bounds.size.height * inv);
    for vertex in &mut path.vertices {
        vertex.xy_position = vertex.xy_position * inv + origin;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = NS_PER_SEC;

    fn label(f: fn(u64, &mut String), ns: u64) -> String {
        let mut out = String::new();
        f(ns, &mut out);
        out
    }

    #[test]
    fn scrubber_time_label_formats() {
        assert_eq!(label(time_label, 45 * S), "-0:45");
        assert_eq!(label(time_label, 125 * S), "-2:05");
        assert_eq!(label(time_label, 3_723 * S), "-1:02:03");
        assert_eq!(label(time_label, 500 * S), "-8:20");
    }

    #[test]
    fn span_label_formats() {
        assert_eq!(label(span_label, 45 * S), "45s");
        assert_eq!(label(span_label, 125 * S), "2m 05s");
        assert_eq!(label(span_label, 3_720 * S), "1h 02m");
    }

    #[test]
    fn labels_drop_overlaps() {
        let labels: Vec<(f32, f32)> = (0..10).map(|i| (i as f32 * 30.0, 40.0)).collect();
        assert_eq!(drop_overlapping(&labels).as_slice(), [0, 2, 4, 6, 8]);
        // Already apart: all kept.
        let apart: Vec<(f32, f32)> = (0..4).map(|i| (i as f32 * 50.0, 40.0)).collect();
        assert_eq!(drop_overlapping(&apart).as_slice(), [0, 1, 2, 3]);
        // 40 wide every 10: every fourth survives.
        let dense: Vec<(f32, f32)> = (0..9).map(|i| (i as f32 * 10.0, 40.0)).collect();
        assert_eq!(drop_overlapping(&dense).as_slice(), [0, 4, 8]);
    }

    #[test]
    fn runs_split_at_nan_and_gaps() {
        let ts = [0, 1, 2, 3, 4, 5];
        let valid = [true, true, false, true, true, true];
        let mut runs = Vec::new();
        for_each_run(6, |i| ts[i], |i| valid[i], &[(3, 4)], |r| runs.push(r));
        // 3 and 4 touch the gap ends but the gap (3, 4) lies between them.
        assert_eq!(runs, [0..2, 3..4, 4..6]);
    }

    fn secs(a: u64, b: u64) -> (TimeNs, TimeNs) {
        (a * S, b * S)
    }

    #[test]
    fn scrubber_zoom_clamps_and_keeps_cursor_time() {
        let span = secs(0, 3600);
        let view = secs(1000, 1600);
        let cursor = 1300 * S;
        let z = zoom(view, cursor, 1.0, span);
        assert_eq!(z.1 - z.0, 480 * S);
        assert_eq!(z, secs(1060, 1540));
        // The cursor keeps its place in the view.
        assert_eq!(x_to_t(0.5, 1.0, z), cursor);
        // Zooming out.
        let out = zoom(view, cursor, -1.0, span);
        assert_eq!(out.1 - out.0, 750 * S);
        // 70 s, 3 notches: clamped to 60 s.
        let z = zoom(secs(1000, 1070), 1035 * S, 3.0, span);
        assert_eq!(z.1 - z.0, 60 * S);
        // Out clamps to the span and stays inside it.
        assert_eq!(zoom(view, cursor, -20.0, span), span);
        // A span shorter than 60 s shows whole.
        let short = secs(10, 40);
        assert_eq!(zoom(secs(10, 40), 20 * S, 5.0, short), short);
        // Zooming at the right edge keeps the window inside the span.
        let z = zoom(secs(3000, 3600), 3600 * S, 1.0, span);
        assert_eq!(z, secs(3120, 3600));
    }

    #[test]
    fn pan_clamped_to_span() {
        let span = secs(100, 1000);
        let view = secs(400, 1000);
        assert_eq!(pan(view, 30 * S as i64, span), view);
        assert_eq!(pan(view, -30 * S as i64, span), secs(370, 970));
        assert_eq!(pan(view, -1000 * S as i64, span), secs(100, 700));
        assert_eq!(pan(secs(100, 700), 50 * S as i64, span), secs(150, 750));
    }

    #[test]
    fn x_to_t_maps() {
        let view = secs(400, 1000);
        assert_eq!(x_to_t(0.0, 200.0, view), 400 * S);
        assert_eq!(x_to_t(50.0, 200.0, view), 550 * S);
        assert_eq!(x_to_t(200.0, 200.0, view), 1000 * S);
        assert_eq!(x_to_t(-5.0, 200.0, view), 400 * S);
        assert_eq!(x_to_t(250.0, 200.0, view), 1000 * S);
        assert_eq!(snap_s(550 * S + S / 2 - 1, secs(100, 1000)), 550 * S);
        assert_eq!(snap_s(2000 * S, secs(100, 1000)), 1000 * S);
    }

    #[test]
    fn series_colours_rotate_hue() {
        let accent = Hsla {
            h: 0.9,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };
        assert_eq!(series_color(accent, 0), accent);
        let c = series_color(accent, 1);
        assert!((c.h - (0.9 + 0.125 - 1.0)).abs() < 1e-6);
        assert_eq!((c.s, c.l, c.a), (0.5, 0.5, 1.0));
    }
}
