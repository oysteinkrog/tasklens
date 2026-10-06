//! Chart geometry unit tests (tasklens-yrf.4 and yrf.5, ADR-009 rules 2 to 7; UI-PRF-007;
//! R-051).

use std::sync::Arc;

use gpui_kit::{Hsla, hsla};
use smallvec::SmallVec;
use tl_model::series::{Sample, SampleFlags, SeriesView};
use tl_ui::chart::geometry::{
    BuildInput, Pt, Run, Scratch, TimeMap, bucket_ns, build, decimate, is_stale, right_edge_ns,
    split_runs, stale_age_ns, window_start, y_of,
};
use tl_ui::chart::{ChartCache, ChartSeries, PathSet};

const MS: u64 = 1_000_000;
const SEC: u64 = 1_000_000_000;

fn series(n: usize, period_ns: u64, t0: u64, value: impl Fn(usize) -> f32) -> Vec<Sample> {
    (0..n)
        .map(|i| Sample::valid(t0 + i as u64 * period_ns, value(i)))
        .collect()
}

fn runs(samples: &[Sample], from: usize) -> Vec<(usize, usize)> {
    let mut out: SmallVec<[Run; 8]> = SmallVec::new();
    split_runs(samples, from, &mut out);
    out.iter().map(|r| (r.start, r.end)).collect()
}

/// Deterministic noise in 0..100.
fn noise(i: usize) -> f32 {
    let x = (i as u64)
        .wrapping_mul(6_364_136_223_846_793_005)
        .rotate_left(17);
    (x % 10_000) as f32 / 100.0
}

#[test]
fn chart_geometry_splits_runs_at_gaps() {
    let mut s = series(100, 100 * MS, 0, |_| 1.0);
    s[50].flags |= SampleFlags::GAP_BEFORE;
    assert_eq!(runs(&s, 0), vec![(0, 50), (50, 100)]);
    // A run starts at `from`.
    assert_eq!(runs(&s, 20), vec![(20, 50), (50, 100)]);
}

#[test]
fn invalid_samples_form_one_gap() {
    let n = 40;
    let mut s = series(n, 100 * MS, 0, |_| 1.0);
    for x in &mut s[10..20] {
        *x = Sample::gap(x.t_ns);
    }
    // GAP_BEFORE on the first valid sample after the gap must not add an empty run.
    s[20].flags |= SampleFlags::GAP_BEFORE;
    // STALE samples stay in their run.
    s[30].flags |= SampleFlags::STALE;
    let r = runs(&s, 0);
    assert_eq!(r, vec![(0, 10), (20, n)]);
    assert!(r.iter().all(|&(a, b)| (10..20).all(|i| i < a || i >= b)));

    // All invalid: no runs. Empty input: no runs.
    let gaps: Vec<Sample> = (0..5).map(|i| Sample::gap(i * MS)).collect();
    assert!(runs(&gaps, 0).is_empty());
    assert!(runs(&[], 0).is_empty());
}

#[test]
fn chart_geometry_keeps_spike_under_decimation() {
    let s = series(2_000, 100 * MS, 0, |i| if i == 1_000 { 100.0 } else { 0.0 });
    let range = 200 * SEC;
    let b = bucket_ns(range, 200);
    let mut out = Vec::new();
    decimate(
        &s,
        Run {
            start: 0,
            end: s.len(),
        },
        b,
        &mut out,
    );
    assert!(out.iter().any(|p| p.value == 100.0));
    assert!(out.len() <= 402, "{} points", out.len());
    assert_eq!(out.first().map(|p| p.t_ns), Some(s[0].t_ns));
    assert_eq!(out.last().map(|p| p.t_ns), Some(s[1_999].t_ns));
    assert!(out.windows(2).all(|w| w[0].t_ns < w[1].t_ns));
}

