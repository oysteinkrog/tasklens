//! Hover readout on large charts and minigraph cards (PERF-023, UI-PRF-004, ADR-009 rule 1).
//!
//! While the pointer is over the plot, the chart pins the nearest sample time, snapped to the
//! sample grid ([`snap_time`]), so a pointer move inside one sample changes nothing and does not
//! notify. Each paint finds
//! that sample by one binary search and draws a vertical marker at its x, a dot per series and a
//! box with the time offset, each series value, avg, peak and the sample count. The marker's x is
//! recomputed every frame, so it moves left with the data. The box text is formatted and shaped
//! only when the sample index or the series data changed. Hover never touches the geometry
//! cache.

use core::fmt::Write;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, BorderStyle, Bounds, EntityId, FontWeight, Pixels, Window, fill, point, px, quad, size,
};
use smallvec::SmallVec;
use tl_model::series::{Sample, SeriesView};
use tl_model::units;

use super::geometry::{self, TimeMap};
use super::labels::{self, CachedLine, LABEL_ROW, LineStyle};
use super::{ChartLabels, ChartSpec, LegendStyle};
use crate::theme::{TlTheme, UiMetrics};

const NS_PER_S: u64 = 1_000_000_000;
/// Ranges above this show minutes and seconds.
const SECONDS_ONLY_MAX_NS: u64 = 60 * NS_PER_S;
/// Box distance from the marker, logical px at 100%.
const BOX_GAP: f32 = 12.0;
/// Box minimum width at 100%.
const BOX_MIN_W: f32 = 140.0;
/// Dot radius at 100%.
const DOT_R: f32 = 3.0;
/// Marker alpha factor.
const MARKER_ALPHA: f32 = 0.8;
/// Gap between a series label and its value at 100%.
const VALUE_GAP: f32 = 16.0;

/// Hover state kept in the `ChartView` between frames.
#[derive(Debug, Default)]
pub struct HoverState {
    /// Pointer time, set by mouse moves over the plot and cleared on mouse down, pointer leave
    /// and window deactivation.
    pub pinned_t_ns: Option<u64>,
    /// Nearest valid sample of series 0 for this paint.
    pub index: Option<usize>,
    /// Offset, one label per series, one value per series, stats, sample count.
    pub(crate) lines: SmallVec<[CachedLine; 5]>,
    /// `index` the lines were formatted for.
    pub last_index: Option<usize>,
    /// Sum of the series versions the lines were formatted for.
    last_versions: u64,
    /// Per-series value at the hovered time, `None` when that series has no valid sample there.
    values: SmallVec<[Option<f32>; 4]>,
    /// Reused formatter buffer.
    scratch: String,
    /// `shape_line` calls over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub shape_count: u32,
    /// The value box of the last paint, `None` when nothing was painted.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_box: Option<Bounds<Pixels>>,
    /// The marker x of the last paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_marker_x: Option<Pixels>,
    /// True when the last prepaint inserted a hitbox.
    #[cfg(any(test, feature = "test-hooks"))]
    pub hitbox_inserted: bool,
}

impl HoverState {
    /// Drops the pin. Returns true when there was one.
    pub fn clear(&mut self) -> bool {
        self.pinned_t_ns.take().is_some()
    }

    /// The text line `i` was last shaped from (offset, labels, values, stats, count).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn line_text(&self, i: usize) -> Option<&str> {
        self.lines.get(i).and_then(CachedLine::text)
    }

    fn sync(&mut self, window: &mut Window, i: usize, style: LineStyle) {
        let shaped = self.lines[i].sync(window, &self.scratch, style);
        #[cfg(any(test, feature = "test-hooks"))]
        if shaped {
            self.shape_count += 1;
        }
        #[cfg(not(any(test, feature = "test-hooks")))]
        let _ = shaped;
    }
}

