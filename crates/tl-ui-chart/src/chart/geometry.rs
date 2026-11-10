//! Chart geometry: time mapping, run splitting, decimation and the stale rule (part 1), and path
//! emission into `ChartCache` (part 2) (ADR-009 rules 2 to 7; UI-PRF-001; UI-PRF-005;
//! UI-PRF-007; PERF-022; PERF-026; R-051).
//!
//! Every function here is pure and allocation-free given the caller's buffers. Fills and strokes
//! skip lyon: chart lines are monotone in x, so `FillMesh` and `StrokeMesh` triangulate them
//! directly (tasklens-wlf.33, tasklens-wlf.35). Time math is `u64` and the pixel scale is `f64`:
//! 60 s in ns against 1,600 px loses precision in `f32`.

use std::collections::VecDeque;

use gpui_kit::{Bounds, Hsla, Path, PathVertex, Pixels, Point, point, px, size};
use smallvec::SmallVec;
use tl_model::series::{Sample, SampleFlags, SeriesView};

use super::{ChartCache, ChartSeries, GapStyle, PathSet};

/// Stale floor from R-051: 1.5 s.
const STALE_FLOOR_NS: u64 = 1_500_000_000;

/// Maps sample time to chart-local physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeMap {
    /// Newest sample time when the geometry was built (ADR-009 rule 4).
    pub anchor_ns: u64,
    /// Time drawn at the plot's right edge.
    pub right_edge_ns: u64,
    pub range_ns: u64,
    /// Physical pixels.
    pub plot_width: f32,
}

impl TimeMap {
    /// `plot_width / range_ns`. A zero range counts as 1 ns.
    pub fn px_per_ns(&self) -> f64 {
        f64::from(self.plot_width) / self.range_ns.max(1) as f64
    }

    /// `(t_ns − anchor_ns) × px_per_ns`, chart-local. Negative for samples older than the anchor.
    pub fn x_local(&self, t_ns: u64) -> f32 {
        (signed_diff(t_ns, self.anchor_ns) as f64 * self.px_per_ns()) as f32
    }

    /// Paint-time shift that puts `right_edge_ns` at `plot_right` (ADR-009 rule 4).
    pub fn dx(&self, plot_right: f32) -> f32 {
        let edge = signed_diff(self.right_edge_ns, self.anchor_ns) as f64 * self.px_per_ns();
        (f64::from(plot_right) - edge) as f32
    }
}

/// `a − b` as a signed value. Boot-clock times fit in `i64`.
fn signed_diff(a: u64, b: u64) -> i64 {
    a.wrapping_sub(b) as i64
}

/// Time at the plot's right edge (ADR-009 rule 5). With `smooth` the chart trails one period
/// behind the clock so a new sample scrolls in instead of popping: `min(now − period, newest)`.
/// Without it the newest sample sits at the edge.
pub fn right_edge_ns(now_boot_ns: u64, newest_ns: u64, period_ns: u64, smooth: bool) -> u64 {
    if smooth {
        now_boot_ns.saturating_sub(period_ns).min(newest_ns)
    } else {
        newest_ns
    }
}

/// Index of the first sample with `t_ns ≥ right_edge_ns − range_ns − 2 × period_ns` (two periods
/// of overscan). Walks back from the newest sample, so the cost is the visible window only.
/// Returns 0 when every sample is inside.
pub fn window_start(
    samples: &[Sample],
    right_edge_ns: u64,
    range_ns: u64,
    period_ns: u64,
) -> usize {
    let start = right_edge_ns
        .saturating_sub(range_ns)
        .saturating_sub(period_ns.saturating_mul(2));
    let mut i = samples.len();
    while i > 0 && samples[i - 1].t_ns >= start {
        i -= 1;
    }
    i
}

/// Half-open index range into the samples. Every sample in it is `VALID`, and only the first may
/// carry `GAP_BEFORE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
}

impl Run {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// Splits `samples[from..]` into runs (PERF-022). Clears `out` first. A sample without `VALID`
/// ends the run and belongs to no run, so consecutive invalid samples form one gap. A sample with
/// `GAP_BEFORE` starts a new run. `STALE` samples stay in their run.
pub fn split_runs(samples: &[Sample], from: usize, out: &mut SmallVec<[Run; 8]>) {
    out.clear();
    let mut start: Option<usize> = None;
    for (i, s) in samples.iter().enumerate().skip(from) {
        if !s.flags.contains(SampleFlags::VALID) {
            if let Some(st) = start.take() {
                out.push(Run { start: st, end: i });
            }
            continue;
        }
        if s.flags.contains(SampleFlags::GAP_BEFORE)
            && let Some(st) = start.replace(i)
        {
            out.push(Run { start: st, end: i });
        }
        start.get_or_insert(i);
    }
    if let Some(st) = start {
        out.push(Run {
            start: st,
            end: samples.len(),
        });
    }
}

/// One decimated point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub t_ns: u64,
    pub value: f32,
}

/// Decimates one run with buckets aligned to absolute time, `bucket = t_ns / bucket_ns`, so the
/// output does not shimmer as the chart scrolls (ADR-009 rule 3). Clears `out` first.
///
/// A bucket with more than one sample keeps its min and max in time order; a bucket with one
/// sample keeps it. The run's first and last samples are always kept. No sample is emitted twice,
/// so the output has at most `2 × buckets + 2` points, counting the buckets the run touches.
pub fn decimate(samples: &[Sample], run: Run, bucket_ns: u64, out: &mut Vec<Pt>) {
    out.clear();
    let Some(run_samples) = samples.get(run.start..run.end) else {
        return;
    };
    let Some((first, rest)) = run_samples.split_first() else {
        return;
    };
    let bucket_ns = bucket_ns.max(1);
    let push = |out: &mut Vec<Pt>, s: &Sample| {
        out.push(Pt {
            t_ns: s.t_ns,
            value: s.value,
        });
    };
    push(out, first);
    let Some((last, middle)) = rest.split_last() else {
        return;
    };

    // Positions are relative to `run_samples`; 0 (the first sample) is already out.
    let mut emitted = 0usize;
    let mut flush = |out: &mut Vec<Pt>, lo: usize, hi: usize| {
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        for p in [a, b] {
            if p > emitted {
                push(out, &run_samples[p]);
                emitted = p;
            }
        }
    };

    let mut bucket = first.t_ns / bucket_ns;
    let (mut lo, mut hi) = (0usize, 0usize);
    for (k, s) in middle.iter().enumerate() {
        let p = k + 1;
        let b = s.t_ns / bucket_ns;
        if b != bucket {
            flush(out, lo, hi);
            bucket = b;
            lo = p;
            hi = p;
        } else {
            if s.value < run_samples[lo].value {
                lo = p;
            }
            if s.value > run_samples[hi].value {
                hi = p;
            }
        }
    }
    // The last sample joins its bucket's min/max, then is kept regardless.
    let p_last = run_samples.len() - 1;
    if last.t_ns / bucket_ns != bucket {
        flush(out, lo, hi);
    } else {
        if last.value < run_samples[lo].value {
            lo = p_last;
        }
        if last.value > run_samples[hi].value {
            hi = p_last;
        }
        flush(out, lo, hi);
    }
    if p_last > emitted {
        push(out, last);
    }
}

/// `max(range_ns / physical_width, 1)`.
pub fn bucket_ns(range_ns: u64, physical_width: u32) -> u64 {
    (range_ns / u64::from(physical_width.max(1))).max(1)
}

/// R-051: stale when `now − newest > max(3 × period, 1.5 s)`.
pub fn is_stale(newest_ns: u64, now_boot_ns: u64, period_ns: u64) -> bool {
    stale_age_ns(newest_ns, now_boot_ns) > period_ns.saturating_mul(3).max(STALE_FLOOR_NS)
}

/// Age of the newest sample, 0 when it is in the future.
pub fn stale_age_ns(newest_ns: u64, now_boot_ns: u64) -> u64 {
    now_boot_ns.saturating_sub(newest_ns)
}

/// Replaces the valid samples of `samples[from..]` with their means over `bucket_ns` buckets
/// (aligned to time 0), each at the time and with the flags of its newest sample. A sample
/// without `VALID` is kept as it is and ends the bucket. Clears `out` first.
pub fn mean_samples(samples: &[Sample], from: usize, bucket_ns: u64, out: &mut Vec<Sample>) {
    /// The bucket being filled: its index, sum, count, newest sample and the first one's
    /// `GAP_BEFORE`.
    struct Open {
        bucket: u64,
        sum: f64,
        n: u32,
        last: Sample,
        gap: SampleFlags,
    }
    fn flush(open: &mut Option<Open>, out: &mut Vec<Sample>) {
        if let Some(o) = open.take() {
            out.push(Sample {
                value: (o.sum / f64::from(o.n)) as f32,
                flags: o.last.flags | o.gap,
                ..o.last
            });
        }
    }
    out.clear();
    let bucket_ns = bucket_ns.max(1);
    let mut open: Option<Open> = None;
    for s in &samples[from..] {
        if !s.is_valid() {
            flush(&mut open, out);
            out.push(*s);
            continue;
        }
        let bucket = s.t_ns / bucket_ns;
        match &mut open {
            Some(o) if o.bucket == bucket && !s.flags.contains(SampleFlags::GAP_BEFORE) => {
                o.sum += f64::from(s.value);
                o.n += 1;
                o.last = *s;
            }
            _ => {
                flush(&mut open, out);
                open = Some(Open {
                    bucket,
                    sum: f64::from(s.value),
                    n: 1,
                    last: *s,
                    gap: s.flags & SampleFlags::GAP_BEFORE,
                });
            }
        }
    }
    flush(&mut open, out);
}

/// Inputs of one geometry build. All lengths are physical pixels.
pub struct BuildInput<'a> {
    /// Spec series paired with the resolved views (`None` = absent).
    pub series: &'a [(ChartSeries, Option<&'a SeriesView>)],
    pub map: TimeMap,
    pub plot_height: f32,
    /// From the scale (E19-004).
    pub y_min: f32,
    pub y_max: f32,
    pub now_boot_ns: u64,
    /// P2 stacked bands (PERF-030): band `k` draws the sum of series `0..=k`.
    pub stack: bool,
    /// `performance.curved_lines` (PERF-021): monotone cubic through the decimated points.
    /// `build_cell` ignores it.
    pub curved: bool,
}

/// Buffers reused across geometry rebuilds.
#[derive(Default)]
pub struct Scratch {
    runs: SmallVec<[Run; 8]>,
    pts: Vec<Pt>,
    stale_pts: Vec<Pt>,
    /// Series drawn as means (`ChartSeries::mean_ns`): the window's means.
    meaned: Vec<Sample>,
    /// Stacked mode: the floor under each point of `pts` or `stale_pts`.
    pt_floors: Vec<f32>,
    /// Stacked mode: the shared time axis and every band's top on it.
    stack: StackAxis,
    /// Grid cells: the decimated points after clipping to the cell, physical px.
    clipped: Vec<Point<Pixels>>,
    /// Grid cells: the last built cell's dotted-gap spans, chart-local physical px.
    cell_gaps: SmallVec<[std::ops::Range<f32>; 4]>,
    /// One run's line and band floor as emitted, physical px, and the curve tangents.
    line: Line,
    live: Builders,
    stale: Builders,
    /// Grid splices: the new segments at one end of a piece, before they move into it.
    splice: SpliceBufs,
}