#[test]
fn chart_bucket_alignment_is_stable() {
    // One long series; two builds see windows 100 ms apart, as when the right edge moves.
    let all = series(2_000, 100 * MS, 37 * SEC + 13 * MS, noise);
    let range = 60 * SEC;
    let b = bucket_ns(range, 173);
    let edge_a = all[1_500].t_ns;
    let edge_b = edge_a + 100 * MS;
    let build = |edge: u64| {
        let from = window_start(&all, edge, range, 100 * MS);
        let end = all.partition_point(|s| s.t_ns <= edge);
        let mut out = Vec::new();
        decimate(&all, Run { start: from, end }, b, &mut out);
        out
    };
    let (a, bb) = (build(edge_a), build(edge_b));

    // Points inside interior buckets, keyed by bucket index.
    let interior = |pts: &[Pt]| -> Vec<(u64, Pt)> {
        let first = pts[0].t_ns / b;
        let last = pts[pts.len() - 1].t_ns / b;
        pts.iter()
            .filter(|p| p.t_ns / b != first && p.t_ns / b != last)
            .map(|p| (p.t_ns / b, *p))
            .collect()
    };
    let (ia, ib) = (interior(&a), interior(&bb));
    let lo = ia[0].0.max(ib[0].0);
    let hi = ia[ia.len() - 1].0.min(ib[ib.len() - 1].0);
    let shared = |v: &[(u64, Pt)]| -> Vec<(u64, Pt)> {
        v.iter()
            .copied()
            .filter(|(k, _)| (lo..=hi).contains(k))
            .collect()
    };
    let (sa, sb) = (shared(&ia), shared(&ib));
    assert!(sa.len() > 100);
    assert_eq!(sa, sb);
}

#[test]
fn right_edge_is_one_period_behind() {
    let now = 10 * SEC;
    let newest = 9_950 * MS;
    let period = 100 * MS;
    assert_eq!(right_edge_ns(now, newest, period, true), 9_900 * MS);
    assert_eq!(right_edge_ns(now, newest, period, false), 9_950 * MS);
    // The newest sample caps the smooth edge.
    assert_eq!(right_edge_ns(now, 9_500 * MS, period, true), 9_500 * MS);
}

#[test]
fn stale_threshold_matches_r051() {
    let newest = 100 * SEC;
    assert!(!is_stale(newest, newest + 1_400 * MS, 100 * MS));
    assert!(is_stale(newest, newest + 1_600 * MS, 100 * MS));
    assert!(!is_stale(newest, newest + 2_900 * MS, SEC));
    assert!(is_stale(newest, newest + 3_100 * MS, SEC));
    assert_eq!(stale_age_ns(newest, newest + 1_600 * MS), 1_600 * MS);
    assert_eq!(stale_age_ns(newest, newest - MS), 0);
}

#[test]
fn decimation_point_count_bound() {
    // 100 s starting on a bucket boundary (2 s) touches exactly 50 buckets.
    let s = series(10_000, 10 * MS, 4 * SEC, noise);
    let range = 100 * SEC;
    let b = bucket_ns(range, 50);
    let mut out = Vec::new();
    decimate(
        &s,
        Run {
            start: 0,
            end: s.len(),
        },
        b,
        &mut out,
    );
    assert!(out.len() <= 102, "{} points", out.len());
    assert!(out.windows(2).all(|w| w[0].t_ns < w[1].t_ns));

    // One sample per bucket keeps every sample.
    let sparse = series(20, SEC, 0, noise);
    decimate(&sparse, Run { start: 0, end: 20 }, SEC, &mut out);
    assert_eq!(out.len(), 20);
    // Short runs.
    decimate(&sparse, Run { start: 3, end: 4 }, SEC, &mut out);
    assert_eq!(out.len(), 1);
    decimate(&sparse, Run { start: 3, end: 3 }, SEC, &mut out);
    assert!(out.is_empty());
}

#[test]
fn window_start_overscans_two_periods() {
    // 600 samples at 10 Hz with the right edge on the newest: every sample is inside.
    let s = series(600, 100 * MS, 100 * SEC, |_| 1.0);
    let edge = s[599].t_ns;
    assert_eq!(window_start(&s, edge, 60 * SEC, 100 * MS), 0);

    // A longer ring: the first kept sample is 60.2 s before the right edge.
    let s = series(1_000, 100 * MS, 100 * SEC, |_| 1.0);
    let edge = s[999].t_ns;
    let i = window_start(&s, edge, 60 * SEC, 100 * MS);
    assert_eq!(s[i].t_ns, edge - 60_200 * MS);
    assert_eq!(i, 999 - 602);
}

