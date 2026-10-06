//! Chart geometry: time mapping, run splitting, decimation and the stale rule (part 1), and path
//! emission into `ChartCache` (part 2) (ADR-009 rules 2 to 7; UI-PRF-001; UI-PRF-005;
//! UI-PRF-007; PERF-022; PERF-026; R-051).
//!
//! Every function here is pure and allocation-free given the caller's buffers, except that `build`
//! allocates inside lyon's tessellator. Time math is `u64` and the pixel scale is `f64`: 60 s in
//! ns against 1,600 px loses precision in `f32`.

use gpui_kit::{Hsla, PathBuilder, point, px};
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
    /// P2 stacked bands (E19-012); not read yet.
    pub stack: bool,
}

/// Buffers reused across rebuilds so the steady state allocates only inside lyon.
#[derive(Default)]
pub struct Scratch {
    runs: SmallVec<[Run; 8]>,
    pts: Vec<Pt>,
    stale_pts: Vec<Pt>,
}

/// `plot_height − (value − y_min) / (y_max − y_min) × plot_height`, clamped to `0..=plot_height`.
/// An empty or inverted scale puts every value on the baseline.
pub fn y_of(value: f32, y_min: f32, y_max: f32, plot_height: f32) -> f32 {
    let span = y_max - y_min;
    if !(span > 0.0) {
        return plot_height;
    }
    (plot_height - (value - y_min) / span * plot_height).clamp(0.0, plot_height.max(0.0))
}

/// Dash pattern for dashed series (PERF-026).
const DASH: [gpui_kit::Pixels; 2] = [px(5.0), px(5.0)];

/// Builders keyed by colour (and dash for strokes) in insertion order (rule 7). Created on first
/// use, so a colour with no points adds no set.
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
                self.fills.push((color, PathBuilder::fill()));
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

    /// One fill polygon (when the series has a fill) and one stroke for `pts`.
    fn emit(&mut self, s: &ChartSeries, pts: &[Pt], input: &BuildInput) {
        let Some((first, rest)) = pts.split_first() else {
            return;
        };
        let h = input.plot_height;
        let xy = |p: &Pt| {
            point(
                px(input.map.x_local(p.t_ns)),
                px(y_of(p.value, input.y_min, input.y_max, h)),
            )
        };
        let p0 = xy(first);
        if let Some(fill) = s.fill
            && !rest.is_empty()
        {
            let b = self.fill(fill);
            b.move_to(point(p0.x, px(h)));
            b.line_to(p0);
            let mut last = p0;
            for p in rest {
                last = xy(p);
                b.line_to(last);
            }
            b.line_to(point(last.x, px(h)));
            b.close();
        }
        let b = self.stroke(s);
        b.move_to(p0);
        if rest.is_empty() {
            b.line_to(point(p0.x + px(1.0), p0.y));
        } else {
            for p in rest {
                b.line_to(xy(p));
            }
        }
    }

    /// Tessellates every builder into `fills` and `strokes`. A failed or empty path adds no set.
    fn finish(self, fills: &mut SmallVec<[PathSet; 2]>, strokes: &mut SmallVec<[PathSet; 3]>) {
        fills.clear();
        strokes.clear();
        fills.extend(
            self.fills
                .into_iter()
                .filter_map(|(color, b)| set(color, false, b)),
        );
        strokes.extend(
            self.strokes
                .into_iter()
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
/// (ADR-009 rules 2, 4, 6, 7; UI-PRF-001; UI-PRF-007). Paths are chart-local physical pixels at
/// a zero origin; the element translates them at paint time and owns the scale factor.
///
/// Fresh samples go to the live paths. `STALE` samples go to the stale paths, joined to their
/// fresh neighbours so the line does not break. When the series is stale (R-051), a flat tail
/// from the newest sample to `map.right_edge_ns` at the newest value goes to the stale paths too.
/// Gaps come from the first series only.
pub fn build(input: &BuildInput, cache: &mut ChartCache, scratch: &mut Scratch) {
    let map = &input.map;
    let mut live = Builders::default();
    let mut stale = Builders::default();
    cache.gaps.clear();
    let bucket = bucket_ns(map.range_ns, map.plot_width.max(1.0).round() as u32);

    for (si, (spec, view)) in input.series.iter().enumerate() {
        let Some(view) = view else {
            continue;
        };
        let samples = &view.samples[..];
        let period = view.period_ns;
        let from = window_start(samples, map.right_edge_ns, map.range_ns, period);
        split_runs(samples, from, &mut scratch.runs);

        if si == 0 {
            push_gaps(samples, &scratch.runs, map, period, &mut cache.gaps);
        }

        for run in &scratch.runs {
            // Alternating stretches of fresh and `STALE` samples within the run.
            let mut i = run.start;
            while i < run.end {
                let is_stale_flag = samples[i].flags.contains(SampleFlags::STALE);
                let mut j = i + 1;
                while j < run.end && samples[j].flags.contains(SampleFlags::STALE) == is_stale_flag
                {
                    j += 1;
                }
                if is_stale_flag {
                    let span = Run {
                        start: i.saturating_sub(1).max(run.start),
                        end: (j + 1).min(run.end),
                    };
                    decimate(samples, span, bucket, &mut scratch.stale_pts);
                    stale.emit(spec, &scratch.stale_pts, input);
                } else {
                    decimate(samples, Run { start: i, end: j }, bucket, &mut scratch.pts);
                    live.emit(spec, &scratch.pts, input);
                }
                i = j;
            }
        }

        // Flat continuation of a stale series to the right edge (UI-PRF-007).
        if let Some(last_run) = scratch.runs.last() {
            let newest = samples[last_run.end - 1];
            if is_stale(newest.t_ns, input.now_boot_ns, period) && map.right_edge_ns > newest.t_ns {
                scratch.stale_pts.clear();
                scratch.stale_pts.push(Pt {
                    t_ns: newest.t_ns,
                    value: newest.value,
                });
                scratch.stale_pts.push(Pt {
                    t_ns: map.right_edge_ns,
                    value: newest.value,
                });
                stale.emit(spec, &scratch.stale_pts, input);
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