impl Scratch {
    /// Starts a full grid build: drops the cache's live sets (a grid paints its pieces) and takes
    /// back the stale paths so their vertex buffers can be reused. This must run before
    /// `finish_grid`.
    pub fn begin_grid(&mut self, cache: &mut ChartCache) {
        cache.fills.clear();
        cache.strokes.clear();
        self.stale.reclaim_fills(&mut cache.stale_fills);
        self.stale.reclaim_strokes(&mut cache.stale_strokes);
    }

    /// Dotted-gap spans of the cell `build_cell` built last, chart-local physical px as
    /// `ChartCache::gaps` holds them.
    pub fn cell_gaps(&self) -> &[std::ops::Range<f32>] {
        &self.cell_gaps
    }
}

/// Sample times less than this after the first time of a cluster join it on the stacked axis.
const AXIS_MERGE_NS: u64 = 1_000_000;

/// Stacked axis mark: the band has a value at this axis time (its own valid sample, or a point
/// inside one of its runs).
const HAS: u8 = 1;
/// Stacked axis mark: one of the band's runs spans the step from the axis time before.
const SPAN: u8 = 2;

/// Stacked mode (UI-PRF-060): every band on one shared time axis, the sorted union of the
/// bands' sample times in the window, with times less than 1 ms apart merged. Each band gets a
/// value at every axis time: its own sample there; else the straight line between its two
/// neighbouring samples when both are valid and in one run; else 0 (in a gap, before its first
/// sample, after its last). A band that starts or stops so ramps from or to 0 over one axis
/// step. The top of band `i` is the sum of the values of bands `0..=i` at the same axis time,
/// and band `i + 1` uses that same number as its floor, so the two always meet: for the bottom
/// band too, for any entry or exit, and for any sample spacing.
#[derive(Default)]
struct StackAxis {
    /// Present series: index into `BuildInput::series` and window start.
    bands: Vec<(usize, usize)>,
    /// The sorted union of sample times, and the buffer each band's times merge into.
    times: Vec<u64>,
    merged: Vec<u64>,
    /// One sample per axis time, valued at the stack total. `VALID` when some band has a value
    /// there (an axis time where every band is in a gap draws nothing), `GAP_BEFORE` when no
    /// band's run spans the step before it (a sweep later than the stale limit breaks every
    /// band at once), `STALE` when a band's own sample there is stale.
    axis: Vec<Sample>,
    /// Band-major, one per band and axis time: the band's top, and its `HAS` and `SPAN` marks.
    tops: Vec<f32>,
    marks: Vec<u8>,
    /// Axis indices that decimation keeps, and per stretch its range in `kept` and whether it
    /// is stale.
    kept: Vec<usize>,
    stretches: Vec<(std::ops::Range<usize>, bool)>,
    /// Decimation scratch: per band the min and max axis index of its top in the open bucket,
    /// and the bucket's kept indices in time order.
    extrema: Vec<(usize, usize)>,
    picks: Vec<usize>,
    /// Per segment of one stretch: the band draws it.
    drawn: Vec<bool>,
}

impl StackAxis {
    /// Lays every present series of `input` on the axis, stacks them, and decimates the axis in
    /// `bucket_ns` buckets by every band's top (`decimate_stack`). All bands keep the same axis
    /// points, so decimation keeps each band's top on the next band's floor.
    fn build(&mut self, input: &BuildInput, bucket_ns: u64, runs: &mut SmallVec<[Run; 8]>) {
        let Self {
            bands,
            times,
            merged,
            axis,
            tops,
            marks,
            kept,
            stretches,
            extrema,
            picks,
            ..
        } = self;
        let map = &input.map;
        bands.clear();
        times.clear();
        for (si, (_, view)) in input.series.iter().enumerate() {
            let Some(view) = view else {
                continue;
            };
            let from = window_start(
                &view.samples,
                map.right_edge_ns,
                map.range_ns,
                view.period_ns,
            );
            bands.push((si, from));
            let own = &view.samples[from..];
            merged.clear();
            let (mut i, mut j) = (0, 0);
            loop {
                let t = match (times.get(i), own.get(j)) {
                    (None, None) => break,
                    (Some(&a), Some(s)) if a <= s.t_ns => {
                        i += 1;
                        j += usize::from(a == s.t_ns);
                        a
                    }
                    (Some(&a), None) => {
                        i += 1;
                        a
                    }
                    (_, Some(s)) => {
                        j += 1;
                        s.t_ns
                    }
                };
                merged.push(t);
            }
            std::mem::swap(times, merged);
        }
        // Each cluster keeps its first time.
        let mut n = 0;
        for k in 0..times.len() {
            let t = times[k];
            if n == 0 || t - times[n - 1] >= AXIS_MERGE_NS {
                times[n] = t;
                n += 1;
            }
        }
        times.truncate(n);

        let nb = bands.len();
        tops.clear();
        tops.resize(nb * n, 0.0);
        marks.clear();
        marks.resize(nb * n, 0);
        axis.clear();
        axis.extend(times.iter().map(|&t_ns| Sample {
            t_ns,
            value: 0.0,
            flags: SampleFlags::empty(),
        }));
        for (b, &(si, from)) in bands.iter().enumerate() {
            let Some(view) = input.series[si].1 else {
                continue;
            };
            let top = &mut tops[b * n..(b + 1) * n];
            let mark = &mut marks[b * n..(b + 1) * n];
            let mut k = 0;
            // The axis index and value of the band's previous sample, when it is valid.
            let mut prev: Option<(usize, f32)> = None;
            for s in &view.samples[from..] {
                while k + 1 < n && times[k + 1] <= s.t_ns {
                    k += 1;
                }
                if !s.is_valid() {
                    prev = None;
                    continue;
                }
                if s.flags.contains(SampleFlags::STALE) {
                    axis[k].flags |= SampleFlags::STALE;
                }
                top[k] = s.value;
                mark[k] |= HAS;
                if let Some((p, v)) = prev
                    && p < k
                    && !s.flags.contains(SampleFlags::GAP_BEFORE)
                {
                    let (t0, t1) = (times[p], times[k]);
                    for q in p + 1..k {
                        let f = (times[q] - t0) as f64 / (t1 - t0) as f64;
                        top[q] = (f64::from(v) + f64::from(s.value - v) * f) as f32;
                        mark[q] |= HAS;
                    }
                    for m in &mut mark[p + 1..=k] {
                        *m |= SPAN;
                    }
                }
                prev = Some((k, s.value));
            }
        }
        for b in 1..nb {
            let (below, rest) = tops.split_at_mut(b * n);
            for (t, f) in rest[..n].iter_mut().zip(&below[(b - 1) * n..]) {
                *t += *f;
            }
        }
        for (k, a) in axis.iter_mut().enumerate() {
            let m = (0..nb).fold(0, |m, b| m | marks[b * n + k]);
            if m & HAS != 0 {
                a.flags |= SampleFlags::VALID;
            }
            if k > 0 && m & SPAN == 0 {
                a.flags |= SampleFlags::GAP_BEFORE;
            }
            a.value = nb.checked_sub(1).map_or(0.0, |b| tops[b * n + k]);
        }

        kept.clear();
        stretches.clear();
        split_runs(axis, 0, runs);
        for run in runs.iter() {
            for_each_stretch(axis, *run, |span, is_stale_flag| {
                let start = kept.len();
                decimate_stack(axis, tops, nb, span, bucket_ns, extrema, picks, kept);
                stretches.push((start..kept.len(), is_stale_flag));
            });
        }
    }
}

/// Decimates one stretch of the stack axis like `decimate`, but in each bucket it keeps the axis
/// times of the min and max of every band's top, not only of the stack total. A band boundary
/// so keeps its spikes when the bands move in opposite directions and the total stays flat.
/// Every band keeps the same axis times, so each top still lies on the next band's floor. The
/// stretch's first and last times are always kept. Appends the kept axis indices to `kept`.
#[allow(clippy::too_many_arguments)]
fn decimate_stack(
    axis: &[Sample],
    tops: &[f32],
    nb: usize,
    span: Run,
    bucket_ns: u64,
    extrema: &mut Vec<(usize, usize)>,
    picks: &mut Vec<usize>,
    kept: &mut Vec<usize>,
) {
    if span.start >= span.end {
        return;
    }
    let n = axis.len();
    let bucket_ns = bucket_ns.max(1);
    kept.push(span.start);
    let last = span.end - 1;
    if last == span.start {
        return;
    }
    let mut emitted = span.start;
    let mut flush = |extrema: &[(usize, usize)], kept: &mut Vec<usize>| {
        picks.clear();
        picks.extend(extrema.iter().flat_map(|&(lo, hi)| [lo, hi]));
        picks.sort_unstable();
        picks.dedup();
        for &p in picks.iter() {
            if p > emitted {
                kept.push(p);
                emitted = p;
            }
        }
    };
    let mut bucket = axis[span.start].t_ns / bucket_ns;
    extrema.clear();
    extrema.resize(nb, (span.start, span.start));
    for k in span.start + 1..=last {
        let b = axis[k].t_ns / bucket_ns;
        if b != bucket {
            flush(extrema, kept);
            bucket = b;
            extrema.fill((k, k));
            continue;
        }
        for (band, (lo, hi)) in extrema.iter_mut().enumerate() {
            let top = &tops[band * n..(band + 1) * n];
            if top[k] < top[*lo] {
                *lo = k;
            }
            if top[k] > top[*hi] {
                *hi = k;
            }
        }
    }
    flush(extrema, kept);
    if last > emitted {
        kept.push(last);
    }
}

/// `plot_height − (value − y_min) / (y_max − y_min) × plot_height`, clamped to `0..=plot_height`.
/// An empty or inverted scale puts every value on the baseline.
pub fn y_of(value: f32, y_min: f32, y_max: f32, plot_height: f32) -> f32 {
    let span = y_max - y_min;
    if span.is_nan() || span <= 0.0 {
        return plot_height;
    }
    (plot_height - (value - y_min) / span * plot_height).clamp(0.0, plot_height.max(0.0))
}

/// Line segments per decimated segment of a curved run.
const CURVE_STEPS: usize = 4;