#[test]
fn time_map_places_edge_at_plot_right() {
    let m = TimeMap {
        anchor_ns: 100 * SEC,
        right_edge_ns: 100 * SEC - 100 * MS,
        range_ns: 60 * SEC,
        plot_width: 1_200.0,
    };
    assert!((m.px_per_ns() - 2e-8).abs() < 1e-15);
    assert_eq!(m.x_local(100 * SEC), 0.0);
    assert!((m.x_local(70 * SEC) + 600.0).abs() < 1e-3);
    // Right edge 100 ms behind the anchor is at −2 px; dx shifts it to plot_right.
    assert!((m.dx(1_200.0) - 1_202.0).abs() < 1e-3);
    assert!((m.x_local(m.right_edge_ns) + m.dx(1_200.0) - 1_200.0).abs() < 1e-3);
    assert_eq!(bucket_ns(60 * SEC, 0), 60 * SEC);
    assert_eq!(bucket_ns(10, 1_000), 1);
}

const RED: Hsla = hsla(0.0, 1.0, 0.5, 1.0);
const BLUE: Hsla = hsla(0.6, 1.0, 0.5, 1.0);

fn view(samples: Vec<Sample>, period_ns: u64) -> SeriesView {
    SeriesView {
        id: ChartSeries::default().id,
        version: 1,
        period_ns,
        samples: Arc::from(samples),
    }
}

fn line(fill: Option<Hsla>, line: Hsla, dashed: bool) -> ChartSeries {
    ChartSeries {
        fill,
        line,
        dashed,
        ..ChartSeries::default()
    }
}

/// A map whose window spans exactly `samples[0] ..= newest`, 100 px per second.
fn exact_map(samples: &[Sample]) -> TimeMap {
    let newest = samples[samples.len() - 1].t_ns;
    let range = newest - samples[0].t_ns;
    TimeMap {
        anchor_ns: newest,
        right_edge_ns: newest,
        range_ns: range,
        plot_width: (range / (10 * MS)) as f32,
    }
}

fn run_build(series: &[(ChartSeries, Option<&SeriesView>)], map: TimeMap, now: u64) -> ChartCache {
    let input = BuildInput {
        series,
        map,
        plot_height: 100.0,
        y_min: 0.0,
        y_max: 100.0,
        now_boot_ns: now,
        stack: false,
    };
    let mut cache = ChartCache::default();
    build(&input, &mut cache, &mut Scratch::default());
    cache
}

fn xs(set: &PathSet) -> impl Iterator<Item = f32> + '_ {
    set.path.vertices.iter().map(|v| v.xy_position.x.as_f32())
}

#[test]
fn build_two_subpaths_and_one_gap() {
    let mut s = series(100, 100 * MS, 10 * SEC, |i| 20.0 + (i % 7) as f32);
    s[50].flags |= SampleFlags::GAP_BEFORE;
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let now = s[99].t_ns;
    let cache = run_build(&[(line(Some(BLUE), RED, false), Some(&v))], map, now);

    assert_eq!(cache.fills.len(), 1);
    assert_eq!(cache.strokes.len(), 1);
    assert!(cache.stale_fills.is_empty() && cache.stale_strokes.is_empty());
    assert_eq!(cache.gaps.len(), 1);

    // Two polygons: fill vertices sit on both sides of the gap and none inside it.
    let (x49, x50) = (map.x_local(s[49].t_ns), map.x_local(s[50].t_ns));
    let fill = &cache.fills[0];
    assert!(xs(fill).all(|x| x <= x49 + 0.01 || x >= x50 - 0.01));
    assert!(xs(fill).any(|x| x <= x49 + 0.01));
    assert!(xs(fill).any(|x| x >= x50 - 0.01));

    assert_eq!(cache.anchor_ns, map.anchor_ns);
    assert_eq!(cache.px_per_ns, map.px_per_ns());
    assert_eq!((cache.y_min, cache.y_max), (0.0, 100.0));
}

#[test]
fn build_merges_equal_colours() {
    let s = series(50, 100 * MS, 10 * SEC, noise);
    let map = exact_map(&s);
    let (a, b) = (view(s.clone(), 100 * MS), view(s.clone(), 100 * MS));
    let cache = run_build(
        &[
            (line(Some(BLUE), RED, false), Some(&a)),
            (line(Some(BLUE), RED, false), Some(&b)),
        ],
        map,
        s[49].t_ns,
    );
    assert_eq!(cache.fills.len(), 1);
    assert_eq!(cache.strokes.len(), 1);
}