/// Charts that show one hover cursor (tasklens-o55.55): the time pinned over one member's plot
/// draws the marker and dots on every member. Members are charts with `spec.hover`.
#[derive(Clone, Default)]
pub struct HoverLink(Rc<HoverLinkInner>);

#[derive(Default)]
struct HoverLinkInner {
    /// The member under the pointer and the time it pinned.
    pin: Cell<Option<(EntityId, u64)>>,
    members: RefCell<SmallVec<[EntityId; 2]>>,
}

impl HoverLink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds chart `id`.
    pub(crate) fn join(&self, id: EntityId) {
        let mut members = self.0.members.borrow_mut();
        if !members.contains(&id) {
            members.push(id);
        }
    }

    /// Takes member `id`'s pin: a time pins it, `None` drops the link's pin when `id` holds it.
    /// Returns the other members when the link's pin changed, so the caller notifies them.
    pub(crate) fn set(&self, id: EntityId, t: Option<u64>) -> SmallVec<[EntityId; 2]> {
        let old = self.0.pin.get();
        let new = match t {
            Some(t) => Some((id, t)),
            None if old.is_some_and(|(owner, _)| owner == id) => None,
            None => old,
        };
        if new == old {
            return SmallVec::new();
        }
        self.0.pin.set(new);
        self.others(id)
    }

    /// The time another member pinned, for member `id` to follow.
    pub(crate) fn follow(&self, id: EntityId) -> Option<u64> {
        self.0
            .pin
            .get()
            .filter(|(owner, _)| *owner != id)
            .map(|(_, t)| t)
    }

    fn others(&self, id: EntityId) -> SmallVec<[EntityId; 2]> {
        let members = self.0.members.borrow();
        members.iter().copied().filter(|m| *m != id).collect()
    }
}

/// Index of the valid sample nearest to `t_ns`, by binary search on `t_ns`; the earlier one on a
/// tie. Invalid samples are skipped. `None` when no sample is valid.
pub fn nearest_index(samples: &[Sample], t_ns: u64) -> Option<usize> {
    let split = samples.partition_point(|s| s.t_ns < t_ns);
    let after = samples[split..]
        .iter()
        .position(Sample::is_valid)
        .map(|i| split + i);
    let before = samples[..split].iter().rposition(Sample::is_valid);
    match (before, after) {
        (Some(b), Some(a)) => {
            if t_ns - samples[b].t_ns <= samples[a].t_ns - t_ns {
                Some(b)
            } else {
                Some(a)
            }
        }
        (b, a) => b.or(a),
    }
}

/// Time under chart-local physical `x_physical`:
/// `right_edge_ns − (plot_right − x) × range_ns / plot_width`. `x` is clamped to the plot.
pub fn pointer_time(map: &TimeMap, plot_right_physical: f32, x_physical: f32) -> u64 {
    let from_right = (plot_right_physical - x_physical).clamp(0.0, map.plot_width.max(0.0));
    let ns = f64::from(from_right) * map.range_ns as f64 / f64::from(map.plot_width.max(1.0));
    map.right_edge_ns.saturating_sub(ns.round() as u64)
}

/// The sample grid a pointer time snaps to: the newest sample time and the sample period.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SampleGrid {
    pub anchor_ns: u64,
    pub period_ns: u64,
}

/// `t_ns` moved to the nearest point of `grid`, `anchor_ns − k × period_ns` for a whole `k ≥ 0`;
/// the older one on a tie, as [`nearest_index`]. Times after the anchor snap to it. A zero
/// period returns `t_ns`.
pub fn snap_time(t_ns: u64, grid: SampleGrid) -> u64 {
    let SampleGrid {
        anchor_ns,
        period_ns,
    } = grid;
    if period_ns == 0 {
        return t_ns;
    }
    if t_ns >= anchor_ns {
        return anchor_ns;
    }
    let k = (anchor_ns - t_ns).saturating_add(period_ns / 2) / period_ns;
    anchor_ns.saturating_sub(k.saturating_mul(period_ns))
}