/// Emits one run of points into `out` as chart-local physical px (PERF-021, UI-PRF-009). Clears
/// `out` first. Straight: one vertex per point. With `curved`: a monotone cubic (Fritsch-Carlson)
/// through the points, flattened to `CURVE_STEPS` line segments per segment, so dashes and fills
/// follow the curve. The curve never goes above or below its two end points on any segment.
/// `tangents` is a reused buffer.
pub fn emit_run(
    pts: &[Pt],
    curved: bool,
    input: &BuildInput,
    tangents: &mut Vec<f32>,
    out: &mut Vec<Point<Pixels>>,
) {
    out.clear();
    let xy = |p: &Pt| {
        (
            input.map.x_local(p.t_ns),
            y_of(p.value, input.y_min, input.y_max, input.plot_height),
        )
    };
    if !curved || pts.len() < 3 {
        out.extend(pts.iter().map(|p| {
            let (x, y) = xy(p);
            point(px(x), px(y))
        }));
        return;
    }

    // Secant slope of segment `k`; 0 when the two x values coincide.
    let slope = |k: usize| {
        let (a, b) = (xy(&pts[k]), xy(&pts[k + 1]));
        let h = b.0 - a.0;
        if h > 0.0 { (b.1 - a.1) / h } else { 0.0 }
    };
    let n = pts.len();
    tangents.clear();
    tangents.push(slope(0));
    for k in 1..n - 1 {
        let (d0, d1) = (slope(k - 1), slope(k));
        tangents.push(if d0 * d1 > 0.0 { (d0 + d1) / 2.0 } else { 0.0 });
    }
    tangents.push(slope(n - 2));
    // Fritsch-Carlson: scale both tangents of a segment down until `α² + β² ≤ 9`.
    for k in 0..n - 1 {
        let d = slope(k);
        if d == 0.0 {
            tangents[k] = 0.0;
            tangents[k + 1] = 0.0;
            continue;
        }
        let (a, b) = (tangents[k] / d, tangents[k + 1] / d);
        let r = a * a + b * b;
        if r > 9.0 {
            let tau = 3.0 / r.sqrt();
            tangents[k] = tau * a * d;
            tangents[k + 1] = tau * b * d;
        }
    }

    let (x0, y0) = xy(&pts[0]);
    out.push(point(px(x0), px(y0)));
    for k in 0..n - 1 {
        let (a, b) = (xy(&pts[k]), xy(&pts[k + 1]));
        let h3 = (b.0 - a.0) / 3.0;
        let c1 = (a.0 + h3, a.1 + tangents[k] * h3);
        let c2 = (b.0 - h3, b.1 - tangents[k + 1] * h3);
        for i in 1..CURVE_STEPS {
            let t = i as f32 / CURVE_STEPS as f32;
            let (x, y) = cubic(a, c1, c2, b, t);
            out.push(point(px(x), px(y)));
        }
        out.push(point(px(b.0), px(b.1)));
    }
}

/// The cubic Bézier `p0, c1, c2, p3` at `t`.
fn cubic(p0: (f32, f32), c1: (f32, f32), c2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        w0 * p0.0 + w1 * c1.0 + w2 * c2.0 + w3 * p3.0,
        w0 * p0.1 + w1 * c1.1 + w2 * c2.1 + w3 * p3.1,
    )
}

/// Texture coordinates that mark a vertex as fully inside the fill (as `PathBuilder` sets them).
const INSIDE: Point<f32> = point(0., 1.);

/// A chart fill built directly as triangles, without lyon (tasklens-wlf.33). Every chart fill
/// is the band between a top chain and a bottom chain that share x and never go left, so each
/// segment is two triangles: O(n), no sort, and no triangle overlaps another.
///
/// Vertices are pushed straight into the path, and the bounds are set once in `finish`, not per
/// triangle as `Path::push_triangle` does (tasklens-wlf.34).
pub struct FillMesh {
    path: Option<Path<Pixels>>,
    /// A vertex buffer from an earlier build, cleared, used for the first triangle.
    spare: Option<Path<Pixels>>,
    /// The smallest and largest x and y of the vertices so far.
    min: (f32, f32),
    max: (f32, f32),
}

impl Default for FillMesh {
    fn default() -> Self {
        Self::reusing(None)
    }
}

impl FillMesh {
    fn reusing(spare: Option<Path<Pixels>>) -> Self {
        Self {
            path: None,
            spare,
            min: (f32::MAX, f32::MAX),
            max: (f32::MIN, f32::MIN),
        }
    }

    fn tri(&mut self, a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>) {
        let path = self.path.get_or_insert_with(|| match self.spare.take() {
            Some(mut path) => {
                path.vertices.clear();
                path
            }
            None => Path::new(a),
        });
        for p in [a, b, c] {
            let (x, y) = (f32::from(p.x), f32::from(p.y));
            self.min = (self.min.0.min(x), self.min.1.min(y));
            self.max = (self.max.0.max(x), self.max.1.max(y));
            path.vertices.push(PathVertex {
                xy_position: p,
                st_position: INSIDE,
                content_mask: Default::default(),
            });
        }
    }

    /// The band between `top` and `bottom(i, top[i])`, two triangles per segment. A segment
    /// whose chains cross is split at the crossing, so both lobes fill once, as the non-zero
    /// rule fills them. Segments with no width add nothing.
    fn band(
        &mut self,
        top: &[Point<Pixels>],
        bottom: impl Fn(usize, Point<Pixels>) -> Point<Pixels>,
    ) {
        for (i, w) in top.windows(2).enumerate() {
            let (t0, t1) = (w[0], w[1]);
            if t1.x <= t0.x {
                continue;
            }
            let (b0, b1) = (bottom(i, t0), bottom(i + 1, t1));
            let d0 = f32::from(t0.y) - f32::from(b0.y);
            let d1 = f32::from(t1.y) - f32::from(b1.y);
            if d0 * d1 < 0.0 {
                let k = d0 / (d0 - d1);
                let c = point(t0.x + (t1.x - t0.x) * k, t0.y + (t1.y - t0.y) * k);
                self.tri(t0, c, b0);
                self.tri(c, t1, b1);
            } else {
                self.tri(t0, t1, b1);
                self.tri(t0, b1, b0);
            }
        }
    }

    /// The finished path with its bounds, or `None` when no triangle was added.
    fn finish(self) -> Option<Path<Pixels>> {
        let mut path = self.path.filter(|p| !p.vertices.is_empty())?;
        path.bounds = Bounds {
            origin: point(px(self.min.0), px(self.min.1)),
            size: size(px(self.max.0 - self.min.0), px(self.max.1 - self.min.1)),
        };
        Some(path)
    }

    /// Vertices pushed so far.
    fn len(&self) -> usize {
        self.path.as_ref().map_or(0, |p| p.vertices.len())
    }

    fn vertices(&self) -> &[PathVertex<Pixels>] {
        self.path.as_ref().map_or(&[], |p| &p.vertices)
    }

    /// The vertex buffer (the unused spare when no triangle was added) with the bounds so far.
    fn into_parts(self) -> MeshParts {
        let vertices = match (self.path, self.spare) {
            (Some(path), _) => path.vertices,
            (None, Some(mut spare)) => {
                spare.vertices.clear();
                spare.vertices
            }
            (None, None) => Vec::new(),
        };
        (vertices, self.min, self.max)
    }
}

/// A mesh's vertices with their smallest and largest x and y.
type MeshParts = (Vec<PathVertex<Pixels>>, (f32, f32), (f32, f32));

/// A path that only carries `vertices`, to hand a vertex buffer to a mesh for reuse.
fn spare_path(vertices: Vec<PathVertex<Pixels>>) -> Path<Pixels> {
    let mut path = Path::new(point(px(0.), px(0.)));
    path.vertices = vertices;
    path
}

/// One run's emitted vertices: the line, the band floor under it (stacked mode) and the tangent
/// buffer `emit_run` uses for both.
#[derive(Default)]
struct Line {
    tangents: Vec<f32>,
    line: Vec<Point<Pixels>>,
    floor: Vec<Point<Pixels>>,
    floor_pts: Vec<Pt>,
}

/// Dash pattern for dashed series (PERF-026).
const DASH: [gpui_kit::Pixels; 2] = [px(5.0), px(5.0)];

/// A chart stroke built directly as triangles. Each segment is a quad. Adjacent quads share a
/// miter cross-section, or use a bevel when the default lyon miter limit of 4 would be exceeded.
/// A sharp bevel whose inner point would reach past both segments folds, as in lyon. Butt caps
/// and the 5/5 dash pattern match `PathBuilder::stroke` as used by charts.
struct StrokeMesh {
    path: Option<Path<Pixels>>,
    spare: Option<Path<Pixels>>,
    min: (f32, f32),
    max: (f32, f32),
    half_width: f32,
    dashed: bool,
    /// GPUI 0.3.7 measures all subpaths as one path before applying the dash array, so these
    /// carry across `stroke` calls to preserve the old `PathBuilder` output.
    dash_on: bool,
    dash_left: f32,
    dash_points: Vec<Point<Pixels>>,
}

impl Default for StrokeMesh {
    fn default() -> Self {
        Self::reusing(None, 1.0, false)
    }
}

impl StrokeMesh {
    fn reusing(spare: Option<Path<Pixels>>, width: f32, dashed: bool) -> Self {
        Self {
            path: None,
            spare,
            min: (f32::MAX, f32::MAX),
            max: (f32::MIN, f32::MIN),
            half_width: width.max(0.0) / 2.0,
            dashed,
            dash_on: true,
            dash_left: f32::from(DASH[0]),
            dash_points: Vec::new(),
        }
    }

    fn tri(&mut self, a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>) {
        let path = self.path.get_or_insert_with(|| match self.spare.take() {
            Some(mut path) => {
                path.vertices.clear();
                path
            }
            None => Path::new(a),
        });
        for p in [a, b, c] {
            let (x, y) = (f32::from(p.x), f32::from(p.y));
            self.min = (self.min.0.min(x), self.min.1.min(y));
            self.max = (self.max.0.max(x), self.max.1.max(y));
            path.vertices.push(PathVertex {
                xy_position: p,
                st_position: INSIDE,
                content_mask: Default::default(),
            });
        }
    }

    fn stroke(&mut self, points: &[Point<Pixels>]) {
        let Some((&first, rest)) = points.split_first() else {
            return;
        };
        // A single point becomes a 1px stub so dashing treats it like any other run.
        let stub = [first, point(first.x + px(1.0), first.y)];
        let points = if rest.is_empty() { &stub[..] } else { points };
        if self.dashed {
            self.stroke_dashed(points);
        } else {
            self.stroke_line(points);
        }
    }

    fn stroke_dashed(&mut self, points: &[Point<Pixels>]) {
        self.dash_points.clear();
        let mut cursor = points[0];
        if self.dash_on {
            self.dash_points.push(cursor);
        }
        for &end in &points[1..] {
            let dx = f32::from(end.x - cursor.x);
            let dy = f32::from(end.y - cursor.y);
            let mut segment_left = (dx * dx + dy * dy).sqrt();
            if segment_left <= f32::EPSILON {
                cursor = end;
                continue;
            }
            let unit = (dx / segment_left, dy / segment_left);
            while segment_left > f32::EPSILON {
                let take = segment_left.min(self.dash_left);
                let next = point(cursor.x + px(unit.0 * take), cursor.y + px(unit.1 * take));
                if self.dash_on {
                    if self.dash_points.is_empty() {
                        self.dash_points.push(cursor);
                    }
                    self.dash_points.push(next);
                }
                cursor = next;
                segment_left -= take;
                self.dash_left -= take;
                if self.dash_left <= f32::EPSILON {
                    if self.dash_on {
                        self.flush_dash();
                    }
                    self.dash_on = !self.dash_on;
                    self.dash_left = f32::from(DASH[usize::from(!self.dash_on)]);
                }
            }
            cursor = end;
        }
        if self.dash_on {
            self.flush_dash();
        } else {
            self.dash_points.clear();
        }
    }