#[test]
fn build_separates_dashed() {
    let s = series(50, 100 * MS, 10 * SEC, noise);
    let map = exact_map(&s);
    let (a, b) = (view(s.clone(), 100 * MS), view(s.clone(), 100 * MS));
    let cache = run_build(
        &[
            (line(None, RED, false), Some(&a)),
            (line(None, RED, true), Some(&b)),
            // An absent series draws nothing.
            (line(None, BLUE, false), None),
        ],
        map,
        s[49].t_ns,
    );
    assert_eq!(cache.strokes.len(), 2);
    assert_eq!(
        cache.strokes.iter().map(|p| p.dashed).collect::<Vec<_>>(),
        vec![false, true]
    );
    // No fill colour: no fill sets.
    assert!(cache.fills.is_empty());
}

#[test]
fn stale_flat_continuation_present() {
    // 1 s period, newest sample 5 s old: stale (> 3 s).
    let s = series(20, SEC, 100 * SEC, |i| 10.0 + i as f32);
    let newest = s[19].t_ns;
    let now = newest + 5 * SEC;
    let map = TimeMap {
        anchor_ns: newest,
        right_edge_ns: now,
        range_ns: 30 * SEC,
        plot_width: 300.0,
    };
    let v = view(s.clone(), SEC);
    let cache = run_build(&[(line(Some(BLUE), RED, false), Some(&v))], map, now);

    assert_eq!(cache.stale_strokes.len(), 1);
    assert_eq!(cache.stale_fills.len(), 1);
    let xr = map.x_local(map.right_edge_ns);
    let y_last = y_of(s[19].value, 0.0, 100.0, 100.0);

    let stroke = &cache.stale_strokes[0];
    let right = stroke
        .path
        .vertices
        .iter()
        .max_by(|a, b| {
            a.xy_position
                .x
                .as_f32()
                .total_cmp(&b.xy_position.x.as_f32())
        })
        .unwrap();
    assert!((right.xy_position.x.as_f32() - xr).abs() < 0.01);
    // The stroke is 1 px wide around the line.
    assert!((right.xy_position.y.as_f32() - y_last).abs() <= 0.51);
    // The fill polygon has the exact corner at the right edge and the last value.
    assert!(cache.stale_fills[0].path.vertices.iter().any(|v| {
        (v.xy_position.x.as_f32() - xr).abs() < 0.01
            && (v.xy_position.y.as_f32() - y_last).abs() < 0.01
    }));
    // The fresh samples stay on the live paths.
    assert_eq!(cache.strokes.len(), 1);
}

#[test]
fn gap_dotted_span_coordinates() {
    let mut s = series(40, 100 * MS, 10 * SEC, |_| 50.0);
    for x in &mut s[10..20] {
        *x = Sample::gap(x.t_ns);
    }
    let map = exact_map(&s);
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(None, RED, false), Some(&v))], map, s[39].t_ns);
    assert_eq!(cache.gaps.len(), 1);
    let g = &cache.gaps[0];
    assert!((g.start - map.x_local(s[9].t_ns)).abs() < 0.01);
    assert!((g.end - map.x_local(s[20].t_ns)).abs() < 0.01);
}

#[test]
fn gap_from_left_edge_when_data_starts_late() {
    let s = series(10, 100 * MS, 50 * SEC, |_| 50.0);
    let newest = s[9].t_ns;
    let map = TimeMap {
        anchor_ns: newest,
        right_edge_ns: newest,
        range_ns: 10 * SEC,
        plot_width: 1_000.0,
    };
    let v = view(s.clone(), 100 * MS);
    let cache = run_build(&[(line(None, RED, false), Some(&v))], map, newest);
    assert_eq!(cache.gaps.len(), 1);
    assert!((cache.gaps[0].start - map.x_local(newest - 10 * SEC)).abs() < 0.01);
    assert!((cache.gaps[0].end - map.x_local(s[0].t_ns)).abs() < 0.01);
}

#[test]
fn y_of_maps_and_clamps() {
    assert_eq!(y_of(0.0, 0.0, 100.0, 100.0), 100.0);
    assert_eq!(y_of(100.0, 0.0, 100.0, 100.0), 0.0);
    assert_eq!(y_of(150.0, 0.0, 100.0, 100.0), 0.0);
    assert_eq!(y_of(-5.0, 0.0, 100.0, 100.0), 100.0);
    assert_eq!(y_of(25.0, 0.0, 100.0, 200.0), 150.0);
    // An empty scale puts the value on the baseline.
    assert_eq!(y_of(5.0, 1.0, 1.0, 100.0), 100.0);
}