/// Appends the hover time offset `dt_ns` before the newest sample, rounded to whole seconds:
/// "-12 s" for ranges up to 60 s, "-4 m 12 s" for 300 and 600 s ("-12 s" below a minute).
/// An hour or more, on the 24 hour battery history, reads "-14 h 5 m". "0 s" at the newest
/// sample.
pub fn offset_text(dt_ns: u64, range_ns: u64, out: &mut String) {
    let secs = (dt_ns + NS_PER_S / 2) / NS_PER_S;
    if secs == 0 {
        out.push_str("0 s");
    } else if secs >= 3600 {
        let _ = write!(out, "-{} h {} m", secs / 3600, secs % 3600 / 60);
    } else if range_ns <= SECONDS_ONLY_MAX_NS || secs < 60 {
        let _ = write!(out, "-{secs} s");
    } else {
        let _ = write!(out, "-{} m {} s", secs / 60, secs % 60);
    }
}

/// Average, peak and count of the valid samples from `from` to the end.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub avg: f32,
    pub peak: f32,
    pub samples: u32,
}

/// `Stats` over the valid samples of `samples[from..]`, the visible window of the first series.
/// All zero when none is valid.
pub fn window_stats(samples: &[Sample], from: usize) -> Stats {
    let mut sum = 0.0f64;
    let mut peak = f32::NEG_INFINITY;
    let mut n = 0u32;
    for s in samples.get(from..).unwrap_or(&[]) {
        if s.is_valid() {
            sum += f64::from(s.value);
            peak = peak.max(s.value);
            n += 1;
        }
    }
    if n == 0 {
        return Stats::default();
    }
    Stats {
        avg: (sum / f64::from(n)) as f32,
        peak,
        samples: n,
    }
}

/// Inputs of one hover paint.
pub struct HoverPaint<'a> {
    pub box_bounds: Bounds<Pixels>,
    pub plot: Bounds<Pixels>,
    /// The box flips to the marker's left where it would pass this x: the plot's right edge on a
    /// single chart, the element's right edge on a grid.
    pub flip_right: Pixels,
    /// Paint-time mapping; `dx` is its shift in physical px.
    pub map: TimeMap,
    pub dx: f32,
    pub scale_factor: f32,
    pub spec: &'a ChartSpec,
    pub labels: &'a ChartLabels,
    /// Resolved series, one per `spec.series` entry.
    pub series: &'a [Option<Arc<SeriesView>>],
    pub y_min: f32,
    pub y_max: f32,
    pub metrics: &'a UiMetrics,
    pub theme: &'a TlTheme,
    pub network_unit: labels::NetFormat,
}

/// Paints the marker, the dots and the value box for `state.pinned_t_ns`. Paints nothing when
/// there is no pin or series 0 has no valid sample in view.
pub fn paint(window: &mut Window, cx: &mut App, state: &mut HoverState, p: &HoverPaint) {
    paint_linked(window, cx, state, p, None);
}