    fn flush_dash(&mut self) {
        if self.dash_points.len() < 2 {
            self.dash_points.clear();
            return;
        }
        let points = std::mem::take(&mut self.dash_points);
        self.stroke_line(&points);
        self.dash_points = points;
        self.dash_points.clear();
    }

    fn stroke_line(&mut self, points: &[Point<Pixels>]) {
        self.stroke_line_marked(points, |_| {});
    }

    /// `stroke_line` that calls `mark` with every vertex so far after each segment.
    fn stroke_line_marked(
        &mut self,
        points: &[Point<Pixels>],
        mut mark: impl FnMut(&[PathVertex<Pixels>]),
    ) {
        if self.half_width <= 0.0 || points.len() < 2 {
            return;
        }
        let Some((mut direction, mut normal, mut length)) =
            segment_frame(points[0], points[1], self.half_width)
        else {
            return;
        };
        let mut start = offset_pair(points[0], normal);
        for i in 0..points.len() - 1 {
            let segment_direction = direction;
            let segment_normal = normal;
            let p = points[i + 1];
            let last = i + 2 == points.len();
            let (mut end, next_start, bevel) = if let Some(&next) = points.get(i + 2) {
                match segment_frame(p, next, self.half_width) {
                    Some((next_direction, next_normal, next_length)) => {
                        let join = stroke_join(
                            p,
                            (direction, normal, length),
                            (next_direction, next_normal, next_length),
                            self.half_width,
                        );
                        direction = next_direction;
                        normal = next_normal;
                        length = next_length;
                        join
                    }
                    None => {
                        let pair = offset_pair(p, normal);
                        (pair, pair, None)
                    }
                }
            } else {
                let pair = offset_pair(p, normal);
                (pair, pair, None)
            };
            // Like lyon, a cap clips the segment's own side lines, not the join vertices: a
            // folded join puts both join vertices on the outer side.
            if i == 0 {
                let side = offset_pair(p, segment_normal);
                start = butt_cap(points[0], start, side, segment_direction, true);
            }
            if last {
                let side = offset_pair(points[i], segment_normal);
                end = butt_cap(p, end, side, segment_direction, false);
            }
            self.quad(start, end);
            if let Some((a, b, c)) = bevel {
                self.tri(a, b, c);
            }
            mark(self.vertices());
            start = next_start;
        }
    }

    /// Segment `i` of `stroke_line(points)` on its own: the start join (or butt cap) at
    /// `points[i]`, the end join (or cap) at `points[i + 1]` and that join's bevel. It reads
    /// `points[i - 1 ..= i + 2]` only. Vertex for vertex the same as `stroke_line` when no
    /// segment of `points` has zero length (`has_short_segment` is false); `stroke_line` carries
    /// the previous direction over such a segment.
    fn segment(&mut self, points: &[Point<Pixels>], i: usize) {
        let hw = self.half_width;
        if hw <= 0.0 {
            return;
        }
        let Some(frame) = segment_frame(points[i], points[i + 1], hw) else {
            return;
        };
        let (direction, normal, _) = frame;
        let mut start = match i
            .checked_sub(1)
            .and_then(|h| segment_frame(points[h], points[i], hw))
        {
            Some(previous) => stroke_join(points[i], previous, frame, hw).1,
            None => offset_pair(points[i], normal),
        };
        let p = points[i + 1];
        let next = points
            .get(i + 2)
            .and_then(|&next| segment_frame(p, next, hw));
        let (mut end, bevel) = match next {
            Some(next) => {
                let (end, _, bevel) = stroke_join(p, frame, next, hw);
                (end, bevel)
            }
            None => (offset_pair(p, normal), None),
        };
        if i == 0 {
            let side = offset_pair(p, normal);
            start = butt_cap(points[0], start, side, direction, true);
        }
        if i + 2 == points.len() {
            let side = offset_pair(points[i], normal);
            end = butt_cap(p, end, side, direction, false);
        }
        self.quad(start, end);
        if let Some((a, b, c)) = bevel {
            self.tri(a, b, c);
        }
    }

    /// Vertices pushed so far.
    fn len(&self) -> usize {
        self.path.as_ref().map_or(0, |p| p.vertices.len())
    }

    fn vertices(&self) -> &[PathVertex<Pixels>] {
        self.path.as_ref().map_or(&[], |p| &p.vertices)
    }

    /// The vertex buffer (the unused spare when no triangle was added) with the bounds so far.
    fn into_parts(self) -> MeshParts {
        let vertices = match (self.path, self.spare) {
            (Some(path), _) => path.vertices,
            (None, Some(mut spare)) => {
                spare.vertices.clear();
                spare.vertices
            }
            (None, None) => Vec::new(),
        };
        (vertices, self.min, self.max)
    }

    fn quad(&mut self, start: (Point<Pixels>, Point<Pixels>), end: (Point<Pixels>, Point<Pixels>)) {
        self.tri(start.1, start.0, end.0);
        self.tri(start.1, end.0, end.1);
    }

    fn finish(&mut self) -> Option<Path<Pixels>> {
        let mut path = self.path.take().filter(|p| !p.vertices.is_empty())?;
        path.bounds = Bounds {
            origin: point(px(self.min.0), px(self.min.1)),
            size: size(px(self.max.0 - self.min.0), px(self.max.1 - self.min.1)),
        };
        Some(path)
    }
}

/// Unit direction, normal scaled to the half width, and length of one segment.
type SegmentFrame = ((f32, f32), (f32, f32), f32);

fn segment_frame(a: Point<Pixels>, b: Point<Pixels>, half_width: f32) -> Option<SegmentFrame> {
    let (dx, dy) = (f32::from(b.x - a.x), f32::from(b.y - a.y));
    let len = (dx * dx + dy * dy).sqrt();
    (len > f32::EPSILON).then(|| {
        let direction = (dx / len, dy / len);
        let normal = (-direction.1 * half_width, direction.0 * half_width);
        (direction, normal, len)
    })
}

fn offset_pair(p: Point<Pixels>, normal: (f32, f32)) -> (Point<Pixels>, Point<Pixels>) {
    (
        point(p.x + px(normal.0), p.y + px(normal.1)),
        point(p.x - px(normal.0), p.y - px(normal.1)),
    )
}

fn butt_cap(
    p: Point<Pixels>,
    pair: (Point<Pixels>, Point<Pixels>),
    other: (Point<Pixels>, Point<Pixels>),
    direction: (f32, f32),
    start: bool,
) -> (Point<Pixels>, Point<Pixels>) {
    let normal = if start {
        (-direction.0, -direction.1)
    } else {
        direction
    };
    let cap_vector = (-normal.1, normal.0);
    (
        cap_intersection(p, cap_vector, pair.0, other.0),
        cap_intersection(p, cap_vector, pair.1, other.1),
    )
}

/// Intersects in f64 and converts back to f32, matching Lyon's butt-cap path exactly.
fn cap_intersection(
    cap_point: Point<Pixels>,
    cap_vector: (f32, f32),
    side_point: Point<Pixels>,
    other_side: Point<Pixels>,
) -> Point<Pixels> {
    let cap = (f32::from(cap_point.x) as f64, f32::from(cap_point.y) as f64);
    let cap_v = (cap_vector.0 as f64, cap_vector.1 as f64);
    let side = (
        f32::from(side_point.x) as f64,
        f32::from(side_point.y) as f64,
    );
    let side_v = (
        (f32::from(side_point.x) - f32::from(other_side.x)) as f64,
        (f32::from(side_point.y) - f32::from(other_side.y)) as f64,
    );
    let cross = |a: (f64, f64), b: (f64, f64)| a.0 * b.1 - a.1 * b.0;
    let det = cross(cap_v, side_v);
    if det.abs() <= f64::EPSILON {
        return side_point;
    }
    let cap_p2 = (cap.0 + cap_v.0, cap.1 + cap_v.1);
    let side_p2 = (side.0 + side_v.0, side.1 + side_v.1);
    let a = cross(cap, cap_p2);
    let b = cross(side, side_p2);
    let inv_det = 1.0 / det;
    point(
        px(((b * cap_v.0 - a * side_v.0) * inv_det) as f32),
        px(((b * cap_v.1 - a * side_v.1) * inv_det) as f32),
    )
}

type StrokeJoin = (
    (Point<Pixels>, Point<Pixels>),
    (Point<Pixels>, Point<Pixels>),
    Option<(Point<Pixels>, Point<Pixels>, Point<Pixels>)>,
);

fn stroke_join(
    p: Point<Pixels>,
    (direction, previous_normal, previous_length): SegmentFrame,
    (next_direction, next_normal, next_length): SegmentFrame,
    half_width: f32,
) -> StrokeJoin {
    let tangent = (
        direction.0 + next_direction.0,
        direction.1 + next_direction.1,
    );
    let tangent_sq = tangent.0 * tangent.0 + tangent.1 * tangent.1;
    let previous_unit_normal = (-direction.1, direction.0);
    let miter_normal = if tangent_sq < 1e-4 {
        (0.0, 0.0)
    } else {
        let tangent_len = tangent_sq.sqrt();
        let unit = (-tangent.1 / tangent_len, tangent.0 / tangent_len);
        let inv_len = unit.0 * previous_unit_normal.0 + unit.1 * previous_unit_normal.1;
        if inv_len.abs() < 1e-4 {
            previous_unit_normal
        } else {
            (unit.0 / inv_len, unit.1 / inv_len)
        }
    };
    let miter_sq = miter_normal.0 * miter_normal.0 + miter_normal.1 * miter_normal.1;
    let miter = (miter_normal.0 * half_width, miter_normal.1 * half_width);
    let miter_pair = offset_pair(p, miter);
    // Lyon defines the miter ratio against the full stroke width, so its default limit of 4
    // permits a normal up to 8 times the half width.
    if miter_sq <= 4.0 * 4.0 * 4.0 {
        return (miter_pair, miter_pair, None);
    }

    let previous = offset_pair(p, previous_normal);
    let next = offset_pair(p, next_normal);
    let cross = direction.0 * next_direction.1 - direction.1 * next_direction.0;
    // `outer` points from `p` to the outer side. The inner miter point is `p - outer`.
    let outer = if cross >= 0.0 {
        (-miter.0, -miter.1)
    } else {
        miter
    };
    // Lyon's fold: on a sharp turn, when the inner point lies past the far end of both
    // segments, it would draw a spike outside the stroke. Both quads then meet at the two
    // outer side points instead, and the join adds no triangle.
    let sharp = direction.0 * next_direction.0 + direction.1 * next_direction.1 < 0.0;
    let past_next = -(outer.0 * next_direction.0 + outer.1 * next_direction.1) - next_length;
    let past_previous = outer.0 * direction.0 + outer.1 * direction.1 - previous_length;
    let fold = sharp && past_next.min(past_previous) > 0.0;
    match (cross >= 0.0, fold) {
        (true, true) => ((next.1, previous.1), (previous.1, next.1), None),
        (false, true) => ((previous.0, next.0), (next.0, previous.0), None),
        (true, false) => (
            (miter_pair.0, previous.1),
            (miter_pair.0, next.1),
            Some((previous.1, miter_pair.0, next.1)),
        ),
        (false, false) => (
            (previous.0, miter_pair.1),
            (next.0, miter_pair.1),
            Some((miter_pair.1, previous.0, next.0)),
        ),
    }
}

