//! Chart geometry: time mapping, run splitting, decimation and the stale rule (part 1), and path
//! emission into `ChartCache` (part 2) (ADR-009 rules 2 to 7; UI-PRF-001; UI-PRF-005;
//! UI-PRF-007; PERF-022; PERF-026; R-051).
//!
//! Every function here is pure and allocation-free given the caller's buffers, except that `build`
//! allocates inside lyon's tessellator. Time math is `u64` and the pixel scale is `f64`: 60 s in
//! ns against 1,600 px loses precision in `f32`.

use gpui_kit::{FillOptions, Hsla, PathBuilder, PathStyle, Pixels, Point, point, px};
use smallvec::SmallVec;
use tl_model::series::{Sample, SampleFlags, SeriesView};

use super::{ChartCache, ChartSeries, PathSet};

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

/// Buffers reused across rebuilds so the steady state allocates only inside lyon.
#[derive(Default)]
pub struct Scratch {
    runs: SmallVec<[Run; 8]>,
    pts: Vec<Pt>,
    stale_pts: Vec<Pt>,
    /// Stacked mode: the current series' window with cumulative values, the band floor under
    /// each of those samples, the floor under each decimated point, and the running sum per
    /// period slot.
    stacked: Vec<Sample>,
    floors: Vec<f32>,
    pt_floors: Vec<f32>,
    acc: Vec<f32>,
    /// Grid cells: the decimated points after clipping to the cell, physical px.
    clipped: Vec<Point<Pixels>>,
    /// Grid cells: the last built cell's dotted-gap spans, chart-local physical px.
    cell_gaps: SmallVec<[std::ops::Range<f32>; 4]>,
    /// One run's line and band floor as emitted, physical px, and the curve tangents.
    line: Line,
    live: Builders,
    stale: Builders,
}

impl Scratch {
    /// Dotted-gap spans of the cell `build_cell` built last, chart-local physical px as
    /// `ChartCache::gaps` holds them.
    pub fn cell_gaps(&self) -> &[std::ops::Range<f32>] {
        &self.cell_gaps
    }
}

/// Period slots that align stacked series by bucket index, so ±period/2 jitter still lands in one
/// slot (ADR-008: per-core series come from one `/proc/stat` read).
struct StackGrid {
    period_ns: u64,
    first_slot: u64,
}

impl StackGrid {
    /// Slots from the window's left overscan edge to the newest sample of any series. The period
    /// is the first present series' period.
    fn new(input: &BuildInput, acc: &mut Vec<f32>) -> Self {
        let period_ns = input
            .series
            .iter()
            .find_map(|(_, v)| v.map(|v| v.period_ns))
            .unwrap_or(1)
            .max(1);
        let map = &input.map;
        let left = map
            .right_edge_ns
            .saturating_sub(map.range_ns)
            .saturating_sub(period_ns.saturating_mul(2));
        let grid = Self {
            period_ns,
            first_slot: left / period_ns,
        };
        let newest = input
            .series
            .iter()
            .filter_map(|(_, v)| v.and_then(|v| v.samples.last()).map(|s| s.t_ns))
            .max()
            .unwrap_or(0);
        let len = grid.slot(newest).map_or(0, |i| i + 1);
        acc.clear();
        acc.resize(len, 0.0);
        grid
    }

    fn slot(&self, t_ns: u64) -> Option<usize> {
        let k = t_ns.saturating_add(self.period_ns / 2) / self.period_ns;
        usize::try_from(k.checked_sub(self.first_slot)?).ok()
    }