/// [`paint`], and without a pin of its own the marker and dots at `follow_t_ns`, the time
/// another chart of its [`HoverLink`] pinned. A followed time draws no value box.
pub fn paint_linked(
    window: &mut Window,
    cx: &mut App,
    state: &mut HoverState,
    p: &HoverPaint,
    follow_t_ns: Option<u64>,
) {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        state.last_box = None;
        state.last_marker_x = None;
    }
    state.index = None;
    let (pinned, own) = match (state.pinned_t_ns, follow_t_ns) {
        (Some(t), _) => (t, true),
        (None, Some(t)) => (t, false),
        (None, None) => return,
    };
    let Some(first) = p.series.first().and_then(Option::as_deref) else {
        return;
    };
    let samples = &first.samples[..];
    let left_ns = p.map.right_edge_ns.saturating_sub(p.map.range_ns);
    let Some(index) = nearest_index(samples, pinned) else {
        return;
    };
    let t_ns = samples[index].t_ns;
    if t_ns < left_ns || t_ns > p.map.right_edge_ns {
        return;
    }
    state.index = Some(index);

    let n = p.spec.series.len();
    let versions = p
        .series
        .iter()
        .flatten()
        .fold(0u64, |acc, s| acc.wrapping_add(s.version));
    let lines_len = 2 * n + 3;
    if state.last_index != Some(index)
        || state.last_versions != versions
        || state.lines.len() != lines_len
    {
        format_lines(window, state, p, index, t_ns, samples, left_ns);
        state.last_index = Some(index);
        state.last_versions = versions;
    }

    let m = p.metrics;
    let s = m.ui_scale;
    let sf = p.scale_factor;
    let one_px = px(1.0 / sf);
    let x_phys = (p.map.x_local(t_ns) + p.dx).round();
    let x = p.plot.left() + px(x_phys / sf);
    let ph = f32::from(p.plot.size.height) * sf;

    // Box size from the shaped widths.
    let pad = m.sp[3];
    let row = px(LABEL_ROW * s);
    let swatch_w = px(12.0 * s);
    let line_w = |i: usize| state.lines[i].width();
    let mut content_w = line_w(0).max(line_w(2 * n + 1)).max(line_w(2 * n + 2));
    for i in 0..n {
        let w = swatch_w + m.sp[2] + line_w(1 + i) + px(VALUE_GAP * s) + line_w(1 + n + i);
        content_w = content_w.max(w);
    }
    let box_w = (content_w + pad * 2.0).max(px(BOX_MIN_W * s));
    let box_h = pad * 2.0 + row * (n + 3) as f32;
    let gap = px(BOX_GAP * s);
    let mut left = x + gap;
    if left + box_w > p.flip_right {
        left = x - gap - box_w;
    }
    let b = p.box_bounds;
    let top = (p.plot.top() + pad).min(b.bottom() - box_h).max(b.top());
    let panel = Bounds::new(point(left, top), size(box_w, box_h));

    window.paint_layer(b, |window| {
        // Marker: one physical px over the plot height.
        let marker = Bounds::new(point(x, p.plot.top()), size(one_px, p.plot.size.height));
        let color = p.theme.text_secondary;
        window.paint_quad(fill(marker, color.alpha(color.a * MARKER_ALPHA)));

        // Dots. Stacked charts raise each series by the ones below it; a missing lower sample
        // counts as 0, as in the geometry.
        let r = px(DOT_R * s);
        let mut acc = 0.0f32;
        for (i, series) in p.spec.series.iter().enumerate() {
            let Some(v) = state.values.get(i).copied().flatten() else {
                continue;
            };
            let v = if p.spec.stack {
                acc += v;
                acc
            } else {
                v
            };
            let y = p.plot.top() + px(geometry::y_of(v, p.y_min, p.y_max, ph) / sf);
            let dot = Bounds::new(point(x - r, y - r), size(r * 2.0, r * 2.0));
            window.paint_quad(quad(
                dot,
                r,
                series.line,
                one_px,
                p.theme.window_bg,
                BorderStyle::Solid,
            ));
        }
        if !own {
            return;
        }

        // Value box.
        window.paint_quad(quad(
            panel,
            m.radius_small,
            p.theme.menu_bg,
            one_px,
            p.theme.divider,
            BorderStyle::Solid,
        ));
        let lx = panel.left() + pad;
        let rx = panel.right() - pad;
        let mut y = panel.top() + pad;
        labels::paint_line(window, cx, &state.lines[0], point(lx, y), row);
        for (i, series) in p.spec.series.iter().enumerate() {
            y += row;
            let style = if series.dashed {
                LegendStyle::Dashed
            } else {
                LegendStyle::Solid
            };
            labels::paint_swatch(
                window,
                point(lx, y + (row - px(2.0 * s)) / 2.0),
                s,
                style,
                series.line,
            );
            let label = point(lx + swatch_w + m.sp[2], y);
            labels::paint_line(window, cx, &state.lines[1 + i], label, row);
            let value = &state.lines[1 + n + i];
            labels::paint_line(window, cx, value, point(rx - value.width(), y), row);
        }
        y += row;
        labels::paint_line(window, cx, &state.lines[2 * n + 1], point(lx, y), row);
        y += row;
        labels::paint_line(window, cx, &state.lines[2 * n + 2], point(lx, y), row);
    });

    #[cfg(any(test, feature = "test-hooks"))]
    {
        state.last_box = own.then_some(panel);
        state.last_marker_x = Some(x);
    }
}