/// Builders keyed by colour (and dash for strokes) in insertion order (rule 7). Created on first
/// use, so a colour with no points adds no set. Kept in `Scratch` so the lists do not reallocate
/// per build when a stacked chart has more colours than the inline size.
#[derive(Default)]
struct Builders {
    fills: SmallVec<[(Hsla, FillMesh); 2]>,
    strokes: SmallVec<[(Hsla, bool, StrokeMesh); 3]>,
    /// Fill paths of the previous build, kept for their vertex buffers.
    spare: Vec<Path<Pixels>>,
    /// Stroke paths of the previous build, kept for their vertex buffers.
    stroke_spare: Vec<Path<Pixels>>,
}

impl Builders {
    fn mesh(&mut self) -> FillMesh {
        FillMesh::reusing(self.spare.pop())
    }

    fn fill(&mut self, color: Hsla) -> &mut FillMesh {
        let i = match self.fills.iter().position(|(c, _)| *c == color) {
            Some(i) => i,
            None => {
                let mesh = self.mesh();
                self.fills.push((color, mesh));
                self.fills.len() - 1
            }
        };
        &mut self.fills[i].1
    }

    /// Keeps the fill paths of `sets` for reuse and empties it.
    fn reclaim_fills(&mut self, sets: &mut SmallVec<[PathSet; 2]>) {
        self.spare.extend(sets.drain(..).map(|set| set.path));
    }

    fn reclaim_strokes(&mut self, sets: &mut SmallVec<[PathSet; 3]>) {
        self.stroke_spare.extend(sets.drain(..).map(|set| set.path));
    }

    fn stroke(&mut self, s: &ChartSeries) -> &mut StrokeMesh {
        let i = match self
            .strokes
            .iter()
            .position(|(c, d, _)| *c == s.line && *d == s.dashed)
        {
            Some(i) => i,
            None => {
                let mesh =
                    StrokeMesh::reusing(self.stroke_spare.pop(), f32::from(s.line_width), s.dashed);
                self.strokes.push((s.line, s.dashed, mesh));
                self.strokes.len() - 1
            }
        };
        &mut self.strokes[i].2
    }

    /// One fill polygon (when the series has a fill) and one stroke for `pts`, straight or curved
    /// per `input.curved`. The polygon goes down to the baseline, or with `floors` (one per point,
    /// stacked bands) back along the floor, which follows the same kind of curve.
    fn emit(
        &mut self,
        s: &ChartSeries,
        pts: &[Pt],
        floors: Option<&[f32]>,
        input: &BuildInput,
        buf: &mut Line,
    ) {
        let Line {
            tangents,
            line,
            floor,
            floor_pts,
        } = buf;
        emit_run(pts, input.curved, input, tangents, line);
        let Some((_, rest)) = line.split_first() else {
            return;
        };
        let h = px(input.plot_height);
        if let Some(fill) = s.fill
            && !rest.is_empty()
        {
            match floors {
                Some(floors) => {
                    floor_pts.clear();
                    floor_pts.extend(pts.iter().zip(floors).map(|(p, f)| Pt {
                        t_ns: p.t_ns,
                        value: *f,
                    }));
                    // Same points in time, same curve kind: `floor` has one vertex per line
                    // vertex, at the same x.
                    emit_run(floor_pts, input.curved, input, tangents, floor);
                    let floor = &floor[..];
                    self.fill(fill)
                        .band(line, |i, p| floor.get(i).copied().unwrap_or(point(p.x, h)));
                }
                None => self.fill(fill).band(line, |_, p| point(p.x, h)),
            }
        }
        self.stroke(s).stroke(line);
    }

    /// One stacked band over a stretch of the shared axis (UI-PRF-060). The top line goes
    /// through `pts` and the floor through `floors` at every point, so with curves the top of a
    /// band and the floor of the band above follow the same curve. Fill and stroke cover only
    /// the segments that `drawn` marks, one flag per segment.
    fn emit_band(
        &mut self,
        s: &ChartSeries,
        pts: &[Pt],
        floors: &[f32],
        drawn: &[bool],
        input: &BuildInput,
        buf: &mut Line,
    ) {
        let Line {
            tangents,
            line,
            floor,
            floor_pts,
        } = buf;
        emit_run(pts, input.curved, input, tangents, line);
        floor_pts.clear();
        floor_pts.extend(pts.iter().zip(floors).map(|(p, f)| Pt {
            t_ns: p.t_ns,
            value: *f,
        }));
        emit_run(floor_pts, input.curved, input, tangents, floor);
        // A curve has `CURVE_STEPS` line segments per segment between points.
        let step = if line.len() > pts.len() {
            CURVE_STEPS
        } else {
            1
        };
        let mut seg = 0;
        while seg < drawn.len() {
            if !drawn[seg] {
                seg += 1;
                continue;
            }
            let start = seg;
            while seg < drawn.len() && drawn[seg] {
                seg += 1;
            }
            let top = &line[start * step..=seg * step];
            let bottom = &floor[start * step..=seg * step];
            if let Some(fill) = s.fill {
                self.fill(fill).band(top, |i, _| bottom[i]);
            }
            self.stroke(s).stroke(top);
        }
    }

    /// Moves the finished direct meshes into the cache and leaves the builder lists empty.
    fn finish(&mut self, fills: &mut SmallVec<[PathSet; 2]>, strokes: &mut SmallVec<[PathSet; 3]>) {
        self.reclaim_fills(fills);
        self.reclaim_strokes(strokes);
        fills.extend(
            self.fills
                .drain(..)
                .filter_map(|(color, mesh)| fill_set(color, mesh)),
        );
        strokes.extend(
            self.strokes
                .drain(..)
                .filter_map(|(color, dashed, mut mesh)| stroke_set(color, dashed, &mut mesh)),
        );
    }
}

fn fill_set(color: Hsla, mesh: FillMesh) -> Option<PathSet> {
    Some(PathSet {
        color,
        dashed: false,
        path: mesh.finish()?,
    })
}

fn stroke_set(color: Hsla, dashed: bool, mesh: &mut StrokeMesh) -> Option<PathSet> {
    Some(PathSet {
        color,
        dashed,
        path: mesh.finish()?,
    })
}

/// Builds fill and stroke paths, their stale sets and the dotted-gap spans into `cache`
/// (ADR-009 rules 2, 4, 6, 7; UI-PRF-001; UI-PRF-007; PERF-030). Paths are chart-local physical
/// pixels at a zero origin; the element translates them at paint time and owns the scale factor.
///
/// Fresh samples go to the live paths. `STALE` samples go to the stale paths, joined to their
/// fresh neighbours so the line does not break. When the series is stale (R-051), a flat tail
/// from the newest sample to `map.right_edge_ns` at the newest value goes to the stale paths too.
/// Gaps come from the first series only. `cache.newest` gets each series' newest drawn point.
///
/// With `input.stack`, series are stacked in order on one shared time axis (`StackAxis`,
/// UI-PRF-060): band `k` goes from the sum of series `0..k` up to the sum of series `0..=k` at
/// every axis time, and its fill goes down to that floor instead of the baseline.
pub fn build(input: &BuildInput, cache: &mut ChartCache, scratch: &mut Scratch) {
    let map = &input.map;
    let Scratch {
        runs,
        pts,
        stale_pts,
        meaned,
        pt_floors,
        stack,
        line,
        live,
        stale,
        ..
    } = scratch;
    cache.gaps.clear();
    cache.newest.clear();
    // The old path vertex buffers serve this build's direct meshes.
    live.reclaim_fills(&mut cache.fills);
    live.reclaim_strokes(&mut cache.strokes);
    stale.reclaim_fills(&mut cache.stale_fills);
    stale.reclaim_strokes(&mut cache.stale_strokes);
    let bucket = bucket_ns(map.range_ns, map.plot_width.max(1.0).round() as u32);

    if input.stack {
        if let Some((spec, Some(view))) = input.series.first() {
            let from = window_start(
                &view.samples,
                map.right_edge_ns,
                map.range_ns,
                view.period_ns,
            );
            split_runs(&view.samples, from, runs);
            first_series_gaps(
                spec,
                &view.samples,
                runs,
                view.period_ns,
                input,
                &mut cache.gaps,
                live,
                pts,
                line,
            );
        }
        stack.build(input, bucket, runs);
        build_stacked(
            input, stack, cache, pts, stale_pts, pt_floors, line, live, stale,
        );
    }

    // Series drawn each on its own: all of them unless stacked.
    let single = if input.stack { &[][..] } else { input.series };
    for (si, (spec, view)) in single.iter().enumerate() {
        let Some(view) = view else {
            continue;
        };
        let period = view.period_ns;
        let from = window_start(&view.samples, map.right_edge_ns, map.range_ns, period);
        let (samples, from): (&[Sample], usize) = if spec.mean_ns > 0 {
            mean_samples(&view.samples, from, spec.mean_ns, meaned);
            (&meaned[..], 0)
        } else {
            (&view.samples[..], from)
        };
        split_runs(samples, from, runs);

        if si == 0 {
            first_series_gaps(
                spec,
                samples,
                runs,
                period,
                input,
                &mut cache.gaps,
                live,
                pts,
                line,
            );
        }

        // Stretches come in time order, so the last line vertex is the newest drawn sample.
        let mut newest = None;
        for run in runs.iter() {
            for_each_stretch(samples, *run, |span, is_stale_flag| {
                let (out, builders) = if is_stale_flag {
                    (&mut *stale_pts, &mut *stale)
                } else {
                    (&mut *pts, &mut *live)
                };
                decimate(samples, span, bucket, out);
                builders.emit(spec, out, None, input, line);
                newest = line.line.last().copied().or(newest);
            });
        }
        if let Some(p) = newest {
            cache.newest.push((p, spec.line));
        }

        // Flat continuation of a stale series to the right edge (UI-PRF-007). Only when the
        // series ends on a valid sample: newer gap samples are a gap, not a stale tail.
        if let Some(last_run) = runs.last().filter(|r| r.end == samples.len()) {
            let newest = samples[last_run.end - 1];
            if is_stale(newest.t_ns, input.now_boot_ns, period) && map.right_edge_ns > newest.t_ns {
                stale_pts.clear();
                stale_pts.push(Pt {
                    t_ns: newest.t_ns,
                    value: newest.value,
                });
                stale_pts.push(Pt {
                    t_ns: map.right_edge_ns,
                    value: newest.value,
                });
                stale.emit(spec, stale_pts, None, input, line);
            }
        }
    }

    live.finish(&mut cache.fills, &mut cache.strokes);
    stale.finish(&mut cache.stale_fills, &mut cache.stale_strokes);
    cache.anchor_ns = map.anchor_ns;
    cache.px_per_ns = map.px_per_ns();
    cache.y_min = input.y_min;
    cache.y_max = input.y_max;
}