    /// Copies `samples` into `out` with each valid value raised by the sum of the lower series in
    /// its slot, and that sum into `floors`. A slot that a lower series missed counts it as 0.
    fn stack(&self, samples: &[Sample], acc: &[f32], out: &mut Vec<Sample>, floors: &mut Vec<f32>) {
        out.clear();
        floors.clear();
        for s in samples {
            let floor = if s.flags.contains(SampleFlags::VALID) {
                self.slot(s.t_ns)
                    .and_then(|i| acc.get(i))
                    .copied()
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            out.push(Sample {
                value: s.value + floor,
                ..*s
            });
            floors.push(floor);
        }
    }

    /// Adds the valid values of `samples` to their slots.
    fn add(&self, samples: &[Sample], acc: &mut [f32]) {
        for s in samples {
            if s.flags.contains(SampleFlags::VALID)
                && let Some(a) = self.slot(s.t_ns).and_then(|i| acc.get_mut(i))
            {
                *a += s.value;
            }
        }
    }
}

/// The floor under each point of `pts`, which were decimated from `samples[span]` (sorted by
/// time, each point one of those samples).
fn floors_of(samples: &[Sample], floors: &[f32], span: Run, pts: &[Pt], out: &mut Vec<f32>) {
    out.clear();
    let mut i = span.start;
    for p in pts {
        while i + 1 < span.end && samples[i].t_ns < p.t_ns {
            i += 1;
        }
        out.push(floors.get(i).copied().unwrap_or(0.0));
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

/// Builders keyed by colour (and dash for strokes) in insertion order (rule 7). Created on first
/// use, so a colour with no points adds no set. Kept in `Scratch` so the lists do not reallocate
/// per build when a stacked chart has more colours than the inline size.
#[derive(Default)]
struct Builders {
    fills: SmallVec<[(Hsla, PathBuilder); 2]>,
    strokes: SmallVec<[(Hsla, bool, PathBuilder); 3]>,
}

impl Builders {
    fn fill(&mut self, color: Hsla) -> &mut PathBuilder {
        let i = match self.fills.iter().position(|(c, _)| *c == color) {
            Some(i) => i,
            None => {
                // Non-zero: with the default even-odd rule, overlapping merged polygons cancel.
                let b = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
                self.fills.push((color, b));
                self.fills.len() - 1
            }
        };
        &mut self.fills[i].1
    }

    fn stroke(&mut self, s: &ChartSeries) -> &mut PathBuilder {
        let i = match self
            .strokes
            .iter()
            .position(|(c, d, _)| *c == s.line && *d == s.dashed)
        {
            Some(i) => i,
            None => {
                let mut b = PathBuilder::stroke(s.line_width);
                if s.dashed {
                    b = b.dash_array(&DASH);
                }
                self.strokes.push((s.line, s.dashed, b));
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
        let Some((&p0, rest)) = line.split_first() else {
            return;
        };
        let h = input.plot_height;
        if let Some(fill) = s.fill
            && let Some(&last) = rest.last()
        {
            let b = self.fill(fill);
            match floors {
                Some(floors) => {
                    floor_pts.clear();
                    floor_pts.extend(pts.iter().zip(floors).map(|(p, f)| Pt {
                        t_ns: p.t_ns,
                        value: *f,
                    }));
                    emit_run(floor_pts, input.curved, input, tangents, floor);
                    b.move_to(p0);
                    for &p in rest {
                        b.line_to(p);
                    }
                    for &p in floor.iter().rev() {
                        b.line_to(p);
                    }
                }
                None => {
                    b.move_to(point(p0.x, px(h)));
                    b.line_to(p0);
                    for &p in rest {
                        b.line_to(p);
                    }
                    b.line_to(point(last.x, px(h)));
                }
            }
            b.close();
        }
        emit_cell_stroke(self.stroke(s), p0, rest);
    }

    /// Tessellates every builder into `fills` and `strokes` and leaves the builder lists empty. A
    /// failed or empty path adds no set.
    fn finish(&mut self, fills: &mut SmallVec<[PathSet; 2]>, strokes: &mut SmallVec<[PathSet; 3]>) {
        fills.clear();
        strokes.clear();
        fills.extend(
            self.fills
                .drain(..)
                .filter_map(|(color, b)| set(color, false, b)),
        );
        strokes.extend(
            self.strokes
                .drain(..)
                .filter_map(|(color, dashed, b)| set(color, dashed, b)),
        );
    }
}

fn set(color: Hsla, dashed: bool, b: PathBuilder) -> Option<PathSet> {
    let path = b.build().ok()?;
    (!path.vertices.is_empty()).then_some(PathSet {
        color,
        dashed,
        path,
    })
}

/// Builds fill and stroke paths, their stale sets and the dotted-gap spans into `cache`
/// (ADR-009 rules 2, 4, 6, 7; UI-PRF-001; UI-PRF-007; PERF-030). Paths are chart-local physical
/// pixels at a zero origin; the element translates them at paint time and owns the scale factor.
///
/// Fresh samples go to the live paths. `STALE` samples go to the stale paths, joined to their
/// fresh neighbours so the line does not break. When the series is stale (R-051), a flat tail
/// from the newest sample to `map.right_edge_ns` at the newest value goes to the stale paths too.
/// Gaps come from the first series only.
///
/// With `input.stack`, series are stacked in order: each valid sample of series `k` is raised by
/// the values of series `0..k` in the same period slot, and its fill goes down to that floor
/// instead of the baseline. Runs, gaps and decimation stay per series.
pub fn build(input: &BuildInput, cache: &mut ChartCache, scratch: &mut Scratch) {
    let map = &input.map;
    let Scratch {
        runs,
        pts,
        stale_pts,
        stacked,
        floors,
        pt_floors,
        acc,
        line,
        live,
        stale,
        ..
    } = scratch;
    cache.gaps.clear();
    let bucket = bucket_ns(map.range_ns, map.plot_width.max(1.0).round() as u32);
    let grid = input.stack.then(|| StackGrid::new(input, acc));

    for (si, (spec, view)) in input.series.iter().enumerate() {
        let Some(view) = view else {
            continue;
        };
        let period = view.period_ns;
        let from = window_start(&view.samples, map.right_edge_ns, map.range_ns, period);
        let (samples, from, band_floors): (&[Sample], usize, Option<&[f32]>) = match &grid {
            Some(g) => {
                let window = &view.samples[from..];
                g.stack(window, acc, stacked, floors);
                g.add(window, acc);
                (&stacked[..], 0, Some(&floors[..]))
            }
            None => (&view.samples[..], from, None),
        };
        split_runs(samples, from, runs);

        if si == 0 {
            push_gaps(samples, runs, map, period, &mut cache.gaps);
        }

        for run in runs.iter() {
            for_each_stretch(samples, *run, |span, is_stale_flag| {
                let (out, builders) = if is_stale_flag {
                    (&mut *stale_pts, &mut *stale)
                } else {
                    (&mut *pts, &mut *live)
                };
                decimate(samples, span, bucket, out);
                if let Some(f) = band_floors {
                    floors_of(samples, f, span, out, pt_floors);
                }
                builders.emit(spec, out, band_floors.map(|_| &pt_floors[..]), input, line);
            });
        }

        // Flat continuation of a stale series to the right edge (UI-PRF-007).
        if let Some(last_run) = runs.last() {
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
                if let Some(f) = band_floors {
                    let floor = f[last_run.end - 1];
                    pt_floors.clear();
                    pt_floors.extend([floor, floor]);
                }
                let floors = band_floors.map(|_| &pt_floors[..]);
                stale.emit(spec, stale_pts, floors, input, line);
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

/// Dotted-gap spans of one series: between consecutive runs, and from the window's left edge to
/// the first run when that run starts more than 2 periods in. The stale tail reaches the right
/// edge, so a stale series adds no span on the right.
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

/// Builds one grid cell into shared builders (UI-PRF-002, ADR-009 rule 7). Series 0 goes to
/// `fill` (when it has a fill colour) and `stroke`; series 1, when present, goes to `overlay`
/// as a stroke only; further series are ignored. Every vertex is moved by `origin` (physical px,
/// the cell's plot corner relative to the element). `STALE` stretches and the stale tail go to
/// the scratch's stale builders, which `finish_grid` tessellates.
///
/// One content mask covers the whole grid, so x is clipped at build time to the `clip_w` wide
/// window that `map` puts on screen (chart-local `−map.dx(clip_w) ..` that plus `clip_w`):
/// points outside are dropped and the boundary point is interpolated. No stacking.
///
/// Series 0's dotted-gap spans replace `scratch.cell_gaps()`, chart-local and unclipped as
/// `build` makes them. The caller paints them as quads per cell (PERF-022).
pub fn build_cell(
    input: &BuildInput,
    origin: (f32, f32),
    clip_w: f32,
    fill: &mut PathBuilder,
    stroke: &mut PathBuilder,
    mut overlay: Option<&mut PathBuilder>,
    scratch: &mut Scratch,
) {
    let map = &input.map;
    let Scratch {
        runs,
        pts,
        stale_pts,
        clipped,
        cell_gaps,
        stale,
        ..
    } = scratch;
    cell_gaps.clear();
    let bucket = bucket_ns(map.range_ns, clip_w.max(1.0).round() as u32);
    let place = CellPlace {
        origin,
        lo: -map.dx(clip_w),
        clip_w,
    };

    for (si, (spec, view)) in input.series.iter().enumerate().take(2) {
        let Some(view) = view else {
            continue;
        };
        let period = view.period_ns;
        let samples = &view.samples[..];
        let from = window_start(samples, map.right_edge_ns, map.range_ns, period);
        split_runs(samples, from, runs);

        if si == 0 {
            push_gaps(samples, runs, map, period, cell_gaps);
        }

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
                    emit_stale(stale, spec, si == 0, clipped, &place, input);
                } else if si == 0 {
                    let f = spec.fill.is_some().then_some(&mut *fill);
                    emit_cell(f, stroke, clipped, &place, input);
                } else if let Some(o) = overlay.as_deref_mut() {
                    emit_cell(None, o, clipped, &place, input);
                }
            });
        }

        // Flat continuation of a stale series to the right edge (UI-PRF-007).
        if let Some(last_run) = runs.last() {
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
                place.clip(stale_pts, input, clipped);
                emit_stale(stale, spec, si == 0, clipped, &place, input);
            }
        }
    }
}

/// Tessellates a grid's shared builders into `cache`: `fills[0]` from `fill`, `strokes[0]` from
/// `stroke` and `strokes[1]` from `overlay`, plus the stale sets from `scratch`. Empty paths add
/// no set. Also stores the time map and y range the geometry was built for.
pub fn finish_grid(
    input: &BuildInput,
    fill: (Hsla, PathBuilder),
    stroke: (Hsla, PathBuilder),
    overlay: Option<(Hsla, PathBuilder)>,
    cache: &mut ChartCache,
    scratch: &mut Scratch,
) {
    cache.fills.clear();
    cache.strokes.clear();
    cache.gaps.clear();
    cache.fills.extend(set(fill.0, false, fill.1));
    cache.strokes.extend(set(stroke.0, false, stroke.1));
    if let Some((color, b)) = overlay {
        cache.strokes.extend(set(color, false, b));
    }
    scratch
        .stale
        .finish(&mut cache.stale_fills, &mut cache.stale_strokes);
    cache.anchor_ns = input.map.anchor_ns;
    cache.px_per_ns = input.map.px_per_ns();
    cache.y_min = input.y_min;
    cache.y_max = input.y_max;
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

/// A fill polygon down to the cell's baseline (when `fill` is given and there are 2 or more
/// points) and a stroke along `pts`. A single point strokes 1 px to the right.
fn emit_cell(
    fill: Option<&mut PathBuilder>,
    stroke: &mut PathBuilder,
    pts: &[Point<Pixels>],
    place: &CellPlace,
    input: &BuildInput,
) {
    let Some((&first, rest)) = pts.split_first() else {
        return;
    };
    if let Some(b) = fill
        && let Some(&last) = rest.last()
    {
        emit_cell_fill(b, first, rest, last, place.baseline(input));
    }
    emit_cell_stroke(stroke, first, rest);
}

fn emit_cell_fill(
    b: &mut PathBuilder,
    first: Point<Pixels>,
    rest: &[Point<Pixels>],
    last: Point<Pixels>,
    baseline: Pixels,
) {
    b.move_to(point(first.x, baseline));
    b.line_to(first);
    for &p in rest {
        b.line_to(p);
    }
    b.line_to(point(last.x, baseline));
    b.close();
}

fn emit_cell_stroke(b: &mut PathBuilder, first: Point<Pixels>, rest: &[Point<Pixels>]) {
    b.move_to(first);
    if rest.is_empty() {
        b.line_to(point(first.x + px(1.0), first.y));
    } else {
        for &p in rest {
            b.line_to(p);
        }
    }
}

/// `emit_cell` into the stale builders: series 0 (`main`) gets its fill when it has one; the
/// overlay gets a stroke only.
fn emit_stale(
    stale: &mut Builders,
    spec: &ChartSeries,
    main: bool,
    pts: &[Point<Pixels>],
    place: &CellPlace,
    input: &BuildInput,
) {
    let Some((&first, rest)) = pts.split_first() else {
        return;
    };
    if main
        && let Some(color) = spec.fill
        && let Some(&last) = rest.last()
    {
        emit_cell_fill(stale.fill(color), first, rest, last, place.baseline(input));
    }
    emit_cell_stroke(stale.stroke(spec), first, rest);
}