/// Formats and shapes every box line for sample `index` of series 0 at `t_ns`, and reads each
/// series' value at that time.
fn format_lines(
    window: &mut Window,
    state: &mut HoverState,
    p: &HoverPaint,
    index: usize,
    t_ns: u64,
    samples: &[Sample],
    left_ns: u64,
) {
    let m = p.metrics;
    let n = p.spec.series.len();
    if state.lines.len() != 2 * n + 3 {
        state.lines.resize_with(2 * n + 3, CachedLine::default);
    }
    let primary = LineStyle {
        size: m.text.xs,
        weight: FontWeight::NORMAL,
        color: p.theme.text_primary,
        tnum: true,
    };
    let secondary = LineStyle {
        color: p.theme.text_secondary,
        tnum: false,
        ..primary
    };

    // Offset from the newest sample of series 0.
    let newest = samples.last().map_or(t_ns, |s| s.t_ns);
    state.scratch.clear();
    offset_text(
        newest.saturating_sub(t_ns),
        p.spec.range_ns,
        &mut state.scratch,
    );
    state.sync(
        window,
        0,
        LineStyle {
            weight: FontWeight::MEDIUM,
            tnum: false,
            ..primary
        },
    );

    // Per series: label and value. Series 0 reads `index`; the others read their own nearest
    // valid sample, missing when it is over one period away.
    state.values.clear();
    for i in 0..n {
        let value = match p.series.get(i).and_then(Option::as_deref) {
            Some(_) if i == 0 => Some(samples[index].value),
            Some(v) => nearest_index(&v.samples, t_ns)
                .map(|j| v.samples[j])
                .filter(|s| s.t_ns.abs_diff(t_ns) <= v.period_ns.max(1))
                .map(|s| s.value),
            None => None,
        };
        state.values.push(value);

        state.scratch.clear();
        match p.labels.legend.get(i) {
            Some((label, _)) => state.scratch.push_str(label),
            None => {
                if let Some(h) = &p.labels.heading {
                    state.scratch.push_str(h);
                }
            }
        }
        state.sync(window, 1 + i, secondary);

        state.scratch.clear();
        match value {
            Some(v) => {
                labels::format_value(p.labels.value_fmt, v, p.network_unit, &mut state.scratch)
            }
            None => state.scratch.push_str(labels::MISSING),
        }
        state.sync(window, 1 + n + i, primary);
    }

    // Stats of series 0 over the visible window.
    let from = samples.partition_point(|s| s.t_ns < left_ns);
    let stats = window_stats(samples, from);
    state.scratch.clear();
    state.scratch.push_str("avg ");
    labels::format_value(
        p.labels.value_fmt,
        stats.avg,
        p.network_unit,
        &mut state.scratch,
    );
    state.scratch.push_str("  peak ");
    labels::format_value(
        p.labels.value_fmt,
        stats.peak,
        p.network_unit,
        &mut state.scratch,
    );
    state.sync(window, 2 * n + 1, secondary);

    state.scratch.clear();
    units::count(&mut state.scratch, u64::from(stats.samples));
    state.scratch.push_str(" samples");
    state.sync(window, 2 * n + 2, secondary);
}