/// The dotted-gap spans of series 0 into `gaps`, or with `GapStyle::Dashed` a dashed line across
/// each gap, from the series' `runs` (PERF-022).
#[allow(clippy::too_many_arguments)]
fn first_series_gaps(
    spec: &ChartSeries,
    samples: &[Sample],
    runs: &[Run],
    period_ns: u64,
    input: &BuildInput,
    gaps: &mut SmallVec<[std::ops::Range<f32>; 4]>,
    live: &mut Builders,
    pts: &mut Vec<Pt>,
    line: &mut Line,
) {
    if spec.gap_style != GapStyle::Dashed {
        push_gaps(samples, runs, &input.map, period_ns, gaps);
        return;
    }
    let dashed = ChartSeries {
        fill: None,
        dashed: true,
        ..spec.clone()
    };
    for w in runs.windows(2) {
        let (a, b) = (samples[w[0].end - 1], samples[w[1].start]);
        pts.clear();
        pts.extend([a, b].map(|s| Pt {
            t_ns: s.t_ns,
            value: s.value,
        }));
        live.emit(&dashed, pts, None, input, line);
    }
}

/// Emits the bands of a built `StackAxis`. A band draws the segments where it has a value at
/// either end, so it ramps to and from zero height beside its gaps and draws nothing inside
/// them. Its newest mark is its newest point with a value. When the stack is stale (R-051),
/// each band with a value at the newest axis time gets a flat stale tail to the right edge.
#[allow(clippy::too_many_arguments)]
fn build_stacked(
    input: &BuildInput,
    stack: &mut StackAxis,
    cache: &mut ChartCache,
    pts: &mut Vec<Pt>,
    stale_pts: &mut Vec<Pt>,
    pt_floors: &mut Vec<f32>,
    line: &mut Line,
    live: &mut Builders,
    stale: &mut Builders,
) {
    let map = &input.map;
    let StackAxis {
        bands,
        axis,
        tops,
        marks,
        kept,
        stretches,
        drawn,
        ..
    } = stack;
    let n = axis.len();
    for (b, &(si, _)) in bands.iter().enumerate() {
        let (spec, Some(view)) = &input.series[si] else {
            continue;
        };
        let top = &tops[b * n..(b + 1) * n];
        let floor = |k: usize| b.checked_sub(1).map_or(0.0, |lo| tops[lo * n + k]);
        let mark = &marks[b * n..(b + 1) * n];
        let mut newest = None;
        for (range, is_stale_flag) in stretches.iter() {
            let kept = &kept[range.clone()];
            drawn.clear();
            drawn.extend(
                kept.windows(2)
                    .map(|w| (mark[w[0]] | mark[w[1]]) & HAS != 0),
            );
            if !drawn.contains(&true) {
                continue;
            }
            pts.clear();
            pt_floors.clear();
            for &k in kept {
                pts.push(Pt {
                    t_ns: axis[k].t_ns,
                    value: top[k],
                });
                pt_floors.push(floor(k));
            }
            let builders = if *is_stale_flag {
                &mut *stale
            } else {
                &mut *live
            };
            builders.emit_band(spec, pts, pt_floors, drawn, input, line);
            if let Some(&k) = kept.iter().rev().find(|&&k| mark[k] & HAS != 0) {
                newest = Some(point(
                    px(map.x_local(axis[k].t_ns)),
                    px(y_of(top[k], input.y_min, input.y_max, input.plot_height)),
                ));
            }
        }
        if let Some(p) = newest {
            cache.newest.push((p, spec.line));
        }

        // Flat continuation to the right edge (UI-PRF-007), at the band's top and floor.
        if let Some(k) = n.checked_sub(1).filter(|&k| mark[k] & HAS != 0) {
            let t = axis[k].t_ns;
            if is_stale(t, input.now_boot_ns, view.period_ns) && map.right_edge_ns > t {
                stale_pts.clear();
                stale_pts.extend([t, map.right_edge_ns].map(|t_ns| Pt {
                    t_ns,
                    value: top[k],
                }));
                pt_floors.clear();
                pt_floors.extend([floor(k); 2]);
                stale.emit(spec, stale_pts, Some(pt_floors), input, line);
            }
        }
    }
}

/// Dotted-gap spans of one series: between consecutive runs, and from the window's left edge to
/// the first run when that run starts more than 2 periods in. Nothing on the right: a stale tail
/// reaches the right edge, and trailing gap samples draw no dotted span (leader note o55.7).
fn push_gaps(
    samples: &[Sample],
    runs: &[Run],
    map: &TimeMap,
    period_ns: u64,
    gaps: &mut SmallVec<[std::ops::Range<f32>; 4]>,
) {
    let Some(first) = runs.first() else {
        return;
    };
    let left_ns = map.right_edge_ns.saturating_sub(map.range_ns);
    let first_ns = samples[first.start].t_ns;
    if first_ns > left_ns.saturating_add(period_ns.saturating_mul(2)) {
        gaps.push(map.x_local(left_ns)..map.x_local(first_ns));
    }
    for w in runs.windows(2) {
        let a = samples[w[0].end - 1].t_ns;
        let b = samples[w[1].start].t_ns;
        gaps.push(map.x_local(a)..map.x_local(b));
    }
}

/// Calls `f` for each stretch of fresh or `STALE` samples within `run`, in order, with whether it
/// is stale. A stale span takes one fresh neighbour on each side so the line does not break.
fn for_each_stretch(samples: &[Sample], run: Run, mut f: impl FnMut(Run, bool)) {
    let mut i = run.start;
    while i < run.end {
        let is_stale_flag = samples[i].flags.contains(SampleFlags::STALE);
        let mut j = i + 1;
        while j < run.end && samples[j].flags.contains(SampleFlags::STALE) == is_stale_flag {
            j += 1;
        }
        let span = if is_stale_flag {
            Run {
                start: i.saturating_sub(1).max(run.start),
                end: (j + 1).min(run.end),
            }
        } else {
            Run { start: i, end: j }
        };
        f(span, is_stale_flag);
        i = j;
    }
}

/// Line width of grid cell strokes, physical px.
const GRID_LINE_WIDTH: f32 = 1.0;

/// One grid cell series' live geometry (tasklens-wlf.39): the clipped points and the fill and
/// stroke vertices built from them, physical px relative to the element. A grid keeps one piece
/// per (cell, series) across builds. When a build sees the same points with new ones at either
/// end, it emits only the segments at the ends and keeps the vertices of the rest
/// (`build_cell` with `append`).
#[derive(Default)]
pub struct CellPiece {
    /// The points `fill` and `stroke` were built from, while `splice` is set.
    pts: Vec<Point<Pixels>>,
    fill: PieceMesh,
    stroke: PieceMesh,
    /// The piece is one live run with no short segment, and both meshes record their vertex
    /// count per segment, so the next build may splice.
    splice: bool,
}

impl CellPiece {
    /// Empties the piece: an offline cell or an absent series.
    pub fn clear(&mut self) {
        self.pts.clear();
        self.fill.clear();
        self.stroke.clear();
        self.splice = false;
    }

    /// Fill and stroke vertices, physical px relative to the element.
    pub fn vertices(&self) -> impl Iterator<Item = &PathVertex<Pixels>> {
        self.fill.verts.iter().chain(&self.stroke.verts)
    }

    pub fn has_fill(&self) -> bool {
        !self.fill.verts.is_empty()
    }

    pub fn has_stroke(&self) -> bool {
        !self.stroke.verts.is_empty()
    }
}

/// One segment of a piece mesh: its vertex count and the smallest and largest x and y of those
/// vertices (`f32::MAX` and `f32::MIN` when it has none).
#[derive(Clone, Copy, Debug)]
struct Seg {
    n: u8,
    min: (f32, f32),
    max: (f32, f32),
}

impl Seg {
    /// The segment made of `verts`.
    fn of(verts: &[PathVertex<Pixels>]) -> Self {
        let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for v in verts {
            let (x, y) = (f32::from(v.xy_position.x), f32::from(v.xy_position.y));
            min = (min.0.min(x), min.1.min(y));
            max = (max.0.max(x), max.1.max(y));
        }
        Self {
            n: verts.len() as u8,
            min,
            max,
        }
    }
}

/// One mesh of a piece. A deque, so dropping old segments at the front and adding new ones at
/// either end moves no other vertex.
#[derive(Default)]
struct PieceMesh {
    verts: VecDeque<PathVertex<Pixels>>,
    /// Each segment's vertex count and bounds, oldest first, while the piece can splice.
    seg: VecDeque<Seg>,
    /// Smallest and largest x and y of `verts`.
    min: (f32, f32),
    max: (f32, f32),
}

impl PieceMesh {
    fn clear(&mut self) {
        self.verts.clear();
        self.seg.clear();
    }

    /// Empties the mesh and hands its vertex buffer to a mesh builder for reuse.
    fn take_buffer(&mut self) -> Path<Pixels> {
        self.seg.clear();
        let mut verts = std::mem::take(&mut self.verts);
        verts.clear();
        spare_path(Vec::from(verts))
    }

    fn set(&mut self, (vertices, min, max): MeshParts) {
        self.verts = VecDeque::from(vertices);
        self.min = min;
        self.max = max;
    }

    /// Drops every segment outside `keep`.
    fn keep(&mut self, keep: std::ops::Range<usize>) {
        let count = |segs: std::collections::vec_deque::Iter<'_, Seg>| -> usize {
            segs.map(|s| usize::from(s.n)).sum()
        };
        let tail = count(self.seg.range(keep.end..));
        self.verts.truncate(self.verts.len() - tail);
        self.seg.truncate(keep.end);
        let head = count(self.seg.range(..keep.start));
        self.verts.drain(..head);
        self.seg.drain(..keep.start);
    }

    /// Adds `verts` with their segments `seg` before (`front`) or after the rest.
    fn add(&mut self, verts: &[PathVertex<Pixels>], seg: &[Seg], front: bool) {
        if front {
            for v in verts.iter().rev() {
                self.verts.push_front(v.clone());
            }
            for &n in seg.iter().rev() {
                self.seg.push_front(n);
            }
        } else {
            self.verts.extend(verts.iter().cloned());
            self.seg.extend(seg);
        }
    }

    /// The bounds again after a splice, from the segments' bounds: the same numbers as a scan
    /// of every vertex.
    fn rebound(&mut self) {
        let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for s in &self.seg {
            min = (min.0.min(s.min.0), min.1.min(s.min.1));
            max = (max.0.max(s.max.0), max.1.max(s.max.1));
        }
        self.min = min;
        self.max = max;
    }
}

/// Vertex buffers for the segments a splice emits at one end of a piece.
#[derive(Default)]
struct SpliceBufs {
    fill: Vec<PathVertex<Pixels>>,
    stroke: Vec<PathVertex<Pixels>>,
    fill_seg: Vec<Seg>,
    stroke_seg: Vec<Seg>,
}

impl SpliceBufs {
    /// Emits segments `range` of `pts` one at a time into the buffers: the fill down to
    /// `baseline` (when given) and the stroke. Returns the vertices emitted.
    fn emit(
        &mut self,
        pts: &[Point<Pixels>],
        range: std::ops::Range<usize>,
        baseline: Option<Pixels>,
    ) -> usize {
        self.fill_seg.clear();
        self.stroke_seg.clear();
        let mut fill = FillMesh::reusing(Some(spare_path(std::mem::take(&mut self.fill))));
        let mut stroke = StrokeMesh::reusing(
            Some(spare_path(std::mem::take(&mut self.stroke))),
            GRID_LINE_WIDTH,
            false,
        );
        for j in range {
            if let Some(b) = baseline {
                let n = fill.len();
                fill.band(&pts[j..j + 2], |_, p| point(p.x, b));
                self.fill_seg.push(Seg::of(&fill.vertices()[n..]));
            }
            let n = stroke.len();
            stroke.segment(pts, j);
            self.stroke_seg.push(Seg::of(&stroke.vertices()[n..]));
        }
        self.fill = fill.into_parts().0;
        self.stroke = stroke.into_parts().0;
        self.fill.len() + self.stroke.len()
    }

    /// Adds the buffers to `piece`'s meshes at the front or the back.
    fn add_to(&self, piece: &mut CellPiece, fill: bool, front: bool) {
        if fill {
            piece.fill.add(&self.fill, &self.fill_seg, front);
        }
        piece.stroke.add(&self.stroke, &self.stroke_seg, front);
    }
}

/// True when two neighbouring points are less than 1/1000 px apart. `stroke_line` carries the
/// previous direction over a zero-length segment, so such a line is never spliced.
fn has_short_segment(pts: &[Point<Pixels>]) -> bool {
    pts.windows(2).any(|w| {
        let (dx, dy) = (f32::from(w[1].x - w[0].x), f32::from(w[1].y - w[0].y));
        dx * dx + dy * dy <= 1e-6
    })
}

/// Splices `new` (one live run, no short segment) into `piece`. Every segment whose input
/// points (`j − 1 ..= j + 2`) are unchanged keeps its vertices; the segments before and after
/// them are emitted again. The point lists match bit for bit where both builds placed the same
/// sample, since an append keeps the anchor and the y range. Returns the vertices emitted, or
/// `None` when no segment can be kept.
fn splice_piece(
    piece: &mut CellPiece,
    new: &[Point<Pixels>],
    baseline: Option<Pixels>,
    bufs: &mut SpliceBufs,
) -> Option<usize> {
    let old = &piece.pts;
    let segs = old.len().checked_sub(1)?;
    let fill = baseline.is_some();
    if !piece.splice
        || new.len() < 4
        || piece.stroke.seg.len() != segs
        || (fill && piece.fill.seg.len() != segs)
    {
        return None;
    }
    // The first stretch of at least 4 new points that are also old points, in order. x grows
    // along both lists. A shorter match is a coincidence at the re-decimated start.
    let (i, k, m) = (0..new.len()).find_map(|i| {
        let k = old.partition_point(|o| o.x < new[i].x);
        if old.get(k) != Some(&new[i]) {
            return None;
        }
        let m = new[i..]
            .iter()
            .zip(&old[k..])
            .take_while(|(a, b)| a == b)
            .count();
        (m >= 4).then_some((i, k, m))
    })?;
    // New segments `i + 1 .. i + m − 2` read matched points only; they are old segments
    // `k + 1 .. k + m − 2`.
    let keep = (i + 1, i + m - 2);
    piece.stroke.keep(k + 1..k + m - 2);
    if fill {
        piece.fill.keep(k + 1..k + m - 2);
    }
    let mut emitted = bufs.emit(new, 0..keep.0, baseline);
    bufs.add_to(piece, fill, true);
    emitted += bufs.emit(new, keep.1..new.len() - 1, baseline);
    bufs.add_to(piece, fill, false);
    piece.stroke.rebound();
    if fill {
        piece.fill.rebound();
    }
    Some(emitted)
}

/// Builds one grid cell (UI-PRF-002, ADR-009 rule 7) into its two pieces: series 0 into
/// `pieces[0]` (fill when it has a fill colour, and stroke), series 1 into `pieces[1]` (stroke
/// only); further series are ignored. Every vertex is moved by `origin` (physical px, the cell's
/// plot corner relative to the element). `STALE` stretches and the stale tail go to the
/// scratch's stale builders, which `finish_grid` finishes.
///
/// One content mask covers the whole grid, so x is clipped at build time to the `clip_w` wide
/// window that `map` puts on screen (chart-local `−map.dx(clip_w) ..` that plus `clip_w`):
/// points outside are dropped and the boundary point is interpolated. No stacking.
///
/// With `append` (tasklens-wlf.39) the pieces hold an earlier build with the same anchor, y
/// range, layout and scale, and a piece that is still one live run is spliced (`splice_piece`);
/// other pieces are emitted in full. Stale content returns `None` and changes no stale set: the
/// stale sets are shared, so the caller builds every cell in full instead. Otherwise returns
/// the vertices emitted.
///
/// Series 0's dotted-gap spans replace `scratch.cell_gaps()`, chart-local and unclipped as
/// `build` makes them. The caller paints them as quads per cell (PERF-022).
pub fn build_cell(
    input: &BuildInput,
    origin: (f32, f32),
    clip_w: f32,
    pieces: &mut [CellPiece; 2],
    append: bool,
    scratch: &mut Scratch,
) -> Option<usize> {
    let map = &input.map;
    let Scratch {
        runs,
        pts,
        stale_pts,
        clipped,
        cell_gaps,
        stale,
        splice,
        ..
    } = scratch;
    cell_gaps.clear();
    let bucket = bucket_ns(map.range_ns, clip_w.max(1.0).round() as u32);
    let place = CellPlace {
        origin,
        lo: -map.dx(clip_w),
        clip_w,
    };
    let mut emitted = 0;

    for (si, piece) in pieces.iter_mut().enumerate() {
        let Some((spec, Some(view))) = input.series.get(si) else {
            piece.clear();
            continue;
        };
        let period = view.period_ns;
        let samples = &view.samples[..];
        let from = window_start(samples, map.right_edge_ns, map.range_ns, period);
        split_runs(samples, from, runs);

        if si == 0 {
            push_gaps(samples, runs, map, period, cell_gaps);
        }

        // Flat continuation of a stale series to the right edge (UI-PRF-007). Only when the
        // series ends on a valid sample: newer gap samples are a gap, not a stale tail.
        let stale_tail = runs
            .last()
            .filter(|r| r.end == samples.len())
            .map(|r| samples[r.end - 1])
            .filter(|s| is_stale(s.t_ns, input.now_boot_ns, period) && map.right_edge_ns > s.t_ns);
        let baseline = (si == 0 && spec.fill.is_some()).then(|| place.baseline(input));

        if append {
            let has_stale = runs.iter().any(|r| {
                samples[r.start..r.end]
                    .iter()
                    .any(|s| s.flags.contains(SampleFlags::STALE))
            });
            if has_stale || stale_tail.is_some() {
                return None;
            }
            if let [run] = runs[..] {
                decimate(samples, run, bucket, pts);
                place.clip(pts, input, clipped);
                if !has_short_segment(clipped)
                    && let Some(n) = splice_piece(piece, clipped, baseline, splice)
                {
                    std::mem::swap(&mut piece.pts, clipped);
                    emitted += n;
                    continue;
                }
            }
        }

        // The whole piece: live stretches into its meshes, `STALE` ones into the stale sets.
        let mut fill = FillMesh::reusing(Some(piece.fill.take_buffer()));
        let mut stroke =
            StrokeMesh::reusing(Some(piece.stroke.take_buffer()), GRID_LINE_WIDTH, false);
        let CellPiece {
            pts: piece_pts,
            fill: piece_fill,
            stroke: piece_stroke,
            splice: piece_splice,
        } = piece;
        piece_pts.clear();
        let mut live = 0;
        let mut splice_ok = stale_tail.is_none();
        for run in runs.iter() {
            for_each_stretch(samples, *run, |span, is_stale_flag| {
                let out = if is_stale_flag {
                    &mut *stale_pts
                } else {
                    &mut *pts
                };
                decimate(samples, span, bucket, out);
                place.clip(out, input, clipped);
                if is_stale_flag {
                    splice_ok = false;
                    emit_stale(stale, spec, si == 0, clipped, &place, input);
                    return;
                }
                if clipped.is_empty() {
                    return;
                }
                live += 1;
                if let Some(b) = baseline {
                    for w in clipped.windows(2) {
                        let n = fill.len();
                        fill.band(w, |_, p| point(p.x, b));
                        piece_fill.seg.push_back(Seg::of(&fill.vertices()[n..]));
                    }
                }
                if clipped.len() >= 2 && !has_short_segment(clipped) {
                    let mut last = stroke.len();
                    stroke.stroke_line_marked(clipped, |verts| {
                        piece_stroke.seg.push_back(Seg::of(&verts[last..]));
                        last = verts.len();
                    });
                    piece_pts.clear();
                    piece_pts.extend_from_slice(clipped);
                } else {
                    splice_ok = false;
                    stroke.stroke(clipped);
                }
            });
        }

        if let Some(newest) = stale_tail {
            stale_pts.clear();
            stale_pts.push(Pt {
                t_ns: newest.t_ns,
                value: newest.value,
            });
            stale_pts.push(Pt {
                t_ns: map.right_edge_ns,
                value: newest.value,
            });
            place.clip(stale_pts, input, clipped);
            emit_stale(stale, spec, si == 0, clipped, &place, input);
        }

        let segs = piece_stroke.seg.len();
        *piece_splice = splice_ok
            && live == 1
            && segs + 1 == piece_pts.len()
            && (baseline.is_none() || piece_fill.seg.len() == segs);
        if !*piece_splice {
            piece_pts.clear();
            piece_fill.seg.clear();
            piece_stroke.seg.clear();
        }
        let (fill, stroke) = (fill.into_parts(), stroke.into_parts());
        emitted += fill.0.len() + stroke.0.len();
        piece_fill.set(fill);
        piece_stroke.set(stroke);
    }
    Some(emitted)
}

/// Finishes a full grid build into `cache`: the stale sets from `scratch` (the live geometry
/// stays in the pieces), and the time map and y range the geometry was built for. Run
/// `Scratch::begin_grid` before the cells.
pub fn finish_grid(input: &BuildInput, cache: &mut ChartCache, scratch: &mut Scratch) {
    cache.gaps.clear();
    scratch
        .stale
        .finish(&mut cache.stale_fills, &mut cache.stale_strokes);
    cache.anchor_ns = input.map.anchor_ns;
    cache.px_per_ns = input.map.px_per_ns();
    cache.y_min = input.y_min;
    cache.y_max = input.y_max;
}

/// Which mesh of a grid piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceLayer {
    Fill,
    Stroke,
}

/// A path's vertices to be moved into window logical px, not yet written: its bounds after the
/// move, its vertex count and the source slices (physical px). A ramped paint splits the
/// triangles between its two bands while it moves them, so each vertex is written once per
/// band it goes to (tasklens-3uosp).
pub struct MovedPath<'a, S: Iterator<Item = &'a [PathVertex<Pixels>]>> {
    pub bounds: Bounds<Pixels>,
    pub len: usize,
    inv: f32,
    offset: Point<Pixels>,
    slices: S,
}

impl<'a, S: Iterator<Item = &'a [PathVertex<Pixels>]>> MovedPath<'a, S> {
    /// `slices` (physical px, `len` vertices in all) divided by `scale_factor` and moved by
    /// `offset`; `bounds` are the source bounds, moved the same way.
    pub fn new(
        bounds: Bounds<Pixels>,
        len: usize,
        offset: Point<Pixels>,
        scale_factor: f32,
        slices: S,
    ) -> Self {
        let inv = 1.0 / scale_factor;
        Self {
            bounds: Bounds::new(
                bounds.origin * inv + offset,
                size(bounds.size.width * inv, bounds.size.height * inv),
            ),
            len,
            inv,
            offset,
            slices,
        }
    }

    /// Calls `f` with every triangle moved, in order, three vertices at a time. A triangle may
    /// span two source slices. Returns the moved vertices of a last partial triangle (0 to 2).
    /// A whole triangle per call keeps the band split free of per-vertex buffering
    /// (tasklens-i5sgo).
    pub fn for_each_triangle(
        self,
        mut f: impl FnMut([PathVertex<Pixels>; 3]),
    ) -> SmallVec<[PathVertex<Pixels>; 2]> {
        let (inv, offset) = (self.inv, self.offset);
        let mut part: SmallVec<[PathVertex<Pixels>; 2]> = SmallVec::new();
        for s in self.slices {
            let mut s = s;
            while !part.is_empty()
                && let Some((v, tail)) = s.split_first()
            {
                s = tail;
                if part.len() == 2 {
                    f([part[0].clone(), part[1].clone(), moved(v, inv, offset)]);
                    part.clear();
                } else {
                    part.push(moved(v, inv, offset));
                }
            }
            let (triangles, rest) = s.as_chunks::<3>();
            for [a, b, c] in triangles {
                f([
                    moved(a, inv, offset),
                    moved(b, inv, offset),
                    moved(c, inv, offset),
                ]);
            }
            part.extend(rest.iter().map(|v| moved(v, inv, offset)));
        }
        part
    }

    /// The moved vertices as one path with the moved bounds. Each vertex is written once.
    pub fn collect(self) -> Path<Pixels> {
        let mut out = Path::new(self.bounds.origin);
        out.bounds = self.bounds;
        out.vertices.reserve_exact(self.len);
        let (inv, offset) = (self.inv, self.offset);
        for s in self.slices {
            out.vertices.extend(s.iter().map(|v| moved(v, inv, offset)));
        }
        out
    }
}

#[inline]
fn moved(v: &PathVertex<Pixels>, inv: f32, offset: Point<Pixels>) -> PathVertex<Pixels> {
    PathVertex {
        xy_position: v.xy_position * inv + offset,
        st_position: v.st_position,
        content_mask: Default::default(),
    }
}

/// One layer of every piece as one path in logical px moved by `offset`, as `translated` makes
/// it from one cached path: the bounds cover every piece, and each vertex is written once, so a
/// paint copies the grid's vertices once. `None` when no piece has a vertex in that layer.
pub fn piece_path<'a>(
    pieces: impl Iterator<Item = &'a CellPiece> + Clone,
    layer: PieceLayer,
    offset: Point<Pixels>,
    scale_factor: f32,
) -> Option<Path<Pixels>> {
    piece_moved(pieces, layer, offset, scale_factor).map(MovedPath::collect)
}

/// [`piece_path`] before the vertices are written.
pub fn piece_moved<'a>(
    pieces: impl Iterator<Item = &'a CellPiece> + Clone,
    layer: PieceLayer,
    offset: Point<Pixels>,
    scale_factor: f32,
) -> Option<MovedPath<'a, impl Iterator<Item = &'a [PathVertex<Pixels>]>>> {
    let mesh = move |p: &'a CellPiece| match layer {
        PieceLayer::Fill => &p.fill,
        PieceLayer::Stroke => &p.stroke,
    };
    let meshes = pieces.map(mesh).filter(|m| !m.verts.is_empty());
    let mut count = 0;
    let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for m in meshes.clone() {
        count += m.verts.len();
        min = (min.0.min(m.min.0), min.1.min(m.min.1));
        max = (max.0.max(m.max.0), max.1.max(m.max.1));
    }
    if count == 0 {
        return None;
    }
    let bounds = Bounds::new(
        point(px(min.0), px(min.1)),
        size(px(max.0 - min.0), px(max.1 - min.1)),
    );
    let slices = meshes.flat_map(|m| {
        let (a, b) = m.verts.as_slices();
        [a, b]
    });
    Some(MovedPath::new(bounds, count, offset, scale_factor, slices))
}

/// Where one grid cell's points go: chart-local x in `lo .. lo + clip_w` is on screen.
struct CellPlace {
    origin: (f32, f32),
    lo: f32,
    clip_w: f32,
}

impl CellPlace {
    /// `pts` mapped to physical px, clipped to the window and moved by `origin`, into `out`.
    /// Points are sorted by time, so x never decreases.
    fn clip(&self, pts: &[Pt], input: &BuildInput, out: &mut Vec<Point<Pixels>>) {
        out.clear();
        let (lo, hi) = (self.lo, self.lo + self.clip_w);
        let (ox, oy) = self.origin;
        let mut push = |x: f32, y: f32| out.push(point(px(ox + x), px(oy + y)));
        let lerp =
            |a: (f32, f32), b: (f32, f32), x: f32| a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0);
        let mut prev: Option<(f32, f32)> = None;
        for p in pts {
            let cur = (
                input.map.x_local(p.t_ns),
                y_of(p.value, input.y_min, input.y_max, input.plot_height),
            );
            if let Some(a) = prev {
                if a.0 < lo && cur.0 > lo {
                    push(lo, lerp(a, cur, lo));
                }
                if a.0 < hi && cur.0 > hi {
                    push(hi, lerp(a, cur, hi));
                }
            }
            if cur.0 > hi {
                break;
            }
            if cur.0 >= lo {
                push(cur.0, cur.1);
            }
            prev = Some(cur);
        }
    }

    /// The plot bottom, physical px relative to the element.
    fn baseline(&self, input: &BuildInput) -> Pixels {
        px(self.origin.1 + input.plot_height)
    }
}

/// The fill under `pts` down to `baseline`.
fn emit_cell_fill(b: &mut FillMesh, pts: &[Point<Pixels>], baseline: Pixels) {
    b.band(pts, |_, p| point(p.x, baseline));
}

/// One stale stretch of a grid cell into the stale builders: a fill polygon down to the cell's
/// baseline for series 0 (`main`) when it has a fill colour, and a stroke along `pts` (a single
/// point strokes 1 px to the right). The overlay gets a stroke only.
fn emit_stale(
    stale: &mut Builders,
    spec: &ChartSeries,
    main: bool,
    pts: &[Point<Pixels>],
    place: &CellPlace,
    input: &BuildInput,
) {
    if pts.is_empty() {
        return;
    }
    if main && let Some(color) = spec.fill {
        emit_cell_fill(stale.fill(color), pts, place.baseline(input));
    }
    stale.stroke(spec).stroke(pts);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(mesh: FillMesh) -> f32 {
        let path = mesh.finish().expect("triangles");
        path.vertices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let p = |i: usize| (f32::from(t[i].xy_position.x), f32::from(t[i].xy_position.y));
                let (a, b, c) = (p(0), p(1), p(2));
                ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() / 2.0
            })
            .sum()
    }

    /// Chains that cross inside a segment fill both lobes once, as non-zero fills them.
    #[test]
    fn crossing_chains_split_at_the_crossing() {
        let top = [point(px(0.0), px(0.0)), point(px(10.0), px(10.0))];
        let bottom = [point(px(0.0), px(10.0)), point(px(10.0), px(0.0))];
        let mut mesh = FillMesh::default();
        mesh.band(&top, |i, _| bottom[i]);
        assert!((area(mesh) - 50.0).abs() < 1e-3);
    }

    /// The bounds cover exactly the vertices, set once in `finish`.
    #[test]
    fn bounds_cover_the_vertices() {
        let top = [point(px(2.0), px(3.0)), point(px(12.0), px(1.0))];
        let mut mesh = FillMesh::default();
        mesh.band(&top, |_, p| point(p.x, px(20.0)));
        let path = mesh.finish().expect("triangles");
        assert_eq!(path.bounds.origin, point(px(2.0), px(1.0)));
        assert_eq!(path.bounds.size, size(px(10.0), px(19.0)));
    }

    /// `StrokeMesh::segment` for every segment gives `stroke_line` vertex for vertex, and the
    /// marks split the line into those segments (tasklens-wlf.39). The zigzag has sharp turns,
    /// so bevels and folded joins are covered.
    #[test]
    fn segments_match_the_whole_line() {
        let pts: Vec<_> = (0..40)
            .map(|i| {
                let y = if i % 3 == 0 {
                    2.0
                } else {
                    40.0 - (i % 7) as f32 * 5.0
                };
                point(px(i as f32 * 1.7), px(y))
            })
            .collect();
        assert!(!has_short_segment(&pts));
        let mut whole = StrokeMesh::reusing(None, GRID_LINE_WIDTH, false);
        let mut marks = Vec::new();
        whole.stroke_line_marked(&pts, |v| marks.push(v.len()));
        let mut parts = StrokeMesh::reusing(None, GRID_LINE_WIDTH, false);
        let mut ends = Vec::new();
        for j in 0..pts.len() - 1 {
            parts.segment(&pts, j);
            ends.push(parts.len());
        }
        assert_eq!(marks, ends);
        let xy =
            |m: StrokeMesh| -> Vec<_> { m.into_parts().0.iter().map(|v| v.xy_position).collect() };
        assert_eq!(xy(whole), xy(parts));
    }

    /// A segment with no width adds no triangle.
    #[test]
    fn vertical_segment_adds_nothing() {
        let top = [point(px(5.0), px(0.0)), point(px(5.0), px(8.0))];
        let mut mesh = FillMesh::default();
        mesh.band(&top, |_, p| point(p.x, px(10.0)));
        assert!(mesh.finish().is_none());
    }
}
